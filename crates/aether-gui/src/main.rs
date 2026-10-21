//! `aether-gui`: desktop control for Aether keyboards. Profiles, lighting,
//! host effects, actuation presets, backups. All device I/O happens on the
//! worker thread (`worker.rs`); this file is only UI.

mod keyboard_view;
mod theme;
mod worker;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aether_device::{Backup, Board, EffectSpec, Layout, PRESETS, Profile, backups_dir, profiles_dir};
use aether_effects::Kind;
use aether_proto::{ActTable, Actuation, DeviceInfo, Lighting, raw_to_mm};
use eframe::egui::{self, Align, Align2, Color32, Layout as Lay, RichText, Vec2};
use keyboard_view::Overlay;
use serde::{Deserialize, Serialize};
use theme::{ACCENT, DANGER, MUTED, OK, WARN};
use worker::{Cmd, Event, Worker};

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Aether HE")
            .with_app_id("aether-he")
            .with_inner_size([1260.0, 800.0])
            .with_min_inner_size([1000.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native("aether-he", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Page {
    Profiles,
    Lighting,
    Effects,
    Actuation,
    Live,
    Backups,
    Device,
}

impl Page {
    const ALL: [(Page, &'static str); 7] = [
        (Page::Profiles, "Profiles"),
        (Page::Lighting, "Lighting"),
        (Page::Effects, "Effects"),
        (Page::Actuation, "Actuation"),
        (Page::Live, "Live depth"),
        (Page::Backups, "Backups"),
        (Page::Device, "Device"),
    ];
}

/// What survives a restart.
#[derive(Serialize, Deserialize)]
struct Saved {
    page: Page,
    fx: EffectSpec,
    active_profile: Option<String>,
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

struct Toast {
    text: String,
    error: bool,
    at: Instant,
}

enum Confirm {
    DeleteProfile(String),
    RestoreBackup(PathBuf, String),
    ResetAll,
}

struct ActForm {
    travel: f64,
    rt: bool,
    press: f64,
    release: f64,
    all_keys: bool,
}

struct App {
    worker: Worker,
    conn: Conn,
    page: Page,
    selected: BTreeSet<usize>,
    hovered: Option<usize>,
    lighting: Option<Lighting>,
    light_form: Option<Lighting>,
    table: Option<ActTable>,
    running: Option<EffectSpec>,
    fx: EffectSpec,
    act: ActForm,
    profiles: Vec<Profile>,
    active_profile: Option<String>,
    new_profile: String,
    include_effect: bool,
    renaming: Option<(String, String)>,
    confirm: Option<Confirm>,
    backups: Vec<(u64, PathBuf)>,
    toasts: Vec<Toast>,
    busy: bool,
    starters_done: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> App {
        theme::install(&cc.egui_ctx);
        let saved: Option<Saved> = cc.storage.and_then(|s| eframe::get_value(s, "state"));
        let (mut page, fx, active_profile) = match saved {
            Some(s) => (s.page, s.fx, s.active_profile),
            None => (Page::Profiles, default_fx(), None),
        };
        // AETHER_PAGE=actuation (etc.) opens on that page; handy for screenshots.
        if let Ok(want) = std::env::var("AETHER_PAGE") {
            if let Some((p, _)) = Page::ALL.iter().find(|(_, n)| n.to_lowercase().starts_with(&want.to_lowercase())) {
                page = *p;
            }
        }
        let mut app = App {
            worker: Worker::spawn(cc.egui_ctx.clone()),
            conn: Conn::Connecting,
            page,
            selected: BTreeSet::new(),
            hovered: None,
            lighting: None,
            light_form: None,
            table: None,
            running: None,
            fx,
            act: ActForm { travel: 1.2, rt: false, press: 0.15, release: 0.15, all_keys: false },
            profiles: Vec::new(),
            active_profile,
            new_profile: String::new(),
            include_effect: false,
            renaming: None,
            confirm: None,
            backups: Vec::new(),
            toasts: Vec::new(),
            busy: true,
            starters_done: false,
        };
        app.reload_profiles();
        app.reload_backups();
        app
    }

    fn send(&mut self, cmd: Cmd) {
        self.busy = true;
        self.worker.send(cmd);
    }

    fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toasts.push(Toast { text: text.into(), error, at: Instant::now() });
    }

    fn connected(&self) -> bool {
        matches!(self.conn, Conn::Up { .. })
    }

    fn board(&self) -> Option<&Board> {
        match &self.conn {
            Conn::Up { board, .. } => Some(board),
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

    fn poll(&mut self) {
        while let Ok(ev) = self.worker.events.try_recv() {
            match ev {
                Event::Connected { board, layout, info } => {
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
                }
                Event::Actuation(t) => self.table = Some(t),
                Event::EffectRunning(spec) => self.running = spec,
                Event::ProfileApplied(name) => self.active_profile = Some(name),
                Event::ProfilesChanged => self.reload_profiles(),
                Event::BackupsChanged => self.reload_backups(),
                Event::Ok(msg) => self.toast(msg, false),
                Event::Err(msg) => self.toast(msg, true),
                Event::Idle => self.busy = false,
            }
        }
    }

    // ------------------------------------------------------------ chrome ----

    fn nav(&mut self, ui: &mut egui::Ui) {
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.label(RichText::new("AETHER").family(theme::strong()).size(22.0).color(ACCENT));
                ui.label(RichText::new("Hall-effect keyboard control").size(11.5).color(MUTED));
            });
        });
        ui.add_space(22.0);
        for (page, name) in Page::ALL {
            let on = self.page == page;
            let text = RichText::new(name).size(14.5).color(if on { theme::TEXT } else { MUTED });
            let btn = egui::Button::new(text)
                .fill(if on { theme::RAISED } else { Color32::TRANSPARENT })
                .stroke(if on { egui::Stroke::new(1.0, theme::BORDER) } else { egui::Stroke::NONE })
                .min_size(Vec2::new(ui.available_width(), 38.0));
            let resp = ui.add(btn);
            if on {
                let r = resp.rect;
                ui.painter().rect_filled(egui::Rect::from_min_size(r.left_top() + Vec2::new(0.0, 9.0), Vec2::new(3.0, r.height() - 18.0)), 2.0, ACCENT);
            }
            if resp.clicked() {
                self.page = page;
            }
        }
        ui.with_layout(Lay::bottom_up(Align::LEFT), |ui| {
            ui.add_space(14.0);
            if ui.add_enabled(self.running.is_none() && !self.busy, egui::Button::new("Reconnect").min_size(Vec2::new(ui.available_width(), 30.0))).clicked() {
                self.conn = Conn::Connecting;
                self.send(Cmd::Connect);
            }
            match &self.conn {
                Conn::Up { .. } => theme::pill(ui, "Connected", OK),
                Conn::Connecting => theme::pill(ui, "Connecting…", WARN),
                Conn::Down(_) => theme::pill(ui, "No keyboard", DANGER),
            }
        });
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            match &self.conn {
                Conn::Up { board, info, .. } => {
                    ui.label(RichText::new(&board.name).family(theme::strong()).size(22.0));
                    ui.label(RichText::new(format!("firmware V{:X}.{:02X}", info.firmware >> 8, info.firmware & 0xFF)).color(MUTED));
                }
                Conn::Connecting => {
                    ui.label(RichText::new("Looking for your keyboard…").size(20.0));
                }
                Conn::Down(_) => {
                    ui.label(RichText::new("No keyboard connected").size(20.0).color(DANGER));
                }
            }
            if self.busy {
                ui.add(egui::Spinner::new().size(16.0).color(ACCENT));
            }
            ui.with_layout(Lay::right_to_left(Align::Center), |ui| {
                let label = self.active_profile.clone().unwrap_or_else(|| "Custom".into());
                let mut pick = None;
                egui::ComboBox::from_id_salt("profile-switch").selected_text(RichText::new(label).family(theme::strong())).width(190.0).show_ui(ui, |ui| {
                    for p in &self.profiles {
                        if ui.selectable_label(self.active_profile.as_deref() == Some(&p.name), &p.name).clicked() {
                            pick = Some(p.clone());
                        }
                    }
                });
                ui.label(RichText::new("Profile").color(MUTED));
                if let Some(p) = pick.filter(|_| self.connected()) {
                    self.send(Cmd::ApplyProfile(p));
                }
                if let Some(spec) = &self.running {
                    theme::pill(ui, &format!("Effect: {}", cap(&spec.kind)), ACCENT);
                }
            });
        });
    }

    fn toasts(&mut self, ctx: &egui::Context) {
        self.toasts.retain(|t| t.at.elapsed() < Duration::from_secs(if t.error { 8 } else { 4 }));
        if self.toasts.is_empty() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(250));
        egui::Area::new(egui::Id::unique("toasts")).anchor(Align2::RIGHT_BOTTOM, Vec2::new(-20.0, -20.0)).order(egui::Order::Foreground).show(ctx, |ui| {
            for t in self.toasts.iter().rev().take(4) {
                let col = if t.error { DANGER } else { OK };
                egui::Frame::new()
                    .fill(theme::RAISED)
                    .stroke(egui::Stroke::new(1.0, col.gamma_multiply(0.6)))
                    .corner_radius(10)
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_max_width(420.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(if t.error { "Error" } else { "Done" }).color(col).family(theme::strong()));
                            ui.label(&t.text);
                        });
                    });
                ui.add_space(6.0);
            }
        });
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(c) = &self.confirm else { return };
        let (title, body, action) = match c {
            Confirm::DeleteProfile(n) => ("Delete profile?", format!("\"{n}\" will be deleted. This can't be undone."), "Delete"),
            Confirm::RestoreBackup(_, when) => ("Restore backup?", format!("The keyboard's lighting and actuation go back to the backup from {when}. A backup of the current state is taken first."), "Restore"),
            Confirm::ResetAll => ("Reset all keys?", "Every key goes back to the firmware default actuation. A backup is taken first.".into(), "Reset all"),
        };
        let mut decision = None;
        egui::Modal::new(egui::Id::unique("confirm")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(RichText::new(title).family(theme::strong()).size(17.0));
            ui.add_space(4.0);
            ui.label(RichText::new(body).color(MUTED));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.add(theme::danger(action)).clicked() {
                    decision = Some(true);
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    decision = Some(false);
                }
            });
        });
        match decision {
            Some(true) => match self.confirm.take() {
                Some(Confirm::DeleteProfile(n)) => {
                    match profiles_dir().and_then(|d| Profile::delete_in(&d, &n)) {
                        Ok(()) => self.toast(format!("Deleted \"{n}\""), false),
                        Err(e) => self.toast(e.to_string(), true),
                    }
                    if self.active_profile.as_deref() == Some(n.as_str()) {
                        self.active_profile = None;
                    }
                    self.reload_profiles();
                }
                Some(Confirm::RestoreBackup(p, _)) => {
                    self.active_profile = None;
                    self.send(Cmd::Restore(p));
                }
                Some(Confirm::ResetAll) => self.write_actuation(true, Actuation::Default),
                None => {}
            },
            Some(false) => self.confirm = None,
            None => {}
        }
    }

    /// The keyboard card, with the quick-select row and the hovered key's details.
    fn keyboard_card(&mut self, ui: &mut egui::Ui, overlay_kind: u8) {
        let Conn::Up { layout, .. } = &self.conn else {
            theme::card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Plug in the keyboard, close the Aula web driver if it's open, then press Reconnect.").color(MUTED));
            });
            return;
        };
        let layout = layout.clone();
        let live = self.worker.live.lock().ok().and_then(|g| *g);
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            let overlay = match (overlay_kind, &live) {
                (1, _) => Overlay::Actuation(self.table.as_ref()),
                (2, Some(f)) => Overlay::Colors(f),
                _ => Overlay::Plain,
            };
            self.hovered = keyboard_view::show(ui, &layout, &mut self.selected, overlay);
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                keyboard_view::quick_select(ui, &layout, &mut self.selected);
                ui.with_layout(Lay::right_to_left(Align::Center), |ui| {
                    if let Some(k) = self.hovered.and_then(|i| layout.key(i)) {
                        let rec = self.table.as_ref().map(|t| t.record(k.index));
                        let info = match rec {
                            Some(r) if r.is_empty() => "firmware default".to_string(),
                            Some(r) if r.rapid_trigger() => format!("{:.2} mm · RT {:.2}/{:.2}", raw_to_mm(r.trip), raw_to_mm(r.press), raw_to_mm(r.release)),
                            Some(r) => format!("{:.2} mm", raw_to_mm(r.trip)),
                            None => String::new(),
                        };
                        ui.label(RichText::new(info).color(MUTED));
                        ui.label(RichText::new(&k.name).family(theme::strong()));
                    } else {
                        ui.label(RichText::new(format!("{} selected", self.selected.len())).color(MUTED));
                    }
                });
            });
        });
    }

    // ------------------------------------------------------------- pages ----

    fn page_profiles(&mut self, ui: &mut egui::Ui) {
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::title(ui, "New profile", "Saves the keyboard's current lighting and every key's actuation under a name.");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.new_profile).hint_text("Profile name").desired_width(240.0));
                ui.checkbox(&mut self.include_effect, format!("Include effect ({})", cap(&self.fx.kind)));
                let ok = self.connected() && !self.new_profile.trim().is_empty();
                let save = ui.add_enabled(ok, theme::primary("Save current setup")).clicked()
                    || (ok && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)));
                if save {
                    if let Ok(dir) = profiles_dir() {
                        let name = self.new_profile.trim().to_string();
                        let effect = self.include_effect.then(|| self.fx.clone());
                        self.send(Cmd::SaveProfile { name: name.clone(), effect, dir });
                        self.active_profile = Some(name);
                        self.new_profile.clear();
                    }
                }
            });
        });
        ui.add_space(12.0);
        let cols = ((ui.available_width() + 12.0) / 300.0).floor().max(1.0) as usize;
        let profiles = self.profiles.clone();
        let mut action: Option<(String, u8)> = None;
        egui::Grid::new("profiles").num_columns(cols).spacing(Vec2::splat(12.0)).show(ui, |ui| {
            for (i, p) in profiles.iter().enumerate() {
                let active = self.active_profile.as_deref() == Some(&p.name);
                let frame = theme::card().stroke(egui::Stroke::new(if active { 1.5 } else { 1.0 }, if active { ACCENT } else { theme::BORDER }));
                frame.show(ui, |ui| ui.vertical(|ui| {
                    ui.set_width(268.0);
                    ui.horizontal(|ui| {
                        if let Some((old, new)) = self.renaming.as_mut().filter(|(o, _)| *o == p.name) {
                            let r = ui.add(egui::TextEdit::singleline(new).desired_width(150.0));
                            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                action = Some((format!("{old}\u{0}{new}"), 5));
                            }
                            if ui.small_button("Save").clicked() {
                                action = Some((format!("{old}\u{0}{new}"), 5));
                            }
                        } else {
                            ui.label(RichText::new(&p.name).family(theme::strong()).size(16.0));
                        }
                        if active {
                            theme::pill(ui, "Active", ACCENT);
                        }
                    });
                    let t = p.actuation_table().unwrap_or_default();
                    let set: Vec<_> = (0..aether_proto::KEY_SLOTS).map(|i| t.record(i)).filter(|r| !r.is_empty()).collect();
                    let act = if set.is_empty() {
                        "All keys on firmware default".to_string()
                    } else {
                        let rt = set.iter().filter(|r| r.rapid_trigger()).count();
                        let lo = set.iter().map(|r| r.trip).min().unwrap_or(0);
                        let hi = set.iter().map(|r| r.trip).max().unwrap_or(0);
                        let range = if lo == hi { format!("{:.2} mm", raw_to_mm(lo)) } else { format!("{:.2}–{:.2} mm", raw_to_mm(lo), raw_to_mm(hi)) };
                        format!("{} keys at {range}{}", set.len(), if rt > 0 { format!(", {rt} with RT") } else { String::new() })
                    };
                    let mode = self.board().and_then(|b| b.lighting.mode(p.lighting.mode)).map_or("Custom".into(), |m| m.name.clone());
                    ui.label(RichText::new(act).color(MUTED));
                    ui.label(RichText::new(format!("Lighting: {mode}")).color(MUTED));
                    ui.label(RichText::new(match &p.effect {
                        Some(e) => format!("Effect: {}", cap(&e.kind)),
                        None => "No host effect".into(),
                    }).color(MUTED));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.add_enabled(self.connected(), theme::primary("Apply")).clicked() {
                            action = Some((p.name.clone(), 0));
                        }
                        ui.menu_button("More", |ui| {
                            if ui.add_enabled(self.connected(), egui::Button::new("Update from current board")).clicked() {
                                action = Some((p.name.clone(), 1));
                            }
                            if ui.button("Set effect to current effect settings").clicked() {
                                action = Some((p.name.clone(), 6));
                            }
                            if ui.add_enabled(p.effect.is_some(), egui::Button::new("Remove effect")).clicked() {
                                action = Some((p.name.clone(), 7));
                            }
                            if ui.button("Rename").clicked() {
                                action = Some((p.name.clone(), 2));
                            }
                            if ui.button("Duplicate").clicked() {
                                action = Some((p.name.clone(), 3));
                            }
                            ui.separator();
                            if ui.add(theme::danger("Delete")).clicked() {
                                action = Some((p.name.clone(), 4));
                            }
                        });
                    });
                }));
                if (i + 1) % cols == 0 {
                    ui.end_row();
                }
            }
        });
        let Some((name, what)) = action else { return };
        let Ok(dir) = profiles_dir() else { return };
        let find = |n: &str| self.profiles.iter().find(|p| p.name == n).cloned();
        match what {
            0 => {
                if let Some(p) = find(&name) {
                    self.send(Cmd::ApplyProfile(p));
                }
            }
            1 => {
                let effect = find(&name).and_then(|p| p.effect);
                self.send(Cmd::SaveProfile { name, effect, dir });
            }
            2 => self.renaming = Some((name.clone(), name)),
            3 => {
                if let Some(mut p) = find(&name) {
                    let mut n = 2;
                    while self.profiles.iter().any(|q| q.name.eq_ignore_ascii_case(&format!("{name} {n}"))) {
                        n += 1;
                    }
                    p.name = format!("{name} {n}");
                    let _ = p.save_in(&dir).map_err(|e| self.toast(e.to_string(), true));
                    self.reload_profiles();
                }
            }
            4 => self.confirm = Some(Confirm::DeleteProfile(name)),
            5 => {
                let (old, new) = name.split_once('\u{0}').map(|(a, b)| (a.to_string(), b.trim().to_string())).unwrap_or_default();
                self.renaming = None;
                if !new.is_empty() && new != old {
                    match Profile::rename_in(&dir, &old, &new) {
                        Ok(_) => {
                            if self.active_profile.as_deref() == Some(old.as_str()) {
                                self.active_profile = Some(new);
                            }
                        }
                        Err(e) => self.toast(e.to_string(), true),
                    }
                    self.reload_profiles();
                }
            }
            6 | 7 => {
                if let Some(mut p) = find(&name) {
                    p.effect = (what == 6).then(|| self.fx.clone());
                    match p.save_in(&dir) {
                        Ok(_) => self.toast(format!("Updated effect of \"{name}\""), false),
                        Err(e) => self.toast(e.to_string(), true),
                    }
                    self.reload_profiles();
                }
            }
            _ => {}
        }
    }

    fn page_lighting(&mut self, ui: &mut egui::Ui) {
        self.keyboard_card(ui, 0);
        ui.add_space(12.0);
        let (Some(board), Some(mut l)) = (self.board().cloned(), self.light_form) else { return };
        let info = &board.lighting;
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::title(ui, "Firmware lighting", "Stored on the keyboard; works without this app running.");
            egui::Grid::new("light").num_columns(2).spacing(Vec2::new(16.0, 10.0)).show(ui, |ui| {
                ui.label("Effect");
                let cur = info.mode(l.mode).map_or("Custom".to_string(), |m| m.name.clone());
                egui::ComboBox::from_id_salt("mode").selected_text(cur).height(420.0).show_ui(ui, |ui| {
                    for m in &info.modes {
                        if ui.selectable_label(l.mode == m.byte, &m.name).clicked() {
                            l.mode = m.byte;
                            l.color_mode = u8::from(!m.color);
                        }
                    }
                });
                ui.end_row();
                let mode = info.mode(l.mode);
                ui.label("Colour");
                if mode.is_some_and(|m| m.color) {
                    if ui.color_edit_button_srgb(&mut l.rgb).changed() {
                        l.alpha = 0xFF;
                        l.color_mode = 0;
                    }
                } else {
                    ui.label(RichText::new("This effect picks its own colours").color(MUTED));
                }
                ui.end_row();
                ui.label("Brightness");
                ui.add(egui::Slider::new(&mut l.brightness, 0..=info.brightness_max));
                ui.end_row();
                if mode.is_none_or(|m| m.speed) {
                    ui.label("Speed");
                    ui.add(egui::Slider::new(&mut l.speed, 0..=info.speed_max));
                    ui.end_row();
                }
            });
            self.light_form = Some(l);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let changed = self.lighting != Some(l);
                if ui.add_enabled(changed && !self.busy, theme::primary("Apply")).clicked() {
                    self.active_profile = None;
                    self.send(Cmd::SetLighting(l));
                }
                if ui.add_enabled(changed, egui::Button::new("Revert")).clicked() {
                    self.light_form = self.lighting;
                }
                if ui.add_enabled(!self.busy, egui::Button::new("Re-read")).on_hover_text("Read the lighting from the keyboard again").clicked() {
                    self.send(Cmd::ReadLighting);
                }
                if self.running.is_some() {
                    ui.label(RichText::new("Applying firmware lighting stops the running effect.").color(WARN));
                }
            });
        });
    }

    fn page_effects(&mut self, ui: &mut egui::Ui) {
        self.keyboard_card(ui, 2);
        ui.add_space(12.0);
        let running = self.running.is_some();
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::title(ui, "Host effects", "Animated by this app and streamed to the keys at 28 fps. They stop when the app closes.");
            ui.add_enabled_ui(!running, |ui| {
                egui::Grid::new("fx").num_columns(2).spacing(Vec2::new(16.0, 10.0)).show(ui, |ui| {
                    ui.label("Effect");
                    egui::ComboBox::from_id_salt("fx-kind").selected_text(cap(&self.fx.kind)).show_ui(ui, |ui| {
                        for k in Kind::ALL {
                            ui.selectable_value(&mut self.fx.kind, k.name().to_string(), cap(k.name()));
                        }
                    });
                    ui.end_row();
                    ui.label("Colours");
                    ui.horizontal(|ui| {
                        let mut remove = None;
                        for (i, c) in self.fx.palette.iter_mut().enumerate() {
                            ui.color_edit_button_srgb(c).context_menu(|ui| {
                                if ui.button("Remove colour").clicked() {
                                    remove = Some(i);
                                }
                            });
                        }
                        if let Some(i) = remove.filter(|_| self.fx.palette.len() > 1) {
                            self.fx.palette.remove(i);
                        }
                        if self.fx.palette.len() < 8 && ui.small_button("+").on_hover_text("Add a colour").clicked() {
                            self.fx.palette.push([255, 255, 255]);
                        }
                        if self.fx.palette.len() > 1 && ui.small_button("-").on_hover_text("Remove the last colour").clicked() {
                            self.fx.palette.pop();
                        }
                    });
                    ui.end_row();
                    ui.label("Background");
                    ui.color_edit_button_srgb(&mut self.fx.bg);
                    ui.end_row();
                    ui.label("Speed");
                    ui.add(egui::Slider::new(&mut self.fx.speed, 0.0..=1.0).show_value(false));
                    ui.end_row();
                    ui.label("Brightness");
                    ui.add(egui::Slider::new(&mut self.fx.brightness, 0.0..=1.0).show_value(false));
                    ui.end_row();
                    ui.label("Direction");
                    ui.horizontal(|ui| {
                        for (i, n) in ["Right", "Left", "Up", "Down"].into_iter().enumerate() {
                            ui.selectable_value(&mut self.fx.direction, i as u8, n);
                        }
                    });
                    ui.end_row();
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if running {
                    if ui.add(theme::danger("Stop effect")).clicked() {
                        self.send(Cmd::StopEffect);
                    }
                } else if ui.add_enabled(self.connected() && !self.busy, theme::primary("Start effect")).clicked() {
                    self.send(Cmd::StartEffect(self.fx.clone()));
                }
                if ui.button("Reset to defaults").clicked() && !running {
                    self.fx = default_fx();
                }
            });
        });
    }

    fn write_actuation(&mut self, all: bool, setting: Actuation) {
        let keys: Vec<usize> = match (&self.conn, all) {
            (Conn::Up { layout, .. }, true) => layout.keys.iter().map(|k| k.index).collect(),
            _ => self.selected.iter().copied().collect(),
        };
        if keys.is_empty() {
            self.toast("Select keys first, or choose All keys", true);
            return;
        }
        self.active_profile = None;
        self.send(Cmd::SetActuation { keys, setting });
    }

    fn page_actuation(&mut self, ui: &mut egui::Ui) {
        self.keyboard_card(ui, 1);
        ui.add_space(12.0);
        let Some(board) = self.board().cloned() else { return };
        // Side by side when there's room, stacked on narrow (tiled) windows.
        if ui.available_width() >= 1000.0 {
            ui.columns(2, |cols| {
                self.presets_card(&mut cols[0]);
                self.custom_card(&mut cols[1], &board);
            });
        } else {
            self.presets_card(ui);
            ui.add_space(12.0);
            self.custom_card(ui, &board);
        }
    }

    fn presets_card(&mut self, ui: &mut egui::Ui) {
        let n = self.selected.len();
        let can = !self.busy;
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::title(ui, "Quick presets", "One click, saved to the keyboard (a backup is taken first).");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.act.all_keys, false, format!("Selected keys ({n})"));
                ui.selectable_value(&mut self.act.all_keys, true, "All keys");
            });
            ui.add_space(4.0);
            for (name, setting) in PRESETS {
                let desc = match setting {
                    Actuation::Fixed { travel_mm } => format!("{travel_mm:.2} mm"),
                    Actuation::RapidTrigger { travel_mm, press_mm, .. } => format!("{travel_mm:.2} mm + rapid trigger {press_mm:.2}"),
                    Actuation::Default => "firmware default".into(),
                };
                ui.horizontal(|ui| {
                    let ok = can && (self.act.all_keys || n > 0);
                    let b = egui::Button::new(RichText::new(name).family(theme::strong())).min_size(Vec2::new(130.0, 30.0));
                    if ui.add_enabled(ok, b).on_disabled_hover_text("Select keys first, or choose All keys").clicked() {
                        self.write_actuation(self.act.all_keys, setting);
                    }
                    ui.label(RichText::new(desc).color(MUTED));
                });
            }
        });
    }

    fn custom_card(&mut self, ui: &mut egui::Ui, board: &Board) {
        let lim = &board.actuation;
        let n = self.selected.len();
        let can = !self.busy;
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::title(ui, "Custom", "Fine-tune, then apply to the selection or the whole board.");
            // Leave room for the label column and the value box.
            ui.spacing_mut().slider_width = (ui.available_width() - 200.0).clamp(120.0, 320.0);
            egui::Grid::new("act").num_columns(2).spacing(Vec2::new(16.0, 10.0)).show(ui, |ui| {
                ui.label("Actuation");
                ui.add(egui::Slider::new(&mut self.act.travel, lim.travel_mm[0]..=lim.travel_mm[1]).step_by(0.01).suffix(" mm"));
                ui.end_row();
                ui.label("Rapid trigger");
                ui.add_enabled(board.capabilities.rapid_trigger, egui::Checkbox::without_text(&mut self.act.rt));
                ui.end_row();
                ui.add_enabled_ui(self.act.rt, |ui| ui.label("Press"));
                ui.add_enabled(self.act.rt, egui::Slider::new(&mut self.act.press, lim.rt_mm[0]..=lim.rt_mm[1]).step_by(0.01).suffix(" mm"));
                ui.end_row();
                ui.add_enabled_ui(self.act.rt, |ui| ui.label("Release"));
                ui.add_enabled(self.act.rt, egui::Slider::new(&mut self.act.release, lim.rt_mm[0]..=lim.rt_mm[1]).step_by(0.01).suffix(" mm"));
                ui.end_row();
            });
            // Slider steps accumulate float error; keep the 0.01 mm the board stores.
            for v in [&mut self.act.travel, &mut self.act.press, &mut self.act.release] {
                *v = (*v * 100.0).round() / 100.0;
            }
            ui.add_space(6.0);
            let setting = if self.act.rt {
                Actuation::RapidTrigger { travel_mm: self.act.travel, press_mm: self.act.press, release_mm: self.act.release }
            } else {
                Actuation::Fixed { travel_mm: self.act.travel }
            };
            ui.horizontal_wrapped(|ui| {
                let all = self.act.all_keys;
                let label = if all { "Apply to all keys".to_string() } else { format!("Apply to {n} key{}", if n == 1 { "" } else { "s" }) };
                if ui.add_enabled(can && (all || n > 0), theme::primary(&label)).on_disabled_hover_text("Select keys first, or choose All keys").clicked() {
                    self.write_actuation(all, setting);
                }
                if ui.add_enabled(can && n > 0, egui::Button::new("Reset selected")).on_hover_text("Back to the firmware default").clicked() {
                    self.write_actuation(false, Actuation::Default);
                }
                if ui.add_enabled(can, theme::danger("Reset all")).clicked() {
                    self.confirm = Some(Confirm::ResetAll);
                }
                if ui.add_enabled(can, egui::Button::new("Read")).on_hover_text("Read the actuation from the keyboard again").clicked() {
                    self.send(Cmd::ReadActuation);
                }
            });
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Colour scale").color(MUTED).size(12.0));
                for mm in [0.1, 1.0, 2.0, 3.4] {
                    let (r, _) = ui.allocate_exact_size(Vec2::new(14.0, 14.0), egui::Sense::hover());
                    ui.painter().rect_filled(r, 3.0, keyboard_view::depth_color(mm));
                    ui.label(RichText::new(format!("{mm} mm")).color(MUTED).size(12.0));
                }
            });
        });
    }

    fn page_live(&mut self, ui: &mut egui::Ui) {
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::title(ui, "Live key depth", "");
            ui.label("This view will show how far each key is pressed, in real time, with your actuation point marked.");
            ui.add_space(4.0);
            ui.label(RichText::new(
                "It needs the keyboard's travel-test mode, which hasn't been verified on the F75 HE yet. \
                 It will be enabled after a short, guarded test on your board.",
            ).color(MUTED));
        });
    }

    fn page_backups(&mut self, ui: &mut egui::Ui) {
        let board = self.board().map(|b| b.slug.clone());
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::title(ui, "Backups", "Taken automatically before every change. Restoring one is verified by reading the keyboard back.");
            ui.horizontal(|ui| {
                if ui.add_enabled(self.connected() && !self.busy, theme::primary("Back up now")).clicked() {
                    self.send(Cmd::Backup);
                }
                let oldest = board.as_deref().and_then(|b| Backup::oldest(b).ok().flatten());
                if ui.add_enabled(self.connected() && oldest.is_some() && !self.busy, egui::Button::new("Restore original"))
                    .on_hover_text("The keyboard as it was before Aether first changed it")
                    .clicked()
                {
                    if let Some(p) = oldest {
                        self.confirm = Some(Confirm::RestoreBackup(p, "before Aether first changed the keyboard".into()));
                    }
                }
                if ui.button("Open folder").clicked() {
                    if let Ok(d) = backups_dir() {
                        let _ = std::process::Command::new("xdg-open").arg(d).spawn();
                    }
                }
                ui.label(RichText::new(format!("{} backups", self.backups.len())).color(MUTED));
            });
        });
        ui.add_space(12.0);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let mut restore = None;
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical().max_height(ui.available_height().max(240.0)).show(ui, |ui| {
                if self.backups.is_empty() {
                    ui.label(RichText::new("No backups yet.").color(MUTED));
                }
                for (t, p) in &self.backups {
                    ui.horizontal(|ui| {
                        let when = ago(now.saturating_sub(*t));
                        ui.label(RichText::new(&when).family(theme::strong()));
                        ui.label(RichText::new(p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()).color(MUTED).size(12.0));
                        ui.with_layout(Lay::right_to_left(Align::Center), |ui| {
                            if ui.add_enabled(self.connected() && !self.busy, egui::Button::new("Restore").small()).clicked() {
                                restore = Some((p.clone(), when));
                            }
                        });
                    });
                    ui.separator();
                }
            });
        });
        if let Some((p, when)) = restore {
            self.confirm = Some(Confirm::RestoreBackup(p, when));
        }
    }

    fn page_device(&mut self, ui: &mut egui::Ui) {
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            match &self.conn {
                Conn::Up { board, layout, info } => {
                    theme::title(ui, &board.name, "");
                    egui::Grid::new("dev").num_columns(2).spacing(Vec2::new(24.0, 8.0)).show(ui, |ui| {
                        let row = |ui: &mut egui::Ui, k: &str, v: String| {
                            ui.label(RichText::new(k).color(MUTED));
                            ui.label(v);
                            ui.end_row();
                        };
                        row(ui, "USB ID", format!("{:04X}:{:04X}", info.vid, info.pid));
                        row(ui, "Firmware", format!("V{:X}.{:02X}", info.firmware >> 8, info.firmware & 0xFF));
                        row(ui, "Keys", layout.keys.len().to_string());
                        row(ui, "Host effects", format!("up to {} fps", board.lighting.host_fps));
                        row(ui, "Actuation range", format!("{:.2}–{:.2} mm, RT {:.2}–{:.2} mm", board.actuation.travel_mm[0], board.actuation.travel_mm[1], board.actuation.rt_mm[0], board.actuation.rt_mm[1]));
                        let c = &board.capabilities;
                        let yes = |b: bool| if b { "yes" } else { "no" };
                        row(ui, "Verified features", format!("lighting {}, host effects {}, actuation {}, rapid trigger {}", yes(c.lighting), yes(c.host_effects), yes(c.actuation), yes(c.rapid_trigger)));
                    });
                }
                Conn::Connecting => {
                    ui.label("Connecting…");
                }
                Conn::Down(why) => {
                    theme::title(ui, "No keyboard", "");
                    ui.label(RichText::new(why).color(DANGER));
                    ui.add_space(6.0);
                    ui.label("Check that:");
                    ui.label("• the keyboard is plugged in with the cable (not the 2.4 GHz dongle),");
                    ui.label("• the Aula web driver isn't open in a browser,");
                    ui.label("• the udev rule is installed (./scripts/install.sh), then replug the keyboard.");
                }
            }
        });
        ui.add_space(12.0);
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::title(ui, "Files", "");
            for (name, dir) in [("Profiles", profiles_dir()), ("Backups", backups_dir())] {
                if let Ok(d) = dir {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(name).color(MUTED));
                        ui.label(d.display().to_string());
                        if ui.small_button("Open").clicked() {
                            let _ = std::fs::create_dir_all(&d);
                            let _ = std::process::Command::new("xdg-open").arg(&d).spawn();
                        }
                    });
                }
            }
        });
    }
}

/// "rain" -> "Rain"
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

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        let ctx = ui.ctx().clone();
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) && self.confirm.is_none() {
            self.selected.clear();
        }
        if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::A)) && !ctx.egui_wants_keyboard_input() {
            if let Conn::Up { layout, .. } = &self.conn {
                self.selected = layout.keys.iter().map(|k| k.index).collect();
            }
        }
        egui::Panel::left("nav")
            .exact_size(214.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(12, 0)))
            .show(ui, |ui| self.nav(ui));
        egui::CentralPanel::default_margins()
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::same(24)))
            .show(ui, |ui| {
                self.header(ui);
                ui.add_space(16.0);
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.page {
                    Page::Profiles => self.page_profiles(ui),
                    Page::Lighting => self.page_lighting(ui),
                    Page::Effects => self.page_effects(ui),
                    Page::Actuation => self.page_actuation(ui),
                    Page::Live => self.page_live(ui),
                    Page::Backups => self.page_backups(ui),
                    Page::Device => self.page_device(ui),
                });
            });
        self.confirm_dialog(&ctx);
        self.toasts(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let s = Saved { page: self.page, fx: self.fx.clone(), active_profile: self.active_profile.clone() };
        eframe::set_value(storage, "state", &s);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.worker.shutdown();
    }
}
