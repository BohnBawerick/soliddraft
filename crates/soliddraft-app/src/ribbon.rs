//! Tabbed command ribbon, modelled on the SolidWorks CommandManager layout:
//! labelled groups, large buttons for the main verbs and stacked small ones
//! for their variants. Commands outside the current phase are shown disabled
//! with the phase that delivers them, so the layout is honest about what works.
//! The tabs, panels and buttons come from the command registry ([`crate::commands`]).

use crate::commands::{self, CommandSpec, Size};
use crate::icons;
use crate::state::{AppState, RibbonTab};
use crate::theme;
use egui::{Align2, CornerRadius, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

/// A button as drawn this frame.
struct Item {
    command: &'static CommandSpec,
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
    ui.painter().rect_filled(strip, CornerRadius::ZERO, theme::PANEL);
    let mut x = strip.left() + 10.0;
    for tab in RibbonTab::ALL {
        let galley = ui.painter().layout_no_wrap(tab.label().to_owned(), theme::font_medium(12.5), theme::INK);
        let w = galley.size().x + 24.0;
        let r = Rect::from_min_size(pos2(x, strip.top() + 4.0), vec2(w, strip.height() - 4.0));
        x += w + 2.0;
        let resp = ui.interact(r, ui.id().with(("ribbon_tab", tab.label())), Sense::click());
        resp.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::Button,
                true,
                state.ribbon_tab == tab,
                format!("{} tab", tab.label()),
            )
        });
        let on = state.ribbon_tab == tab;
        if on {
            ui.painter().rect(
                r,
                CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 },
                theme::BG,
                Stroke::new(1.0, theme::LINE),
                StrokeKind::Inside,
            );
            ui.painter().hline(r.x_range().shrink(1.0), r.bottom() - 0.5, Stroke::new(1.5, theme::BG));
        } else if resp.hovered() {
            ui.painter().rect_filled(r, CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, theme::HOVER);
        }
        let color = if on { theme::INK } else { theme::MUTED };
        ui.painter().galley(r.center() - galley.size() / 2.0, galley, color);
        if resp.clicked() {
            state.ribbon_tab = tab;
        }
    }
    ui.painter().hline(strip.x_range(), strip.bottom() - 0.5, Stroke::new(1.0, theme::LINE));
    // Redraw the active tab's bottom edge so it merges into the body.
    let mut x = strip.left() + 10.0;
    for tab in RibbonTab::ALL {
        let galley = ui.painter().layout_no_wrap(tab.label().to_owned(), theme::font_medium(12.5), theme::INK);
        let w = galley.size().x + 24.0;
        if state.ribbon_tab == tab {
            ui.painter().hline((x + 1.0)..=(x + w - 1.0), strip.bottom() - 0.5, Stroke::new(1.0, theme::BG));
        }
        x += w + 2.0;
    }

    body(ui, state, commands::Tab::Part(state.ribbon_tab));
}

/// The assembly's ribbon: its one tab, in the part ribbon's grammar.
fn show_assembly(ui: &mut egui::Ui, state: &mut AppState) {
    let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width(), theme::RIBBON_TABS_HEIGHT), Sense::hover());
    ui.painter().rect_filled(strip, CornerRadius::ZERO, theme::PANEL);
    let tab = commands::Tab::Assembly;
    let galley = ui.painter().layout_no_wrap(tab.label().to_owned(), theme::font_medium(12.5), theme::INK);
    let r = Rect::from_min_size(
        pos2(strip.left() + 10.0, strip.top() + 4.0),
        vec2(galley.size().x + 24.0, strip.height() - 4.0),
    );
    ui.painter().rect(
        r,
        CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 },
        theme::BG,
        Stroke::new(1.0, theme::LINE),
        StrokeKind::Inside,
    );
    ui.painter().galley(r.center() - galley.size() / 2.0, galley, theme::INK);
    ui.painter().hline(strip.x_range(), strip.bottom() - 0.5, Stroke::new(1.0, theme::LINE));
    ui.painter().hline((r.left() + 1.0)..=(r.right() - 1.0), strip.bottom() - 0.5, Stroke::new(1.0, theme::BG));
    body(ui, state, tab);
}

/// A tab's panels of buttons.
fn body(ui: &mut egui::Ui, state: &mut AppState, tab: commands::Tab) {
    let (body, _) = ui.allocate_exact_size(vec2(ui.available_width(), theme::RIBBON_BODY_HEIGHT), Sense::hover());
    ui.painter().rect_filled(body, CornerRadius::ZERO, theme::BG);
    ui.painter().hline(body.x_range(), body.bottom() - 0.5, Stroke::new(1.0, theme::LINE));
    let groups = groups(state, tab);
    let mut x = body.left() + 8.0;
    let top = body.top() + 6.0;
    let item_h = body.height() - 26.0;
    let mut pending: Option<&'static CommandSpec> = None;
    for (gi, group) in groups.iter().enumerate() {
        let start_x = x;
        let mut i = 0;
        while i < group.items.len() {
            let item = &group.items[i];
            if item.size == Size::Large {
                let r = Rect::from_min_size(pos2(x, top), vec2(58.0, item_h));
                if draw_item(ui, item, r, (gi, i)).clicked() {
                    pending = Some(item.command);
                }
                x += 58.0 + 2.0;
                i += 1;
            } else {
                let mut count = 0;
                let mut widest: f32 = 0.0;
                while i + count < group.items.len() && group.items[i + count].size == Size::Small && count < 3 {
                    let g = ui.painter().layout_no_wrap(
                        group.items[i + count].command.label.to_owned(),
                        theme::font(11.0),
                        theme::INK,
                    );
                    widest = widest.max(g.size().x);
                    count += 1;
                }
                let w = widest + 30.0;
                let row_h = item_h / 3.0;
                for k in 0..count {
                    let item = &group.items[i + k];
                    let r = Rect::from_min_size(pos2(x, top + row_h * k as f32), vec2(w, row_h - 1.0));
                    if draw_item(ui, item, r, (gi, i + k)).clicked() {
                        pending = Some(item.command);
                    }
                }
                x += w + 2.0;
                i += count;
            }
        }
        x += 6.0;
        ui.painter().text(
            pos2((start_x + x - 6.0) / 2.0, body.bottom() - 10.0),
            Align2::CENTER_CENTER,
            group.label,
            theme::font(10.5),
            theme::MUTED,
        );
        ui.painter().vline(x, (body.top() + 8.0)..=(body.bottom() - 8.0), Stroke::new(1.0, theme::LINE));
        x += 8.0;
    }
    if let Some(c) = pending {
        state.run_command(c);
    }
}

fn draw_item(ui: &mut egui::Ui, item: &Item, rect: Rect, key: (usize, usize)) -> egui::Response {
    let enabled = item.enabled;
    let label = item.command.label;
    let resp = ui.interact(
        rect,
        ui.id().with(("ribbon_item", key)),
        if enabled { Sense::click_and_drag() } else { Sense::hover() },
    );
    // A button dragged onto the command line's chip row becomes a chip there.
    resp.dnd_set_drag_payload(crate::command_line::ToolDrag { id: item.command.id });
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, item.active, label));
    let painter = ui.painter();
    if item.active {
        painter.rect_filled(rect, CornerRadius::same(7), theme::SELECTION_BG);
    } else if enabled && resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(7), theme::HOVER);
    }
    let color = if !enabled {
        theme::MUTED.gamma_multiply(0.55)
    } else if item.active {
        theme::SELECTION_INK
    } else {
        theme::INK
    };
    match item.size {
        Size::Large => {
            let icon_rect = Rect::from_center_size(pos2(rect.center().x, rect.top() + 24.0), vec2(26.0, 26.0));
            icons::paint(painter, item.command.icon, icon_rect, color);
            painter.text(
                pos2(rect.center().x, rect.bottom() - 11.0),
                Align2::CENTER_CENTER,
                label,
                theme::font(11.0),
                color,
            );
        }
        Size::Small => {
            let icon_rect = Rect::from_center_size(pos2(rect.left() + 13.0, rect.center().y), vec2(15.0, 15.0));
            icons::paint(painter, item.command.icon, icon_rect, color);
            painter.text(
                pos2(rect.left() + 25.0, rect.center().y),
                Align2::LEFT_CENTER,
                label,
                theme::font(11.0),
                color,
            );
        }
    }
    if resp.hovered() && enabled {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.on_hover_text(item.tip.clone())
}
