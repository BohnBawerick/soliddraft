//! Tabbed command ribbon: labelled groups of tiles, each an icon with its label underneath,
//! a larger icon for the main verbs and a smaller one for their variants. Commands outside
//! the current phase are shown disabled with the phase that delivers them, so the layout is
//! honest about what works. The tabs, panels and buttons come from the command registry
//! ([`crate::commands`]).

use crate::commands::{self, CommandSpec, Size};
use crate::icons;
use crate::state::{AppState, RibbonTab};
use crate::theme;
use egui::text::LayoutJob;
use egui::{Align, Align2, CornerRadius, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

/// From the edge of the ribbon to its first group.
const BODY_MARGIN: f32 = 8.0;
/// Between two groups, with the separator in the middle.
const GROUP_GAP: f32 = 14.0;
const TILE_GAP: f32 = 2.0;
const TILE_PADDING: f32 = 10.0;
const TILE_MIN_WIDTH: f32 = 46.0;
/// A collapsed group's button, wider if its caption needs it.
const COMPACT_MIN_WIDTH: f32 = 52.0;
/// From the top of a tile to the top of its icon's square.
const ICON_INSET: f32 = 3.0;
const ICON_LARGE: f32 = 26.0;
const ICON_SMALL: f32 = 22.0;
const LABEL_SIZE: f32 = 11.0;
/// How far a label runs before it wraps, unless two lines need more.
const LABEL_WRAP: f32 = 58.0;
const CAPTION_SIZE: f32 = 10.5;
const CHEVRON_SIZE: f32 = 10.0;
/// From a collapsed group's button down to the row it opens, which clears the ribbon.
const ROW_GAP: f32 = 4.0;

/// A button as drawn this frame.
struct Item {
    command: &'static CommandSpec,
    label: String,
    size: Size,
    enabled: bool,
    active: bool,
    tip: String,
}

struct Group {
    label: &'static str,
    items: Vec<Item>,
}

/// A tab's panels, from the command registry.
fn groups(state: &AppState, tab: commands::Tab) -> Vec<Group> {
    commands::ribbon(tab)
        .into_iter()
        .map(|(label, items)| Group {
            label,
            items: items
                .into_iter()
                .filter(|(command, _)| (command.shown)(state))
                .map(|(command, size)| Item {
                    command,
                    label: command.label_in(state),
                    size,
                    enabled: command.is_enabled(state),
                    active: (command.active)(state),
                    tip: command.tip(state),
                })
                .collect(),
        })
        .collect()
}

pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    // The tabs and the body fill the panel between them.
    ui.spacing_mut().item_spacing.y = 0.0;
    if state.drawing.is_active() {
        crate::drawing::ribbon_drawing::show(ui, state);
        return;
    }
    if state.assembly.is_active() {
        show_assembly(ui, state);
        return;
    }
    // Tab strip.
    let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width(), theme::RIBBON_TABS_HEIGHT), Sense::hover());
    ui.painter().rect_filled(strip, CornerRadius::ZERO, theme::chrome());
    let mut x = strip.left() + 10.0;
    for tab in RibbonTab::ALL {
        let galley = ui.painter().layout_no_wrap(tab.label().to_owned(), theme::font_medium(12.5), theme::ink());
        let w = galley.size().x + 24.0;
        let r = Rect::from_min_size(pos2(x, strip.top() + 4.0), vec2(w, strip.height() - 4.0));
        x += w + 2.0;
        let planned = tab.planned();
        let sense = if planned.is_some() { Sense::hover() } else { Sense::click() };
        let resp = ui.interact(r, ui.id().with(("ribbon_tab", tab.label())), sense);
        resp.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::Button,
                planned.is_none(),
                state.ribbon_tab == tab,
                format!("{} tab", tab.label()),
            )
        });
        let on = state.ribbon_tab == tab;
        if on {
            ui.painter().rect(
                r,
                CornerRadius { nw: theme::RADIUS, ne: theme::RADIUS, sw: 0, se: 0 },
                theme::panel(),
                Stroke::new(1.0, theme::line()),
                StrokeKind::Inside,
            );
            ui.painter().hline(r.x_range().shrink(1.0), r.bottom() - 0.5, Stroke::new(1.5, theme::panel()));
        } else if planned.is_none() && resp.hovered() {
            ui.painter().rect_filled(
                r,
                CornerRadius { nw: theme::RADIUS, ne: theme::RADIUS, sw: 0, se: 0 },
                theme::hover(),
            );
        }
        let color = if planned.is_some() {
            theme::muted().gamma_multiply(0.55)
        } else if on {
            theme::ink()
        } else {
            theme::muted()
        };
        ui.painter().galley(r.center() - galley.size() / 2.0, galley, color);
        if let Some(why) = planned {
            resp.on_hover_text(why);
        } else if resp.clicked() {
            state.ribbon_tab = tab;
        }
    }
    ui.painter().hline(strip.x_range(), strip.bottom() - 0.5, Stroke::new(1.0, theme::line()));
    // Redraw the active tab's bottom edge so it merges into the body.
    let mut x = strip.left() + 10.0;
    for tab in RibbonTab::ALL {
        let galley = ui.painter().layout_no_wrap(tab.label().to_owned(), theme::font_medium(12.5), theme::ink());
        let w = galley.size().x + 24.0;
        if state.ribbon_tab == tab {
            ui.painter().hline((x + 1.0)..=(x + w - 1.0), strip.bottom() - 0.5, Stroke::new(1.0, theme::panel()));
        }
        x += w + 2.0;
    }

    body(ui, state, state.ribbon_tab.into());
}

/// The ribbon while an assembly is the active document: its one tab, in the part
/// ribbon's grammar.
fn show_assembly(ui: &mut egui::Ui, state: &mut AppState) {
    let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width(), theme::RIBBON_TABS_HEIGHT), Sense::hover());
    ui.painter().rect_filled(strip, CornerRadius::ZERO, theme::panel());
    let tab = commands::Tab::Assembly;
    let galley = ui.painter().layout_no_wrap(tab.label().to_owned(), theme::font_medium(12.5), theme::ink());
    let r = Rect::from_min_size(
        pos2(strip.left() + 10.0, strip.top() + 4.0),
        vec2(galley.size().x + 24.0, strip.height() - 4.0),
    );
    ui.painter().rect(
        r,
        CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 },
        theme::chrome(),
        Stroke::new(1.0, theme::line()),
        StrokeKind::Inside,
    );
    ui.painter().galley(r.center() - galley.size() / 2.0, galley, theme::ink());
    ui.painter().hline(strip.x_range(), strip.bottom() - 0.5, Stroke::new(1.0, theme::line()));
    ui.painter().hline((r.left() + 1.0)..=(r.right() - 1.0), strip.bottom() - 0.5, Stroke::new(1.0, theme::chrome()));
    body(ui, state, tab);
}

/// A tab's panels of tiles. A window too narrow for all of them collapses groups from the
/// right until the rest fit, each to a button that opens its tiles in a row under the ribbon.
pub fn body(ui: &mut egui::Ui, state: &mut AppState, tab: commands::Tab) {
    let (body, _) = ui.allocate_exact_size(vec2(ui.available_width(), theme::RIBBON_BODY_HEIGHT), Sense::hover());
    ui.painter().rect_filled(body, CornerRadius::ZERO, theme::panel());
    ui.painter().hline(body.x_range(), body.bottom() - 0.5, Stroke::new(1.0, theme::line()));
    let groups = groups(state, tab);
    let collapsed = to_collapse(ui, &groups, body.width() - 2.0 * BODY_MARGIN);
    let top = body.top() + 6.0;
    let tile_h = body.height() - 26.0;
    let caption_y = body.bottom() - 10.0;
    let mut x = body.left() + BODY_MARGIN;
    let mut pending: Option<&'static CommandSpec> = None;
    for (gi, group) in groups.iter().enumerate() {
        let width = if collapsed[gi] {
            let width = compact_width(ui, group);
            let button = Rect::from_min_max(pos2(x, top), pos2(x + width, body.bottom() - 2.0));
            pending = collapsed_group(ui, group, gi, tab, button, caption_y).or(pending);
            width
        } else {
            let width = group_width(ui, group);
            let tiles = row(ui, group, gi, x + (width - row_width(ui, group)) / 2.0, top, tile_h);
            pending = tiles.into_iter().find(|(_, resp)| resp.clicked()).map(|(c, _)| c).or(pending);
            caption(ui, group.label, pos2(x + width / 2.0, caption_y));
            width
        };
        x += width + GROUP_GAP;
        let line = x - GROUP_GAP / 2.0 + 1.0;
        ui.painter().vline(line, (body.top() + 8.0)..=(body.bottom() - 8.0), Stroke::new(1.0, theme::line()));
    }
    if let Some(c) = pending {
        state.run_command(c);
    }
}

/// Which groups collapse: from the right, until the rest fit. A group no wider than its
/// collapsed button stays as it is.
fn to_collapse(ui: &egui::Ui, groups: &[Group], room: f32) -> Vec<bool> {
    let mut total: f32 = groups.iter().map(|g| group_width(ui, g) + GROUP_GAP).sum();
    let mut collapsed = vec![false; groups.len()];
    for (i, group) in groups.iter().enumerate().rev() {
        if total <= room {
            break;
        }
        let saved = group_width(ui, group) - compact_width(ui, group);
        if saved > 0.0 {
            collapsed[i] = true;
            total -= saved;
        }
    }
    collapsed
}

/// The label of a tile, on at most two lines and never broken inside a word.
fn label_galley(ui: &egui::Ui, label: &str, color: egui::Color32) -> std::sync::Arc<egui::Galley> {
    let font = theme::font(LABEL_SIZE);
    let width = |text: String| ui.painter().layout_no_wrap(text, font.clone(), color).size().x;
    let words: Vec<&str> = label.split_whitespace().collect();
    let split = |i: usize| width(words[..i].join(" ")).max(width(words[i..].join(" ")));
    let lines = (1..words.len()).map(split).fold(f32::INFINITY, f32::min);
    let lines = if lines.is_finite() { lines } else { width(label.to_owned()) };
    // The wrap counts the space at the end of the first line.
    let mut job = LayoutJob::simple(label.to_owned(), font.clone(), color, LABEL_WRAP.max(lines.ceil() + 4.0));
    job.halign = Align::Center;
    ui.painter().layout_job(job)
}

fn tile_width(ui: &egui::Ui, item: &Item) -> f32 {
    (label_galley(ui, &item.label, theme::ink()).size().x + TILE_PADDING).max(TILE_MIN_WIDTH)
}

fn row_width(ui: &egui::Ui, group: &Group) -> f32 {
    let tiles: f32 = group.items.iter().map(|item| tile_width(ui, item) + TILE_GAP).sum();
    tiles - TILE_GAP
}

fn caption_width(ui: &egui::Ui, group: &Group) -> f32 {
    ui.painter().layout_no_wrap(group.label.to_owned(), theme::font(CAPTION_SIZE), theme::muted()).size().x
}

/// A group laid out in full: its tiles, or its caption if that is wider.
fn group_width(ui: &egui::Ui, group: &Group) -> f32 {
    row_width(ui, group).max(caption_width(ui, group) + TILE_PADDING)
}

fn compact_width(ui: &egui::Ui, group: &Group) -> f32 {
    (caption_width(ui, group) + CHEVRON_SIZE + TILE_PADDING).max(COMPACT_MIN_WIDTH)
}

/// A group's tiles, left to right from a point.
fn row(
    ui: &mut egui::Ui,
    group: &Group,
    gi: usize,
    left: f32,
    top: f32,
    height: f32,
) -> Vec<(&'static CommandSpec, egui::Response)> {
    let mut x = left;
    let mut drawn = Vec::new();
    for (i, item) in group.items.iter().enumerate() {
        let width = tile_width(ui, item);
        let rect = Rect::from_min_size(pos2(x, top), vec2(width, height));
        drawn.push((item.command, draw_item(ui, item, rect, (gi, i))));
        x += width + TILE_GAP;
    }
    drawn
}

fn caption(ui: &egui::Ui, label: &str, centre: egui::Pos2) {
    ui.painter().text(centre, Align2::CENTER_CENTER, label, theme::font(CAPTION_SIZE), theme::muted());
}

/// A group as one button: its first command's art over its name and a chevron. Clicking it
/// opens the group's tiles in a row, which goes when a command is chosen, with a click
/// elsewhere or with Escape.
fn collapsed_group(
    ui: &mut egui::Ui,
    group: &Group,
    gi: usize,
    tab: commands::Tab,
    rect: Rect,
    caption_y: f32,
) -> Option<&'static CommandSpec> {
    let resp = ui.interact(rect, ui.id().with(("ribbon_group", tab.label(), group.label)), Sense::click());
    let active = group.items.iter().any(|item| item.active);
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, true, active, format!("{} group", group.label))
    });
    if active {
        ui.painter().rect_filled(rect, CornerRadius::same(7), theme::selection_bg());
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(7), theme::hover());
    }
    let treatment =
        if group.items.iter().any(|item| item.enabled) { icons::Treatment::Normal } else { icons::Treatment::Disabled };
    if let Some(first) = group.items.first() {
        let icon =
            Rect::from_center_size(pos2(rect.center().x, (rect.top() + caption_y) / 2.0), vec2(ICON_LARGE, ICON_LARGE));
        icons::paint_as(ui.painter(), first.command.icon, icon, theme::ink(), treatment);
    }
    let name = caption_width(ui, group);
    caption(ui, group.label, pos2(rect.center().x - CHEVRON_SIZE / 2.0, caption_y));
    let chevron = pos2(rect.center().x + name / 2.0 + 1.0, caption_y);
    let chevron = Rect::from_center_size(chevron, vec2(CHEVRON_SIZE, CHEVRON_SIZE));
    icons::paint(ui.painter(), icons::Icon::Glyph("chevd"), chevron, theme::muted());
    let opened = egui::Popup::from_toggle_button_response(&resp)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .gap(ROW_GAP)
        .show(|ui| {
            let height = theme::RIBBON_BODY_HEIGHT - 26.0;
            let width = group_width(ui, group);
            let (area, _) = ui.allocate_exact_size(vec2(width, height + 20.0), Sense::hover());
            let left = area.left() + (width - row_width(ui, group)) / 2.0;
            let tiles = row(ui, group, gi, left, area.top(), height);
            caption(ui, group.label, pos2(area.center().x, area.bottom() - 8.0));
            let chosen = tiles.iter().find(|(_, resp)| resp.clicked()).map(|(c, _)| *c);
            if chosen.is_some() || tiles.iter().any(|(_, resp)| resp.drag_stopped()) {
                ui.close();
            }
            chosen
        });
    if opened.is_none() {
        let names: Vec<&str> = group.items.iter().map(|item| item.label.as_str()).collect();
        resp.on_hover_text(names.join(", "));
    }
    opened.and_then(|shown| shown.inner)
}

fn draw_item(ui: &mut egui::Ui, item: &Item, rect: Rect, key: (usize, usize)) -> egui::Response {
    let enabled = item.enabled;
    let resp = ui.interact(
        rect,
        ui.id().with(("ribbon_item", key)),
        if enabled { Sense::click_and_drag() } else { Sense::hover() },
    );
    // A button dragged onto the command line's chip row becomes a chip there.
    resp.dnd_set_drag_payload(crate::command_line::ToolDrag { id: item.command.id });
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, item.active, &item.label));
    let fill = if item.active { Some(theme::selection_bg()) } else { (enabled && resp.hovered()).then(theme::hover) };
    if let Some(fill) = fill {
        ui.painter().rect_filled(rect, CornerRadius::same(7), fill);
    }
    let (icon, text) = if !enabled {
        (theme::muted().gamma_multiply(0.55), theme::muted())
    } else if item.active {
        (theme::selection_ink(), theme::selection_ink())
    } else {
        (theme::ink(), theme::ink())
    };
    let treatment = if enabled { icons::Treatment::Normal } else { icons::Treatment::Disabled };
    let side = if item.size == Size::Large { ICON_LARGE } else { ICON_SMALL };
    // Both sizes centre in the large square, so the labels of a row start level.
    let centre = pos2(rect.center().x, rect.top() + ICON_INSET + ICON_LARGE / 2.0);
    icons::paint_as(ui.painter(), item.command.icon, Rect::from_center_size(centre, vec2(side, side)), icon, treatment);
    let label = pos2(rect.center().x, rect.top() + ICON_INSET + ICON_LARGE + 2.0);
    let galley = label_galley(ui, &item.label, text);
    ui.painter().galley(label, galley, text);
    if resp.hovered() && enabled {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.on_hover_text(item.tip.clone())
}
