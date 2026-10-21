//! Look and feel: one dark palette with a teal accent, Inter, 8/12/16 spacing.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin, RichText, Shadow,
    Stroke, TextStyle, Vec2,
};

pub const BG: Color32 = Color32::from_rgb(13, 17, 23);
pub const PANEL: Color32 = Color32::from_rgb(18, 23, 31);
pub const CARD: Color32 = Color32::from_rgb(24, 30, 40);
pub const RAISED: Color32 = Color32::from_rgb(33, 41, 54);
pub const BORDER: Color32 = Color32::from_rgb(42, 51, 66);
pub const TEXT: Color32 = Color32::from_rgb(220, 226, 234);
pub const MUTED: Color32 = Color32::from_rgb(138, 150, 166);
pub const ACCENT: Color32 = Color32::from_rgb(45, 212, 191);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(19, 78, 74);
pub const OK: Color32 = Color32::from_rgb(74, 222, 128);
pub const WARN: Color32 = Color32::from_rgb(251, 191, 36);
pub const DANGER: Color32 = Color32::from_rgb(248, 113, 113);

pub const KEY_FACE: Color32 = Color32::from_rgb(44, 53, 68);
pub const KEY_EDGE: Color32 = Color32::from_rgb(28, 34, 45);
pub const KEY_SHADOW: Color32 = Color32::from_rgb(8, 10, 14);

/// Bold-ish family for headings and key labels.
pub fn strong() -> FontFamily {
    FontFamily::Name("semibold".into())
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("inter".into(), Arc::new(FontData::from_static(include_bytes!("../assets/inter-400.ttf"))));
    fonts.font_data.insert("inter-sb".into(), Arc::new(FontData::from_static(include_bytes!("../assets/inter-600.ttf"))));
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "inter".into());
    let mut sb = vec!["inter-sb".to_string()];
    sb.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    fonts.families.insert(strong(), sb);
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::new(20.0, strong())),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
        let s = &mut style.spacing;
        s.item_spacing = Vec2::new(8.0, 8.0);
        s.button_padding = Vec2::new(12.0, 6.0);
        s.interact_size.y = 30.0;
        s.slider_width = 200.0;
        s.combo_width = 220.0;

        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.panel_fill = BG;
        v.window_fill = CARD;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.extreme_bg_color = PANEL;
        v.faint_bg_color = CARD;
        v.override_text_color = Some(TEXT);
        v.window_corner_radius = CornerRadius::same(12);
        v.menu_corner_radius = CornerRadius::same(10);
        v.popup_shadow = Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(120) };
        v.window_shadow = v.popup_shadow;
        v.selection.bg_fill = ACCENT_DIM;
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.hyperlink_color = ACCENT;
        v.slider_trailing_fill = true;
        let r = CornerRadius::same(8);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = CARD;
        w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        for (wv, fill, stroke) in [
            (&mut w.inactive, RAISED, BORDER),
            (&mut w.hovered, Color32::from_rgb(44, 54, 70), Color32::from_rgb(64, 76, 96)),
            (&mut w.active, ACCENT_DIM, ACCENT),
            (&mut w.open, RAISED, ACCENT),
        ] {
            wv.bg_fill = fill;
            wv.weak_bg_fill = fill;
            wv.bg_stroke = Stroke::new(1.0, stroke);
            wv.corner_radius = r;
            wv.fg_stroke = Stroke::new(1.0, TEXT);
            wv.expansion = 0.0;
        }
    });
}

/// A rounded content card.
pub fn card() -> Frame {
    Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(16))
}

/// Card title with an optional muted subtitle.
pub fn title(ui: &mut egui::Ui, text: &str, sub: &str) {
    ui.label(RichText::new(text).family(strong()).size(16.0));
    if !sub.is_empty() {
        ui.label(RichText::new(sub).color(MUTED).size(12.5));
    }
    ui.add_space(4.0);
}

/// The filled accent button used for the main action of a card.
pub fn primary(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_string()).color(Color32::from_rgb(4, 30, 28)).family(strong()))
        .fill(ACCENT)
        .stroke(Stroke::NONE)
}

pub fn danger(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_string()).color(DANGER)).stroke(Stroke::new(1.0, Color32::from_rgb(110, 50, 50)))
}

/// Small rounded pill (status, tags).
pub fn pill(ui: &mut egui::Ui, text: &str, color: Color32) {
    Frame::new()
        .fill(color.gamma_multiply(0.16))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(Margin::symmetric(10, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(color).size(12.0));
        });
}
