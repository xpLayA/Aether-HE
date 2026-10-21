//! The device worker: one thread owns the keyboard. The UI sends [`Cmd`]s and
//! receives [`Event`]s, so it never blocks on USB and never holds a lock.
//!
//! While a host effect streams, every write pauses it (end stream, restore the
//! firmware lighting), runs with a normal backup + verify, then resumes. That
//! keeps backups and profiles from ever capturing the temporary custom mode.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use aether_device::{Backup, Board, EffectSpec, Keyboard, Layout, Profile};
use aether_effects::{Direction, Engine, Frame, KeyPos, Kind, Params};
use aether_proto::{ActTable, Actuation, DeviceInfo, Lighting};

pub enum Cmd {
    Connect,
    ReadLighting,
    SetLighting(Lighting),
    ReadActuation,
    SetActuation { keys: Vec<usize>, setting: Actuation },
    StartEffect(EffectSpec),
    StopEffect,
    Backup,
    Restore(PathBuf),
    /// Capture the board as a profile (with `effect`) and save it.
    SaveProfile { name: String, effect: Option<EffectSpec>, dir: PathBuf },
    ApplyProfile(Profile),
    EnsureStarters(PathBuf),
}

pub enum Event {
    Connected { board: Board, layout: Layout, info: DeviceInfo },
    Disconnected(String),
    Lighting(Lighting),
    Actuation(ActTable),
    EffectRunning(Option<EffectSpec>),
    ProfileApplied(String),
    ProfilesChanged,
    BackupsChanged,
    Ok(String),
    Err(String),
    Idle,
}

pub struct Worker {
    tx: Sender<Cmd>,
    pub events: Receiver<Event>,
    /// The frame currently on the keys while an effect runs (for the on-screen mirror).
    pub live: Arc<Mutex<Option<Frame>>>,
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn spawn(ctx: eframe::egui::Context) -> Worker {
        let (tx, rx) = channel();
        let (etx, events) = channel();
        let live = Arc::new(Mutex::new(None));
        let state = State { kb: None, rx, tx: etx, ctx, effect: None, live: live.clone() };
        let handle = std::thread::Builder::new().name("aether-device".into()).spawn(move || state.run()).ok();
        Worker { tx, events, live, handle }
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    /// Stop any effect (restoring the lighting) and end the thread.
    pub fn shutdown(&mut self) {
        let (tx, _) = channel();
        drop(std::mem::replace(&mut self.tx, tx));
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

pub fn spec_to_params(s: &EffectSpec) -> Params {
    Params {
        kind: Kind::from_name(&s.kind).unwrap_or(Kind::Wave),
        palette: if s.palette.is_empty() { vec![[255; 3]] } else { s.palette.clone() },
        bg: s.bg,
        speed: s.speed,
        brightness: s.brightness,
        direction: [Direction::Right, Direction::Left, Direction::Up, Direction::Down][s.direction.min(3) as usize],
    }
}

struct Running {
    spec: EffectSpec,
    engine: Engine,
    /// The firmware lighting to hand back when the stream stops.
    saved: Lighting,
    t0: Instant,
    next: Instant,
    interval: Duration,
}

struct State {
    kb: Option<Box<dyn Keyboard>>,
    rx: Receiver<Cmd>,
    tx: Sender<Event>,
    ctx: eframe::egui::Context,
    effect: Option<Running>,
    live: Arc<Mutex<Option<Frame>>>,
}

type R<T> = aether_device::Result<T>;

impl State {
    fn emit(&self, e: Event) {
        let _ = self.tx.send(e);
        self.ctx.request_repaint();
    }

    fn run(mut self) {
        self.connect();
        loop {
            let wait = match &self.effect {
                Some(r) => r.next.saturating_duration_since(Instant::now()),
                None => Duration::from_millis(250),
            };
            match self.rx.recv_timeout(wait) {
                Ok(cmd) => {
                    self.handle(cmd);
                    // Commands queued behind this one run before the next frame.
                    while let Ok(cmd) = self.rx.try_recv() {
                        self.handle(cmd);
                    }
                    self.emit(Event::Idle);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.tick_effect();
        }
        if let Err(e) = self.stop_effect() {
            log::warn!("stopping the effect on exit: {e}");
        }
    }

    fn connect(&mut self) {
        let _ = self.stop_effect();
        self.kb = None;
        match aether_device::connect() {
            Ok((kb, info)) => {
                let (board, layout) = (kb.board().clone(), kb.layout().clone());
                self.kb = Some(kb);
                self.emit(Event::Connected { board, layout, info });
                self.read_lighting();
                self.read_actuation();
            }
            Err(e) => self.emit(Event::Disconnected(e.to_string())),
        }
    }

    fn kb(&mut self) -> R<&mut Box<dyn Keyboard>> {
        self.kb.as_mut().ok_or(aether_device::Error::NotFound)
    }

    fn report<T>(&mut self, r: R<T>, ok: impl FnOnce(T) -> Option<String>) {
        match r {
            Ok(v) => {
                if let Some(msg) = ok(v) {
                    self.emit(Event::Ok(msg));
                }
            }
            Err(e) => {
                let lost = matches!(e, aether_device::Error::Hid(_) | aether_device::Error::NotFound);
                self.emit(Event::Err(e.to_string()));
                if lost {
                    self.effect = None;
                    self.kb = None;
                    self.emit(Event::EffectRunning(None));
                    self.emit(Event::Disconnected(e.to_string()));
                }
            }
        }
    }

    fn read_lighting(&mut self) {
        let r = self.kb().and_then(|kb| kb.lighting());
        match r {
            Ok(l) => self.emit(Event::Lighting(l)),
            Err(e) => self.report::<()>(Err(e), |_| None),
        }
    }

    fn read_actuation(&mut self) {
        let r = self.kb().and_then(|kb| kb.actuation_table());
        match r {
            Ok(t) => self.emit(Event::Actuation(t)),
            Err(e) => self.report::<()>(Err(e), |_| None),
        }
    }

    /// Run a write with any effect paused around it.
    fn paused<T>(&mut self, f: impl FnOnce(&mut dyn Keyboard) -> R<T>) -> R<T> {
        let resume = self.effect.as_ref().map(|r| r.spec.clone());
        self.stop_effect()?;
        let out = f(self.kb()?.as_mut());
        if let Some(spec) = resume {
            if let Err(e) = self.start_effect(spec) {
                self.emit(Event::Err(format!("effect not resumed: {e}")));
            }
        }
        out
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Connect => self.connect(),
            Cmd::ReadLighting => self.read_lighting(),
            Cmd::ReadActuation => self.read_actuation(),
            Cmd::SetLighting(l) => {
                // Choosing firmware lighting ends a host effect for good.
                let r = self.stop_effect().and_then(|_| {
                    let kb = self.kb()?;
                    Backup::snapshot(kb.as_mut())?;
                    kb.set_lighting(&l)
                });
                self.report(r, |_| Some("Lighting applied (backup saved)".into()));
                self.emit(Event::BackupsChanged);
                self.read_lighting();
            }
            Cmd::SetActuation { keys, setting } => {
                let r = self.paused(|kb| {
                    kb.validate_actuation(&keys, setting)?;
                    Backup::snapshot(kb)?;
                    kb.set_actuation(&keys, setting)
                });
                let n = keys.len();
                self.report(r, |_| Some(format!("Actuation set on {n} key{}, verified", if n == 1 { "" } else { "s" })));
                self.emit(Event::BackupsChanged);
                self.read_actuation();
            }
            Cmd::StartEffect(spec) => {
                let _ = self.stop_effect();
                let name = spec.kind.clone();
                let r = self.start_effect(spec);
                self.report(r, |_| Some(format!("Effect {name} running")));
            }
            Cmd::StopEffect => {
                let r = self.stop_effect();
                self.report(r, |_| Some("Effect stopped, lighting restored".into()));
            }
            Cmd::Backup => {
                let r = self.paused(Backup::snapshot);
                self.report(r, |p| Some(format!("Saved {}", p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default())));
                self.emit(Event::BackupsChanged);
            }
            Cmd::Restore(path) => {
                let r = self.stop_effect().and_then(|_| {
                    let b = Backup::load(&path)?;
                    b.restore(self.kb()?.as_mut())
                });
                self.report(r, |_| Some("Backup restored (verified)".into()));
                self.read_lighting();
                self.read_actuation();
            }
            Cmd::SaveProfile { name, effect, dir } => {
                let r = self.paused(|kb| {
                    let mut p = Profile::capture(kb, &name)?;
                    p.effect = effect;
                    p.save_in(&dir)
                });
                self.report(r, |_| Some(format!("Profile \"{name}\" saved")));
                self.emit(Event::ProfilesChanged);
            }
            Cmd::ApplyProfile(p) => {
                let r = self.stop_effect().and_then(|_| p.apply(self.kb()?.as_mut()));
                let ok = r.is_ok();
                self.report(r, |_| Some(format!("Profile \"{}\" applied", p.name)));
                self.emit(Event::BackupsChanged);
                if ok {
                    self.emit(Event::ProfileApplied(p.name.clone()));
                    if let Some(spec) = p.effect.clone() {
                        let r = self.start_effect(spec);
                        self.report(r, |_| None);
                    }
                }
                self.read_lighting();
                self.read_actuation();
            }
            Cmd::EnsureStarters(dir) => {
                let r = self.kb().and_then(|kb| Profile::ensure_starters(kb.as_mut(), &dir));
                self.report(r, |_| None);
                self.emit(Event::ProfilesChanged);
            }
        }
    }

    fn start_effect(&mut self, spec: EffectSpec) -> R<()> {
        let kb = self.kb()?;
        let fps = kb.board().lighting.host_fps.max(1.0);
        let keys: Vec<KeyPos> =
            kb.layout().keys.iter().map(|k| KeyPos { index: k.index, x: k.x + k.width / 2.0, y: k.y + k.height / 2.0 }).collect();
        let saved = kb.lighting()?;
        kb.stream_begin()?;
        let now = Instant::now();
        self.effect = Some(Running {
            engine: Engine::new(&keys, spec_to_params(&spec), fps),
            spec: spec.clone(),
            saved,
            t0: now,
            next: now,
            interval: Duration::from_secs_f32(1.0 / fps),
        });
        self.emit(Event::EffectRunning(Some(spec)));
        Ok(())
    }

    /// End the stream and hand the LEDs back to the saved firmware effect.
    fn stop_effect(&mut self) -> R<()> {
        let Some(r) = self.effect.take() else { return Ok(()) };
        *self.live.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.emit(Event::EffectRunning(None));
        let kb = self.kb()?;
        let end = kb.stream_end();
        std::thread::sleep(Duration::from_millis(150));
        let restore = kb.set_lighting(&r.saved);
        end.and(restore)
    }

    fn tick_effect(&mut self) {
        let Some(r) = self.effect.as_mut() else { return };
        let now = Instant::now();
        if now < r.next {
            return;
        }
        let frame = r.engine.render(now.duration_since(r.t0).as_secs_f32());
        r.next = (r.next + r.interval).max(now);
        let sent = match self.kb.as_mut() {
            Some(kb) => kb.stream_frame(&frame),
            None => Err(aether_device::Error::NotFound),
        };
        match sent {
            Ok(()) => {
                *self.live.lock().unwrap_or_else(|e| e.into_inner()) = Some(frame);
                self.ctx.request_repaint();
            }
            Err(e) => {
                self.effect = None;
                *self.live.lock().unwrap_or_else(|e| e.into_inner()) = None;
                self.emit(Event::EffectRunning(None));
                self.report::<()>(Err(e), |_| None);
            }
        }
    }
}
