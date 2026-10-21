//! `aether-gui`: a simple window for the connected keyboard. A keyboard view on
//! top, and Lighting / Effects / Actuation / Backups tabs. Every change saves a
//! backup first; actuation writes are verified by read-back.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use aether_device::{Backup, Board, Keyboard, Layout, backups_dir};
use aether_effects::{Direction, Engine, Frame, KeyPos, Kind, Params};
use aether_proto::{ActTable, Actuation, DeviceInfo, KEY_SLOTS, Lighting, raw_to_mm};
use eframe::egui::{self, Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, Vec2};
use serde::{Deserialize, Serialize};

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Aether HE")
            .with_app_id("aether-he")
            .with_inner_size([1180.0, 720.0])
            .with_min_inner_size([900.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native("aether-he", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

type SharedKb = Arc<Mutex<Box<dyn Keyboard>>>;

fn lock(kb: &SharedKb) -> MutexGuard<'_, Box<dyn Keyboard>> {
    kb.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Lighting,
    Effects,
    Actuation,
    Backups,
}

/// Effect choices, remembered between runs.
#[derive(Clone, Serialize, Deserialize)]
struct FxSettings {
    kind: String,
    palette: Vec<[u8; 3]>,
    bg: [u8; 3],
    speed: f32,
    brightness: f32,
    direction: u8,
}

impl Default for FxSettings {
    fn default() -> Self {
        let p = Params::default();
        FxSettings { kind: p.kind.name().into(), palette: p.palette, bg: p.bg, speed: p.speed, brightness: p.brightness, direction: 0 }
    }
}

impl FxSettings {
    fn params(&self) -> Params {
        Params {
            kind: Kind::from_name(&self.kind).unwrap_or(Kind::Wave),
            palette: self.palette.clone(),
            bg: self.bg,
            speed: self.speed,
            brightness: self.brightness,
            direction: [Direction::Right, Direction::Left, Direction::Up, Direction::Down][self.direction.min(3) as usize],
        }
    }
}

struct Connected {
    kb: SharedKb,
    board: Board,
    layout: Layout,
    info: DeviceInfo,
}

struct Runner {
    stop: Arc<AtomicBool>,
    live: Arc<Mutex<Frame>>,
    handle: JoinHandle<Result<(), String>>,
}

struct App {
    dev: Option<Connected>,
    status: String,
    error: bool,
    tab: Tab,
    selected: BTreeSet<usize>,
    light: Option<Lighting>,
    fx: FxSettings,
    runner: Option<Runner>,
    table: Option<ActTable>,
    travel: f64,
    rt: bool,
    press: f64,
    release: f64,
    backups: Vec<PathBuf>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> App {
        let fx = cc.storage.and_then(|s| eframe::get_value(s, "fx")).unwrap_or_default();
        let mut app = App {
            dev: None,
            status: String::new(),
            error: false,
            tab: Tab::Lighting,
            selected: BTreeSet::new(),
            light: None,
            fx,
            runner: None,
            table: None,
            travel: 2.0,
            rt: false,
            press: 0.3,
            release: 0.3,
            backups: Vec::new(),
        };
        app.connect();
        app
    }

    fn ok(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.error = false;
    }

    fn fail(&mut self, msg: impl std::fmt::Display) {
        self.status = msg.to_string();
        self.error = true;
    }

    fn connect(&mut self) {
        self.stop_effect();
        self.dev = None;
        match aether_device::connect() {
            Ok((kb, info)) => {
                let board = kb.board().clone();
                let layout = kb.layout().clone();
                self.dev = Some(Connected { kb: Arc::new(Mutex::new(kb)), board, layout, info });
                self.ok("Connected");
                self.read_lighting();
                self.read_actuation();
                self.refresh_backups();
            }
            Err(e) => self.fail(format!("Not connected: {e}")),
        }
    }

    fn streaming(&self) -> bool {
        self.runner.is_some()
    }

    /// Run `f` on the keyboard, reporting errors in the status line.
    fn with_kb<T>(&mut self, f: impl FnOnce(&mut dyn Keyboard) -> aether_device::Result<T>) -> Option<T> {
        let kb = self.dev.as_ref()?.kb.clone();
        let r = f(lock(&kb).as_mut());
        r.map_err(|e| self.fail(e)).ok()
    }

    /// Save a backup, then run the write `f`.
    fn write<T>(&mut self, what: &str, f: impl FnOnce(&mut dyn Keyboard) -> aether_device::Result<T>) -> Option<T> {
        let r = self.with_kb(|kb| {
            Backup::snapshot(kb)?;
            f(kb)
        });
        if r.is_some() {
            self.ok(format!("{what} (backup saved)"));
            self.refresh_backups();
        }
        r
    }

    fn read_lighting(&mut self) {
        self.light = self.with_kb(|kb| kb.lighting());
    }

    fn read_actuation(&mut self) {
        self.table = self.with_kb(|kb| kb.actuation_table());
    }

    fn refresh_backups(&mut self) {
        let mut v: Vec<PathBuf> = backups_dir()
            .ok()
            .and_then(|d| std::fs::read_dir(d).ok())
            .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "json")).collect())
            .unwrap_or_default();
        v.sort();
        v.reverse();
        self.backups = v;
    }

    fn start_effect(&mut self) {
        let Some(dev) = &self.dev else { return };
        let kb = dev.kb.clone();
        let fps = dev.board.lighting.host_fps;
        let keys: Vec<KeyPos> =
            dev.layout.keys.iter().map(|k| KeyPos { index: k.index, x: k.x + k.width / 2.0, y: k.y + k.height / 2.0 }).collect();
        let params = self.fx.params();
        let stop = Arc::new(AtomicBool::new(false));
        let live = Arc::new(Mutex::new([[0u8; 3]; KEY_SLOTS]));
        let (stop2, live2) = (stop.clone(), live.clone());
        let handle = std::thread::spawn(move || -> Result<(), String> {
            let mut kb = lock(&kb);
            let saved = kb.lighting().map_err(|e| e.to_string())?;
            kb.stream_begin().map_err(|e| e.to_string())?;
            let mut engine = Engine::new(&keys, params, fps);
            let res = aether_effects::run(
                &mut engine,
                fps,
                || !stop2.load(Ordering::Relaxed),
                |f| {
                    *live2.lock().unwrap_or_else(|e| e.into_inner()) = *f;
                    kb.stream_frame(f)
                },
            );
            // Always hand the LEDs back to the firmware effect.
            let end = kb.stream_end();
            std::thread::sleep(Duration::from_millis(200));
            let restore = kb.set_lighting(&saved);
            res.and(end).and(restore).map_err(|e| e.to_string())
        });
        self.runner = Some(Runner { stop, live, handle });
        self.ok(format!("Running {}", self.fx.kind));
    }

    fn stop_effect(&mut self) {
        if let Some(r) = self.runner.take() {
            r.stop.store(true, Ordering::Relaxed);
            match r.handle.join() {
                Ok(Ok(())) => self.ok("Effect stopped, lighting restored"),
                Ok(Err(e)) => self.fail(format!("Effect: {e}")),
                Err(_) => self.fail("Effect thread crashed"),
            }
        }
    }

    /// The effect thread ended on its own (e.g. the board was unplugged).
    fn reap_effect(&mut self) {
        if self.runner.as_ref().is_some_and(|r| r.handle.is_finished()) {
            self.stop_effect();
        }
    }

    // ---------------------------------------------------------------- UI ----

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Aether HE");
            ui.separator();
            match &self.dev {
                Some(d) => {
                    ui.label(format!("{}  ·  firmware V{:X}.{:02X}", d.board.name, d.info.firmware >> 8, d.info.firmware & 0xFF));
                    ui.colored_label(Color32::from_rgb(80, 200, 120), "Connected");
                }
                None => {
                    ui.colored_label(Color32::from_rgb(230, 90, 80), "No keyboard");
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(!self.streaming(), egui::Button::new("Reconnect")).clicked() {
                    self.connect();
                }
            });
        });
    }

    fn keyboard(&mut self, ui: &mut egui::Ui) {
        let Some(dev) = &self.dev else {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label("Plug in the keyboard, close the Aula web driver if it is open, then press Reconnect.");
                ui.label("If it still does not connect, install the udev rule: see scripts/build.sh output.");
            });
            return;
        };
        let keys = &dev.layout.keys;
        let max_x = keys.iter().map(|k| k.x + k.width).fold(0.0, f32::max);
        let max_y = keys.iter().map(|k| k.y + k.height).fold(0.0, f32::max);
        let avail = ui.available_width();
        let scale = (avail / (max_x + 20.0)).min(1.6);
        let (area, _) = ui.allocate_exact_size(Vec2::new(avail, (max_y + 20.0) * scale), Sense::hover());
        let origin = area.min + Vec2::new((avail - max_x * scale) / 2.0, 10.0 * scale);

        let live = self.runner.as_ref().map(|r| *r.live.lock().unwrap_or_else(|e| e.into_inner()));
        let shift = ui.input(|i| i.modifiers.shift || i.modifiers.command);
        let mut clicked = None;
        for k in keys {
            let rect = Rect::from_min_size(origin + Vec2::new(k.x, k.y) * scale, Vec2::new(k.width, k.height) * scale).shrink(1.5);
            let resp = ui.interact(rect, ui.scope_id().with(("key", k.index)), Sense::click());
            if resp.clicked() {
                clicked = Some(k.index);
            }
            let selected = self.selected.contains(&k.index);
            let fill = match live {
                Some(f) => Color32::from_rgb(f[k.index][0], f[k.index][1], f[k.index][2]),
                None if selected => Color32::from_rgb(52, 92, 160),
                None if resp.hovered() => Color32::from_gray(70),
                None => Color32::from_gray(48),
            };
            let p = ui.painter();
            p.rect_filled(rect, 5.0, fill);
            let stroke = if selected { Stroke::new(2.0, Color32::from_rgb(120, 180, 255)) } else { Stroke::new(1.0, Color32::from_gray(25)) };
            p.rect_stroke(rect, 5.0, stroke, StrokeKind::Inside);
            let text_col = if live.is_some() && (fill.r() as u32 + fill.g() as u32 + fill.b() as u32) > 380 { Color32::BLACK } else { Color32::from_gray(230) };
            p.text(rect.center() - Vec2::new(0.0, 5.0 * scale), Align2::CENTER_CENTER, &k.name, FontId::proportional(11.0 * scale.max(0.8)), text_col);
            if self.tab == Tab::Actuation && live.is_none() {
                let r = self.table.as_ref().map(|t| t.record(k.index));
                let label = match r {
                    Some(r) if !r.is_empty() => format!("{:.2}{}", raw_to_mm(r.trip), if r.rapid_trigger() { " RT" } else { "" }),
                    Some(_) => "def".into(),
                    None => String::new(),
                };
                p.text(rect.center() + Vec2::new(0.0, 9.0 * scale), Align2::CENTER_CENTER, label, FontId::proportional(9.0 * scale.max(0.8)), Color32::from_rgb(160, 200, 255));
            }
        }
        if let Some(i) = clicked {
            if shift {
                if !self.selected.remove(&i) {
                    self.selected.insert(i);
                }
            } else if self.selected.len() == 1 && self.selected.contains(&i) {
                self.selected.clear();
            } else {
                self.selected = BTreeSet::from([i]);
            }
        }
        ui.horizontal(|ui| {
            ui.label(format!("{} selected", self.selected.len()));
            if ui.small_button("Select all").clicked() {
                self.selected = keys.iter().map(|k| k.index).collect();
            }
            if ui.small_button("Clear").clicked() {
                self.selected.clear();
            }
            ui.weak("Click a key to select it, Shift+click to add more.");
        });
    }

    fn lighting_tab(&mut self, ui: &mut egui::Ui) {
        let Some(info) = self.dev.as_ref().map(|d| d.board.lighting.clone()) else { return };
        let Some(mut l) = self.light else {
            if ui.button("Read lighting").clicked() {
                self.read_lighting();
            }
            return;
        };
        let current = info.mode(l.mode).map_or("Custom".to_string(), |m| m.name.clone());
        egui::ComboBox::from_label("Mode").selected_text(current).height(400.0).show_ui(ui, |ui| {
            for m in &info.modes {
                if ui.selectable_label(l.mode == m.byte, &m.name).clicked() {
                    l.mode = m.byte;
                    l.color_mode = if m.color { 0 } else { 1 };
                }
            }
        });
        let mode = info.mode(l.mode);
        if mode.is_some_and(|m| m.color) {
            ui.horizontal(|ui| {
                ui.label("Colour");
                if ui.color_edit_button_srgb(&mut l.rgb).changed() {
                    l.alpha = 0xFF;
                    l.color_mode = 0;
                }
            });
        } else {
            ui.weak("This effect picks its own colours.");
        }
        let mut b = l.brightness;
        ui.add(egui::Slider::new(&mut b, 0..=info.brightness_max).text("Brightness"));
        l.brightness = b;
        if mode.is_none_or(|m| m.speed) {
            let mut s = l.speed;
            ui.add(egui::Slider::new(&mut s, 0..=info.speed_max).text("Speed"));
            l.speed = s;
        }
        self.light = Some(l);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(!self.streaming(), egui::Button::new("Apply")).clicked() {
                self.write("Lighting applied", |kb| kb.set_lighting(&l));
            }
            if ui.add_enabled(!self.streaming(), egui::Button::new("Re-read")).clicked() {
                self.read_lighting();
            }
        });
        ui.add_space(6.0);
        ui.weak("Note: the board reports its stored colour, which can differ from what a fixed-colour effect shows.");
    }

    fn effects_tab(&mut self, ui: &mut egui::Ui) {
        let running = self.streaming();
        ui.add_enabled_ui(!running, |ui| {
            egui::ComboBox::from_label("Effect").selected_text(&self.fx.kind).show_ui(ui, |ui| {
                for k in Kind::ALL {
                    ui.selectable_value(&mut self.fx.kind, k.name().to_string(), k.name());
                }
            });
            ui.horizontal(|ui| {
                ui.label("Colours");
                let mut remove = None;
                for (i, c) in self.fx.palette.iter_mut().enumerate() {
                    ui.color_edit_button_srgb(c).context_menu(|ui| {
                        if ui.button("Remove").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove.filter(|_| self.fx.palette.len() > 1) {
                    self.fx.palette.remove(i);
                }
                if self.fx.palette.len() < 6 && ui.small_button("+").clicked() {
                    self.fx.palette.push([255, 255, 255]);
                }
                if self.fx.palette.len() > 1 && ui.small_button("-").clicked() {
                    self.fx.palette.pop();
                }
            });
            ui.horizontal(|ui| {
                ui.label("Background");
                ui.color_edit_button_srgb(&mut self.fx.bg);
            });
            ui.add(egui::Slider::new(&mut self.fx.speed, 0.0..=1.0).text("Speed"));
            ui.add(egui::Slider::new(&mut self.fx.brightness, 0.0..=1.0).text("Brightness"));
            egui::ComboBox::from_label("Direction")
                .selected_text(["Right", "Left", "Up", "Down"][self.fx.direction.min(3) as usize])
                .show_ui(ui, |ui| {
                    for (i, n) in ["Right", "Left", "Up", "Down"].into_iter().enumerate() {
                        ui.selectable_value(&mut self.fx.direction, i as u8, n);
                    }
                });
        });
        ui.add_space(8.0);
        if running {
            if ui.button("Stop").clicked() {
                self.stop_effect();
            }
            ui.weak("Effects are streamed from this app; closing it stops them.");
        } else if ui.add_enabled(self.dev.is_some(), egui::Button::new("Start")).clicked() {
            self.start_effect();
        }
    }

    fn actuation_tab(&mut self, ui: &mut egui::Ui) {
        let Some(lim) = self.dev.as_ref().map(|d| d.board.actuation.clone()) else { return };
        let rt_ok = self.dev.as_ref().is_some_and(|d| d.board.capabilities.rapid_trigger);
        ui.add(egui::Slider::new(&mut self.travel, lim.travel_mm[0]..=lim.travel_mm[1]).step_by(0.01).text("Actuation (mm)"));
        if rt_ok {
            ui.checkbox(&mut self.rt, "Rapid trigger");
            ui.add_enabled_ui(self.rt, |ui| {
                ui.add(egui::Slider::new(&mut self.press, lim.rt_mm[0]..=lim.rt_mm[1]).step_by(0.01).text("Press (mm)"));
                ui.add(egui::Slider::new(&mut self.release, lim.rt_mm[0]..=lim.rt_mm[1]).step_by(0.01).text("Release (mm)"));
            });
        }
        // Slider steps accumulate float error (0.1 + 330 * 0.01 = 3.4000000000000004);
        // keep exactly the 0.01 mm values the board stores.
        for v in [&mut self.travel, &mut self.press, &mut self.release] {
            *v = (*v * 100.0).round() / 100.0;
        }
        ui.add_space(8.0);
        let n = self.selected.len();
        ui.horizontal(|ui| {
            let apply = egui::Button::new(format!("Apply to {n} key{}", if n == 1 { "" } else { "s" }));
            if ui.add_enabled(n > 0 && !self.streaming(), apply).clicked() {
                let keys: Vec<usize> = self.selected.iter().copied().collect();
                let setting = if self.rt {
                    Actuation::RapidTrigger { travel_mm: self.travel, press_mm: self.press, release_mm: self.release }
                } else {
                    Actuation::Fixed { travel_mm: self.travel }
                };
                let valid = self.with_kb(|kb| kb.validate_actuation(&keys, setting)).is_some();
                if valid && self.write(&format!("Actuation set on {n} key(s), verified"), |kb| kb.set_actuation(&keys, setting)).is_some() {
                    self.read_actuation();
                }
            }
            if ui.add_enabled(!self.streaming(), egui::Button::new("Read")).clicked() {
                self.read_actuation();
                if !self.error {
                    self.ok("Actuation read");
                }
            }
        });
        if n == 0 {
            ui.weak("Select keys on the keyboard above.");
        }
        ui.weak("\"def\" = never configured, the firmware default applies.");
    }

    fn backups_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.add_enabled(self.dev.is_some() && !self.streaming(), egui::Button::new("Back up now")).clicked() {
                if let Some(p) = self.with_kb(|kb| Backup::snapshot(kb)) {
                    self.ok(format!("Saved {}", p.display()));
                    self.refresh_backups();
                }
            }
            if ui.button("Refresh").clicked() {
                self.refresh_backups();
            }
        });
        ui.add_space(6.0);
        let mut restore = None;
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            if self.backups.is_empty() {
                ui.weak("No backups yet.");
            }
            for p in &self.backups {
                ui.horizontal(|ui| {
                    if ui.add_enabled(self.dev.is_some() && !self.runner.is_some(), egui::Button::new("Restore")).clicked() {
                        restore = Some(p.clone());
                    }
                    ui.label(p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
                });
            }
        });
        if let Some(p) = restore {
            let r = self.with_kb(|kb| Backup::load(&p)?.restore(kb));
            if r.is_some() {
                self.ok(format!("Restored {} (verified)", p.display()));
                self.read_lighting();
                self.read_actuation();
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.reap_effect();
        if self.streaming() {
            ui.ctx().request_repaint_after(Duration::from_millis(33));
        }
        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            self.top_bar(ui);
            ui.add_space(4.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            let col = if self.error { Color32::from_rgb(230, 110, 100) } else { Color32::from_gray(170) };
            ui.colored_label(col, &self.status);
        });
        egui::Panel::left("tabs").exact_size(330.0).resizable(false).show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Lighting, "Lighting");
                ui.selectable_value(&mut self.tab, Tab::Effects, "Effects");
                ui.selectable_value(&mut self.tab, Tab::Actuation, "Actuation");
                ui.selectable_value(&mut self.tab, Tab::Backups, "Backups");
            });
            ui.separator();
            if self.dev.is_none() && self.tab != Tab::Backups {
                ui.weak("Connect a keyboard to use this tab.");
                return;
            }
            match self.tab {
                Tab::Lighting => self.lighting_tab(ui),
                Tab::Effects => self.effects_tab(ui),
                Tab::Actuation => self.actuation_tab(ui),
                Tab::Backups => self.backups_tab(ui),
            }
        });
        egui::CentralPanel::default_margins().show(ui, |ui| self.keyboard(ui));
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "fx", &self.fx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop_effect();
    }
}
