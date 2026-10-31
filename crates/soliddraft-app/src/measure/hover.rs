//! The proven midpoint target of a hovered straight edge, and the Measure panel's picks.
//!
//! The id buffer says which edge is under the pointer; the coordinate comes from the
//! kernel's midpoint proof, never from the pick ray or a display polyline. Pixel reach
//! only decides whether the pointer is on the target.

use crate::{
    document::{Document, PickEntry, Selection},
    state::AppState,
    theme,
    viewport::{ViewportEvents, render::Picked},
};
use egui::{Pos2, Stroke};
use glam::Vec3;
use soliddraft_kernel_api::EdgeId;

/// How close the pointer must be to a midpoint, in points, to aim at it.
const REACH: f32 = 8.0;

/// A proven midpoint, cached for the geometry it was proved on.
#[derive(Clone, Debug)]
pub struct Proven {
    edge: EdgeId,
    geometry: soliddraft_document::ContentHash,
    point: Option<[f64; 3]>,
}

/// One end of a Measure pick: an edge or face, or a straight edge's proven midpoint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pick {
    pub selection: Selection,
    pub midpoint: bool,
}

/// The proven midpoint of `edge` on the current geometry, if the edge has one.
fn proven(cache: &mut Vec<Proven>, doc: &Document, edge: EdgeId) -> Option<[f64; 3]> {
    if doc.proof_pending() || !doc.proof_state().current_proved_model() {
        return None;
    }
    let geometry = doc.model().geometry_hash();
    cache.retain(|p| p.geometry == geometry);
    if let Some(p) = cache.iter().find(|p| p.edge == edge) {
        return p.point;
    }
    let point = super::midpoint(doc, Selection::Edge(edge)).ok().and_then(|v| {
        let c = |i: usize| v["coordinates"][i]["value"].as_f64();
        Some([c(0)?, c(1)?, c(2)?])
    });
    cache.push(Proven { edge, geometry, point });
    point
}

/// "(0, -15, 0) mm", as the panel and the hover label show a point.
pub fn format(p: [f64; 3]) -> String {
    let [x, y, z] = p.map(crate::document::fmt_mm);
    format!("({x}, {y}, {z}) mm")
}

/// Runs after the viewport each frame: aims at the hovered edge's proven midpoint while
/// Measure is open, marks it, and records viewport clicks as Measure picks.
pub fn frame(ui: &mut egui::Ui, st: &mut AppState, events: &ViewportEvents) {
    st.measure.target = None;
    if !st.measure.open || st.sketch.is_some() {
        return;
    }
    let (camera, rect) = (&st.viewport.camera, st.viewport.rect);
    let pointer = ui.input(|i| i.pointer.hover_pos()).filter(|p| rect.contains(*p));
    if let (Some(Picked::Edge(pick)), Some(pointer)) = (st.viewport.hover, pointer)
        && let Some(PickEntry::Edge(_, edge)) = st.doc.pick(pick)
        && let Some(point) = proven(&mut st.measure.proven, &st.doc, edge)
        && let Some(at) = camera.project(Vec3::new(point[0] as f32, point[1] as f32, point[2] as f32), rect)
        && at.distance(pointer) <= REACH
    {
        st.measure.target = Some((edge, point));
        mark(ui, at, point);
    }
    let Some(clicked) = events.clicked else { return };
    let selection = clicked.and_then(|p| match st.doc.pick(p.pick())? {
        PickEntry::Face(_, f) => Some(Selection::Face(f)),
        PickEntry::Edge(_, e) => Some(Selection::Edge(e)),
        PickEntry::Vertex(..) => None,
    });
    let Some(selection) = selection else {
        st.measure.picks.clear();
        return;
    };
    let midpoint = st.measure.target.is_some_and(|(edge, _)| selection == Selection::Edge(edge));
    st.measure.pick(Pick { selection, midpoint });
}

fn mark(ui: &mut egui::Ui, at: Pos2, point: [f64; 3]) {
    let painter = ui.painter();
    painter.circle(at, 5.0, theme::hud_bg(), Stroke::new(2.0, theme::accent()));
    painter.circle_filled(at, 1.8, theme::accent());
    let text = format!("Midpoint {}", format(point));
    let size = egui::vec2(text.len() as f32 * 7.0 + 12.0, 22.0);
    ui.put(
        egui::Rect::from_min_size(at + egui::vec2(10.0, -28.0), size),
        egui::Label::new(egui::RichText::new(text).color(theme::ink()).background_color(theme::hud_bg())),
    );
}
