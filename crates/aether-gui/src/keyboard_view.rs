//! The on-screen keyboard: keycaps drawn from the layout file, click / Shift+click /
//! drag-rectangle selection, and overlays (actuation values, live effect colours).

use std::collections::BTreeSet;

use aether_device::Layout;
use aether_effects::Frame;
use aether_proto::{ActTable, raw_to_mm};
use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::theme;

pub enum Overlay<'a> {
    Plain,
    /// Configured keys tinted by actuation depth, value printed on the cap.
    Actuation(Option<&'a ActTable>),
    /// The colours currently streamed to the board.
    Colors(&'a Frame),
}

/// Teal (shallow, fast) to violet (deep) for 0.1..3.4 mm.
pub fn depth_color(mm: f64) -> Color32 {
    let t = ((mm - 0.1) / 3.3).clamp(0.0, 1.0) as f32;
    let (a, b) = ([45.0, 212.0, 191.0], [139.0, 92.0, 246.0]);
    let c = |i: usize| (a[i] + (b[i] - a[i]) * t) as u8;
    Color32::from_rgb(c(0), c(1), c(2))
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

fn readable_on(c: Color32) -> Color32 {
    let l = 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
    if l > 150.0 { Color32::from_rgb(12, 16, 22) } else { theme::TEXT }
}

/// Draw the keyboard and handle selection. Returns the index of the hovered key.
pub fn show(ui: &mut egui::Ui, layout: &Layout, selected: &mut BTreeSet<usize>, overlay: Overlay) -> Option<usize> {
    let keys = &layout.keys;
    let max_x = keys.iter().map(|k| k.x + k.width).fold(0.0, f32::max);
    let max_y = keys.iter().map(|k| k.y + k.height).fold(0.0, f32::max);
    let min_x = keys.iter().map(|k| k.x).fold(f32::MAX, f32::min);
    let min_y = keys.iter().map(|k| k.y).fold(f32::MAX, f32::min);
    let (w, h) = (max_x - min_x, max_y - min_y);
    let avail = ui.available_width();
    let s = (avail / (w + 8.0)).clamp(0.4, 1.5);
    let (area, resp) = ui.allocate_exact_size(Vec2::new(avail, h * s + 12.0), Sense::click_and_drag());
    let origin = area.min + Vec2::new((avail - w * s) / 2.0 - min_x * s, 6.0 - min_y * s);
    let key_rect = |k: &aether_device::Key| Rect::from_min_size(origin + Vec2::new(k.x, k.y) * s, Vec2::new(k.width, k.height) * s).shrink(2.0 * s);

    let pointer = resp.hover_pos();
    let hovered = pointer.and_then(|p| keys.iter().find(|k| key_rect(k).contains(p))).map(|k| k.index);
    let additive = ui.input(|i| i.modifiers.shift || i.modifiers.command);

    // Rectangle selection.
    let drag_id = resp.id.with("drag-start");
    if resp.drag_started() {
        if let Some(p) = resp.interact_pointer_pos() {
            ui.data_mut(|d| d.insert_temp(drag_id, p));
        }
    }
    let drag_start: Option<Pos2> = ui.data(|d| d.get_temp(drag_id));
    let drag_rect = match (drag_start, resp.dragged() || resp.drag_stopped(), pointer.or(resp.interact_pointer_pos())) {
        (Some(a), true, Some(b)) => Some(Rect::from_two_pos(a, b)),
        _ => None,
    };
    if resp.drag_stopped() {
        if let Some(r) = drag_rect.filter(|r| r.width() > 4.0 || r.height() > 4.0) {
            if !additive {
                selected.clear();
            }
            selected.extend(keys.iter().filter(|k| key_rect(k).intersects(r)).map(|k| k.index));
        }
        ui.data_mut(|d| d.remove::<Pos2>(drag_id));
    } else if resp.clicked() {
        match hovered {
            Some(i) if additive => {
                if !selected.remove(&i) {
                    selected.insert(i);
                }
            }
            Some(i) if selected.len() == 1 && selected.contains(&i) => selected.clear(),
            Some(i) => *selected = BTreeSet::from([i]),
            None if !additive => selected.clear(),
            None => {}
        }
    }

    let painter = ui.painter_at(area.expand(4.0));
    let r = CornerRadius::same((6.0 * s).round() as u8);
    for k in keys {
        let rect = key_rect(k);
        let sel = selected.contains(&k.index);
        let hov = hovered == Some(k.index) && drag_rect.is_none();
        let (mut face, mut label_col, mut value) = (theme::KEY_FACE, theme::TEXT, None::<(String, Color32)>);
        match &overlay {
            Overlay::Plain => {}
            Overlay::Colors(f) => {
                let [r, g, b] = f[k.index];
                face = Color32::from_rgb(r, g, b);
                label_col = readable_on(face);
            }
            Overlay::Actuation(t) => {
                if let Some(rec) = t.map(|t| t.record(k.index)).filter(|r| !r.is_empty()) {
                    let mm = raw_to_mm(rec.trip);
                    face = mix(theme::KEY_FACE, depth_color(mm), 0.55);
                    let txt = if rec.rapid_trigger() { format!("{mm:.2} RT") } else { format!("{mm:.2}") };
                    value = Some((txt, Color32::WHITE));
                }
            }
        }
        if hov {
            face = mix(face, Color32::WHITE, 0.10);
        }
        painter.rect_filled(rect.translate(Vec2::new(0.0, 2.5 * s)), r, theme::KEY_SHADOW);
        painter.rect_filled(rect, r, face);
        painter.rect_stroke(rect, r, Stroke::new(1.0, theme::KEY_EDGE), StrokeKind::Inside);
        let inset = 6.0 * s;
        painter.line_segment(
            [rect.left_top() + Vec2::new(inset, 1.5), rect.right_top() + Vec2::new(-inset, 1.5)],
            Stroke::new(1.0, Color32::from_white_alpha(22)),
        );
        if sel {
            painter.rect_stroke(rect.expand(1.0), r, Stroke::new(2.0, theme::ACCENT), StrokeKind::Outside);
        }
        let name_pos = if value.is_some() { rect.center() - Vec2::new(0.0, 7.0 * s) } else { rect.center() };
        let size = if k.name.chars().count() > 4 { 10.5 } else { 12.0 } * s.max(0.75);
        painter.text(name_pos, Align2::CENTER_CENTER, &k.name, FontId::new(size, theme::strong()), label_col);
        if let Some((txt, col)) = value {
            painter.text(rect.center() + Vec2::new(0.0, 9.0 * s), Align2::CENTER_CENTER, txt, FontId::proportional(10.0 * s.max(0.8)), col);
        }
    }
    if let Some(r) = drag_rect {
        painter.rect_filled(r, CornerRadius::same(4), theme::ACCENT.gamma_multiply(0.12));
        painter.rect_stroke(r, CornerRadius::same(4), Stroke::new(1.0, theme::ACCENT), StrokeKind::Inside);
    }
    hovered
}

/// Selection shortcuts under the keyboard.
pub fn quick_select(ui: &mut egui::Ui, layout: &Layout, selected: &mut BTreeSet<usize>) {
    let by_code = |f: &dyn Fn(&str) -> bool| -> BTreeSet<usize> {
        layout.keys.iter().filter(|k| f(&k.code)).map(|k| k.index).collect()
    };
    let groups: [(&str, BTreeSet<usize>); 6] = [
        ("All", layout.keys.iter().map(|k| k.index).collect()),
        ("WASD", by_code(&|c| matches!(c, "KeyW" | "KeyA" | "KeyS" | "KeyD"))),
        ("Letters", by_code(&|c| c.len() == 4 && c.starts_with("Key"))),
        ("Numbers", by_code(&|c| c.starts_with("Digit"))),
        ("Arrows", by_code(&|c| c.starts_with("Arrow"))),
        (
            "Modifiers",
            by_code(&|c| {
                ["Shift", "Control", "Alt", "Meta"].iter().any(|m| c.starts_with(m)) || matches!(c, "KeyFn" | "CapsLock" | "Tab" | "Space")
            }),
        ),
    ];
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("Select").color(theme::MUTED));
        for (name, set) in groups {
            if ui.small_button(name).clicked() {
                if ui.input(|i| i.modifiers.shift) {
                    selected.extend(set);
                } else {
                    *selected = set;
                }
            }
        }
        if ui.add_enabled(!selected.is_empty(), egui::Button::new("Clear").small()).clicked() {
            selected.clear();
        }
    });
}
