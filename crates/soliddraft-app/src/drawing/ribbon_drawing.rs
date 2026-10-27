//! The drawing's ribbon: Views, Annotate, Sheet, View and Assistant tabs, in the same
//! grammar as the part's ribbon (labelled groups, large and stacked small buttons,
//! commands of later phases disabled with the phase named). The tabs, panels and buttons
//! come from the command registry ([`crate::commands`]).

use super::{DrawingAction, DrawingTab};
use crate::commands::{self, CommandSpec, Size};
use crate::icons;
use crate::state::{AppAction, AppState};
use crate::theme;
use egui::{Align2, CornerRadius, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

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

/// The current tab's panels, from the command registry.
fn groups(state: &AppState) -> Vec<Group> {
    let Some(s) = state.drawing.session.as_ref() else { return Vec::new() };
    commands::ribbon(s.tab)
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
    let Some(current) = state.drawing.session.as_ref().map(|s| s.tab) else { return };
    let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width(), theme::RIBBON_TABS_HEIGHT), Sense::hover());
    ui.painter().rect_filled(strip, CornerRadius::ZERO, theme::chrome());
    let mut x = strip.left() + 10.0;
    let mut chosen = None;
    for tab in DrawingTab::ALL {
        let galley = ui.painter().layout_no_wrap(tab.label().to_owned(), theme::font_medium(12.5), theme::ink());
        let w = galley.size().x + 24.0;
        let r = Rect::from_min_size(pos2(x, strip.top() + 4.0), vec2(w, strip.height() - 4.0));
        x += w + 2.0;
        let resp = ui.interact(r, ui.id().with(("drawing_ribbon_tab", tab.label())), Sense::click());
        resp.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Button, true, current == tab, format!("{} tab", tab.label()))
        });
        let on = current == tab;
        if on {
            ui.painter().rect(
                r,
                CornerRadius { nw: theme::RADIUS, ne: theme::RADIUS, sw: 0, se: 0 },
                theme::panel(),
                Stroke::new(1.0, theme::line()),
                StrokeKind::Inside,
            );
        } else if resp.hovered() {
            ui.painter().rect_filled(
                r,
                CornerRadius { nw: theme::RADIUS, ne: theme::RADIUS, sw: 0, se: 0 },
                theme::hover(),
            );
        }
        let color = if on { theme::ink() } else { theme::muted() };
        ui.painter().galley(r.center() - galley.size() / 2.0, galley, color);
        if resp.clicked() {
            chosen = Some(tab);
        }
    }
    ui.painter().hline(strip.x_range(), strip.bottom() - 0.5, Stroke::new(1.0, theme::line()));
    let mut x = strip.left() + 10.0;
    for tab in DrawingTab::ALL {
        let galley = ui.painter().layout_no_wrap(tab.label().to_owned(), theme::font_medium(12.5), theme::ink());
        let w = galley.size().x + 24.0;
        if current == tab {
            ui.painter().hline((x + 1.0)..=(x + w - 1.0), strip.bottom() - 0.5, Stroke::new(1.0, theme::panel()));
        }
        x += w + 2.0;
    }
    // A pill at the right of the strip names the document the ribbon drives.
    if let Some(s) = state.drawing.session.as_ref() {
        let text = format!("Drawing · {}", s.title());
        let galley = ui.painter().layout_no_wrap(text, theme::font(10.5), theme::muted());
        let r = Rect::from_min_size(
            pos2(strip.right() - galley.size().x - 26.0, strip.top() + 5.0),
            galley.size() + vec2(16.0, 6.0),
        );
        ui.painter().rect(r, CornerRadius::same(99), theme::accent_soft(), Stroke::NONE, StrokeKind::Inside);
        ui.painter().galley(r.center() - galley.size() / 2.0, galley, theme::selection_ink());
    }

    let (body, _) = ui.allocate_exact_size(vec2(ui.available_width(), theme::RIBBON_BODY_HEIGHT), Sense::hover());
    ui.painter().rect_filled(body, CornerRadius::ZERO, theme::panel());
    ui.painter().hline(body.x_range(), body.bottom() - 0.5, Stroke::new(1.0, theme::line()));
    let groups = groups(state);
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
                let galley = ui.painter().layout(item.label.clone(), theme::font(11.0), theme::ink(), 64.0);
                let w = galley.size().x.max(44.0) + 14.0;
                let r = Rect::from_min_size(pos2(x, top), vec2(w, item_h));
                if draw_item(ui, item, r, (gi, i)).clicked() {
                    pending = Some(item.command);
                }
                x += w + 2.0;
                i += 1;
            } else {
                let mut count = 0;
                let mut widest: f32 = 0.0;
                while i + count < group.items.len() && group.items[i + count].size == Size::Small && count < 3 {
                    let g = ui.painter().layout_no_wrap(
                        group.items[i + count].label.clone(),
                        theme::font(11.0),
                        theme::ink(),
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
            theme::muted(),
        );
        ui.painter().vline(x, (body.top() + 8.0)..=(body.bottom() - 8.0), Stroke::new(1.0, theme::line()));
        x += 8.0;
    }
    if let Some(t) = chosen {
        state.push(AppAction::Drawing(DrawingAction::SetTab(t)));
    }
    if let Some(c) = pending {
        state.run_command(c);
    }
}

fn draw_item(ui: &mut egui::Ui, item: &Item, rect: Rect, key: (usize, usize)) -> egui::Response {
    let enabled = item.enabled;
    let resp = ui.interact(
        rect,
        ui.id().with(("drawing_ribbon_item", key)),
        if enabled { Sense::click_and_drag() } else { Sense::hover() },
    );
    // A button dragged onto the command line's chip row becomes a chip there.
    resp.dnd_set_drag_payload(crate::command_line::ToolDrag { id: item.command.id });
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, item.active, &item.label));
    let painter = ui.painter();
    if item.active {
        painter.rect_filled(rect, CornerRadius::same(7), theme::selection_bg());
    } else if enabled && resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(7), theme::hover());
    }
    let color = if !enabled {
        theme::muted().gamma_multiply(0.55)
    } else if item.active {
        theme::selection_ink()
    } else {
        theme::ink()
    };
    match item.size {
        Size::Large => {
            let icon_rect = Rect::from_center_size(pos2(rect.center().x, rect.top() + 22.0), vec2(24.0, 24.0));
            icons::paint(painter, item.command.icon, icon_rect, color);
            let galley = painter.layout(item.label.clone(), theme::font(11.0), color, rect.width() - 6.0);
            let size = galley.size();
            painter.galley(pos2(rect.center().x - size.x / 2.0, rect.bottom() - 6.0 - size.y), galley, color);
        }
        Size::Small => {
            let icon_rect = Rect::from_center_size(pos2(rect.left() + 13.0, rect.center().y), vec2(15.0, 15.0));
            icons::paint(painter, item.command.icon, icon_rect, color);
            painter.text(
                pos2(rect.left() + 25.0, rect.center().y),
                Align2::LEFT_CENTER,
                &item.label,
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
