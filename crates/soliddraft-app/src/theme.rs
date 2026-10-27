//! The Workshop look: light, dense, SolidWorks-faithful. Design tokens come
//! from the interface colour palette; everything egui draws
//! is derived from them here so panels, dock and custom widgets agree.

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle, Vec2};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(0xf7, 0xf8, 0xfa);
pub const PANEL: Color32 = Color32::WHITE;
pub const INK: Color32 = Color32::from_rgb(0x1b, 0x22, 0x30);
pub const MUTED: Color32 = Color32::from_rgb(0x6b, 0x74, 0x83);
pub const LINE: Color32 = Color32::from_rgb(0xd9, 0xdd, 0xe4);
pub const ACCENT: Color32 = Color32::from_rgb(0x1f, 0x5f, 0xd0);
pub const ACCENT_SOFT: Color32 = Color32::from_rgb(0xe6, 0xee, 0xfc);
pub const SELECTION_BG: Color32 = Color32::from_rgb(0xd7, 0xe3, 0xfb);
pub const SELECTION_INK: Color32 = Color32::from_rgb(0x0f, 0x2a, 0x66);
pub const HOVER: Color32 = Color32::from_rgb(0xe9, 0xed, 0xf3);
pub const OK: Color32 = Color32::from_rgb(0x2e, 0x9e, 0x5b);
pub const WARN: Color32 = Color32::from_rgb(0xc7, 0x7d, 0x12);
pub const ERROR: Color32 = Color32::from_rgb(0xc8, 0x3a, 0x3a);
pub const FIELD_BG: Color32 = Color32::from_rgb(0xfb, 0xfc, 0xfe);
/// The origin axes, in the viewport, the triad and the feature tree.
pub const AXIS_X: Color32 = Color32::from_rgb(0xd6, 0x45, 0x45);
pub const AXIS_Y: Color32 = Color32::from_rgb(0x2e, 0x9e, 0x5b);
pub const AXIS_Z: Color32 = Color32::from_rgb(0x1f, 0x5f, 0xd0);

pub fn axis_color(axis: crate::document::AxisKind) -> Color32 {
    match axis {
        crate::document::AxisKind::X => AXIS_X,
        crate::document::AxisKind::Y => AXIS_Y,
        crate::document::AxisKind::Z => AXIS_Z,
    }
}

// The command line and the assistant session, with the Studio look's light values. The
// names are the facelift palette's, so its mode-aware accessors take their place.
/// Tool chips: the brightest surface.
pub fn chip() -> Color32 {
    Color32::WHITE
}
/// The command line's ground.
pub fn command_line() -> Color32 {
    Color32::from_rgb(0xf7, 0xf8, 0xfa)
}
/// The command line's prompt, and the chip row's lit chips.
pub fn command_prompt() -> Color32 {
    Color32::from_rgb(0x25, 0x63, 0xeb)
}
/// The session's dark ground.
pub fn terminal_bg() -> Color32 {
    Color32::from_rgb(0x14, 0x18, 0x1f)
}
/// The session's text.
pub fn terminal_ink() -> Color32 {
    Color32::from_rgb(0xe6, 0xe9, 0xee)
}
/// The session's prompt, its own lines and its proposal cards.
pub fn terminal_accent() -> Color32 {
    Color32::from_rgb(0x7a, 0xa2, 0xff)
}
/// The session's quiet text: notes, tool lines, the header.
pub fn terminal_muted() -> Color32 {
    terminal_ink().gamma_multiply(0.6)
}
/// The session's separator lines.
pub fn terminal_line() -> Color32 {
    terminal_ink().gamma_multiply(0.16)
}

/// Corner radius used by buttons, fields and cards.
pub const RADIUS: u8 = 6;

pub const MENU_HEIGHT: f32 = 30.0;
pub const RIBBON_TABS_HEIGHT: f32 = 26.0;
pub const RIBBON_BODY_HEIGHT: f32 = 92.0;
pub const STATUS_HEIGHT: f32 = 24.0;
pub const COMMAND_LINE_HEIGHT: f32 = 32.0;
pub const ROW_HEIGHT: f32 = 23.0;

pub const FONT_REGULAR: &str = "Inter";
pub const FONT_MEDIUM: &str = "InterMedium";
pub const FONT_SEMIBOLD: &str = "InterSemiBold";

pub fn medium() -> FontFamily {
    FontFamily::Name(Arc::from(FONT_MEDIUM))
}

pub fn semibold() -> FontFamily {
    FontFamily::Name(Arc::from(FONT_SEMIBOLD))
}

pub fn font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn font_medium(size: f32) -> FontId {
    FontId::new(size, medium())
}

pub fn font_semibold(size: f32) -> FontId {
    FontId::new(size, semibold())
}

/// The command line's and the session's typeface.
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

pub fn hud_bg() -> Color32 {
    Color32::from_rgba_unmultiplied(255, 255, 255, 235)
}

pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    ctx.set_theme(egui::Theme::Light);
    ctx.all_styles_mut(apply_style);
}

fn fonts() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        FONT_REGULAR.to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!("../assets/fonts/Inter-Regular.ttf"))),
    );
    fonts.font_data.insert(
        FONT_MEDIUM.to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!("../assets/fonts/Inter-Medium.ttf"))),
    );
    fonts.font_data.insert(
        FONT_SEMIBOLD.to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!("../assets/fonts/Inter-SemiBold.ttf"))),
    );
    // Keep egui's bundled fonts as fallbacks for glyphs Inter lacks (symbols, emoji).
    let fallbacks = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let with = |first: &str| {
        let mut v = vec![first.to_owned()];
        v.extend(fallbacks.iter().cloned());
        v
    };
    fonts.families.insert(FontFamily::Proportional, with(FONT_REGULAR));
    fonts.families.insert(medium(), with(FONT_MEDIUM));
    fonts.families.insert(semibold(), with(FONT_SEMIBOLD));
    fonts
}

fn apply_style(style: &mut egui::Style) {
    style.text_styles = [
        (TextStyle::Small, FontId::new(10.5, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(12.5, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(12.0, FontFamily::Proportional)),
        (TextStyle::Heading, FontId::new(13.5, semibold())),
        (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
    ]
    .into();

    let spacing = &mut style.spacing;
    spacing.item_spacing = Vec2::new(6.0, 4.0);
    spacing.button_padding = Vec2::new(8.0, 3.0);
    spacing.interact_size = Vec2::new(40.0, 22.0);
    spacing.indent = 16.0;
    spacing.icon_width = 14.0;
    spacing.icon_width_inner = 9.0;
    spacing.menu_margin = Margin::same(6);
    spacing.window_margin = Margin::same(10);
    spacing.combo_width = 120.0;
    spacing.text_edit_width = 160.0;
    spacing.scroll.bar_width = 6.0;
    spacing.scroll.floating = true;
    spacing.scroll.handle_min_length = 24.0;
    spacing.tooltip_width = 320.0;

    style.interaction.selectable_labels = false;
    style.interaction.tooltip_delay = 0.35;
    style.animation_time = 0.08;
    style.visuals = visuals();
}

fn visuals() -> egui::Visuals {
    let mut v = egui::Visuals::light();
    let r = CornerRadius::same(RADIUS);
    v.override_text_color = None;
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.window_stroke = Stroke::new(1.0, LINE);
    v.window_corner_radius = CornerRadius::same(8);
    v.window_shadow = egui::Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(28) };
    v.popup_shadow = egui::Shadow { offset: [0, 4], blur: 12, spread: 0, color: Color32::from_black_alpha(24) };
    v.menu_corner_radius = CornerRadius::same(8);
    v.extreme_bg_color = FIELD_BG;
    v.faint_bg_color = BG;
    v.code_bg_color = BG;
    v.hyperlink_color = ACCENT;
    v.warn_fg_color = WARN;
    v.error_fg_color = ERROR;
    v.selection.bg_fill = SELECTION_BG;
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.text_cursor.stroke = Stroke::new(1.5, ACCENT);
    v.striped = false;
    v.button_frame = true;
    v.collapsing_header_frame = false;
    v.indent_has_left_vline = false;
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    v.resize_corner_size = 10.0;
    v.weak_text_alpha = 0.6;

    let w = &mut v.widgets;
    w.noninteractive = egui::style::WidgetVisuals {
        weak_bg_fill: PANEL,
        bg_fill: PANEL,
        bg_stroke: Stroke::new(1.0, LINE),
        fg_stroke: Stroke::new(1.0, INK),
        corner_radius: r,
        expansion: 0.0,
    };
    w.inactive = egui::style::WidgetVisuals {
        weak_bg_fill: BG,
        bg_fill: BG,
        bg_stroke: Stroke::new(1.0, LINE),
        fg_stroke: Stroke::new(1.0, INK),
        corner_radius: r,
        expansion: 0.0,
    };
    w.hovered = egui::style::WidgetVisuals {
        weak_bg_fill: HOVER,
        bg_fill: HOVER,
        bg_stroke: Stroke::new(1.0, Color32::from_rgb(0xc4, 0xcb, 0xd6)),
        fg_stroke: Stroke::new(1.0, INK),
        corner_radius: r,
        expansion: 0.0,
    };
    w.active = egui::style::WidgetVisuals {
        weak_bg_fill: SELECTION_BG,
        bg_fill: SELECTION_BG,
        bg_stroke: Stroke::new(1.0, ACCENT),
        fg_stroke: Stroke::new(1.0, SELECTION_INK),
        corner_radius: r,
        expansion: 0.0,
    };
    w.open = egui::style::WidgetVisuals {
        weak_bg_fill: ACCENT_SOFT,
        bg_fill: ACCENT_SOFT,
        bg_stroke: Stroke::new(1.0, LINE),
        fg_stroke: Stroke::new(1.0, INK),
        corner_radius: r,
        expansion: 0.0,
    };
    v
}

/// egui_dock style matched to the panel chrome: a thin tab strip, no body
/// frame, one-pixel separators that light up in the accent when hovered.
pub fn dock_style(ctx: &egui::Context) -> egui_dock::Style {
    let mut s = egui_dock::Style::from_egui(&ctx.global_style());
    s.dock_area_padding = None;
    s.main_surface_border_stroke = Stroke::NONE;
    s.main_surface_border_rounding = CornerRadius::ZERO;

    s.separator.width = 1.0;
    s.separator.extra_interact_width = 5.0;
    s.separator.extra = 0.0;
    s.separator.color_idle = LINE;
    s.separator.color_hovered = ACCENT;
    s.separator.color_dragged = ACCENT;

    s.tab_bar.bg_fill = PANEL;
    s.tab_bar.height = 28.0;
    s.tab_bar.hline_color = LINE;
    s.tab_bar.fill_tab_bar = false;
    s.tab_bar.corner_radius = CornerRadius::ZERO;
    s.tab_bar.inner_margin = Margin::symmetric(6, 0);

    let base = egui_dock::TabInteractionStyle {
        outline_color: Color32::TRANSPARENT,
        corner_radius: CornerRadius::ZERO,
        bg_fill: PANEL,
        text_color: MUTED,
    };
    s.tab.inactive = base.clone();
    s.tab.hovered = egui_dock::TabInteractionStyle { text_color: INK, ..base.clone() };
    s.tab.inactive_with_kb_focus = base.clone();
    let active = egui_dock::TabInteractionStyle { text_color: INK, ..base };
    s.tab.active = active.clone();
    s.tab.focused = active.clone();
    s.tab.active_with_kb_focus = active.clone();
    s.tab.focused_with_kb_focus = active;
    s.tab.hline_below_active_tab_name = true;
    s.tab.spacing = 10.0;
    s.tab.minimum_width = None;
    s.tab.tab_body.inner_margin = Margin::ZERO;
    s.tab.tab_body.stroke = Stroke::NONE;
    s.tab.tab_body.corner_radius = CornerRadius::ZERO;
    s.tab.tab_body.bg_fill = PANEL;

    s.overlay.selection_color = Color32::from_rgba_unmultiplied(0x1f, 0x5f, 0xd0, 60);
    s.overlay.selection_stroke_width = 1.0;
    s.overlay.button_color = ACCENT;
    s.overlay.button_border_stroke = Stroke::new(1.0, ACCENT);
    s.overlay.hovered_leaf_highlight.color = Color32::from_rgba_unmultiplied(0x1f, 0x5f, 0xd0, 24);
    s.overlay.hovered_leaf_highlight.stroke = Stroke::new(1.0, ACCENT);
    s.buttons.close_tab_color = MUTED;
    s.buttons.close_tab_active_color = INK;
    s.buttons.close_tab_bg_fill = HOVER;
    s.buttons.add_tab_color = MUTED;
    s.buttons.add_tab_active_color = INK;
    s.buttons.add_tab_bg_fill = HOVER;
    s.buttons.add_tab_border_color = LINE;
    s
}

/// A 1-pixel separator line across the full width of `ui`.
pub fn hline(ui: &mut egui::Ui) {
    let rect = ui.available_rect_before_wrap();
    let y = ui.cursor().top();
    ui.painter().hline(rect.x_range(), y, Stroke::new(1.0, LINE));
    ui.add_space(1.0);
}

/// Section header inside panels: small muted semibold text.
pub fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).family(semibold()).size(11.0).color(MUTED));
}
