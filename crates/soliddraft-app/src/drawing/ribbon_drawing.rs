//! The drawing's ribbon: Views, Annotate, Sheet, View and Assistant tabs over the part's
//! tiles and narrow-window handling ([`crate::ribbon::body`]). The tabs, panels and buttons
//! come from the command registry ([`crate::commands`]).

use super::{DrawingAction, DrawingTab};
use crate::state::{AppAction, AppState};
use crate::theme;
use egui::{CornerRadius, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

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

    crate::ribbon::body(ui, state, current.into());
    if let Some(t) = chosen {
        state.push(AppAction::Drawing(DrawingAction::SetTab(t)));
    }
}
