//! Visual theme: a cohesive dark (default) and light palette shared by UI chrome and canvases.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Visuals};

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    pub bg_canvas: Color32,
    pub bg_panel: Color32,
    pub bg_sidebar: Color32,
    pub bg_widget: Color32,
    pub bg_hover: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_weak: Color32,
    pub text_strong: Color32,
    pub accent: Color32,
    pub schematic: Color32,
    pub simulation: Color32,
    pub layout: Color32,
    pub danger: Color32,
    pub warning: Color32,
    pub ok: Color32,
    // canvas
    pub grid: Color32,
    pub grid_major: Color32,
    pub wire: Color32,
    pub symbol: Color32,
    pub pin: Color32,
    pub label: Color32,
    pub selection: Color32,
    pub highlight: Color32,
}

pub const DARK: Palette = Palette {
    dark: true,
    bg_canvas: Color32::from_rgb(0x12, 0x14, 0x18),
    bg_panel: Color32::from_rgb(0x1a, 0x1d, 0x23),
    bg_sidebar: Color32::from_rgb(0x15, 0x17, 0x1c),
    bg_widget: Color32::from_rgb(0x25, 0x29, 0x31),
    bg_hover: Color32::from_rgb(0x2e, 0x33, 0x3d),
    border: Color32::from_rgb(0x2b, 0x30, 0x39),
    text: Color32::from_rgb(0xd8, 0xdd, 0xe6),
    text_weak: Color32::from_rgb(0x87, 0x90, 0x9f),
    text_strong: Color32::from_rgb(0xf5, 0xf7, 0xfa),
    accent: Color32::from_rgb(0x4c, 0x9b, 0xff),
    schematic: Color32::from_rgb(0x4c, 0xc3, 0x8a),
    simulation: Color32::from_rgb(0xf2, 0xa9, 0x3b),
    layout: Color32::from_rgb(0xb5, 0x7b, 0xff),
    danger: Color32::from_rgb(0xf0, 0x5d, 0x5e),
    warning: Color32::from_rgb(0xf2, 0xc1, 0x4e),
    ok: Color32::from_rgb(0x4c, 0xc3, 0x8a),
    grid: Color32::from_rgb(0x26, 0x2a, 0x31),
    grid_major: Color32::from_rgb(0x33, 0x38, 0x41),
    wire: Color32::from_rgb(0x56, 0xd3, 0x9a),
    symbol: Color32::from_rgb(0xcf, 0xd6, 0xe0),
    pin: Color32::from_rgb(0xf0, 0x5d, 0x5e),
    label: Color32::from_rgb(0xf2, 0xc1, 0x4e),
    selection: Color32::from_rgb(0x4c, 0x9b, 0xff),
    highlight: Color32::from_rgb(0xff, 0x8a, 0x3d),
};

pub const LIGHT: Palette = Palette {
    dark: false,
    bg_canvas: Color32::from_rgb(0xfb, 0xfb, 0xfc),
    bg_panel: Color32::from_rgb(0xf3, 0xf4, 0xf6),
    bg_sidebar: Color32::from_rgb(0xe9, 0xeb, 0xef),
    bg_widget: Color32::from_rgb(0xe2, 0xe5, 0xea),
    bg_hover: Color32::from_rgb(0xd6, 0xdb, 0xe2),
    border: Color32::from_rgb(0xd3, 0xd7, 0xde),
    text: Color32::from_rgb(0x1d, 0x22, 0x2b),
    text_weak: Color32::from_rgb(0x5e, 0x65, 0x71),
    text_strong: Color32::from_rgb(0x0b, 0x0e, 0x13),
    accent: Color32::from_rgb(0x18, 0x5f, 0xcd),
    schematic: Color32::from_rgb(0x0f, 0x74, 0x42),
    simulation: Color32::from_rgb(0x98, 0x53, 0x00),
    layout: Color32::from_rgb(0x7d, 0x45, 0xcb),
    danger: Color32::from_rgb(0xc0, 0x2a, 0x2a),
    warning: Color32::from_rgb(0x84, 0x5e, 0x00),
    ok: Color32::from_rgb(0x0f, 0x74, 0x42),
    grid: Color32::from_rgb(0xe4, 0xe7, 0xeb),
    grid_major: Color32::from_rgb(0xcf, 0xd4, 0xdb),
    wire: Color32::from_rgb(0x12, 0x8a, 0x52),
    symbol: Color32::from_rgb(0x24, 0x2a, 0x35),
    pin: Color32::from_rgb(0xd6, 0x33, 0x33),
    label: Color32::from_rgb(0x8d, 0x59, 0x00),
    selection: Color32::from_rgb(0x18, 0x5f, 0xcd),
    highlight: Color32::from_rgb(0xe8, 0x6a, 0x10),
};

/// System fonts with Korean, Chinese and Japanese glyphs, most specific first. The first one
/// found is the last fallback of both families, so names, labels and copilot answers in those
/// scripts render instead of tofu.
const CJK_FONTS: &[&str] = &[
    "C:\\Windows\\Fonts\\malgun.ttf",
    "C:\\Windows\\Fonts\\gulim.ttc",
    "C:\\Windows\\Fonts\\msyh.ttc",
    "C:\\Windows\\Fonts\\YuGothR.ttc",
    "/System/Library/Fonts/AppleSDGothicNeo.ttc",
    "/System/Library/Fonts/Supplemental/AppleGothic.ttf",
    "/System/Library/Fonts/PingFang.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/nanum/NanumGothic.ttf",
];

/// Use the monospace font (Hack) as a fallback for UI text so arrows and geometric
/// symbols render instead of tofu, and a system CJK font after it when there is one.
pub fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mono: Vec<String> = fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    if let Some(prop) = fonts.families.get_mut(&FontFamily::Proportional) {
        for f in mono {
            if !prop.contains(&f) {
                prop.push(f);
            }
        }
    }
    if let Some(bytes) = CJK_FONTS.iter().find_map(|p| std::fs::read(p).ok()) {
        fonts.font_data.insert("system-cjk".into(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
        for fam in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts.families.entry(fam).or_default().push("system-cjk".into());
        }
    }
    ctx.set_fonts(fonts);
}

pub fn palette(ctx: &egui::Context) -> Palette {
    if ctx.global_style().visuals.dark_mode { DARK } else { LIGHT }
}

pub fn apply(ctx: &egui::Context, dark: bool) {
    let p = if dark { DARK } else { LIGHT };
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = p.bg_panel;
    v.window_fill = p.bg_panel;
    v.extreme_bg_color = if dark { Color32::from_rgb(0x10, 0x12, 0x16) } else { Color32::WHITE };
    v.faint_bg_color = if dark { Color32::from_rgb(0x1f, 0x23, 0x2a) } else { Color32::from_rgb(0xec, 0xee, 0xf1) };
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.hyperlink_color = p.accent;
    v.weak_text_color = Some(p.text_weak);
    v.selection.bg_fill = p.accent.gamma_multiply(if dark { 0.55 } else { 0.35 });
    v.selection.stroke = Stroke::new(1.0, p.text);
    let r = CornerRadius::same(5);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.bg_panel;
    w.noninteractive.weak_bg_fill = p.bg_panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r;
    w.inactive.bg_fill = p.bg_widget;
    w.inactive.weak_bg_fill = p.bg_widget;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = r;
    w.hovered.bg_fill = p.bg_hover;
    w.hovered.weak_bg_fill = p.bg_hover;
    w.hovered.bg_stroke = Stroke::new(1.0, p.border);
    w.hovered.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.corner_radius = r;
    w.active.bg_fill = p.accent.gamma_multiply(0.6);
    w.active.weak_bg_fill = p.accent.gamma_multiply(0.6);
    // egui derives `strong_text_color()` (RichText::strong) from the active widget's fg.
    w.active.fg_stroke = Stroke::new(1.0, p.text_strong);
    w.active.corner_radius = r;
    w.open.bg_fill = p.bg_hover;
    w.open.weak_bg_fill = p.bg_hover;
    w.open.fg_stroke = Stroke::new(1.0, p.text);
    w.open.corner_radius = r;

    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(8.0, 4.0);
        s.spacing.interact_size.y = 24.0;
        s.spacing.menu_margin = egui::Margin::same(6);
        s.text_styles.insert(TextStyle::Body, FontId::new(13.5, FontFamily::Proportional));
        s.text_styles.insert(TextStyle::Button, FontId::new(13.5, FontFamily::Proportional));
        s.text_styles.insert(TextStyle::Small, FontId::new(11.0, FontFamily::Proportional));
        s.text_styles.insert(TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace));
        s.text_styles.insert(TextStyle::Heading, FontId::new(20.0, FontFamily::Proportional));
    });
}

/// Text color for labels painted on a saturated fill (buttons, badges): whichever of
/// near-black or white has the higher contrast.
pub fn on_fill(fill: Color32) -> Color32 {
    let lin = |c: u8| {
        let v = c as f32 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    let l = 0.2126 * lin(fill.r()) + 0.7152 * lin(fill.g()) + 0.0722 * lin(fill.b());
    if (l + 0.05) / 0.06 > 1.05 / (l + 0.05) { DARK.bg_canvas } else { Color32::WHITE }
}

/// Small uppercase section header used in side panels.
pub fn section(ui: &mut egui::Ui, text: &str) {
    let p = palette(ui.ctx());
    ui.add_space(4.0);
    ui.label(egui::RichText::new(text.to_uppercase()).size(10.5).color(p.text_weak).strong());
    ui.add_space(2.0);
}
