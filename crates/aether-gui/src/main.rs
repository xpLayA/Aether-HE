//! `aether-gui`: desktop control for Aether keyboards, drawn with Slint
//! (`ui/*.slint`). This file is the glue: it turns worker events into UI
//! state and UI actions into worker commands. All device I/O is on the
//! worker thread (`worker.rs`).

mod firmware_sim;
mod worker;

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aether_device::{Backup, Board, EffectSpec, Layout, PRESETS, Profile, backups_dir, profiles_dir};
use aether_effects::Kind;
use aether_proto::{ActTable, Actuation, DeviceInfo, Lighting, raw_to_mm};
use serde::{Deserialize, Serialize};
use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};
use firmware_sim::Sim;
use worker::{Cmd, Event, Worker};

slint::include_modules!();

const PAGES: [&str; 7] = ["profiles", "lighting", "effects", "actuation", "live", "backups", "device"];
const P_LIGHTING: i32 = 1;
const P_EFFECTS: i32 = 2;
/// Minimum gap between lighting previews sent while you drag a slider.
const PREVIEW_GAP: Duration = Duration::from_millis(250);
const P_ACTUATION: i32 = 3;

fn main() -> Result<(), slint::PlatformError> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let ui = AppWindow::new()?;
    // Matches the .desktop file, so the compositor and menus know the window.
    // Needs the platform (created by AppWindow::new) and must precede show().
    slint::set_xdg_app_id("aether-he")?;
    let saved = Saved::load();
    let mut page = saved.page.clamp(0, PAGES.len() as i32 - 1);
    // AETHER_PAGE=actuation (etc.) opens on that page; handy for screenshots.
    if let Ok(want) = std::env::var("AETHER_PAGE") {
        if let Some(i) = PAGES.iter().position(|p| p.starts_with(&want.to_lowercase())) {
            page = i as i32;
        }
    }
    ui.global::<UiState>().set_page(page);
    // AETHER_SCROLL=600 starts the page scrolled down (screenshots of small windows).
    if let Some(px) = std::env::var("AETHER_SCROLL").ok().and_then(|v| v.parse::<f32>().ok()) {
        ui.global::<UiState>().set_start_scroll(px);
    }

    let state = Rc::new(RefCell::new(State::new(saved)));
    state.borrow_mut().reload_profiles();
    state.borrow_mut().reload_backups();
    state.borrow().push_all(&ui);
    wire(&ui, &state);

    let timer = slint::Timer::default();
    {
        let (state, weak) = (state.clone(), ui.as_weak());
        timer.start(slint::TimerMode::Repeated, Duration::from_millis(33), move || {
            if let Some(ui) = weak.upgrade() {
                state.borrow_mut().tick(&ui);
            }
        });
    }

    ui.run()?;
    let mut s = state.borrow_mut();
    s.sync_fx_from_ui(&ui);
    s.saved(&ui).store();
    s.worker.shutdown();
    Ok(())
}

// ------------------------------------------------------------------ state ----

/// What survives a restart (`~/.config/aether-he/gui.json`).
#[derive(Serialize, Deserialize)]
struct Saved {
    page: i32,
    fx: EffectSpec,
    active_profile: Option<String>,
    #[serde(default = "yes")]
    live_preview: bool,
    #[serde(default = "yes")]
    simulate: bool,
}

fn yes() -> bool {
    true
}

impl Saved {
    fn path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "aether-he").map(|d| d.config_dir().join("gui.json"))
    }

    fn load() -> Saved {
        Saved::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Saved { page: 0, fx: default_fx(), active_profile: None, live_preview: true, simulate: true })
    }

    fn store(&self) {
        if let Some(p) = Saved::path() {
            let _ = std::fs::create_dir_all(p.parent().unwrap_or(&p));
            let _ = serde_json::to_string_pretty(self).map(|t| std::fs::write(p, t));
        }
    }
}

fn default_fx() -> EffectSpec {
    EffectSpec {
        kind: "wave".into(),
        palette: vec![[255, 0, 80], [255, 160, 0], [0, 230, 120], [0, 150, 255], [170, 60, 255]],
        bg: [0, 0, 0],
        speed: 0.5,
        brightness: 1.0,
        direction: 0,
    }
}

enum Conn {
    Connecting,
    Up { board: Board, layout: Layout, info: DeviceInfo },
    Down(String),
}

enum Confirm {
    DeleteProfile(String),
    Restore(PathBuf),
    ResetAll,
}

struct State {
    worker: Worker,
    conn: Conn,
    selected: BTreeSet<usize>,
    hovered: Option<usize>,
    lighting: Option<Lighting>,
    light_form: Option<Lighting>,
    color_touched: bool,
    table: Option<ActTable>,
    running: Option<EffectSpec>,
    fx: EffectSpec,
    profiles: Vec<Profile>,
    active_profile: Option<String>,
    backups: Vec<(u64, PathBuf)>,
    toasts: Vec<(String, bool, Instant)>,
    confirm: Option<Confirm>,
    busy: bool,
    starters_done: bool,
    /// Layout offset so the drawn keyboard starts at (0, 0).
    origin: (f32, f32),
    live_preview: bool,
    /// The keyboard is showing an unapplied lighting preview.
    previewing: bool,
    preview_pending: bool,
    preview_sent: Instant,
    /// On-screen look-alike of the firmware effect (Lighting page).
    simulate: bool,
    sim: Option<Sim>,
    sim_frame: Option<aether_effects::Frame>,
}

fn rgb(c: [u8; 3]) -> Color {
    Color::from_rgb_u8(c[0], c[1], c[2])
}

fn to_rgb(c: Color) -> [u8; 3] {
    [c.red(), c.green(), c.blue()]
}

fn to_hex(c: Color) -> String {
    format!("#{:02X}{:02X}{:02X}", c.red(), c.green(), c.blue())
}

/// "#ff3d9a", "FF3D9A", "#f0a" -> colour; anything else -> None.
fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    if !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let full = match h.len() {
        3 => h.chars().flat_map(|c| [c, c]).collect::<String>(),
        6 => h.to_string(),
        _ => return None,
    };
    let v = u32::from_str_radix(&full, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

fn ago(secs: u64) -> String {
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86400 => format!("{} h ago", secs / 3600),
        _ => format!("{} days ago", secs / 86400),
    }
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

/// Teal (shallow, fast) to violet (deep) for 0.1..3.4 mm.
fn depth_color(mm: f64) -> [u8; 3] {
    let t = ((mm - 0.1) / 3.3).clamp(0.0, 1.0);
    let (a, b) = ([45.0, 212.0, 191.0], [139.0, 92.0, 246.0]);
    [0, 1, 2].map(|i| (a[i] + (b[i] - a[i]) * t) as u8)
}

fn mix(a: [u8; 3], b: [u8; 3], t: f64) -> [u8; 3] {
    [0, 1, 2].map(|i| (a[i] as f64 + (b[i] as f64 - a[i] as f64) * t) as u8)
}

fn ink_on(c: [u8; 3]) -> Color {
    let l = 0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32;
    if l > 150.0 { Color::from_rgb_u8(12, 16, 22) } else { Color::from_rgb_u8(231, 234, 239) }
}

const KEY_FACE: [u8; 3] = [38, 45, 56];

impl State {
    fn new(saved: Saved) -> State {
        State {
            worker: Worker::spawn(),
            conn: Conn::Connecting,
            selected: BTreeSet::new(),
            hovered: None,
            lighting: None,
            light_form: None,
            color_touched: false,
            table: None,
            running: None,
            fx: saved.fx,
            profiles: Vec::new(),
            active_profile: saved.active_profile,
            backups: Vec::new(),
            toasts: Vec::new(),
            confirm: None,
            busy: true,
            starters_done: false,
            origin: (0.0, 0.0),
            live_preview: saved.live_preview,
            previewing: false,
            preview_pending: false,
            preview_sent: Instant::now(),
            simulate: saved.simulate,
            sim: None,
            sim_frame: None,
        }
    }

    fn saved(&self, ui: &AppWindow) -> Saved {
        Saved {
            page: ui.global::<UiState>().get_page(),
            fx: self.fx.clone(),
            active_profile: self.active_profile.clone(),
            live_preview: self.live_preview,
            simulate: self.simulate,
        }
    }

    fn send(&mut self, cmd: Cmd) {
        self.busy = true;
        self.worker.send(cmd);
    }

    fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toasts.push((text.into(), error, Instant::now()));
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
    }

    fn up(&self) -> Option<(&Board, &Layout)> {
        match &self.conn {
            Conn::Up { board, layout, .. } => Some((board, layout)),
            _ => None,
        }
    }

    fn reload_profiles(&mut self) {
        self.profiles = profiles_dir().map(|d| Profile::list_in(&d)).unwrap_or_default();
    }

    fn reload_backups(&mut self) {
        let mut v = Backup::list().unwrap_or_default();
        v.reverse();
        self.backups = v;
    }

    /// Timer: worker events, live colours, toast expiry.
    fn tick(&mut self, ui: &AppWindow) {
        let mut changed = false;
        while let Ok(ev) = self.worker.events.try_recv() {
            changed = true;
            match ev {
                Event::Connected { board, layout, info } => {
                    let min_x = layout.keys.iter().map(|k| k.x).fold(f32::MAX, f32::min);
                    let min_y = layout.keys.iter().map(|k| k.y).fold(f32::MAX, f32::min);
                    self.origin = (min_x, min_y);
                    let centres: Vec<(usize, f32, f32)> =
                        layout.keys.iter().map(|k| (k.index, k.x + k.width / 2.0, k.y + k.height / 2.0)).collect();
                    self.sim = Some(Sim::new(&centres));
                    self.conn = Conn::Up { board, layout, info };
                    if !self.starters_done {
                        self.starters_done = true;
                        if let Ok(d) = profiles_dir() {
                            self.send(Cmd::EnsureStarters(d));
                        }
                    }
                }
                Event::Disconnected(why) => {
                    self.conn = Conn::Down(why);
                    self.running = None;
                }
                Event::Lighting(l) => {
                    self.lighting = Some(l);
                    self.light_form = Some(l);
                    self.color_touched = false;
                }
                Event::Actuation(t) => self.table = Some(t),
                Event::EffectRunning(spec) => self.running = spec,
                Event::Previewing(on) => self.previewing = on,
                Event::ProfileApplied(name) => self.active_profile = Some(name),
                Event::ProfilesChanged => self.reload_profiles(),
                Event::BackupsChanged => self.reload_backups(),
                Event::Ok(m) => self.toast(m, false),
                Event::Err(m) => self.toast(m, true),
                Event::Idle => self.busy = false,
            }
        }
        // Trailing debounce: the last edit is always shown, at most every PREVIEW_GAP.
        if self.preview_pending && self.preview_sent.elapsed() >= PREVIEW_GAP {
            if let Some(l) = self.light_form.filter(|_| matches!(self.conn, Conn::Up { .. })) {
                // Not `send`: a preview shouldn't flash the busy spinner or disable buttons.
                self.worker.send(Cmd::PreviewLighting(l));
            }
            self.preview_pending = false;
            self.preview_sent = Instant::now();
        }
        let before = self.toasts.len();
        self.toasts.retain(|(_, err, at)| at.elapsed() < Duration::from_secs(if *err { 8 } else { 4 }));
        let page = ui.global::<UiState>().get_page();
        self.sim_frame = None;
        if page == P_LIGHTING && self.simulate {
            if let (Some(sim), Some(l), Some((board, _))) = (self.sim.as_mut(), self.light_form, match &self.conn {
                Conn::Up { board, layout, .. } => Some((board, layout)),
                _ => None,
            }) {
                let multicolor = l.color_mode == 1 || board.lighting.mode(l.mode).is_some_and(|m| !m.color);
                self.sim_frame = Some(sim.render(&l, multicolor));
            }
        }
        if changed {
            self.push_all(ui);
        } else {
            if before != self.toasts.len() {
                self.push_toasts(ui);
            }
            if (self.running.is_some() && page == P_EFFECTS) || self.sim_frame.is_some() {
                self.push_keys(ui);
            }
        }
    }

    // ---- state -> UI ----

    fn push_all(&self, ui: &AppWindow) {
        self.push_header(ui);
        self.push_keys(ui);
        self.push_profiles(ui);
        self.push_lighting(ui);
        self.push_effects(ui);
        self.push_actuation(ui);
        self.push_backups(ui);
        self.push_device(ui);
        self.push_toasts(ui);
    }

    fn push_header(&self, ui: &AppWindow) {
        let g = ui.global::<UiState>();
        match &self.conn {
            Conn::Connecting => g.set_connection(0),
            Conn::Up { board, info, .. } => {
                g.set_connection(1);
                g.set_board_name(board.name.clone().into());
                g.set_firmware(format!("firmware V{:X}.{:02X}", info.firmware >> 8, info.firmware & 0xFF).into());
            }
            Conn::Down(why) => {
                g.set_connection(2);
                g.set_down_reason(why.clone().into());
            }
        }
        g.set_busy(self.busy);
        g.set_effect_running(self.running.as_ref().map(|s| cap(&s.kind)).unwrap_or_default().into());
        g.set_active_profile(self.active_profile.clone().unwrap_or_default().into());
        let idx = self.active_profile.as_ref().and_then(|a| self.profiles.iter().position(|p| &p.name == a));
        g.set_active_profile_index(idx.map_or(-1, |i| i as i32));
        g.set_profile_names(model(self.profiles.iter().map(|p| SharedString::from(p.name.as_str())).collect()));
    }

    fn key_info(&self, idx: usize) -> String {
        match self.table.as_ref().map(|t| t.record(idx)) {
            Some(r) if r.is_empty() => "firmware default".into(),
            Some(r) if r.rapid_trigger() => {
                format!("{:.2} mm · rapid trigger {:.2} / {:.2}", raw_to_mm(r.trip), raw_to_mm(r.press), raw_to_mm(r.release))
            }
            Some(r) => format!("{:.2} mm", raw_to_mm(r.trip)),
            None => String::new(),
        }
    }

    fn push_keys(&self, ui: &AppWindow) {
        let g = ui.global::<UiState>();
        let Some((_, layout)) = self.up() else {
            g.set_keys(model(Vec::new()));
            return;
        };
        let page = g.get_page();
        let live = match page {
            P_EFFECTS => self.worker.live.lock().ok().and_then(|f| *f),
            P_LIGHTING => self.sim_frame,
            _ => None,
        };
        g.set_keys_badge(if live.is_some() && page == P_LIGHTING { "Simulation".into() } else { "".into() });
        let (ox, oy) = self.origin;
        let keys: Vec<KeyCap> = layout
            .keys
            .iter()
            .map(|k| {
                let (mut fill, mut value) = (KEY_FACE, String::new());
                if let Some(f) = &live {
                    fill = f[k.index];
                } else if page == P_ACTUATION {
                    if let Some(r) = self.table.as_ref().map(|t| t.record(k.index)).filter(|r| !r.is_empty()) {
                        let mm = raw_to_mm(r.trip);
                        fill = mix(KEY_FACE, depth_color(mm), 0.55);
                        value = if r.rapid_trigger() { format!("{mm:.2} RT") } else { format!("{mm:.2}") };
                    }
                }
                KeyCap {
                    index: k.index as i32,
                    x: k.x - ox,
                    y: k.y - oy,
                    w: k.width,
                    h: k.height,
                    label: k.name.clone().into(),
                    value: value.into(),
                    fill: rgb(fill),
                    ink: if live.is_some() { ink_on(fill) } else { Color::from_rgb_u8(231, 234, 239) },
                    selected: self.selected.contains(&k.index),
                }
            })
            .collect();
        let w = layout.keys.iter().map(|k| k.x + k.width - ox).fold(0.0, f32::max);
        let h = layout.keys.iter().map(|k| k.y + k.height - oy).fold(0.0, f32::max);
        g.set_board_w(w);
        g.set_board_h(h);
        g.set_keys(model(keys));
        g.set_selected_count(self.selected.len() as i32);
        match self.hovered.and_then(|i| layout.key(i)) {
            Some(k) => {
                g.set_hover_key(k.name.clone().into());
                g.set_hover_info(self.key_info(k.index).into());
            }
            None => {
                g.set_hover_key("".into());
                g.set_hover_info("".into());
            }
        }
    }

    fn push_profiles(&self, ui: &AppWindow) {
        let g = ui.global::<UiState>();
        let modes = self.up().map(|(b, _)| b.lighting.clone());
        let cards: Vec<ProfileCard> = self
            .profiles
            .iter()
            .map(|p| {
                let t = p.actuation_table().unwrap_or_default();
                let set: Vec<_> = (0..aether_proto::KEY_SLOTS).map(|i| t.record(i)).filter(|r| !r.is_empty()).collect();
                let act = if set.is_empty() {
                    "All keys on firmware default".to_string()
                } else {
                    let rt = set.iter().filter(|r| r.rapid_trigger()).count();
                    let lo = set.iter().map(|r| r.trip).min().unwrap_or(0);
                    let hi = set.iter().map(|r| r.trip).max().unwrap_or(0);
                    let range = if lo == hi { format!("{:.2} mm", raw_to_mm(lo)) } else { format!("{:.2}–{:.2} mm", raw_to_mm(lo), raw_to_mm(hi)) };
                    format!("{} keys at {range}{}", set.len(), if rt > 0 { format!(" · {rt} with rapid trigger") } else { String::new() })
                };
                let mode = modes.as_ref().and_then(|m| m.mode(p.lighting.mode)).map_or("Custom".into(), |m| m.name.clone());
                ProfileCard {
                    name: p.name.clone().into(),
                    actuation: act.into(),
                    lighting: format!("Lighting: {mode}").into(),
                    effect: p.effect.as_ref().map_or("No host effect".into(), |e| format!("Effect: {}", cap(&e.kind))).into(),
                    active: self.active_profile.as_deref() == Some(p.name.as_str()),
                }
            })
            .collect();
        g.set_profiles(model(cards));
        g.set_fx_name(cap(&self.fx.kind).into());
    }

    fn push_lighting(&self, ui: &AppWindow) {
        let g = ui.global::<UiState>();
        let (Some((board, _)), Some(l)) = (self.up(), self.light_form) else { return };
        let info = &board.lighting;
        g.set_light_modes(model(info.modes.iter().map(|m| SharedString::from(m.name.as_str())).collect()));
        g.set_light_mode(info.modes.iter().position(|m| m.byte == l.mode).map_or(-1, |i| i as i32));
        let mode = info.mode(l.mode);
        g.set_light_color_ok(mode.is_some_and(|m| m.color));
        g.set_light_speed_ok(mode.is_none_or(|m| m.speed));
        g.set_light_color(rgb(l.rgb));
        g.set_light_brightness(l.brightness as f32);
        g.set_light_speed(l.speed as f32);
        g.set_brightness_max(info.brightness_max as f32);
        g.set_speed_max(info.speed_max as f32);
        g.set_light_dirty(self.lighting != Some(l));
        g.set_live_preview(self.live_preview);
        g.set_previewing(self.previewing);
        g.set_simulate(self.simulate);
    }

    fn push_effects(&self, ui: &AppWindow) {
        let g = ui.global::<UiState>();
        g.set_fx_kinds(model(Kind::ALL.iter().map(|k| SharedString::from(cap(k.name()))).collect()));
        g.set_fx_kind(Kind::ALL.iter().position(|k| k.name() == self.fx.kind).unwrap_or(0) as i32);
        g.set_fx_palette(model(self.fx.palette.iter().map(|c| rgb(*c)).collect()));
        g.set_fx_bg(rgb(self.fx.bg));
        g.set_fx_speed(self.fx.speed);
        g.set_fx_brightness(self.fx.brightness);
        g.set_fx_direction(self.fx.direction as i32);
    }

    fn push_actuation(&self, ui: &AppWindow) {
        let g = ui.global::<UiState>();
        g.set_presets(model(
            PRESETS
                .iter()
                .map(|(name, s)| Preset {
                    name: (*name).into(),
                    detail: match s {
                        Actuation::Fixed { travel_mm } => format!("{travel_mm:.2} mm"),
                        Actuation::RapidTrigger { travel_mm, press_mm, .. } => format!("{travel_mm:.2} mm + rapid trigger {press_mm:.2}"),
                        Actuation::Default => "Firmware default".into(),
                    }
                    .into(),
                })
                .collect(),
        ));
        if let Some((board, _)) = self.up() {
            let a = &board.actuation;
            g.set_travel_min(a.travel_mm[0] as f32);
            g.set_travel_max(a.travel_mm[1] as f32);
            g.set_rt_min(a.rt_mm[0] as f32);
            g.set_rt_max(a.rt_mm[1] as f32);
            g.set_rt_ok(board.capabilities.rapid_trigger);
        }
    }

    fn push_backups(&self, ui: &AppWindow) {
        let g = ui.global::<UiState>();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        g.set_backups(model(
            self.backups
                .iter()
                .map(|(t, p)| BackupRow {
                    when: ago(now.saturating_sub(*t)).into(),
                    file: p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default().into(),
                })
                .collect(),
        ));
        let has = self.up().is_some_and(|(b, _)| Backup::oldest(&b.slug).ok().flatten().is_some());
        g.set_has_original(has);
    }

    fn push_device(&self, ui: &AppWindow) {
        let g = ui.global::<UiState>();
        g.set_profiles_dir(profiles_dir().map(|d| d.display().to_string()).unwrap_or_default().into());
        g.set_backups_dir(backups_dir().map(|d| d.display().to_string()).unwrap_or_default().into());
        let Conn::Up { board, layout, info } = &self.conn else { return };
        let yes = |b: bool| if b { "yes" } else { "no" };
        let c = &board.capabilities;
        let a = &board.actuation;
        let rows = vec![
            ("USB ID", format!("{:04X}:{:04X}", info.vid, info.pid)),
            ("Firmware", format!("V{:X}.{:02X}", info.firmware >> 8, info.firmware & 0xFF)),
            ("Keys", layout.keys.len().to_string()),
            ("Host effects", format!("up to {} fps", board.lighting.host_fps)),
            ("Actuation range", format!("{:.2}–{:.2} mm, rapid trigger {:.2}–{:.2} mm", a.travel_mm[0], a.travel_mm[1], a.rt_mm[0], a.rt_mm[1])),
            (
                "Verified features",
                format!("lighting {}, host effects {}, actuation {}, rapid trigger {}", yes(c.lighting), yes(c.host_effects), yes(c.actuation), yes(c.rapid_trigger)),
            ),
        ];
        g.set_device_info(model(rows.into_iter().map(|(k, v)| InfoRow { key: k.into(), value: v.into() }).collect()));
    }

    fn push_toasts(&self, ui: &AppWindow) {
        let g = ui.global::<UiState>();
        g.set_toasts(model(self.toasts.iter().map(|(t, e, _)| ToastItem { text: t.clone().into(), error: *e }).collect()));
        let (open, title, body, action) = match &self.confirm {
            None => (false, "", String::new(), ""),
            Some(Confirm::DeleteProfile(n)) => (true, "Delete profile?", format!("\"{n}\" will be deleted. This can't be undone."), "Delete"),
            Some(Confirm::Restore(_)) => (
                true,
                "Restore backup?",
                "The keyboard's lighting and actuation go back to this backup. The current state is backed up first.".into(),
                "Restore",
            ),
            Some(Confirm::ResetAll) => (
                true,
                "Reset all keys?",
                "Every key goes back to the firmware default actuation. A backup is taken first.".into(),
                "Reset all",
            ),
        };
        g.set_confirm_open(open);
        g.set_confirm_title(title.into());
        g.set_confirm_body(body.into());
        g.set_confirm_action(action.into());
    }

    // ---- UI -> state ----

    fn sync_fx_from_ui(&mut self, ui: &AppWindow) {
        let g = ui.global::<UiState>();
        if let Some(k) = Kind::ALL.get(g.get_fx_kind().max(0) as usize) {
            self.fx.kind = k.name().into();
        }
        self.fx.bg = to_rgb(g.get_fx_bg());
        self.fx.speed = g.get_fx_speed();
        self.fx.brightness = g.get_fx_brightness();
        self.fx.direction = g.get_fx_direction().clamp(0, 3) as u8;
    }

    fn light_from_ui(&mut self, ui: &AppWindow) {
        let g = ui.global::<UiState>();
        let Some(mut l) = self.light_form else { return };
        l.brightness = g.get_light_brightness().round() as u8;
        l.speed = g.get_light_speed().round() as u8;
        if self.color_touched {
            l.rgb = to_rgb(g.get_light_color());
            l.alpha = 0xFF;
            l.color_mode = 0;
        }
        self.light_form = Some(l);
        g.set_light_dirty(self.lighting != Some(l));
        if self.live_preview {
            self.preview_pending = true;
        }
    }

    /// Drop an unapplied preview and put the keyboard back.
    fn cancel_preview(&mut self) {
        self.preview_pending = false;
        if self.previewing {
            self.send(Cmd::CancelPreview);
        }
    }

    fn hit(&self, x: f32, y: f32) -> Option<usize> {
        let (_, layout) = self.up()?;
        let (ox, oy) = self.origin;
        layout
            .keys
            .iter()
            .find(|k| x >= k.x - ox && x < k.x - ox + k.width && y >= k.y - oy && y < k.y - oy + k.height)
            .map(|k| k.index)
    }

    fn write_actuation(&mut self, all: bool, setting: Actuation) {
        let keys: Vec<usize> = match (self.up(), all) {
            (Some((_, layout)), true) => layout.keys.iter().map(|k| k.index).collect(),
            _ => self.selected.iter().copied().collect(),
        };
        if keys.is_empty() {
            self.toast("Select keys first, or choose All keys", true);
            return;
        }
        self.active_profile = None;
        self.send(Cmd::SetActuation { keys, setting });
    }

    fn quick_select(&mut self, group: &str) {
        let Some((_, layout)) = self.up() else { return };
        let pick = |f: &dyn Fn(&str) -> bool| -> BTreeSet<usize> { layout.keys.iter().filter(|k| f(&k.code)).map(|k| k.index).collect() };
        self.selected = match group {
            "All" => layout.keys.iter().map(|k| k.index).collect(),
            "WASD" => pick(&|c| matches!(c, "KeyW" | "KeyA" | "KeyS" | "KeyD")),
            "Letters" => pick(&|c| c.len() == 4 && c.starts_with("Key")),
            "Numbers" => pick(&|c| c.starts_with("Digit")),
            "Arrows" => pick(&|c| c.starts_with("Arrow")),
            "Modifiers" => pick(&|c| {
                ["Shift", "Control", "Alt", "Meta"].iter().any(|m| c.starts_with(m)) || matches!(c, "KeyFn" | "CapsLock" | "Tab" | "Space")
            }),
            _ => BTreeSet::new(),
        };
    }

    fn profile_action(&mut self, name: &str, action: &str, ui: &AppWindow) {
        let Ok(dir) = profiles_dir() else { return };
        let found = self.profiles.iter().find(|p| p.name == name).cloned();
        match action {
            "update" => {
                let effect = found.and_then(|p| p.effect);
                self.send(Cmd::SaveProfile { name: name.into(), effect, dir });
            }
            "effect" | "noeffect" => {
                self.sync_fx_from_ui(ui);
                if let Some(mut p) = found {
                    p.effect = (action == "effect").then(|| self.fx.clone());
                    match p.save_in(&dir) {
                        Ok(_) => self.toast(format!("Updated the effect of \"{name}\""), false),
                        Err(e) => self.toast(e.to_string(), true),
                    }
                    self.reload_profiles();
                }
            }
            "rename" => {
                let g = ui.global::<UiState>();
                g.set_renaming(name.into());
                g.set_rename_text(name.into());
            }
            "duplicate" => {
                if let Some(mut p) = found {
                    let mut n = 2;
                    while self.profiles.iter().any(|q| q.name.eq_ignore_ascii_case(&format!("{name} {n}"))) {
                        n += 1;
                    }
                    p.name = format!("{name} {n}");
                    if let Err(e) = p.save_in(&dir) {
                        self.toast(e.to_string(), true);
                    }
                    self.reload_profiles();
                }
            }
            "delete" => self.confirm = Some(Confirm::DeleteProfile(name.into())),
            _ => {}
        }
    }
}

// ----------------------------------------------------------------- wiring ----

/// Connect every UI action to the state. Each handler mutates state, then
/// pushes the UI back in sync.
fn wire(ui: &AppWindow, state: &Rc<RefCell<State>>) {
    let a = ui.global::<Actions>();
    a.on_to_hex(|c| to_hex(c).into());
    // Transparent means "not a colour"; the picker checks alpha.
    a.on_parse_hex(|s| parse_hex(&s).map_or(Color::from_argb_u8(0, 0, 0, 0), rgb));
    macro_rules! on {
        ($setter:ident, |$s:ident, $ui:ident $(, $arg:ident : $t:ty)*| $body:block) => {{
            let (st, weak) = (state.clone(), ui.as_weak());
            a.$setter(move |$($arg: $t),*| {
                let Some($ui) = weak.upgrade() else { return };
                let mut $s = st.borrow_mut();
                $body
                $s.push_all(&$ui);
            });
        }};
    }

    on!(on_nav, |s, ui, i: i32| {
        let g = ui.global::<UiState>();
        if g.get_page() == P_LIGHTING && i != P_LIGHTING {
            s.cancel_preview();
            s.light_form = s.lighting;
            s.color_touched = false;
        }
        g.set_page(i);
        s.hovered = None;
    });
    on!(on_reconnect, |s, ui| {
        s.conn = Conn::Connecting;
        s.send(Cmd::Connect);
    });
    on!(on_switch_profile, |s, ui, name: SharedString| {
        if let Some(p) = s.profiles.iter().find(|p| p.name == name.as_str()).cloned() {
            s.send(Cmd::ApplyProfile(p));
        }
    });

    on!(on_board_click, |s, ui, x: f32, y: f32, add: bool| {
        let hit = s.hit(x, y);
        if let (Some(i), Some(sim)) = (hit, s.sim.as_mut()) {
            sim.press(i);
        }
        match hit {
            Some(i) if add => {
                if !s.selected.remove(&i) {
                    s.selected.insert(i);
                }
            }
            Some(i) if s.selected.len() == 1 && s.selected.contains(&i) => s.selected.clear(),
            Some(i) => s.selected = BTreeSet::from([i]),
            None if !add => s.selected.clear(),
            None => {}
        }
    });
    on!(on_board_rect, |s, ui, x0: f32, y0: f32, x1: f32, y1: f32, add: bool| {
        if let Some((_, layout)) = s.up() {
            let (ox, oy) = s.origin;
            let hit: Vec<usize> = layout
                .keys
                .iter()
                .filter(|k| k.x - ox < x1 && k.x - ox + k.width > x0 && k.y - oy < y1 && k.y - oy + k.height > y0)
                .map(|k| k.index)
                .collect();
            if !add {
                s.selected.clear();
            }
            s.selected.extend(hit);
        }
    });
    {
        let (st, weak) = (state.clone(), ui.as_weak());
        a.on_board_hover(move |x, y| {
            let Some(ui) = weak.upgrade() else { return };
            let mut s = st.borrow_mut();
            let h = s.hit(x, y);
            if h != s.hovered {
                s.hovered = h;
                let g = ui.global::<UiState>();
                match h.and_then(|i| s.up().and_then(|(_, l)| l.key(i)).map(|k| (k.index, k.name.clone()))) {
                    Some((i, n)) => {
                        g.set_hover_key(n.into());
                        g.set_hover_info(s.key_info(i).into());
                    }
                    None => {
                        g.set_hover_key("".into());
                        g.set_hover_info("".into());
                    }
                }
            }
        });
    }
    {
        let (st, weak) = (state.clone(), ui.as_weak());
        a.on_board_leave(move || {
            let Some(ui) = weak.upgrade() else { return };
            st.borrow_mut().hovered = None;
            let g = ui.global::<UiState>();
            g.set_hover_key("".into());
            g.set_hover_info("".into());
        });
    }
    on!(on_quick_select, |s, ui, g: SharedString| {
        s.quick_select(g.as_str());
    });

    on!(on_profile_apply, |s, ui, name: SharedString| {
        if let Some(p) = s.profiles.iter().find(|p| p.name == name.as_str()).cloned() {
            s.send(Cmd::ApplyProfile(p));
        }
    });
    on!(on_profile_action, |s, ui, name: SharedString, action: SharedString| {
        s.profile_action(name.as_str(), action.as_str(), &ui);
    });
    on!(on_profile_save_new, |s, ui| {
        let g = ui.global::<UiState>();
        let name = g.get_new_profile_name().trim().to_string();
        if !name.is_empty() {
            if let Ok(dir) = profiles_dir() {
                s.sync_fx_from_ui(&ui);
                let effect = g.get_include_effect().then(|| s.fx.clone());
                s.send(Cmd::SaveProfile { name: name.clone(), effect, dir });
                s.active_profile = Some(name);
                g.set_new_profile_name("".into());
            }
        }
    });
    on!(on_profile_rename_done, |s, ui| {
        let g = ui.global::<UiState>();
        let (old, new) = (g.get_renaming().to_string(), g.get_rename_text().trim().to_string());
        g.set_renaming("".into());
        if !new.is_empty() && new != old {
            match profiles_dir().and_then(|d| Profile::rename_in(&d, &old, &new)) {
                Ok(_) => {
                    if s.active_profile.as_deref() == Some(old.as_str()) {
                        s.active_profile = Some(new);
                    }
                }
                Err(e) => s.toast(e.to_string(), true),
            }
            s.reload_profiles();
        }
    });

    on!(on_light_mode_picked, |s, ui, i: i32| {
        let mode = s.up().and_then(|(b, _)| b.lighting.modes.get(i.max(0) as usize).cloned());
        if let (Some(m), Some(mut l)) = (mode, s.light_form) {
            l.mode = m.byte;
            l.color_mode = u8::from(!m.color);
            s.light_form = Some(l);
        }
        s.light_from_ui(&ui);
    });
    {
        let (st, weak) = (state.clone(), ui.as_weak());
        a.on_light_edited(move || {
            let Some(ui) = weak.upgrade() else { return };
            let mut s = st.borrow_mut();
            // Colour edits switch a fixed-colour effect to that colour.
            if Some(ui.global::<UiState>().get_light_color()) != s.light_form.map(|l| rgb(l.rgb)) {
                s.color_touched = true;
            }
            s.light_from_ui(&ui);
        });
    }
    on!(on_light_apply, |s, ui| {
        s.light_from_ui(&ui);
        s.preview_pending = false;
        if let Some(l) = s.light_form {
            s.active_profile = None;
            s.send(Cmd::SetLighting(l));
        }
    });
    on!(on_light_revert, |s, ui| {
        s.cancel_preview();
        s.light_form = s.lighting;
        s.color_touched = false;
    });
    on!(on_light_sim_toggled, |s, ui, on: bool| {
        s.simulate = on;
    });
    on!(on_light_preview_toggled, |s, ui, on: bool| {
        s.live_preview = on;
        if on {
            s.preview_pending = s.lighting != s.light_form;
        } else {
            s.cancel_preview();
        }
    });
    on!(on_light_reread, |s, ui| {
        s.send(Cmd::ReadLighting);
    });

    on!(on_fx_pick, |s, ui, i: i32| {
        s.sync_fx_from_ui(&ui);
        if let Some(k) = Kind::ALL.get(i.max(0) as usize) {
            s.fx.kind = k.name().into();
        }
    });
    on!(on_fx_set_color, |s, ui, i: i32, c: Color| {
        s.sync_fx_from_ui(&ui);
        if let Some(slot) = s.fx.palette.get_mut(i.max(0) as usize) {
            *slot = to_rgb(c);
        }
    });
    on!(on_fx_add_color, |s, ui| {
        s.sync_fx_from_ui(&ui);
        s.fx.palette.push([255, 255, 255]);
    });
    on!(on_fx_remove_color, |s, ui, i: i32| {
        s.sync_fx_from_ui(&ui);
        if s.fx.palette.len() > 1 && (i as usize) < s.fx.palette.len() {
            s.fx.palette.remove(i as usize);
        }
    });
    on!(on_fx_start, |s, ui| {
        s.sync_fx_from_ui(&ui);
        let spec = s.fx.clone();
        s.send(Cmd::StartEffect(spec));
    });
    on!(on_fx_stop, |s, ui| {
        s.send(Cmd::StopEffect);
    });
    on!(on_fx_defaults, |s, ui| {
        s.fx = default_fx();
    });

    on!(on_preset, |s, ui, i: i32| {
        let all = ui.global::<UiState>().get_scope() == 1;
        if let Some((_, setting)) = PRESETS.get(i.max(0) as usize) {
            s.write_actuation(all, *setting);
        }
    });
    on!(on_act_apply, |s, ui| {
        let g = ui.global::<UiState>();
        let r = |v: f32| ((v as f64) * 100.0).round() / 100.0;
        let setting = if g.get_rt() {
            Actuation::RapidTrigger { travel_mm: r(g.get_travel()), press_mm: r(g.get_press()), release_mm: r(g.get_release()) }
        } else {
            Actuation::Fixed { travel_mm: r(g.get_travel()) }
        };
        s.write_actuation(g.get_scope() == 1, setting);
    });
    on!(on_act_reset_selected, |s, ui| {
        s.write_actuation(false, Actuation::Default);
    });
    on!(on_act_reset_all, |s, ui| {
        s.confirm = Some(Confirm::ResetAll);
    });
    on!(on_act_read, |s, ui| {
        s.send(Cmd::ReadActuation);
    });

    on!(on_backup_now, |s, ui| {
        s.send(Cmd::Backup);
    });
    on!(on_backup_restore, |s, ui, i: i32| {
        if let Some((_, p)) = s.backups.get(i.max(0) as usize) {
            s.confirm = Some(Confirm::Restore(p.clone()));
        }
    });
    on!(on_backup_original, |s, ui| {
        if let Some(p) = s.up().and_then(|(b, _)| Backup::oldest(&b.slug).ok().flatten()) {
            s.confirm = Some(Confirm::Restore(p));
        }
    });
    on!(on_open_dir, |s, ui, d: SharedString| {
        let _ = std::fs::create_dir_all(d.as_str());
        if let Err(e) = std::process::Command::new("xdg-open").arg(d.as_str()).spawn() {
            s.toast(format!("Couldn't open {d}: {e}"), true);
        }
    });

    on!(on_confirm_yes, |s, ui| {
        match s.confirm.take() {
            Some(Confirm::DeleteProfile(n)) => {
                match profiles_dir().and_then(|d| Profile::delete_in(&d, &n)) {
                    Ok(()) => s.toast(format!("Deleted \"{n}\""), false),
                    Err(e) => s.toast(e.to_string(), true),
                }
                if s.active_profile.as_deref() == Some(n.as_str()) {
                    s.active_profile = None;
                }
                s.reload_profiles();
            }
            Some(Confirm::Restore(p)) => {
                s.active_profile = None;
                s.send(Cmd::Restore(p));
            }
            Some(Confirm::ResetAll) => s.write_actuation(true, Actuation::Default),
            None => {}
        }
    });
    on!(on_confirm_no, |s, ui| {
        s.confirm = None;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip_and_tolerant_parsing() {
        assert_eq!(parse_hex("#ff3d9a"), Some([0xFF, 0x3D, 0x9A]));
        assert_eq!(parse_hex("FF3D9A"), Some([0xFF, 0x3D, 0x9A]));
        assert_eq!(parse_hex("  #f0a "), Some([0xFF, 0x00, 0xAA]));
        for bad in ["", "#", "#12345", "#gg0000", "red", "#1234567"] {
            assert_eq!(parse_hex(bad), None, "{bad:?}");
        }
        assert_eq!(to_hex(rgb([255, 61, 154])), "#FF3D9A");
    }
}
