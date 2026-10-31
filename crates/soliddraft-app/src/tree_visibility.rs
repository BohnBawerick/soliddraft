//! Showing and hiding from the tree. Every feature, reference plane and origin axis row
//! ends in an eye: one click on it shows or hides the item and does nothing else, and a
//! hidden row and its eye turn grey. A click on the row's name still selects. The choice
//! is the part's, saved in its file (see [`Document::set_view_choice`]), never a change
//! to the model.
//!
//! An item the user has not chosen for follows the defaults: the body shows, a plane
//! follows the Show origin planes setting and shows while it is selected, the axes show
//! while a reference is selected, while a Revolve or Pattern editor is open or when the
//! user keeps them on, and a sketch shows quietly while it is selected. A choice wins
//! over its default, so selecting the origin cannot bring back an axis the user hid.
//!
//! The part has one body, so a hidden feature that shapes it hides all of it, and
//! showing any of them shows it again. A hidden sketch hides its wires, not the
//! extrusion made from it.

use crate::document::{AxisKind, Document, FeatureId, PlaneKind, Reference, Selection, ViewItem};
use crate::state::{AppState, EditorParams};
use crate::theme;
use egui::{Rect, Sense, Stroke, pos2, vec2};

/// What the viewport draws of the part's items this frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Visible {
    pub body: bool,
    /// In [`PlaneKind::ALL`] order.
    pub planes: [bool; 3],
    /// In [`AxisKind::ALL`] order.
    pub axes: [bool; 3],
    /// The sketches drawn on their planes.
    pub sketches: Vec<FeatureId>,
}

/// Whether `item` shows in the viewport now.
pub fn shown(state: &AppState, item: ViewItem) -> bool {
    let doc = &state.doc;
    let choice = doc.view_choice(item);
    let references = &state.viewport.references;
    match item {
        ViewItem::Plane(plane) => choice.unwrap_or(
            references.show_origin_planes || doc.selection == Some(Selection::Reference(Reference::Plane(plane))),
        ),
        ViewItem::Axis(_) => choice.unwrap_or(
            references.always_show_origin_axes
                || matches!(doc.selection, Some(Selection::Reference(_)))
                || tool_wants_an_axis(state),
        ),
        ViewItem::Feature(id) => match doc.feature(id) {
            Some(f) if f.is_sketch() => choice.unwrap_or(state.selected_sketch() == Some(id)),
            Some(_) => body_shown(doc),
            None => false,
        },
    }
}

/// An axis is one of the open editor's inputs, so it has to be there to pick.
fn tool_wants_an_axis(state: &AppState) -> bool {
    state.editor.as_ref().is_some_and(|e| matches!(e.params, EditorParams::Revolve(_) | EditorParams::Pattern(_)))
}

/// The body shows unless a feature that shapes it is hidden.
fn body_shown(doc: &Document) -> bool {
    !doc.features.iter().any(|f| !f.is_sketch() && doc.view_choice(ViewItem::Feature(f.id)) == Some(false))
}

/// What the viewport draws this frame.
pub fn visible(state: &AppState) -> Visible {
    Visible {
        body: body_shown(&state.doc),
        planes: PlaneKind::ALL.map(|p| shown(state, ViewItem::Plane(p))),
        axes: AxisKind::ALL.map(|a| shown(state, ViewItem::Axis(a))),
        sketches: state
            .doc
            .features
            .iter()
            .filter(|f| f.is_sketch() && shown(state, ViewItem::Feature(f.id)))
            .map(|f| f.id)
            .collect(),
    }
}

/// Shows `item` when it is hidden and hides it when it shows. Showing a feature of the
/// body shows the body, so every hidden feature of it goes back to its default.
pub fn toggle(state: &mut AppState, item: ViewItem) {
    let show = !shown(state, item);
    let doc = &mut state.doc;
    if show
        && let ViewItem::Feature(id) = item
        && doc.feature(id).is_some_and(|f| !f.is_sketch())
    {
        let hidden: Vec<FeatureId> = doc
            .features
            .iter()
            .filter(|f| !f.is_sketch() && doc.view_choice(ViewItem::Feature(f.id)) == Some(false))
            .map(|f| f.id)
            .collect();
        for other in hidden {
            doc.set_view_choice(ViewItem::Feature(other), None);
        }
    }
    doc.set_view_choice(item, Some(show));
}

/// The width a row keeps free at its right end for the eye.
pub const EYE_WIDTH: f32 = 24.0;

/// The eye at the right end of `row`, the row of `item` called `name`: open while the
/// item shows, grey and struck through while it is hidden. One click shows or hides the
/// item; it selects, renames, edits and discloses nothing.
pub fn eye(ui: &mut egui::Ui, state: &mut AppState, row: Rect, item: ViewItem, name: &str) {
    let shown = shown(state, item);
    let rect = Rect::from_center_size(
        pos2(row.right() - 4.0 - EYE_WIDTH / 2.0, row.center().y),
        vec2(EYE_WIDTH - 4.0, row.height() - 6.0),
    );
    let resp = ui.interact(rect, ui.id().with(("eye", format!("{item:?}"))), Sense::click());
    let action = format!("{} {name}", if shown { "Hide" } else { "Show" });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &action));
    ui.ctx().accesskit_node_builder(resp.id, |node| {
        node.set_state_description(if shown { "Shown" } else { "Hidden" });
    });
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if resp.hovered() {
            painter.rect_filled(rect, egui::CornerRadius::same(4), theme::hover());
        }
        let color = if shown { theme::ink() } else { theme::muted() };
        let c = rect.center();
        // A lens of two arcs around a pupil.
        let lid = |sign: f32| -> Vec<egui::Pos2> {
            (0..=12)
                .map(|i| {
                    let u = i as f32 / 6.0 - 1.0;
                    pos2(c.x + 6.5 * u, c.y + sign * 4.0 * (1.0 - u * u))
                })
                .collect()
        };
        let stroke = Stroke::new(1.2, color);
        painter.add(egui::Shape::line(lid(-1.0), stroke));
        painter.add(egui::Shape::line(lid(1.0), stroke));
        if shown {
            painter.circle_filled(c, 2.0, color);
        } else {
            painter.circle_stroke(c, 1.8, stroke);
            painter.line_segment([c + vec2(-6.0, 5.0), c + vec2(6.0, -5.0)], stroke);
        }
    }
    let clicked = resp.clicked();
    resp.on_hover_text(action);
    if clicked {
        toggle(state, item);
    }
}
