//! The sketcher: draws the open sketch on its plane in the viewport and turns
//! pointer input into entities, relations and dimensions. Everything here is
//! in screen space on top of the 3D view; the plane maps points both ways.
//!
//! Inference happens while the pointer moves: a point within reach snaps to an
//! existing point, the middle of a line or a curve, and a new line that is
//! nearly level or plumb with its start snaps exactly and gets the relation.
//! Every snap that lands a click becomes a relation, so what the user saw is
//! what the solver keeps.

use crate::document::{Command, Document};
use crate::sketch::{
    Align, Constraint, ConstraintId, ConstraintKind, DistanceKind, Entity, EntityId, EntityKind, P2, PointRef, Sketch,
    SolveStatus, fmt_value, solve,
};
use crate::state::{AppAction, DimEdit, SketchDrag, SketchSel, SketchSession, Snap, Tool};
use crate::theme;
use crate::viewport::camera::Camera;
use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};
use glam::Vec3;
use soliddraft_kernel_api::Plane;
use std::ops::{Add, Sub};

const POINT_REACH: f32 = 8.0;
const CURVE_REACH: f32 = 6.0;
const ALIGN_REACH: f32 = 6.0;

/// Screen mapping of a sketch plane through the camera.
pub struct Frame<'a> {
    pub plane: Plane,
    pub camera: &'a Camera,
    pub rect: Rect,
}

fn gv(v: soliddraft_kernel_api::Vec3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

impl Frame<'_> {
    pub fn world(&self, p: P2) -> Vec3 {
        gv(self.plane.to_world(soliddraft_kernel_api::Point2::new(p.u, p.v)))
    }

    pub fn screen(&self, p: P2) -> Option<Pos2> {
        self.camera.project(self.world(p), self.rect)
    }

    /// Plane coordinates under a screen position.
    pub fn plane_at(&self, pos: Pos2) -> Option<P2> {
        let (o, d) = self.camera.ray(pos, self.rect);
        let n = gv(self.plane.normal);
        let denom = d.dot(n);
        if denom.abs() < 1e-6 {
            return None;
        }
        let origin = gv(self.plane.origin);
        let t = (origin - o).dot(n) / denom;
        let hit = o + d * t;
        let rel = hit - origin;
        let x = gv(self.plane.x_dir);
        let y = gv(self.plane.y_dir());
        Some(P2::new(rel.dot(x) as f64, rel.dot(y) as f64))
    }

    /// Screen pixels per millimetre at the plane origin.
    pub fn scale(&self) -> f32 {
        let a = self.screen(P2::ZERO);
        let b = self.screen(P2::new(1.0, 0.0));
        match (a, b) {
            (Some(a), Some(b)) => (b - a).length().max(1e-3),
            _ => 1.0,
        }
    }
}

fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() < 1e-6 { 0.0 } else { ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) };
    (a + ab * t - p).length()
}

/// Distance in pixels from `pos` to an entity's curve.
fn entity_distance(frame: &Frame<'_>, e: &Entity, pos: Pos2) -> f32 {
    let poly: Vec<Pos2> = e.polyline(64).into_iter().filter_map(|p| frame.screen(p)).collect();
    if poly.len() == 1 {
        return (poly[0] - pos).length();
    }
    poly.windows(2).map(|w| dist_to_segment(pos, w[0], w[1])).fold(f32::INFINITY, f32::min)
}

/// What the pointer is over, in the sketch.
fn hit_test(frame: &Frame<'_>, sketch: &Sketch, pos: Pos2, glyphs: &[(ConstraintId, Rect)]) -> Option<SketchSel> {
    let mut best_point: Option<(f32, PointRef)> = None;
    for e in &sketch.entities {
        for i in 0..e.point_count() {
            let Some(sp) = e.point(i).and_then(|p| frame.screen(p)) else { continue };
            let d = (sp - pos).length();
            if d <= POINT_REACH && best_point.is_none_or(|(bd, _)| d < bd) {
                best_point = Some((d, PointRef::new(e.id, i)));
            }
        }
    }
    if let Some((_, r)) = best_point {
        return Some(SketchSel::Point(r));
    }
    for (id, r) in glyphs {
        if r.contains(pos) {
            return Some(SketchSel::Constraint(*id));
        }
    }
    let mut best: Option<(f32, EntityId)> = None;
    for e in sketch.drawn() {
        let d = entity_distance(frame, e, pos);
        if d <= CURVE_REACH && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, e.id));
        }
    }
    best.map(|(_, id)| SketchSel::Entity(id))
}

/// Snaps a cursor position and names the inferences that moved it.
fn infer(
    frame: &Frame<'_>,
    sketch: &Sketch,
    pos: Pos2,
    prev: Option<P2>,
    ignore: &[EntityId],
) -> Option<(P2, Vec<Snap>)> {
    let raw = frame.plane_at(pos)?;
    let mut snaps = Vec::new();
    // 1. An existing point.
    let mut best: Option<(f32, PointRef, P2)> = None;
    for e in &sketch.entities {
        if ignore.contains(&e.id) {
            continue;
        }
        for i in 0..e.point_count() {
            let Some(p) = e.point(i) else { continue };
            let Some(sp) = frame.screen(p) else { continue };
            let d = (sp - pos).length();
            if d <= POINT_REACH && best.is_none_or(|(bd, _, _)| d < bd) {
                best = Some((d, PointRef::new(e.id, i), p));
            }
        }
    }
    if let Some((_, r, p)) = best {
        snaps.push(Snap::Point(r));
        return Some((p, snaps));
    }
    // 2. The middle of a line.
    for e in sketch.drawn() {
        if ignore.contains(&e.id) {
            continue;
        }
        if let EntityKind::Line { a, b } = e.kind {
            let m = a.mid(b);
            if frame.screen(m).is_some_and(|sp| (sp - pos).length() <= POINT_REACH) {
                snaps.push(Snap::Midpoint(e.id));
                return Some((m, snaps));
            }
        }
    }
    // 3. Level or plumb with the previous point.
    let mut p = raw;
    if let Some(prev) = prev
        && let (Some(sp), Some(sprev)) = (frame.screen(raw), frame.screen(prev))
    {
        let far = (sp - sprev).length() > 14.0;
        let dh = (frame.screen(P2::new(raw.u, prev.v)).unwrap_or(sp) - sp).length();
        let dv = (frame.screen(P2::new(prev.u, raw.v)).unwrap_or(sp) - sp).length();
        if far && dh <= ALIGN_REACH && dh <= dv {
            p = P2::new(raw.u, prev.v);
            snaps.push(Snap::Horizontal);
        } else if far && dv <= ALIGN_REACH {
            p = P2::new(prev.u, raw.v);
            snaps.push(Snap::Vertical);
        }
    }
    // 4. On a curve.
    if snaps.is_empty() {
        let mut best: Option<(f32, EntityId, P2)> = None;
        for e in sketch.drawn() {
            if ignore.contains(&e.id) || e.is_point() {
                continue;
            }
            let (q, _) = e.nearest(raw);
            let Some(sq) = frame.screen(q) else { continue };
            let d = (sq - pos).length();
            if d <= CURVE_REACH && best.is_none_or(|(bd, _, _)| d < bd) {
                best = Some((d, e.id, q));
            }
        }
        if let Some((_, id, q)) = best {
            snaps.push(Snap::On(id));
            p = q;
        }
    }
    Some((p, snaps))
}

/// Adds the relations a snap implies for a newly placed point.
fn apply_snaps(sketch: &mut Sketch, point: PointRef, snaps: &[Snap], new_line: Option<EntityId>) {
    for s in snaps {
        match s {
            Snap::Point(r) if r.entity != point.entity => {
                sketch.constrain(ConstraintKind::Coincident { a: point, b: *r });
            }
            Snap::Midpoint(e) => {
                sketch.constrain(ConstraintKind::Midpoint { point, line: *e });
            }
            Snap::On(e) => {
                sketch.constrain(ConstraintKind::PointOn { point, entity: *e });
            }
            Snap::Horizontal => {
                if let Some(l) = new_line {
                    sketch.constrain(ConstraintKind::Horizontal { target: Align::Line(l) });
                }
            }
            Snap::Vertical => {
                if let Some(l) = new_line {
                    sketch.constrain(ConstraintKind::Vertical { target: Align::Line(l) });
                }
            }
            _ => {}
        }
    }
}

fn commit(s: &SketchSession, actions: &mut Vec<AppAction>) {
    actions.push(AppAction::Command(Command::EditSketch { id: s.feature, sketch: s.sketch.clone() }));
}

/// Runs the sketcher for one frame: input first, then drawing.
pub fn run(
    ui: &mut egui::Ui,
    s: &mut SketchSession,
    doc: &Document,
    camera: &Camera,
    rect: Rect,
    resp: &egui::Response,
) -> Vec<AppAction> {
    let mut actions = Vec::new();
    let frame = Frame { plane: s.plane, camera, rect };
    let painter = ui.painter().with_clip_rect(rect);
    let glyphs = glyph_rects(&frame, &s.sketch);
    let hover_pos = resp.hover_pos().filter(|_| !resp.dragged());
    s.cursor = hover_pos.and_then(|p| frame.plane_at(p));

    // Hover and inference.
    s.hover = None;
    s.snaps.clear();
    let drawing = !matches!(s.tool, Tool::Select | Tool::Dimension);
    let mut snapped: Option<P2> = None;
    if let Some(pos) = hover_pos {
        if drawing {
            let prev = s.pending.last().map(|(p, _)| *p);
            let ignore: Vec<EntityId> = Vec::new();
            if let (Some(raw), Some(inferred)) = (frame.plane_at(pos), infer(&frame, &s.sketch, pos, prev, &ignore)) {
                let (p, snaps) = crate::measure::guides::offered(s.auto_relations, raw, inferred);
                snapped = Some(p);
                s.snaps = snaps;
            }
        } else {
            s.hover = hit_test(&frame, &s.sketch, pos, &glyphs);
            if s.hover.is_none()
                && let Some((id, _)) = label_rects(&frame, &s.sketch, ui).into_iter().find(|(_, r)| r.contains(pos))
            {
                s.hover = Some(SketchSel::Constraint(id));
            }
        }
    }
    if hover_pos.is_some() {
        ui.ctx().set_cursor_icon(if drawing { egui::CursorIcon::Crosshair } else { egui::CursorIcon::Default });
    }

    let modifiers = ui.input(|i| i.modifiers);
    let clicked = resp.clicked_by(egui::PointerButton::Primary);
    let double = resp.double_clicked_by(egui::PointerButton::Primary);
    let secondary = resp.secondary_clicked();

    // Tool input.
    match s.tool {
        Tool::Select => select_input(ui, s, &frame, resp, clicked, double, modifiers, &mut actions),
        Tool::Dimension => {
            if clicked && let Some(pos) = hover_pos {
                dimension_click(s, &frame, pos, &mut actions);
            }
        }
        _ => {
            if (clicked || double)
                && let Some(p) = snapped
            {
                let snaps = s.snaps.clone();
                tool_click(s, p, snaps, double, &mut actions);
            }
        }
    }
    if secondary && drawing {
        // Right click ends the current chain or tool.
        if s.pending.is_empty() {
            s.set_tool(Tool::Select);
        } else {
            s.pending.clear();
            s.chain_from = None;
        }
    }

    // Drawing.
    paint_plane(&painter, &frame, doc, &s.sketch);
    paint_regions(&painter, &frame, &s.sketch);
    paint_entities(&painter, &frame, s);
    paint_glyphs(&painter, s, &glyphs);
    paint_dimensions(&painter, &frame, s);
    paint_rubber_band(&painter, &frame, s, snapped);
    paint_snaps(&painter, &frame, s, hover_pos, snapped);
    dim_editor(ui, s, &frame, &mut actions);
    paint_status(ui, s, rect);
    crate::measure::guides::switches(ui, s, rect);
    actions
}

/// Pointer input for the select tool: clicks pick, drags move, double-clicks edit.
#[allow(clippy::too_many_arguments)]
fn select_input(
    ui: &mut egui::Ui,
    s: &mut SketchSession,
    frame: &Frame<'_>,
    resp: &egui::Response,
    clicked: bool,
    double: bool,
    modifiers: egui::Modifiers,
    actions: &mut Vec<AppAction>,
) {
    // Drag start: a point, an entity or a dimension label.
    if resp.drag_started_by(egui::PointerButton::Primary)
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let labels = label_rects(frame, &s.sketch, ui);
        let hit = hit_test(frame, &s.sketch, pos, &[]);
        let label = labels.into_iter().find(|(_, r)| r.contains(pos)).map(|(id, _)| id);
        s.drag = match (hit, label) {
            (Some(SketchSel::Point(p)), _) if p.entity != crate::sketch::ORIGIN => {
                Some(SketchDrag::Point { point: p, moved: false })
            }
            (Some(SketchSel::Entity(e)), _) => {
                frame.plane_at(pos).map(|last| SketchDrag::Entity { entity: e, last, moved: false })
            }
            (_, Some(c)) => {
                let label = s.sketch.constraint(c).and_then(|c| c.label()).unwrap_or(P2::ZERO);
                let at = frame.plane_at(pos).unwrap_or(label);
                Some(SketchDrag::Label { constraint: c, offset: label.sub(at) })
            }
            _ => None,
        };
    }
    if resp.dragged_by(egui::PointerButton::Primary)
        && let Some(pos) = resp.interact_pointer_pos()
        && let Some(at) = frame.plane_at(pos)
    {
        match s.drag.clone() {
            Some(SketchDrag::Point { point, .. }) => {
                solve::solve(&mut s.sketch, Some(solve::Drag { point, to: at }), false);
                s.drag = Some(SketchDrag::Point { point, moved: true });
            }
            Some(SketchDrag::Entity { entity, last, .. }) => {
                let d = at.sub(last);
                if let Some(e) = s.sketch.entity_mut(entity) {
                    e.translate(d);
                }
                // Fixed-value relations hold; the rest of the sketch follows the moved entity.
                solve::solve(&mut s.sketch, None, false);
                s.drag = Some(SketchDrag::Entity { entity, last: at, moved: true });
            }
            Some(SketchDrag::Label { constraint, offset }) => {
                if let Some(c) = s.sketch.constraint_mut(constraint) {
                    c.set_label(at.add(offset));
                }
            }
            None => {}
        }
    }
    if resp.drag_stopped_by(egui::PointerButton::Primary) {
        match s.drag.take() {
            Some(SketchDrag::Point { moved: true, .. }) | Some(SketchDrag::Entity { moved: true, .. }) => {
                solve::solve(&mut s.sketch, None, true);
                commit(s, actions);
            }
            Some(SketchDrag::Label { .. }) => commit(s, actions),
            _ => {}
        }
    }
    if double {
        if let Some(SketchSel::Constraint(c)) = s.hover
            && let Some(con) = s.sketch.constraint(c)
            && con.is_dimension()
        {
            s.dim_edit = Some(DimEdit { constraint: c, text: fmt_value(con.value().unwrap_or(0.0)), focus: true });
        }
        return;
    }
    if clicked {
        match s.hover {
            Some(h) => {
                if modifiers.ctrl || modifiers.shift {
                    if let Some(i) = s.selection.iter().position(|x| *x == h) {
                        s.selection.remove(i);
                    } else {
                        s.selection.push(h);
                    }
                } else {
                    s.selection = vec![h];
                }
            }
            None => s.selection.clear(),
        }
    }
}

/// A click with a drawing tool.
fn tool_click(s: &mut SketchSession, p: P2, snaps: Vec<Snap>, double: bool, actions: &mut Vec<AppAction>) {
    match s.tool {
        Tool::Point => {
            let id = s.sketch.add(EntityKind::Point { p }, false);
            apply_snaps(&mut s.sketch, PointRef::new(id, 0), &snaps, None);
            solve::solve(&mut s.sketch, None, true);
            commit(s, actions);
        }
        Tool::Line => {
            if double {
                // A double click ends the chain; the first click of it already landed.
                s.pending.clear();
                s.chain_from = None;
                return;
            }
            match s.pending.pop() {
                None => s.pending.push((p, snaps)),
                Some((a, a_snaps)) => {
                    if a.dist(p) < 1e-6 {
                        s.pending.push((a, a_snaps));
                        return;
                    }
                    let id = s.sketch.add(EntityKind::Line { a, b: p }, s.construction);
                    apply_snaps(&mut s.sketch, PointRef::new(id, 0), &a_snaps, Some(id));
                    apply_snaps(&mut s.sketch, PointRef::new(id, 1), &snaps, Some(id));
                    solve::solve(&mut s.sketch, None, true);
                    commit(s, actions);
                    let closed = snaps.iter().any(|x| matches!(x, Snap::Point(_)));
                    if closed {
                        s.chain_from = None;
                    } else {
                        let end = PointRef::new(id, 1);
                        s.chain_from = Some(end);
                        s.pending.push((p, vec![Snap::Point(end)]));
                    }
                }
            }
        }
        Tool::Rectangle => match s.pending.pop() {
            None => s.pending.push((p, snaps)),
            Some((a, a_snaps)) => {
                if (a.u - p.u).abs() < 1e-6 || (a.v - p.v).abs() < 1e-6 {
                    s.pending.push((a, a_snaps));
                    return;
                }
                let c = [a, P2::new(p.u, a.v), p, P2::new(a.u, p.v)];
                let ids: Vec<EntityId> = (0..4)
                    .map(|i| s.sketch.add(EntityKind::Line { a: c[i], b: c[(i + 1) % 4] }, s.construction))
                    .collect();
                for i in 0..4 {
                    s.sketch.constrain(ConstraintKind::Coincident {
                        a: PointRef::new(ids[i], 1),
                        b: PointRef::new(ids[(i + 1) % 4], 0),
                    });
                }
                s.sketch.constrain(ConstraintKind::Horizontal { target: Align::Line(ids[0]) });
                s.sketch.constrain(ConstraintKind::Horizontal { target: Align::Line(ids[2]) });
                s.sketch.constrain(ConstraintKind::Vertical { target: Align::Line(ids[1]) });
                s.sketch.constrain(ConstraintKind::Vertical { target: Align::Line(ids[3]) });
                let keep = |v: &Vec<Snap>| {
                    v.iter().filter(|x| !matches!(x, Snap::Horizontal | Snap::Vertical)).cloned().collect::<Vec<_>>()
                };
                apply_snaps(&mut s.sketch, PointRef::new(ids[0], 0), &keep(&a_snaps), None);
                apply_snaps(&mut s.sketch, PointRef::new(ids[2], 0), &keep(&snaps), None);
                solve::solve(&mut s.sketch, None, true);
                commit(s, actions);
            }
        },
        Tool::Circle => match s.pending.pop() {
            None => s.pending.push((p, snaps)),
            Some((c, c_snaps)) => {
                let r = c.dist(p);
                if r < 1e-6 {
                    s.pending.push((c, c_snaps));
                    return;
                }
                let id = s.sketch.add(EntityKind::Circle { center: c, radius: r }, s.construction);
                apply_snaps(&mut s.sketch, PointRef::new(id, 0), &c_snaps, None);
                if let Some(Snap::Point(r)) = snaps.iter().find(|x| matches!(x, Snap::Point(_))) {
                    s.sketch.constrain(ConstraintKind::PointOn { point: *r, entity: id });
                }
                solve::solve(&mut s.sketch, None, true);
                commit(s, actions);
            }
        },
        Tool::Arc => {
            if s.pending.len() < 2 {
                s.pending.push((p, snaps));
                return;
            }
            let (start, start_snaps) = s.pending.remove(1);
            let (c, c_snaps) = s.pending.remove(0);
            let r = c.dist(start);
            if r < 1e-6 || c.dist(p) < 1e-6 {
                return;
            }
            let end = c.add(p.sub(c).normalized().scale(r));
            let mut sweep = end.sub(c).angle() - start.sub(c).angle();
            while sweep < 0.0 {
                sweep += std::f64::consts::TAU;
            }
            // Arcs are stored counter-clockwise; a clockwise pick swaps the ends.
            let (a, b, a_snaps, b_snaps) = if sweep <= std::f64::consts::PI {
                (start, end, start_snaps, snaps)
            } else {
                (end, start, snaps, start_snaps)
            };
            let id = s.sketch.add(EntityKind::Arc { center: c, start: a, end: b }, s.construction);
            apply_snaps(&mut s.sketch, PointRef::new(id, 0), &c_snaps, None);
            apply_snaps(&mut s.sketch, PointRef::new(id, 1), &a_snaps, None);
            apply_snaps(&mut s.sketch, PointRef::new(id, 2), &b_snaps, None);
            solve::solve(&mut s.sketch, None, true);
            commit(s, actions);
        }
        Tool::Slot => {
            if s.pending.len() < 2 {
                s.pending.push((p, snaps));
                return;
            }
            let (c1, c1_snaps) = s.pending.remove(1);
            let (c0, c0_snaps) = s.pending.remove(0);
            let d = c1.sub(c0);
            if d.len() < 1e-6 {
                return;
            }
            let n = d.normalized().perp();
            let r = p.sub(c0).dot(n).abs().max(d.len() * 0.05);
            let top = s.sketch.add(EntityKind::Line { a: c0.add(n.scale(r)), b: c1.add(n.scale(r)) }, s.construction);
            let right = s.sketch.add(
                EntityKind::Arc { center: c1, start: c1.sub(n.scale(r)), end: c1.add(n.scale(r)) },
                s.construction,
            );
            let bottom =
                s.sketch.add(EntityKind::Line { a: c1.sub(n.scale(r)), b: c0.sub(n.scale(r)) }, s.construction);
            let left = s.sketch.add(
                EntityKind::Arc { center: c0, start: c0.add(n.scale(r)), end: c0.sub(n.scale(r)) },
                s.construction,
            );
            let axis = s.sketch.add(EntityKind::Line { a: c0, b: c1 }, true);
            for (a, b) in [
                (PointRef::new(top, 1), PointRef::new(right, 2)),
                (PointRef::new(right, 1), PointRef::new(bottom, 0)),
                (PointRef::new(bottom, 1), PointRef::new(left, 2)),
                (PointRef::new(left, 1), PointRef::new(top, 0)),
                (PointRef::new(axis, 0), PointRef::new(left, 0)),
                (PointRef::new(axis, 1), PointRef::new(right, 0)),
            ] {
                s.sketch.constrain(ConstraintKind::Coincident { a, b });
            }
            for (a, b) in [(top, right), (bottom, right), (top, left), (bottom, left)] {
                s.sketch.constrain(ConstraintKind::Tangent { a, b });
            }
            s.sketch.constrain(ConstraintKind::Equal { a: left, b: right });
            apply_snaps(&mut s.sketch, PointRef::new(axis, 0), &c0_snaps, Some(axis));
            apply_snaps(&mut s.sketch, PointRef::new(axis, 1), &c1_snaps, Some(axis));
            solve::solve(&mut s.sketch, None, true);
            commit(s, actions);
        }
        Tool::Select | Tool::Dimension => {}
    }
}

/// Smart dimension: pick one or two items, then place the value.
fn dimension_click(s: &mut SketchSession, frame: &Frame<'_>, pos: Pos2, actions: &mut Vec<AppAction>) {
    let hit = hit_test(frame, &s.sketch, pos, &[]).filter(|h| !matches!(h, SketchSel::Constraint(_)));
    let Some(at) = frame.plane_at(pos) else { return };
    match (s.dim_first, hit) {
        (None, Some(h)) => {
            s.dim_first = Some(h);
            s.say(match h {
                SketchSel::Entity(_) => "Click to place the dimension, or pick a second entity",
                _ => "Pick a second point or line, or click to place a distance to the origin",
            });
        }
        (None, None) => s.say("Pick a line, arc, circle or point to dimension"),
        (Some(first), Some(second)) if second != first && s.dim_second.is_none() && !is_curve_sel(&s.sketch, first) => {
            s.dim_second = Some(second);
            s.say("Click to place the dimension");
        }
        (Some(first), _) => {
            let kind = dimension_kind(&s.sketch, first, s.dim_second, at);
            s.dim_first = None;
            s.dim_second = None;
            if let Some(kind) = kind {
                let id = s.sketch.constrain(kind);
                solve::solve(&mut s.sketch, None, true);
                commit(s, actions);
                let value = s.sketch.constraint(id).and_then(|c| c.value()).unwrap_or(0.0);
                s.dim_edit = Some(DimEdit { constraint: id, text: fmt_value(value), focus: true });
            } else {
                s.say("Those two cannot be dimensioned together");
            }
        }
    }
}

fn is_curve_sel(sketch: &Sketch, sel: SketchSel) -> bool {
    matches!(sel, SketchSel::Entity(e) if sketch.entity(e).is_some_and(|e| e.is_curve()))
}

/// The dimension two picks and a placement call for, measured from the sketch.
fn dimension_kind(sketch: &Sketch, first: SketchSel, second: Option<SketchSel>, label: P2) -> Option<ConstraintKind> {
    let ent = |sel: SketchSel| match sel {
        SketchSel::Entity(e) => sketch.entity(e).copied(),
        _ => None,
    };
    let point_of = |sel: SketchSel| match sel {
        SketchSel::Point(p) => Some(p),
        SketchSel::Entity(e) => sketch.entity(e).filter(|e| e.is_point()).map(|_| PointRef::new(e, 0)),
        _ => None,
    };
    let between = |a: PointRef, b: PointRef| {
        let (pa, pb) = (sketch.point(a)?, sketch.point(b)?);
        let (lo, hi) = (P2::new(pa.u.min(pb.u), pa.v.min(pb.v)), P2::new(pa.u.max(pb.u), pa.v.max(pb.v)));
        let axis_aligned = (pa.u - pb.u).abs() < 1e-6 || (pa.v - pb.v).abs() < 1e-6;
        let kind = if axis_aligned {
            DistanceKind::Aligned
        } else if label.u < lo.u || label.u > hi.u {
            DistanceKind::Vertical
        } else if label.v < lo.v || label.v > hi.v {
            DistanceKind::Horizontal
        } else {
            DistanceKind::Aligned
        };
        let value = match kind {
            DistanceKind::Aligned => pa.dist(pb),
            DistanceKind::Horizontal => (pa.u - pb.u).abs(),
            DistanceKind::Vertical => (pa.v - pb.v).abs(),
        };
        Some(ConstraintKind::Distance { a, b, kind, value, label })
    };
    match (first, second) {
        (SketchSel::Entity(e), None) => {
            let ent = sketch.entity(e)?;
            match ent.kind {
                EntityKind::Line { .. } => between(PointRef::new(e, 0), PointRef::new(e, 1)),
                EntityKind::Circle { radius, .. } => {
                    Some(ConstraintKind::Radius { entity: e, value: radius * 2.0, diameter: true, label })
                }
                EntityKind::Arc { .. } => {
                    Some(ConstraintKind::Radius { entity: e, value: ent.radius()?, diameter: false, label })
                }
                EntityKind::Point { .. } => between(PointRef::new(e, 0), PointRef::ORIGIN),
            }
        }
        (SketchSel::Point(p), None) => between(p, PointRef::ORIGIN),
        (a, Some(b)) => {
            if let (Some(pa), Some(pb)) = (point_of(a), point_of(b)) {
                return between(pa, pb);
            }
            match (ent(a), ent(b)) {
                (Some(la), Some(lb)) if la.is_line() && lb.is_line() => {
                    let (EntityKind::Line { a: a0, b: a1 }, EntityKind::Line { a: b0, b: b1 }) = (la.kind, lb.kind)
                    else {
                        return None;
                    };
                    let (da, db) = (a1.sub(a0).normalized(), b1.sub(b0).normalized());
                    if da.cross(db).abs() < 1e-3 {
                        let foot = lb.nearest(a0).0;
                        return Some(ConstraintKind::PointLineDistance {
                            point: PointRef::new(la.id, 0),
                            line: lb.id,
                            value: a0.dist(foot),
                            label,
                        });
                    }
                    let mut deg = (db.angle() - da.angle()).to_degrees().rem_euclid(360.0);
                    if deg > 180.0 {
                        deg -= 180.0;
                    }
                    Some(ConstraintKind::Angle { a: la.id, b: lb.id, value: deg, label })
                }
                (Some(l), _) if l.is_line() => {
                    let p = point_of(b)?;
                    let v = l.nearest(sketch.point(p)?).0.dist(sketch.point(p)?);
                    Some(ConstraintKind::PointLineDistance { point: p, line: l.id, value: v, label })
                }
                (_, Some(l)) if l.is_line() => {
                    let p = point_of(a)?;
                    let v = l.nearest(sketch.point(p)?).0.dist(sketch.point(p)?);
                    Some(ConstraintKind::PointLineDistance { point: p, line: l.id, value: v, label })
                }
                _ => None,
            }
        }
        (SketchSel::Constraint(_), _) => None,
    }
}

// Painting ---------------------------------------------------------------------

fn entity_color(s: &SketchSession, e: &Entity) -> Color32 {
    let conflicting = match &s.sketch.status {
        SolveStatus::OverDefined { conflicting } => conflicting.clone(),
        _ => Vec::new(),
    };
    let in_conflict = s.sketch.constraints_of(e.id).any(|c| conflicting.contains(&c.id));
    if in_conflict {
        theme::error()
    } else if e.construction {
        theme::muted()
    } else if s.sketch.free.contains(&e.id) {
        theme::accent()
    } else {
        theme::ink()
    }
}

fn paint_plane(painter: &egui::Painter, frame: &Frame<'_>, doc: &Document, sketch: &Sketch) {
    let mut half = 100.0_f64;
    if let Some((lo, hi)) = doc.bounds() {
        half = half.max(((hi - lo).length() as f64) * 0.65);
    }
    if let Some((lo, hi)) = sketch.bounds() {
        half = half.max(lo.u.abs().max(lo.v.abs()).max(hi.u.abs()).max(hi.v.abs()) * 1.3);
    }
    let corners = [P2::new(-half, -half), P2::new(half, -half), P2::new(half, half), P2::new(-half, half)];
    let pts: Vec<Pos2> = corners.iter().filter_map(|p| frame.screen(*p)).collect();
    if pts.len() == 4 {
        painter.add(egui::Shape::convex_polygon(
            pts.clone(),
            theme::accent_alpha(10),
            Stroke::new(1.0, theme::accent().gamma_multiply(0.35)),
        ));
    }
    // Axes of the sketch frame through the origin.
    let axis = |a: P2, b: P2, color: Color32| {
        if let (Some(p), Some(q)) = (frame.screen(a), frame.screen(b)) {
            painter.line_segment([p, q], Stroke::new(1.0, color));
        }
    };
    axis(P2::new(-half, 0.0), P2::new(half, 0.0), theme::line().gamma_multiply(1.3));
    axis(P2::new(0.0, -half), P2::new(0.0, half), theme::line().gamma_multiply(1.3));
}

fn paint_regions(painter: &egui::Painter, frame: &Frame<'_>, sketch: &Sketch) {
    for outline in crate::sketch::profile::region_outlines(sketch) {
        let pts: Vec<Pos2> = outline.iter().filter_map(|p| frame.screen(*p)).collect();
        if pts.len() >= 3 {
            let mesh = polygon_fill(&pts, theme::accent_alpha(18));
            painter.add(mesh);
        }
    }
}

/// A fan triangulation is enough for the convex and mildly concave shapes a
/// sketch draws; the fill is a hint, not geometry.
fn polygon_fill(pts: &[Pos2], color: Color32) -> egui::Shape {
    let mut mesh = egui::Mesh::default();
    let c = pts.iter().fold(egui::Vec2::ZERO, |a, p| a + p.to_vec2()) / pts.len() as f32;
    mesh.vertices.push(egui::epaint::Vertex { pos: c.to_pos2(), uv: egui::epaint::WHITE_UV, color });
    for p in pts {
        mesh.vertices.push(egui::epaint::Vertex { pos: *p, uv: egui::epaint::WHITE_UV, color });
    }
    let n = pts.len() as u32;
    for i in 0..n {
        mesh.indices.extend([0, 1 + i, 1 + (i + 1) % n]);
    }
    egui::Shape::mesh(mesh)
}

fn dashed(painter: &egui::Painter, pts: &[Pos2], stroke: Stroke) {
    for w in pts.windows(2) {
        painter.add(egui::Shape::dashed_line(&[w[0], w[1]], stroke, 5.0, 4.0));
    }
}

fn paint_entities(painter: &egui::Painter, frame: &Frame<'_>, s: &SketchSession) {
    let selected = |sel: SketchSel| s.selection.contains(&sel);
    for e in s.sketch.drawn() {
        let color = entity_color(s, e);
        let is_sel = selected(SketchSel::Entity(e.id));
        let is_hover = s.hover == Some(SketchSel::Entity(e.id));
        let pts: Vec<Pos2> = e.polyline(96).into_iter().filter_map(|p| frame.screen(p)).collect();
        if is_sel || is_hover {
            let halo = Stroke::new(
                if is_sel { 6.0 } else { 5.0 },
                theme::selection_bg().gamma_multiply(if is_sel { 1.0 } else { 0.7 }),
            );
            if pts.len() >= 2 {
                painter.add(egui::Shape::line(pts.clone(), halo));
            }
        }
        let stroke = Stroke::new(if e.construction { 1.0 } else { 1.8 }, if is_sel { theme::accent() } else { color });
        if e.is_point() {
            continue;
        }
        if pts.len() >= 2 {
            if e.construction {
                dashed(painter, &pts, stroke);
            } else {
                painter.add(egui::Shape::line(pts, stroke));
            }
        }
    }
    // Points: filled when joined to something, hollow when free.
    for e in &s.sketch.entities {
        let color = entity_color(s, e);
        for i in 0..e.point_count() {
            let r = PointRef::new(e.id, i);
            let Some(sp) = e.point(i).and_then(|p| frame.screen(p)) else { continue };
            let is_center = matches!(e.kind, EntityKind::Arc { .. } | EntityKind::Circle { .. }) && i == 0;
            let is_sel = selected(SketchSel::Point(r));
            let is_hover = s.hover == Some(SketchSel::Point(r));
            let joined = s.sketch.constraints.iter().any(|c| c.points().contains(&r));
            if e.id == crate::sketch::ORIGIN {
                painter.line_segment([sp + vec2(-6.0, 0.0), sp + vec2(6.0, 0.0)], Stroke::new(1.2, theme::muted()));
                painter.line_segment([sp + vec2(0.0, -6.0), sp + vec2(0.0, 6.0)], Stroke::new(1.2, theme::muted()));
                painter.circle_stroke(sp, 3.0, Stroke::new(1.2, theme::muted()));
                continue;
            }
            if is_sel || is_hover {
                painter.circle_filled(sp, 7.0, theme::selection_bg());
            }
            let fill = if is_sel { theme::accent() } else { color };
            if is_center {
                painter.line_segment([sp + vec2(-4.0, 0.0), sp + vec2(4.0, 0.0)], Stroke::new(1.2, fill));
                painter.line_segment([sp + vec2(0.0, -4.0), sp + vec2(0.0, 4.0)], Stroke::new(1.2, fill));
            } else if joined || e.is_point() {
                painter.rect_filled(Rect::from_center_size(sp, vec2(5.0, 5.0)), CornerRadius::ZERO, fill);
            } else {
                painter.rect(
                    Rect::from_center_size(sp, vec2(5.0, 5.0)),
                    CornerRadius::ZERO,
                    theme::panel(),
                    Stroke::new(1.2, fill),
                    StrokeKind::Middle,
                );
            }
        }
    }
}

/// Where each relation's glyph sits on screen.
fn glyph_rects(frame: &Frame<'_>, sketch: &Sketch) -> Vec<(ConstraintId, Rect)> {
    let mut out = Vec::new();
    let mut per_anchor: std::collections::HashMap<(i32, i32), usize> = std::collections::HashMap::new();
    for c in &sketch.constraints {
        if c.is_dimension() {
            continue;
        }
        for anchor in glyph_anchors(sketch, c) {
            let Some(sp) = frame.screen(anchor.0) else { continue };
            let base = sp + anchor.1;
            let key = ((base.x / 4.0) as i32, (base.y / 4.0) as i32);
            let n = per_anchor.entry(key).or_insert(0);
            let pos = base + vec2(16.0 * *n as f32, 0.0);
            *n += 1;
            out.push((c.id, Rect::from_center_size(pos, vec2(14.0, 14.0))));
        }
    }
    out
}

/// Sketch point and screen offset where a relation's glyph goes, one per
/// entity it relates.
fn glyph_anchors(sketch: &Sketch, c: &Constraint) -> Vec<(P2, egui::Vec2)> {
    let beside = |id: EntityId| -> Option<(P2, egui::Vec2)> {
        let e = sketch.entity(id)?;
        Some((e.midpoint(), vec2(0.0, -14.0)))
    };
    let at_point = |r: PointRef| sketch.point(r).map(|p| (p, vec2(10.0, -10.0)));
    match c.kind {
        ConstraintKind::Coincident { .. } => Vec::new(),
        ConstraintKind::PointOn { point, .. }
        | ConstraintKind::Midpoint { point, .. }
        | ConstraintKind::Fix { point, .. } => at_point(point).into_iter().collect(),
        ConstraintKind::Horizontal { target } | ConstraintKind::Vertical { target } => match target {
            Align::Line(l) => beside(l).into_iter().collect(),
            Align::Points(a, _) => at_point(a).into_iter().collect(),
        },
        ConstraintKind::Parallel { a, b }
        | ConstraintKind::Perpendicular { a, b }
        | ConstraintKind::Tangent { a, b }
        | ConstraintKind::Equal { a, b }
        | ConstraintKind::Concentric { a, b } => beside(a).into_iter().chain(beside(b)).collect(),
        _ => Vec::new(),
    }
}

fn paint_glyphs(painter: &egui::Painter, s: &SketchSession, glyphs: &[(ConstraintId, Rect)]) {
    let conflicting = match &s.sketch.status {
        SolveStatus::OverDefined { conflicting } => conflicting.clone(),
        _ => Vec::new(),
    };
    for (id, r) in glyphs {
        let Some(c) = s.sketch.constraint(*id) else { continue };
        let sel = s.selection.contains(&SketchSel::Constraint(*id));
        let hover = s.hover == Some(SketchSel::Constraint(*id));
        let bad = conflicting.contains(id);
        let (bg, fg) = if sel {
            (theme::accent(), theme::on_accent())
        } else if bad {
            (theme::error(), theme::on_accent())
        } else if hover {
            (theme::selection_bg(), theme::selection_ink())
        } else {
            (theme::hud_bg(), theme::muted())
        };
        painter.rect(
            *r,
            CornerRadius::same(3),
            bg,
            Stroke::new(1.0, if sel || bad { bg } else { theme::line() }),
            StrokeKind::Inside,
        );
        let g = r.shrink(3.0);
        let st = Stroke::new(1.3, fg);
        let (l, t, rr, b) = (g.left(), g.top(), g.right(), g.bottom());
        let (cx, cy) = (g.center().x, g.center().y);
        match c.kind {
            ConstraintKind::Horizontal { .. } => {
                painter.text(g.center(), Align2::CENTER_CENTER, "H", theme::font_semibold(9.5), fg);
            }
            ConstraintKind::Vertical { .. } => {
                painter.text(g.center(), Align2::CENTER_CENTER, "V", theme::font_semibold(9.5), fg);
            }
            ConstraintKind::Parallel { .. } => {
                painter.line_segment([pos2(l + 1.0, b), pos2(cx - 0.5, t)], st);
                painter.line_segment([pos2(cx + 1.5, b), pos2(rr, t)], st);
            }
            ConstraintKind::Perpendicular { .. } => {
                painter.line_segment([pos2(l, b), pos2(rr, b)], st);
                painter.line_segment([pos2(cx, b), pos2(cx, t)], st);
            }
            ConstraintKind::Equal { .. } => {
                painter.line_segment([pos2(l, cy - 2.0), pos2(rr, cy - 2.0)], st);
                painter.line_segment([pos2(l, cy + 2.0), pos2(rr, cy + 2.0)], st);
            }
            ConstraintKind::Tangent { .. } => {
                painter.circle_stroke(pos2(cx, cy + 1.0), 3.0, st);
                painter.line_segment([pos2(l, cy - 2.0), pos2(rr, cy - 2.0)], st);
            }
            ConstraintKind::Concentric { .. } => {
                painter.circle_stroke(g.center(), 3.5, st);
                painter.circle_stroke(g.center(), 1.2, st);
            }
            ConstraintKind::Midpoint { .. } => {
                painter.line_segment([pos2(l, cy), pos2(rr, cy)], st);
                painter.circle_filled(g.center(), 1.8, fg);
            }
            ConstraintKind::PointOn { .. } => {
                painter.line_segment([pos2(l, b - 1.0), pos2(rr, t + 1.0)], st);
                painter.circle_filled(g.center(), 1.8, fg);
            }
            ConstraintKind::Fix { .. } => {
                painter.rect_stroke(
                    Rect::from_min_max(pos2(l + 1.0, cy - 0.5), pos2(rr - 1.0, b)),
                    CornerRadius::same(1),
                    st,
                    StrokeKind::Middle,
                );
                painter.line_segment([pos2(cx, t), pos2(cx, cy)], st);
            }
            _ => {}
        }
    }
}

pub fn arrow(painter: &egui::Painter, tip: Pos2, dir: egui::Vec2, color: Color32) {
    let d = dir.normalized();
    let side = vec2(-d.y, d.x);
    painter.add(egui::Shape::convex_polygon(
        vec![tip, tip - d * 8.0 + side * 2.6, tip - d * 8.0 - side * 2.6],
        color,
        Stroke::NONE,
    ));
}

/// Screen rectangles of the dimension labels.
fn label_rects(frame: &Frame<'_>, sketch: &Sketch, ui: &egui::Ui) -> Vec<(ConstraintId, Rect)> {
    sketch
        .constraints
        .iter()
        .filter(|c| c.is_dimension())
        .filter_map(|c| {
            let sp = frame.screen(c.label()?)?;
            let text = dim_text(c);
            let galley = ui.painter().layout_no_wrap(text, theme::font_medium(11.5), theme::ink());
            Some((c.id, Rect::from_center_size(sp, galley.size() + vec2(12.0, 6.0))))
        })
        .collect()
}

fn dim_text(c: &Constraint) -> String {
    match c.kind {
        ConstraintKind::Radius { value, diameter: true, .. } => format!("Ø{}", fmt_value(value)),
        ConstraintKind::Radius { value, diameter: false, .. } => format!("R{}", fmt_value(value)),
        ConstraintKind::Angle { value, .. } => format!("{}°", fmt_value(value)),
        _ => fmt_value(c.value().unwrap_or(0.0)),
    }
}

fn paint_dimensions(painter: &egui::Painter, frame: &Frame<'_>, s: &SketchSession) {
    let conflicting = match &s.sketch.status {
        SolveStatus::OverDefined { conflicting } => conflicting.clone(),
        _ => Vec::new(),
    };
    for c in s.sketch.constraints.iter().filter(|c| c.is_dimension()) {
        let sel = s.selection.contains(&SketchSel::Constraint(c.id));
        let hover = s.hover == Some(SketchSel::Constraint(c.id));
        let bad = conflicting.contains(&c.id);
        let color = if bad {
            theme::error()
        } else if sel {
            theme::accent()
        } else {
            theme::muted().gamma_multiply(1.15)
        };
        let Some(label) = c.label() else { continue };
        let Some(lp) = frame.screen(label) else { continue };
        paint_dimension_lines(painter, frame, &s.sketch, c, label, lp, color);
        let bg = if sel {
            theme::selection_bg()
        } else if hover {
            theme::hover()
        } else {
            theme::hud_bg()
        };
        let ink = if bad { theme::error() } else { theme::ink() };
        value_pill(
            painter,
            lp,
            dim_text(c),
            ink,
            bg,
            Stroke::new(1.0, if sel { theme::accent() } else { theme::line() }),
        );
    }
}

/// Draws a dimension's extension and dimension lines, arrows and leader in `color`.
fn paint_dimension_lines(
    painter: &egui::Painter,
    frame: &Frame<'_>,
    sk: &Sketch,
    c: &Constraint,
    label: P2,
    lp: Pos2,
    color: Color32,
) {
    let st = Stroke::new(1.0, color);
    match c.kind {
        ConstraintKind::Distance { a, b, kind, .. } => {
            let (Some(pa), Some(pb)) = (sk.point(a), sk.point(b)) else { return };
            // Dimension line end points in plane coordinates.
            let (da, db) = match kind {
                DistanceKind::Aligned => {
                    let dir = pb.sub(pa).normalized();
                    let n = dir.perp();
                    let off = label.sub(pa).dot(n);
                    (pa.add(n.scale(off)), pb.add(n.scale(off)))
                }
                DistanceKind::Horizontal => (P2::new(pa.u, label.v), P2::new(pb.u, label.v)),
                DistanceKind::Vertical => (P2::new(label.u, pa.v), P2::new(label.u, pb.v)),
            };
            let (Some(sa), Some(sb), Some(spa), Some(spb)) =
                (frame.screen(da), frame.screen(db), frame.screen(pa), frame.screen(pb))
            else {
                return;
            };
            let ext = |from: Pos2, to: Pos2| {
                let d = (to - from).normalized();
                painter.line_segment([from + d * 4.0, to + d * 5.0], Stroke::new(0.8, color.gamma_multiply(0.8)));
            };
            ext(spa, sa);
            ext(spb, sb);
            painter.line_segment([sa, sb], st);
            let dir = sb - sa;
            if dir.length() > 20.0 {
                arrow(painter, sa, -dir, color);
                arrow(painter, sb, dir, color);
            }
        }
        ConstraintKind::PointLineDistance { point, line, .. } => {
            let (Some(p), Some(l)) = (sk.point(point), sk.entity(line)) else { return };
            let EntityKind::Line { a, b } = l.kind else { return };
            // The foot on the infinite line, so the dimension works past the ends.
            let dir = b.sub(a).normalized();
            let foot = a.add(dir.scale(p.sub(a).dot(dir)));
            let along = label.sub(foot).dot(dir);
            let (da, db) = (p.add(dir.scale(along)), foot.add(dir.scale(along)));
            let (Some(sa), Some(sb), Some(sp), Some(sf)) =
                (frame.screen(da), frame.screen(db), frame.screen(p), frame.screen(foot))
            else {
                return;
            };
            painter.line_segment([sp, sa], Stroke::new(0.8, color.gamma_multiply(0.8)));
            painter.line_segment([sf, sb], Stroke::new(0.8, color.gamma_multiply(0.8)));
            painter.line_segment([sa, sb], st);
            let d = sb - sa;
            if d.length() > 20.0 {
                arrow(painter, sa, -d, color);
                arrow(painter, sb, d, color);
            }
        }
        ConstraintKind::Angle { a, b, .. } => {
            let (Some(la), Some(lb)) = (sk.entity(a), sk.entity(b)) else { return };
            let (EntityKind::Line { a: a0, b: a1 }, EntityKind::Line { a: b0, b: b1 }) = (la.kind, lb.kind) else {
                return;
            };
            // Intersection of the two lines.
            let (d1, d2) = (a1.sub(a0), b1.sub(b0));
            let den = d1.cross(d2);
            if den.abs() < 1e-9 {
                return;
            }
            let t = b0.sub(a0).cross(d2) / den;
            let x = a0.add(d1.scale(t));
            let r = label.dist(x);
            let ang = |d: P2| {
                // Point each direction toward the label's side.
                let d = d.normalized();
                if label.sub(x).dot(d) < 0.0 { d.scale(-1.0).angle() } else { d.angle() }
            };
            let (mut a_from, mut a_to) = (ang(d1), ang(d2));
            if a_to < a_from {
                std::mem::swap(&mut a_from, &mut a_to);
            }
            if a_to - a_from > std::f64::consts::PI {
                std::mem::swap(&mut a_from, &mut a_to);
                a_to += std::f64::consts::TAU;
            }
            let n = 24;
            let pts: Vec<Pos2> = (0..=n)
                .filter_map(|i| {
                    frame.screen(x.add(P2::from_angle(a_from + (a_to - a_from) * i as f64 / n as f64).scale(r)))
                })
                .collect();
            if pts.len() >= 2 {
                painter.add(egui::Shape::line(pts, st));
            }
            for d in [d1, d2] {
                let end = x.add(P2::from_angle(ang(d)).scale(r * 1.1));
                if let (Some(sx), Some(se)) = (frame.screen(x), frame.screen(end)) {
                    painter.line_segment([sx, se], Stroke::new(0.8, color.gamma_multiply(0.8)));
                }
            }
        }
        ConstraintKind::Radius { entity, diameter, .. } => {
            let Some(e) = sk.entity(entity) else { return };
            let (Some(center), Some(r)) = (e.point(0), e.radius()) else { return };
            let dir = label.sub(center).normalized();
            let on = center.add(dir.scale(r));
            let (Some(son), Some(sc)) = (frame.screen(on), frame.screen(center)) else { return };
            if diameter {
                let opposite = center.sub(dir.scale(r));
                if let Some(so) = frame.screen(opposite) {
                    painter.line_segment([so, son], st);
                    arrow(painter, so, so - son, color);
                }
            } else {
                painter.line_segment([sc, son], st);
            }
            arrow(painter, son, son - sc, color);
            painter.line_segment([son, lp], st);
        }
        _ => {}
    }
}

/// Draws a dimension's value pill centred on `at` and returns its rectangle.
pub fn value_pill(painter: &egui::Painter, at: Pos2, text: String, ink: Color32, bg: Color32, stroke: Stroke) -> Rect {
    let galley = painter.layout_no_wrap(text, theme::font_medium(11.5), ink);
    let r = Rect::from_center_size(at, galley.size() + vec2(12.0, 6.0));
    painter.rect(r, CornerRadius::same(4), bg, stroke, StrokeKind::Inside);
    painter.galley(r.center() - galley.size() / 2.0, galley, ink);
    r
}

/// Draws the dimensions of a sketch that is not open, on its plane, and returns each
/// value's rectangle with its constraint. A value that would cover one of the `taken`
/// values, or an earlier one of this sketch, moves clear of it on a short leader. A value
/// under `pointer` shows as hovered.
pub fn paint_closed_dimensions(
    painter: &egui::Painter,
    camera: &Camera,
    rect: Rect,
    plane: Plane,
    sketch: &Sketch,
    taken: &[Rect],
    pointer: Option<Pos2>,
) -> Vec<(ConstraintId, Rect)> {
    let frame = Frame { plane, camera, rect };
    let mut taken = taken.to_vec();
    let mut shown = Vec::new();
    for c in sketch.constraints.iter().filter(|c| c.is_dimension()) {
        let Some(label) = c.label() else { continue };
        let Some(lp) = frame.screen(label) else { continue };
        let text = dim_text(c);
        let size =
            painter.layout_no_wrap(text.clone(), theme::font_medium(11.5), theme::ink()).size() + vec2(12.0, 6.0);
        let at = clear_spot(lp, size, &taken);
        let hover = pointer.is_some_and(|p| Rect::from_center_size(at, size).contains(p));
        let color = if hover { theme::accent() } else { theme::ink().gamma_multiply(0.7) };
        paint_dimension_lines(painter, &frame, sketch, c, label, lp, color);
        if at != lp {
            painter.line_segment([lp, at], Stroke::new(1.0, color));
        }
        let (bg, stroke) = if hover { (theme::hover(), theme::accent()) } else { (theme::hud_bg(), theme::line()) };
        let r = value_pill(painter, at, text, theme::ink(), bg, Stroke::new(1.0, stroke));
        taken.push(r);
        shown.push((c.id, r));
    }
    shown
}

/// Where a value of `size` centred near `at` covers none of `taken`: `at` itself, or just
/// below, above, right or left of the first value it would cover.
fn clear_spot(at: Pos2, size: egui::Vec2, taken: &[Rect]) -> Pos2 {
    let free = |p: Pos2| !taken.iter().any(|t| t.intersects(Rect::from_center_size(p, size).expand(2.0)));
    let Some(t) = taken.iter().find(|t| t.intersects(Rect::from_center_size(at, size).expand(2.0))) else {
        return at;
    };
    let gap = 4.0;
    [
        pos2(at.x, t.bottom() + gap + size.y / 2.0),
        pos2(at.x, t.top() - gap - size.y / 2.0),
        pos2(t.right() + gap + size.x / 2.0, at.y),
        pos2(t.left() - gap - size.x / 2.0, at.y),
    ]
    .into_iter()
    .find(|&p| free(p))
    .unwrap_or(at)
}

fn paint_rubber_band(painter: &egui::Painter, frame: &Frame<'_>, s: &SketchSession, snapped: Option<P2>) {
    let Some(p) = snapped else { return };
    let st = Stroke::new(1.5, theme::accent().gamma_multiply(0.8));
    let seg = |a: P2, b: P2| {
        if let (Some(sa), Some(sb)) = (frame.screen(a), frame.screen(b)) {
            painter.line_segment([sa, sb], st);
        }
    };
    let curve = |pts: Vec<P2>| {
        let sp: Vec<Pos2> = pts.into_iter().filter_map(|q| frame.screen(q)).collect();
        if sp.len() >= 2 {
            painter.add(egui::Shape::line(sp, st));
        }
    };
    match (s.tool, s.pending.as_slice()) {
        (Tool::Line, [(a, _)]) => seg(*a, p),
        (Tool::Rectangle, [(a, _)]) => {
            let c = [*a, P2::new(p.u, a.v), p, P2::new(a.u, p.v)];
            for i in 0..4 {
                seg(c[i], c[(i + 1) % 4]);
            }
        }
        (Tool::Circle, [(c, _)]) => {
            let e = Entity { id: 0, kind: EntityKind::Circle { center: *c, radius: c.dist(p) }, construction: false };
            curve(e.polyline(64));
        }
        (Tool::Arc, [(c, _)]) => seg(*c, p),
        (Tool::Arc, [(c, _), (start, _)]) => {
            let r = c.dist(*start);
            if r > 1e-6 && c.dist(p) > 1e-6 {
                let end = c.add(p.sub(*c).normalized().scale(r));
                let mut sweep = end.sub(*c).angle() - start.sub(*c).angle();
                while sweep < 0.0 {
                    sweep += std::f64::consts::TAU;
                }
                let (a, b) = if sweep <= std::f64::consts::PI { (*start, end) } else { (end, *start) };
                let e = Entity { id: 0, kind: EntityKind::Arc { center: *c, start: a, end: b }, construction: false };
                curve(e.polyline(64));
            }
        }
        (Tool::Slot, [(c0, _)]) => seg(*c0, p),
        (Tool::Slot, [(c0, _), (c1, _)]) => {
            let d = c1.sub(*c0);
            if d.len() > 1e-6 {
                let n = d.normalized().perp();
                let r = p.sub(*c0).dot(n).abs().max(d.len() * 0.05);
                seg(c0.add(n.scale(r)), c1.add(n.scale(r)));
                seg(c0.sub(n.scale(r)), c1.sub(n.scale(r)));
                let right = Entity {
                    id: 0,
                    kind: EntityKind::Arc { center: *c1, start: c1.sub(n.scale(r)), end: c1.add(n.scale(r)) },
                    construction: false,
                };
                let left = Entity {
                    id: 0,
                    kind: EntityKind::Arc { center: *c0, start: c0.add(n.scale(r)), end: c0.sub(n.scale(r)) },
                    construction: false,
                };
                curve(right.polyline(32));
                curve(left.polyline(32));
            }
        }
        _ => {}
    }
}

fn paint_snaps(
    painter: &egui::Painter,
    frame: &Frame<'_>,
    s: &SketchSession,
    hover: Option<Pos2>,
    snapped: Option<P2>,
) {
    let Some(p) = snapped else { return };
    let Some(sp) = frame.screen(p) else { return };
    if hover.is_none() {
        return;
    }
    let accent = theme::accent();
    // Inference lines back to the previous point.
    if let Some((prev, _)) = s.pending.last()
        && let Some(sprev) = frame.screen(*prev)
    {
        for snap in &s.snaps {
            if matches!(snap, Snap::Horizontal | Snap::Vertical) {
                painter.add(egui::Shape::dashed_line(
                    &[sprev, sp],
                    Stroke::new(1.0, accent.gamma_multiply(0.6)),
                    4.0,
                    4.0,
                ));
            }
        }
    }
    // The snap marker and its glyph.
    let mut x = sp.x + 12.0;
    for snap in &s.snaps {
        let r = Rect::from_min_size(pos2(x, sp.y + 10.0), vec2(16.0, 16.0));
        painter.rect(r, CornerRadius::same(3), theme::hud_bg(), Stroke::new(1.0, accent), StrokeKind::Inside);
        let st = Stroke::new(1.3, accent);
        let g = r.shrink(3.5);
        match snap {
            Snap::Point(_) => {
                painter.rect_filled(Rect::from_center_size(g.center(), vec2(5.0, 5.0)), CornerRadius::ZERO, accent);
            }
            Snap::Midpoint(_) => {
                painter.line_segment([g.left_center(), g.right_center()], st);
                painter.circle_filled(g.center(), 1.8, accent);
            }
            Snap::On(_) => {
                painter.line_segment([g.left_bottom(), g.right_top()], st);
                painter.circle_filled(g.center(), 1.8, accent);
            }
            Snap::Horizontal => {
                painter.text(g.center(), Align2::CENTER_CENTER, "H", theme::font_semibold(9.5), accent);
            }
            Snap::Vertical => {
                painter.text(g.center(), Align2::CENTER_CENTER, "V", theme::font_semibold(9.5), accent);
            }
        }
        x += 18.0;
    }
    if s.snaps.iter().any(|x| matches!(x, Snap::Point(_) | Snap::Midpoint(_) | Snap::On(_))) {
        painter.circle_stroke(sp, 6.0, Stroke::new(1.5, accent));
    } else {
        painter.line_segment([sp + vec2(-5.0, 0.0), sp + vec2(5.0, 0.0)], Stroke::new(1.0, accent));
        painter.line_segment([sp + vec2(0.0, -5.0), sp + vec2(0.0, 5.0)], Stroke::new(1.0, accent));
    }
    // Coordinates by the cursor.
    let text = format!("{}, {}", fmt_value((p.u * 100.0).round() / 100.0), fmt_value((p.v * 100.0).round() / 100.0));
    painter.text(sp + vec2(12.0, -10.0), Align2::LEFT_BOTTOM, text, theme::font(10.5), theme::muted());
}

/// The popup where a dimension value is typed.
fn dim_editor(ui: &mut egui::Ui, s: &mut SketchSession, frame: &Frame<'_>, actions: &mut Vec<AppAction>) {
    let Some(edit) = s.dim_edit.clone() else { return };
    let Some(c) = s.sketch.constraint(edit.constraint).copied() else {
        s.dim_edit = None;
        return;
    };
    let Some(anchor) = c.label().and_then(|l| frame.screen(l)) else { return };
    let is_angle = matches!(c.kind, ConstraintKind::Angle { .. });
    let mut close = false;
    let mut apply = false;
    egui::Area::new(egui::Id::new(("dim_edit", edit.constraint)))
        .order(egui::Order::Foreground)
        .fixed_pos(anchor + vec2(14.0, 12.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(theme::panel())
                .stroke(Stroke::new(1.0, theme::line()))
                .corner_radius(CornerRadius::same(7))
                .shadow(ui.style().visuals.popup_shadow)
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(c.name()).size(11.0).color(theme::muted()));
                        let mut text = edit.text.clone();
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut text).desired_width(64.0).font(theme::font_medium(12.5)),
                        );
                        resp.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Dimension value")
                        });
                        if edit.focus {
                            resp.request_focus();
                        }
                        ui.label(
                            egui::RichText::new(if is_angle { "°" } else { "mm" })
                                .size(11.0)
                                .italics()
                                .color(theme::muted()),
                        );
                        if let Some(e) = s.dim_edit.as_mut() {
                            e.text = text;
                            e.focus = false;
                        }
                        // Enter makes the field give up focus; Esc closes without applying.
                        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            close = true;
                        } else if resp.lost_focus() {
                            apply = true;
                        }
                        if ui.add(egui::Button::new("OK").small()).clicked() {
                            apply = true;
                        }
                    });
                });
        });
    if apply {
        let text = s.dim_edit.as_ref().map(|e| e.text.clone()).unwrap_or_default();
        match text.trim().trim_end_matches("mm").trim().parse::<f64>() {
            Ok(v) if v.is_finite() && (v > 0.0 || is_angle) => {
                let before = s.sketch.clone();
                if let Some(c) = s.sketch.constraint_mut(edit.constraint) {
                    c.set_value(v);
                }
                solve::solve(&mut s.sketch, None, true);
                if matches!(s.sketch.status, SolveStatus::Failed(_)) {
                    s.sketch = before;
                    s.say("The solver could not apply that value");
                } else {
                    commit(s, actions);
                }
                close = true;
            }
            _ => s.say("Enter a number"),
        }
    }
    if close {
        s.dim_edit = None;
    }
}

/// The sketch status pill and the tool hint, top left of the viewport.
fn paint_status(ui: &mut egui::Ui, s: &mut SketchSession, rect: Rect) {
    let painter = ui.painter().with_clip_rect(rect);
    let (status_color, status) = match &s.sketch.status {
        SolveStatus::Empty => (theme::muted(), "Empty sketch".to_owned()),
        SolveStatus::UnderDefined => (theme::accent(), "Under defined".to_owned()),
        SolveStatus::FullyDefined => (theme::ok(), "Fully defined".to_owned()),
        SolveStatus::OverDefined { conflicting } => {
            (theme::error(), format!("Over defined · {} conflicting", conflicting.len()))
        }
        SolveStatus::Failed(_) => (theme::error(), "Solver failed".to_owned()),
    };
    let hint = match (s.tool, s.pending.len()) {
        (Tool::Select, _) => "Click to select, drag to move, double-click a dimension to edit",
        (Tool::Line, 0) => "Line: click the start point",
        (Tool::Line, _) => "Line: click the end point; right-click or Esc ends the chain",
        (Tool::Rectangle, 0) => "Rectangle: click the first corner",
        (Tool::Rectangle, _) => "Rectangle: click the opposite corner",
        (Tool::Circle, 0) => "Circle: click the centre",
        (Tool::Circle, _) => "Circle: click a point on the circle",
        (Tool::Arc, 0) => "Arc: click the centre",
        (Tool::Arc, 1) => "Arc: click the start point",
        (Tool::Arc, _) => "Arc: click the end point",
        (Tool::Slot, 0) => "Slot: click the first centre",
        (Tool::Slot, 1) => "Slot: click the second centre",
        (Tool::Slot, _) => "Slot: click to set the width",
        (Tool::Point, _) => "Point: click to place",
        (Tool::Dimension, _) => {
            if s.dim_first.is_some() {
                "Click to place the dimension"
            } else {
                "Smart dimension: pick an entity or point"
            }
        }
    };
    let message = s.message.as_ref().filter(|(_, at)| at.elapsed().as_secs_f32() < 4.0).map(|(m, _)| m.clone());
    let line2 = message.unwrap_or_else(|| hint.to_owned());
    let g1 = painter.layout_no_wrap(status.clone(), theme::font_semibold(11.5), status_color);
    let g2 = painter.layout_no_wrap(line2, theme::font(11.0), theme::muted());
    let w = g1.size().x.max(g2.size().x) + 20.0;
    let r = Rect::from_min_size(pos2(rect.left() + 12.0, rect.top() + 12.0), vec2(w, 40.0));
    painter.rect(r, CornerRadius::same(8), theme::hud_bg(), Stroke::new(1.0, theme::line()), StrokeKind::Inside);
    painter.circle_filled(pos2(r.left() + 10.0, r.top() + 13.0), 3.5, status_color);
    painter.galley(pos2(r.left() + 18.0, r.top() + 6.0), g1, status_color);
    painter.galley(pos2(r.left() + 10.0, r.top() + 22.0), g2, theme::muted());
}

/// Draws a sketch that is not open, in a quiet colour, for selected sketches.
pub fn paint_passive(
    painter: &egui::Painter,
    camera: &Camera,
    rect: Rect,
    plane: Plane,
    sketch: &Sketch,
    color: Color32,
) {
    let frame = Frame { plane, camera, rect };
    for e in sketch.drawn() {
        let pts: Vec<Pos2> = e.polyline(64).into_iter().filter_map(|p| frame.screen(p)).collect();
        let stroke = Stroke::new(if e.construction { 1.0 } else { 1.6 }, color);
        if e.is_point() {
            if let Some(p) = pts.first() {
                painter.rect_filled(Rect::from_center_size(*p, vec2(4.0, 4.0)), CornerRadius::ZERO, color);
            }
        } else if pts.len() >= 2 {
            if e.construction {
                dashed(painter, &pts, stroke);
            } else {
                painter.add(egui::Shape::line(pts, stroke));
            }
        }
    }
}

/// Relations that can be added to the current selection, for the property panel.
pub fn applicable_relations(sketch: &Sketch, selection: &[SketchSel]) -> Vec<(&'static str, ConstraintKind)> {
    let ents: Vec<&Entity> = selection
        .iter()
        .filter_map(|s| match s {
            SketchSel::Entity(e) => sketch.entity(*e),
            _ => None,
        })
        .collect();
    let points: Vec<PointRef> = selection
        .iter()
        .filter_map(|s| match s {
            SketchSel::Point(p) => Some(*p),
            SketchSel::Entity(e) => sketch.entity(*e).filter(|e| e.is_point()).map(|_| PointRef::new(*e, 0)),
            _ => None,
        })
        .collect();
    let lines: Vec<EntityId> = ents.iter().filter(|e| e.is_line()).map(|e| e.id).collect();
    let curves: Vec<EntityId> = ents.iter().filter(|e| e.is_curve()).map(|e| e.id).collect();
    let mut out: Vec<(&'static str, ConstraintKind)> = Vec::new();
    match (lines.len(), curves.len(), points.len()) {
        (1, 0, 0) => {
            out.push(("Horizontal", ConstraintKind::Horizontal { target: Align::Line(lines[0]) }));
            out.push(("Vertical", ConstraintKind::Vertical { target: Align::Line(lines[0]) }));
        }
        (2, 0, 0) => {
            out.push(("Parallel", ConstraintKind::Parallel { a: lines[0], b: lines[1] }));
            out.push(("Perpendicular", ConstraintKind::Perpendicular { a: lines[0], b: lines[1] }));
            out.push(("Equal", ConstraintKind::Equal { a: lines[0], b: lines[1] }));
        }
        (1, 1, 0) => out.push(("Tangent", ConstraintKind::Tangent { a: lines[0], b: curves[0] })),
        (0, 2, 0) => {
            out.push(("Tangent", ConstraintKind::Tangent { a: curves[0], b: curves[1] }));
            out.push(("Equal", ConstraintKind::Equal { a: curves[0], b: curves[1] }));
            out.push(("Concentric", ConstraintKind::Concentric { a: curves[0], b: curves[1] }));
        }
        (0, 0, 1) => {
            if points[0].entity != crate::sketch::ORIGIN {
                out.push((
                    "Fix",
                    ConstraintKind::Fix { point: points[0], at: sketch.point(points[0]).unwrap_or(P2::ZERO) },
                ));
            }
        }
        (0, 0, 2) => {
            out.push(("Coincident", ConstraintKind::Coincident { a: points[0], b: points[1] }));
            out.push(("Horizontal", ConstraintKind::Horizontal { target: Align::Points(points[0], points[1]) }));
            out.push(("Vertical", ConstraintKind::Vertical { target: Align::Points(points[0], points[1]) }));
        }
        (1, 0, 1) => {
            out.push(("On line", ConstraintKind::PointOn { point: points[0], entity: lines[0] }));
            out.push(("Midpoint", ConstraintKind::Midpoint { point: points[0], line: lines[0] }));
        }
        (0, 1, 1) => out.push(("On curve", ConstraintKind::PointOn { point: points[0], entity: curves[0] })),
        _ => {}
    }
    out
}

/// A rectangle, for drawing from automation: the same entities and relations
/// the rectangle tool makes. Returns the four line ids, bottom first.
pub fn add_rectangle(sketch: &mut Sketch, a: P2, b: P2) -> [EntityId; 4] {
    let c = [a, P2::new(b.u, a.v), b, P2::new(a.u, b.v)];
    let ids: Vec<EntityId> =
        (0..4).map(|i| sketch.add(EntityKind::Line { a: c[i], b: c[(i + 1) % 4] }, false)).collect();
    for i in 0..4 {
        sketch.constrain(ConstraintKind::Coincident {
            a: PointRef::new(ids[i], 1),
            b: PointRef::new(ids[(i + 1) % 4], 0),
        });
    }
    sketch.constrain(ConstraintKind::Horizontal { target: Align::Line(ids[0]) });
    sketch.constrain(ConstraintKind::Horizontal { target: Align::Line(ids[2]) });
    sketch.constrain(ConstraintKind::Vertical { target: Align::Line(ids[1]) });
    sketch.constrain(ConstraintKind::Vertical { target: Align::Line(ids[3]) });
    [ids[0], ids[1], ids[2], ids[3]]
}
