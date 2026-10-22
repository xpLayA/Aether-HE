//! The device worker: one thread owns the keyboard. The UI sends [`Cmd`]s and
//! polls [`Event`]s, so it never blocks on USB and never holds a lock.
//!
//! While a host effect streams, every write pauses it (end stream, restore the
//! firmware lighting), runs with a normal backup + verify, then resumes. That
//! keeps backups and profiles from ever capturing the temporary custom mode.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use aether_device::{Advanced, Backup, Board, Colors, EffectSpec, KeyPresses, KeyTables, Keyboard, Layout, Profile, plan_advanced};
use aether_effects::{Direction, Engine, Frame, KeyPos, Kind, Params};
use aether_proto::{ActTable, Actuation, DeviceInfo, Lighting};

pub enum Cmd {
    Connect,
    ReadLighting,
    /// Firmware lighting, and per-key colours to store first (mode 20).
    SetLighting(Lighting, Option<Box<Colors>>),
    /// Show `Lighting` on the keyboard without saving or backing up.
    PreviewLighting(Lighting),
    /// Put back the lighting that was active before previewing started.
    CancelPreview,
    ReadActuation,
    SetActuation { keys: Vec<usize>, setting: Actuation },
    ReadKeys,
    /// Put an advanced function on (or take it off) one key.
    SetAdvanced { key: usize, adv: Advanced },
    StartEffect(EffectSpec),
    StopEffect,
    /// A key clicked on screen, for reactive effects.
    Press(usize),
    Backup,
    Restore(PathBuf),
    /// Capture the board (lighting, actuation, running effect) into the
    /// custom profile `name`. `new` refuses an existing name; otherwise the
    /// profile is created or updated (auto-save after every edit).
    SaveProfile { name: String, dir: PathBuf, new: bool },
    ApplyProfile(Profile),
}

pub enum Event {
    Connected { board: Board, layout: Layout, info: DeviceInfo },
    Disconnected(String),
    /// Only a Bluetooth connection: typing works, settings don't.
    Bluetooth { name: String, battery: Option<u8> },
    /// A fresh 0x10 reply (battery) over a wireless link.
    Info(DeviceInfo),
    Lighting(Lighting),
    /// The per-key colours stored on the board.
    CustomLeds(Box<Colors>),
    Actuation(ActTable),
    /// Key records and DKS table (boards with advanced keys).
    Keys(Box<KeyTables>),
    EffectRunning(Option<EffectSpec>),
    Previewing(bool),
    ProfileApplied(Profile),
    /// A profile was saved; `new` when it didn't exist before.
    ProfileSaved { name: String, new: bool },
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
    pub fn spawn() -> Worker {
        let (tx, rx) = channel();
        let (etx, events) = channel();
        let live = Arc::new(Mutex::new(None));
        let state = State { kb: None, rx, tx: etx, effect: None, preview_base: None, live: live.clone(), battery_at: Instant::now() };
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
    /// Presses on the keyboard, for reactive effects.
    keys: Option<KeyPresses>,
}

struct State {
    kb: Option<Box<dyn Keyboard>>,
    rx: Receiver<Cmd>,
    tx: Sender<Event>,
    effect: Option<Running>,
    /// Lighting to restore when a preview is cancelled (or the app exits mid-preview).
    preview_base: Option<Lighting>,
    live: Arc<Mutex<Option<Frame>>>,
    /// Next battery read over a wireless link.
    battery_at: Instant,
}

type R<T> = aether_device::Result<T>;

const BATTERY_EVERY: Duration = Duration::from_secs(60);

impl State {
    fn emit(&self, e: Event) {
        let _ = self.tx.send(e);
    }

    fn run(mut self) {
        self.connect();
        self.emit(Event::Idle);
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
            self.tick_battery();
        }
        if let Err(e) = self.stop_effect() {
            log::warn!("stopping the effect on exit: {e}");
        }
        if let Err(e) = self.cancel_preview() {
            log::warn!("restoring the lighting after a preview on exit: {e}");
        }
    }

    /// Re-read the battery once a minute over a wireless link, when idle. No
    /// reply means the keyboard is asleep, not gone: the next tick tries again.
    fn tick_battery(&mut self) {
        let now = Instant::now();
        if self.effect.is_some() || now < self.battery_at {
            return;
        }
        let Some(kb) = self.kb.as_mut().filter(|kb| kb.board().wireless) else { return };
        self.battery_at = now + BATTERY_EVERY;
        match kb.device_info() {
            Ok(info) => self.emit(Event::Info(info)),
            Err(aether_device::Error::Timeout { .. }) => {}
            Err(e) => self.report::<()>(Err(e), |_| None),
        }
    }

    /// Restore the pre-preview lighting, if a preview is open.
    fn cancel_preview(&mut self) -> R<()> {
        let Some(base) = self.preview_base.take() else { return Ok(()) };
        self.emit(Event::Previewing(false));
        self.kb()?.set_lighting(&base)
    }

    fn connect(&mut self) {
        let _ = self.stop_effect();
        let _ = self.cancel_preview();
        self.kb = None;
        match aether_device::connect() {
            Ok((kb, info)) => {
                let (board, layout) = (kb.board().clone(), kb.layout().clone());
                self.kb = Some(kb);
                self.emit(Event::Connected { board, layout, info });
                self.read_lighting();
                self.read_actuation();
                self.read_keys();
            }
            Err(aether_device::Error::BluetoothOnly(bt)) => {
                self.emit(Event::Bluetooth { name: bt.board.name.clone(), battery: bt.battery })
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
                let asleep = matches!(e, aether_device::Error::Timeout { .. })
                    && self.kb.as_ref().is_some_and(|kb| kb.board().wireless);
                // Only a keypress wakes the board over 2.4 GHz; requests don't.
                self.emit(Event::Err(match asleep {
                    true => "The keyboard is asleep: press a key and try again".into(),
                    false => e.to_string(),
                }));
                if lost {
                    self.effect = None;
                    self.preview_base = None;
                    self.kb = None;
                    self.emit(Event::EffectRunning(None));
                    self.emit(Event::Disconnected(e.to_string()));
                }
            }
        }
    }

    fn read_lighting(&mut self) {
        if !self.kb.as_ref().is_some_and(|kb| kb.board().capabilities.lighting) {
            return;
        }
        let r = self.kb().and_then(|kb| kb.lighting());
        match r {
            Ok(l) => self.emit(Event::Lighting(l)),
            Err(e) => return self.report::<()>(Err(e), |_| None),
        }
        let r = self.kb().and_then(|kb| match kb.board().capabilities.per_key_store {
            true => kb.custom_leds().map(Some),
            false => Ok(None),
        });
        match r {
            Ok(Some(c)) => self.emit(Event::CustomLeds(Box::new(c))),
            Ok(None) => {}
            Err(e) => self.report::<()>(Err(e), |_| None),
        }
    }

    fn read_actuation(&mut self) {
        if !self.kb.as_ref().is_some_and(|kb| kb.board().capabilities.actuation) {
            return;
        }
        let r = self.kb().and_then(|kb| kb.actuation_table());
        match r {
            Ok(t) => self.emit(Event::Actuation(t)),
            Err(e) => self.report::<()>(Err(e), |_| None),
        }
    }

    fn read_keys(&mut self) {
        let r = self.kb().and_then(|kb| KeyTables::read(kb.as_mut()));
        match r {
            Ok(Some(t)) => self.emit(Event::Keys(Box::new(t))),
            Ok(None) => {}
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
            Cmd::SetLighting(l, leds) => {
                // Choosing firmware lighting ends a host effect for good.
                let base = self.preview_base.take();
                let r = self.stop_effect().and_then(|_| {
                    let kb = self.kb()?;
                    // Mid-preview, the keyboard shows the preview: if this creates the
                    // original backup, record what was there before it.
                    Backup::ensure_original_in(&aether_device::backups_dir()?, kb.as_mut(), base)?;
                    if let Some(c) = leds {
                        kb.set_custom_leds(&c)?;
                    }
                    kb.set_lighting(&l)
                });
                if base.is_some() {
                    self.emit(Event::Previewing(false));
                }
                self.report(r, |_| Some("Lighting applied".into()));
                self.emit(Event::BackupsChanged);
                self.read_lighting();
            }
            Cmd::PreviewLighting(l) => {
                let r = self.stop_effect().and_then(|_| {
                    let kb = self.kb.as_mut().ok_or(aether_device::Error::NotFound)?;
                    if self.preview_base.is_none() {
                        self.preview_base = Some(kb.lighting()?);
                    }
                    kb.set_lighting(&l)
                });
                let ok = r.is_ok();
                self.report(r, |_| None);
                if ok {
                    self.emit(Event::Previewing(true));
                }
            }
            Cmd::CancelPreview => {
                let r = self.cancel_preview();
                self.report(r, |_| Some("Preview cancelled, lighting restored".into()));
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
            Cmd::ReadKeys => self.read_keys(),
            Cmd::SetAdvanced { key, adv } => {
                let r = self.paused(|kb| {
                    let (keys, dks) = (kb.key_table()?, kb.dks_table()?);
                    let slug = kb.board().slug.clone();
                    let original = Backup::original_key_table(&slug)?.unwrap_or_else(|| keys.clone());
                    let layout = kb.layout().clone();
                    let (new_keys, new_dks) = plan_advanced(&keys, &dks, &layout, &original, key, adv)?;
                    Backup::snapshot(kb)?;
                    if new_dks != dks {
                        kb.write_dks_table(&new_dks)?;
                    }
                    kb.write_key_table(&new_keys)?;
                    Ok(layout.key(key).map_or(String::new(), |k| k.name.clone()))
                });
                let off = adv == Advanced::Off;
                self.report(r, |name| Some(if off { format!("{name} is an ordinary key again") } else { format!("{name} saved to the keyboard (verified)") }));
                self.emit(Event::BackupsChanged);
                self.read_keys();
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
            Cmd::Press(idx) => {
                if let Some(r) = self.effect.as_mut() {
                    r.engine.press(idx);
                }
            }
            Cmd::Backup => {
                // An explicit backup is always a new file, unlike the automatic original.
                let r = self.paused(|kb| Backup::capture(kb)?.save_in(&aether_device::backups_dir()?));
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
                self.read_keys();
            }
            Cmd::SaveProfile { name, dir, new } => {
                let r = self.capture_profile(&name).and_then(|p| if new { p.save_new_in(&dir) } else { p.save_in(&dir) });
                let ok = r.is_ok();
                self.report(r, |_| new.then(|| format!("Profile \"{name}\" saved")));
                if ok {
                    self.emit(Event::ProfileSaved { name, new });
                }
                self.emit(Event::ProfilesChanged);
            }
            Cmd::ApplyProfile(p) => {
                // A profile without lighting (the built-ins) leaves the lighting
                // and any running effect as they are.
                let r = if p.lighting.is_some() {
                    self.stop_effect().and_then(|_| p.apply(self.kb()?.as_mut()))
                } else {
                    self.paused(|kb| p.apply(kb))
                };
                let ok = r.is_ok();
                self.report(r, |_| Some(format!("Profile \"{}\" applied", p.name)));
                self.emit(Event::BackupsChanged);
                self.read_keys();
                if ok {
                    if let Some(spec) = p.effect.clone() {
                        let r = self.start_effect(spec);
                        self.report(r, |_| None);
                    }
                    self.emit(Event::ProfileApplied(p));
                }
                self.read_lighting();
                self.read_actuation();
            }
        }
    }

    /// The board as a profile, without pausing a running effect: its
    /// firmware lighting is the one saved when it started (likewise the
    /// pre-preview lighting), and the effect itself is part of the profile.
    fn capture_profile(&mut self, name: &str) -> R<Profile> {
        let held = self.effect.as_ref().map(|r| r.saved).or(self.preview_base);
        let effect = self.effect.as_ref().map(|r| r.spec.clone());
        let kb = self.kb()?;
        let lighting = match held {
            Some(l) => l,
            None => kb.lighting()?,
        };
        let table = kb.actuation_table()?;
        let leds = match kb.board().capabilities.per_key_store {
            true => Some(kb.custom_leds()?),
            false => None,
        };
        let tables = KeyTables::read(kb.as_mut())?;
        Ok(Profile::new(name, &kb.board().slug, Some(lighting.into()), &table, effect)
            .with_custom_leds(leds.as_ref())
            .with_key_tables(tables.as_ref()))
    }

    fn start_effect(&mut self, spec: EffectSpec) -> R<()> {
        let kb = self.kb()?;
        let fps = kb.board().lighting.host_fps.max(1.0);
        let keys: Vec<KeyPos> =
            kb.layout().keys.iter().map(|k| KeyPos { index: k.index, x: k.x + k.width / 2.0, y: k.y + k.height / 2.0 }).collect();
        let saved = kb.lighting()?;
        let params = spec_to_params(&spec);
        // A reactive effect without key presses still runs (clicks on screen work).
        let presses = match params.kind.reactive().then(|| KeyPresses::open(kb.board(), kb.layout())) {
            Some(Ok(l)) => Some(l),
            Some(Err(e)) => {
                let _ = self.tx.send(Event::Err(format!("can't see key presses ({e}); click keys on screen instead")));
                None
            }
            None => None,
        };
        let kb = self.kb()?;
        kb.stream_begin()?;
        let now = Instant::now();
        self.effect = Some(Running {
            engine: Engine::new(&keys, params, fps),
            spec: spec.clone(),
            saved,
            t0: now,
            next: now,
            interval: Duration::from_secs_f32(1.0 / fps),
            keys: presses,
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
        if let Some(k) = &r.keys {
            for idx in k.drain() {
                r.engine.press(idx);
            }
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
