//! The phase D3 canvas tools, each a small state machine over the session's
//! [`ToolDraft`](super::ToolDraft): section, detail, auxiliary, crop and broken-out on
//! views; ordinate, baseline and chain dimension sets; geometric tolerance, datum,
//! surface finish and revision symbols. The views and symbols are made by
//! `soliddraft-drawing` (`derived`, `annotate`'s rules); the tools only collect the
//! clicks, preview the result and push the commands.

use super::canvas::{Hit, Sheet2, card, ghost_annotation, ghost_view, to_view, view_at};
use super::part::PartContext;
use super::{DrawingAction, DrawingSel, DrawingSession, DrawingTool, PendingDerived, SHEET};
use crate::document::Document;
use crate::state::AppState;
use crate::theme;
use egui::{Stroke, vec2};
use soliddraft_drawing::display::{self as layout, Owner};
use soliddraft_drawing::measure::{CurveShape, precision_for, shape_of};
use soliddraft_drawing::project::{self, view_label};
use soliddraft_drawing::{
    AlignAxis, Anchor, Annotation, AnnotationKind, AnnotationState, Attach, Boundary, BrokenOut, Command, CurveKind,
    DimType, DimensionSet, DrawingError, EntityRef, LinearOrientation, Part, PartIndex, SetKind, StyleOverrides,
    ViewId, ViewKind, derived,
};
use soliddraft_kernel_api::Point2;

/// Sheet millimetres a click snaps to a circle's centre from, at the current zoom.
fn snap_mm(s: &DrawingSession) -> f64 {
    f64::from(8.0 / s.camera.ppm.max(0.05))
}

/// Runs `f` with the part and its index, or says why it cannot.
fn with_part<R>(
    doc: &Document,
    f: impl FnOnce(&Part<'_>, &PartIndex<'_>) -> Result<R, DrawingError>,
) -> Result<R, String> {
    let ctx = PartContext::new(doc).ok_or("The part has no body; extrude something first")?;
    let part = ctx.part();
    let index = PartIndex::build(&part).map_err(|e| e.to_string())?;
    f(&part, &index).map_err(|e| e.to_string())
}

/// The view a click lands in: the hit curve's view, or the view whose bounds hold the
/// point.
fn view_of_click(s: &DrawingSession, hit: Option<Hit>, p: Point2) -> Option<ViewId> {
    match hit.map(|h| h.owner) {
        Some(Owner::Curve(v, _)) | Some(Owner::View(v)) => Some(v),
        _ => view_at(s, p),
    }
}

/// A point in a view's millimetres, moved onto the nearest face-on circle's centre
/// when one is within reach.
fn snap_centre(s: &DrawingSession, view: ViewId, p: Point2) -> Point2 {
    let reach = snap_mm(s);
    s.doc
        .circles
        .get(&view)
        .and_then(|cs| {
            cs.iter()
                .map(|c| (c.centre, (c.centre.u - p.u).hypot(c.centre.v - p.v)))
                .filter(|(_, d)| *d <= reach)
                .min_by(|a, b| a.1.total_cmp(&b.1))
        })
        .map(|(c, _)| c)
        .unwrap_or(p)
}

/// The anchor a pick on a curve makes: the snapped end, a round's centre, or the
/// curve's middle.
fn anchor_from_hit(s: &DrawingSession, hit: Hit) -> Option<(ViewId, Anchor)> {
    let Owner::Curve(view, index) = hit.owner else { return None };
    let v = s.sheet().view(view)?;
    let c = v.geometry.curves.get(index)?;
    let shape = shape_of(&c.path);
    // A circle's start is where its path begins, not a corner: the centre wins.
    let (attach, at) = match (&shape, hit.end) {
        (CurveShape::Circle { centre, .. }, _) => (Attach::Centre, *centre),
        (_, Some(Attach::Start)) => (Attach::Start, shape.start()),
        (_, Some(Attach::End)) => (Attach::End, shape.end()),
        _ => (Attach::Mid, shape.mid()),
    };
    Some((view, Anchor { entity: c.source.clone(), attach, cached: at }))
}

/// The cutting line across a view through a point of it, horizontal or vertical,
/// reaching 6 mm past the line work.
fn cut_line(s: &DrawingSession, view: ViewId, through: Point2, horizontal: bool) -> Option<[Point2; 2]> {
    let [x0, y0, x1, y1] = s.doc.drawing.view(view)?.geometry.bbox;
    let m = 6.0;
    Some(if horizontal {
        [Point2::new(x0 - m, through.v), Point2::new(x1 + m, through.v)]
    } else {
        [Point2::new(through.u, y0 - m), Point2::new(through.u, y1 + m)]
    })
}

fn pending_of((view, ctx): (soliddraft_drawing::View, project::ViewContext), sheet: usize) -> PendingDerived {
    PendingDerived { view, projected: ctx.projected, sheet }
}

fn compute_section(
    s: &DrawingSession,
    doc: &Document,
    sheet: usize,
    parent: ViewId,
    line: [Point2; 2],
    flip: bool,
) -> Result<PendingDerived, String> {
    with_part(doc, |part, index| derived::new_section(&s.doc.drawing, part, index, sheet, parent, line, flip, None))
        .map(|made| pending_of(made, sheet))
}

#[allow(clippy::too_many_arguments)]
fn compute_detail(
    s: &DrawingSession,
    doc: &Document,
    sheet: usize,
    parent: ViewId,
    center: Point2,
    radius: f64,
    profile: Vec<Point2>,
    factor: f64,
) -> Result<PendingDerived, String> {
    with_part(doc, |part, index| {
        derived::new_detail(&s.doc.drawing, part, index, sheet, parent, center, radius, profile, factor, None)
    })
    .map(|made| pending_of(made, sheet))
}

fn compute_auxiliary(
    s: &DrawingSession,
    doc: &Document,
    sheet: usize,
    parent: ViewId,
    edge: EntityRef,
    flip: bool,
) -> Result<PendingDerived, String> {
    with_part(doc, |part, index| derived::new_auxiliary(&s.doc.drawing, part, index, sheet, parent, edge, flip, None))
        .map(|made| pending_of(made, sheet))
}

/// Keeps a computed view for placing, or places it at once on another sheet.
fn take_pending(s: &mut DrawingSession, pending: PendingDerived, push: &mut impl FnMut(DrawingAction)) {
    if pending.sheet != s.sheet {
        let id = pending.view.id;
        s.doc.remember(id, &pending.projected);
        push(DrawingAction::Command(Command::PlaceView { sheet: pending.sheet, view: pending.view.clone() }));
        push(DrawingAction::SetTool(DrawingTool::Select));
        push(DrawingAction::SetSheet(pending.sheet));
        push(DrawingAction::Select(Some(DrawingSel::View(id))));
        s.say("Placed on the other sheet; location labels point both ways");
        return;
    }
    s.draft.pending = Some(pending);
}

/// The preview stroke of a line being drawn.
fn preview_stroke() -> Stroke {
    Stroke::new(1.5, theme::ACCENT)
}

/// The derived-view tools: section, detail, auxiliary, crop and broken-out.
#[allow(clippy::too_many_arguments)]
pub(super) fn derived_tool(
    s: &mut DrawingSession,
    doc: &Document,
    sheet2: &Sheet2<'_>,
    lctx: &layout::Context<'_>,
    hit: Option<Hit>,
    pointer: Option<Point2>,
    ctrl: bool,
    clicked: bool,
    double_clicked: bool,
    push: &mut impl FnMut(DrawingAction),
) {
    let Some(p) = pointer else {
        paint_draft(s, sheet2, None);
        return;
    };
    if let Some(pending) = s.draft.pending.clone() {
        place_pending(s, sheet2, lctx, pending, p, ctrl, clicked, push);
        return;
    }
    match s.tool.clone() {
        DrawingTool::Section { replace } => section_tool(s, doc, sheet2, hit, p, clicked, replace, push),
        DrawingTool::Detail => detail_tool(s, doc, sheet2, hit, p, clicked, double_clicked, push),
        DrawingTool::Auxiliary => auxiliary_tool(s, doc, hit, clicked, push),
        DrawingTool::Crop => crop_tool(s, sheet2, hit, p, clicked, push),
        DrawingTool::BrokenOut => broken_out_tool(s, sheet2, hit, p, clicked, push),
        _ => {}
    }
}

/// Paints what the draft has so far: the cutting line, circle, profile or rectangle
/// being drawn, following the pointer.
fn paint_draft(s: &DrawingSession, sheet2: &Sheet2<'_>, pointer: Option<Point2>) {
    let Some(view) = s.draft.view else { return };
    let Some(v) = s.doc.drawing.view(view) else { return };
    let o = v.position;
    let at = |q: Point2| sheet2.s(Point2::new(q.u + o.u, q.v + o.v));
    let painter = sheet2.painter;
    let stroke = preview_stroke();
    let pv = pointer.map(|p| to_view(s, view, p));
    match (&s.tool, s.draft.points.as_slice()) {
        (DrawingTool::Section { .. }, [start]) => {
            if let Some(line) = cut_line(s, view, *start, s.draft.horizontal) {
                painter.extend(egui::Shape::dashed_line(&[at(line[0]), at(line[1])], stroke, 8.0, 3.0));
            }
            painter.circle_filled(at(*start), 4.0, theme::ACCENT);
        }
        (DrawingTool::Detail, [centre]) if !s.draft.profile => {
            if let Some(q) = pv {
                let r = ((q.u - centre.u).hypot(q.v - centre.v) * f64::from(s.camera.ppm)) as f32;
                painter.circle_stroke(at(*centre), r.max(1.0), stroke);
            }
            painter.circle_filled(at(*centre), 4.0, theme::ACCENT);
        }
        (DrawingTool::Detail, pts) if s.draft.profile && !pts.is_empty() => {
            let mut poly: Vec<egui::Pos2> = pts.iter().map(|q| at(*q)).collect();
            if let Some(q) = pv {
                poly.push(at(q));
            }
            painter.add(egui::Shape::line(poly, stroke));
            for q in pts {
                painter.circle_filled(at(*q), 3.5, theme::ACCENT);
            }
            if pts.len() >= 3 {
                painter.circle_stroke(at(pts[0]), 7.0, Stroke::new(1.0, theme::ACCENT));
            }
        }
        (DrawingTool::Crop, [a]) => {
            if let Some(q) = pv {
                let r = egui::Rect::from_two_pos(at(*a), at(q));
                painter.extend(egui::Shape::dashed_line(
                    &[r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()],
                    stroke,
                    6.0,
                    3.0,
                ));
            }
            painter.circle_filled(at(*a), 4.0, theme::ACCENT);
        }
        (DrawingTool::BrokenOut, [centre]) => {
            if let Some(q) = pv {
                let r = ((q.u - centre.u).hypot(q.v - centre.v) * f64::from(s.camera.ppm)) as f32;
                painter.circle_stroke(at(*centre), r.max(1.0), stroke);
            }
            painter.circle_filled(at(*centre), 4.0, theme::ACCENT);
        }
        _ => {}
    }
}

/// The ghost of a computed view at the pointer, along its alignment unless Ctrl is
/// held; a click places it.
#[allow(clippy::too_many_arguments)]
fn place_pending(
    s: &mut DrawingSession,
    sheet2: &Sheet2<'_>,
    lctx: &layout::Context<'_>,
    mut pending: PendingDerived,
    p: Point2,
    ctrl: bool,
    clicked: bool,
    push: &mut impl FnMut(DrawingAction),
) {
    let parent = pending.view.parent().and_then(|id| s.doc.drawing.view(id).cloned());
    pending.view.position = p;
    match (pending.view.alignment, &parent) {
        (Some(al), Some(pv)) if !ctrl => match al.axis {
            AlignAxis::Horizontal => pending.view.position.v = pv.position.v,
            AlignAxis::Vertical => pending.view.position.u = pv.position.u,
        },
        _ => pending.view.alignment = None,
    }
    ghost_view(s, sheet2, lctx, &pending.view, pending.sheet);
    // Alignment guides, as when a projected view is placed.
    if let (Some(al), Some(pv)) = (pending.view.alignment, &parent) {
        let [x0, y0, x1, y1] = project::view_bounds(pv);
        let child = pending.view.position;
        let stroke = Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.8));
        let lines: Vec<(Point2, Point2)> = match al.axis {
            AlignAxis::Horizontal => {
                let (a, b) = if child.u > pv.position.u { (x1, child.u + 40.0) } else { (child.u - 40.0, x0) };
                vec![(Point2::new(a, y0), Point2::new(b, y0)), (Point2::new(a, y1), Point2::new(b, y1))]
            }
            AlignAxis::Vertical => {
                let (a, b) = if child.v > pv.position.v { (y1, child.v + 40.0) } else { (child.v - 40.0, y0) };
                vec![(Point2::new(x0, a), Point2::new(x0, b)), (Point2::new(x1, a), Point2::new(x1, b))]
            }
        };
        for (a, b) in lines {
            sheet2.painter.extend(egui::Shape::dashed_line(&[sheet2.s(a), sheet2.s(b)], stroke, 6.0, 4.0));
        }
    }
    let name = {
        let mut tmp = s.sheet().clone();
        tmp.views.push(pending.view.clone());
        view_label(&tmp, &pending.view)
    };
    let line2 = match (&pending.view.alignment, &parent) {
        (Some(_), Some(pv)) => format!("Aligned with {} · Ctrl frees it", view_label(s.sheet(), pv)),
        (None, Some(pv)) if !ctrl => format!("Free · from {}", view_label(s.sheet(), pv)),
        _ => "Free (Ctrl held)".to_owned(),
    };
    card(sheet2.painter, sheet2.s(pending.view.position) + vec2(30.0, -30.0), &name, &line2);
    if clicked {
        let id = pending.view.id;
        s.doc.remember(id, &pending.projected);
        push(DrawingAction::Command(Command::PlaceView { sheet: pending.sheet, view: pending.view.clone() }));
        if let (ViewKind::Section { .. }, Some(hatch)) = (&pending.view.kind, s.draft.hatch.clone()) {
            let mut kind = pending.view.kind.clone();
            if let ViewKind::Section { hatch: h, .. } = &mut kind {
                *h = Some(hatch);
            }
            push(DrawingAction::Command(Command::EditView { view: id, kind }));
        }
        push(DrawingAction::SetTool(DrawingTool::Select));
        push(DrawingAction::Select(Some(DrawingSel::View(id))));
        s.say(format!("{name} placed"));
        s.draft.pending = None;
    } else {
        s.draft.pending = Some(pending);
    }
}

#[allow(clippy::too_many_arguments)]
fn section_tool(
    s: &mut DrawingSession,
    doc: &Document,
    sheet2: &Sheet2<'_>,
    hit: Option<Hit>,
    p: Point2,
    clicked: bool,
    replace: Option<ViewId>,
    push: &mut impl FnMut(DrawingAction),
) {
    match (s.draft.view, s.draft.points.first().copied()) {
        (Some(view), Some(start)) => {
            // The pointer picks the line's direction until the click finishes it.
            let q = to_view(s, view, p);
            let (du, dv) = ((q.u - start.u).abs(), (q.v - start.v).abs());
            if du.max(dv) > 1.0 {
                s.draft.horizontal = du >= dv;
            }
            paint_draft(s, sheet2, Some(p));
            if !clicked {
                return;
            }
            let Some(line) = cut_line(s, view, start, s.draft.horizontal) else { return };
            match replace {
                Some(section) => redraw_section(s, doc, section, line, push),
                None => match compute_section(s, doc, s.sheet, view, line, s.draft.flip) {
                    Ok(pending) => {
                        s.draft.points.clear();
                        take_pending(s, pending, push);
                    }
                    Err(e) => {
                        s.say(format!("Section: {e}"));
                        s.draft.reset();
                    }
                },
            }
        }
        _ => {
            if !clicked {
                return;
            }
            // With a parent chosen beforehand, the line must be drawn on it.
            let Some(view) = replace
                .and_then(|r| s.doc.drawing.view(r).and_then(|v| v.parent()))
                .or_else(|| view_of_click(s, hit, p))
            else {
                s.say("Click on the view to cut");
                return;
            };
            if matches!(s.doc.drawing.view(view).map(|v| &v.kind), Some(ViewKind::Detail { .. })) {
                s.say("A detail view cannot be sectioned; cut its parent");
                return;
            }
            let start = snap_centre(s, view, to_view(s, view, p));
            s.draft.view = Some(view);
            s.draft.points = vec![start];
            s.say("Move for a horizontal or vertical line, click to finish it");
        }
    }
}

/// Gives an existing section a new cutting line, keeping its letter, direction and
/// hatch.
fn redraw_section(
    s: &mut DrawingSession,
    doc: &Document,
    section: ViewId,
    line: [Point2; 2],
    push: &mut impl FnMut(DrawingAction),
) {
    let Some(ViewKind::Section { parent, flip, label, hatch, .. }) =
        s.doc.drawing.view(section).map(|v| v.kind.clone())
    else {
        s.set_tool(DrawingTool::Select);
        return;
    };
    let model_line = with_part(doc, |part, index| {
        let ctx = derived::context(&s.doc.drawing, part, index, parent)?;
        Ok(line.iter().map(|q| ctx.xf.unproject(*q)).collect::<Vec<_>>())
    });
    match model_line {
        Ok(model_line) => {
            let kind = ViewKind::Section { parent, line: line.to_vec(), flip, label, model_line, hatch };
            push(DrawingAction::Command(Command::EditView { view: section, kind }));
            push(DrawingAction::SetTool(DrawingTool::Select));
            push(DrawingAction::Select(Some(DrawingSel::View(section))));
            s.say("Cutting line redrawn");
        }
        Err(e) => {
            s.say(format!("Section: {e}"));
            s.draft.reset();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn detail_tool(
    s: &mut DrawingSession,
    doc: &Document,
    sheet2: &Sheet2<'_>,
    hit: Option<Hit>,
    p: Point2,
    clicked: bool,
    double_clicked: bool,
    push: &mut impl FnMut(DrawingAction),
) {
    paint_draft(s, sheet2, Some(p));
    let Some(view) = s.draft.view else {
        if !clicked {
            return;
        }
        let Some(view) = view_of_click(s, hit, p) else {
            s.say("Click on the view to magnify");
            return;
        };
        let centre = snap_centre(s, view, to_view(s, view, p));
        s.draft.view = Some(view);
        s.draft.points = vec![centre];
        return;
    };
    let q = to_view(s, view, p);
    if s.draft.profile {
        let pts = s.draft.points.clone();
        let near_first =
            pts.len() >= 3 && pts.first().is_some_and(|f| (f.u - q.u).hypot(f.v - q.v) <= snap_mm(s).max(2.0));
        if double_clicked || (clicked && near_first) {
            if pts.len() >= 3 {
                finish_detail(s, doc, push);
            } else {
                s.say("A profile needs at least three corners");
            }
        } else if clicked {
            s.draft.points.push(q);
        }
        return;
    }
    if clicked {
        let centre = s.draft.points[0];
        let radius = (q.u - centre.u).hypot(q.v - centre.v);
        if radius < 1.0 {
            s.say("Move further out for the circle's radius");
            return;
        }
        s.draft.points.push(q);
        finish_detail(s, doc, push);
    }
}

/// The sheet a detail goes to, adding one when the draft asks for a new sheet.
fn target_sheet(s: &mut DrawingSession) -> usize {
    match s.draft.target_sheet {
        Some(t) if t < s.doc.drawing.sheets.len() => t,
        Some(_) => {
            let template = s.doc.drawing.sheets[0].template.clone();
            let name = super::sheets::fresh_name(&s.doc.drawing);
            match s.doc.apply(Command::AddSheet { name, template }, None) {
                Ok(()) => s.doc.drawing.sheets.len() - 1,
                Err(e) => {
                    s.say(e);
                    s.sheet
                }
            }
        }
        None => s.sheet,
    }
}

/// Computes the detail the draft describes and keeps it for placing.
fn finish_detail(s: &mut DrawingSession, doc: &Document, push: &mut impl FnMut(DrawingAction)) {
    let Some(view) = s.draft.view else { return };
    let pts = s.draft.points.clone();
    let (centre, radius, profile) = if s.draft.profile {
        let n = pts.len() as f64;
        let c = Point2::new(pts.iter().map(|q| q.u).sum::<f64>() / n, pts.iter().map(|q| q.v).sum::<f64>() / n);
        let r = pts.iter().map(|q| (q.u - c.u).hypot(q.v - c.v)).fold(0.0, f64::max);
        (c, r.max(1.0), pts)
    } else {
        let c = pts[0];
        let q = pts[1];
        (c, (q.u - c.u).hypot(q.v - c.v), Vec::new())
    };
    let sheet = target_sheet(s);
    match compute_detail(s, doc, sheet, view, centre, radius, profile, s.draft.factor) {
        Ok(pending) => {
            s.draft.points.clear();
            take_pending(s, pending, push);
        }
        Err(e) => {
            s.say(format!("Detail: {e}"));
            s.draft.reset();
        }
    }
}

fn auxiliary_tool(
    s: &mut DrawingSession,
    doc: &Document,
    hit: Option<Hit>,
    clicked: bool,
    push: &mut impl FnMut(DrawingAction),
) {
    if !clicked {
        return;
    }
    let Some(Hit { owner: Owner::Curve(view, index), .. }) = hit else {
        s.say("Click an edge of a view; the auxiliary view unfolds square to it");
        return;
    };
    let Some(c) = s.sheet().view(view).and_then(|v| v.geometry.curves.get(index)).cloned() else { return };
    if c.kind != CurveKind::Edge || c.source.kind != soliddraft_drawing::EntityKind::Edge {
        s.say("That is a silhouette, not an edge; click a model edge");
        return;
    }
    s.draft.view = Some(view);
    s.draft.anchors = vec![Anchor { entity: c.source.clone(), attach: Attach::Mid, cached: shape_of(&c.path).mid() }];
    match compute_auxiliary(s, doc, s.sheet, view, c.source.clone(), s.draft.flip) {
        Ok(pending) => take_pending(s, pending, push),
        Err(e) => {
            s.say(format!("Auxiliary view: {e}"));
            s.draft.reset();
        }
    }
}

fn crop_tool(
    s: &mut DrawingSession,
    sheet2: &Sheet2<'_>,
    hit: Option<Hit>,
    p: Point2,
    clicked: bool,
    push: &mut impl FnMut(DrawingAction),
) {
    paint_draft(s, sheet2, Some(p));
    if !clicked {
        return;
    }
    match (s.draft.view, s.draft.points.first().copied()) {
        (Some(view), Some(a)) => {
            let b = to_view(s, view, p);
            if (a.u - b.u).abs() < 1.0 || (a.v - b.v).abs() < 1.0 {
                s.say("Make the rectangle bigger");
                return;
            }
            push(DrawingAction::Command(Command::SetViewCrop { view, crop: Some(Boundary::rectangle(a, b)) }));
            push(DrawingAction::SetTool(DrawingTool::Select));
            push(DrawingAction::Select(Some(DrawingSel::View(view))));
            s.say("View cropped; the view's properties remove the crop");
        }
        _ => {
            let Some(view) = view_of_click(s, hit, p) else {
                s.say("Click on the view to crop");
                return;
            };
            s.draft.view = Some(view);
            s.draft.points = vec![to_view(s, view, p)];
        }
    }
}

fn broken_out_tool(
    s: &mut DrawingSession,
    sheet2: &Sheet2<'_>,
    hit: Option<Hit>,
    p: Point2,
    clicked: bool,
    push: &mut impl FnMut(DrawingAction),
) {
    paint_draft(s, sheet2, Some(p));
    if !clicked {
        return;
    }
    match (s.draft.view, s.draft.points.first().copied()) {
        (Some(view), Some(center)) => {
            let q = to_view(s, view, p);
            let radius = (q.u - center.u).hypot(q.v - center.v);
            if radius < 1.0 {
                s.say("Move further out for the radius");
                return;
            }
            let boundary = Boundary::Circle { center, radius };
            push(DrawingAction::Command(Command::AddBrokenOut {
                view,
                broken_out: BrokenOut { boundary, depth: s.draft.depth },
            }));
            push(DrawingAction::SetTool(DrawingTool::Select));
            push(DrawingAction::Select(Some(DrawingSel::View(view))));
            s.say(format!("Broken out {} mm deep", s.draft.depth));
        }
        _ => {
            let Some(view) = view_of_click(s, hit, p) else {
                s.say("Click on the view to break out");
                return;
            };
            if matches!(s.doc.drawing.view(view).map(|v| &v.kind), Some(ViewKind::Detail { .. })) {
                s.say("Break out the parent; the detail shows it magnified");
                return;
            }
            s.draft.view = Some(view);
            s.draft.points = vec![snap_centre(s, view, to_view(s, view, p))];
        }
    }
}

/// Flips the direction of the section or auxiliary view being placed, or of the
/// section whose line is being redrawn.
pub fn flip_pending(state: &mut AppState) {
    let doc = &state.doc;
    let Some(s) = state.drawing.session.as_mut() else { return };
    s.draft.flip = !s.draft.flip;
    let Some(pending) = s.draft.pending.clone() else {
        s.say(if s.draft.flip { "The view will look the other way" } else { "Direction restored" });
        return;
    };
    let made = match &pending.view.kind {
        ViewKind::Section { parent, line, .. } if line.len() >= 2 => {
            compute_section(s, doc, pending.sheet, *parent, [line[0], line[1]], s.draft.flip)
        }
        ViewKind::Auxiliary { parent, edge, .. } => {
            compute_auxiliary(s, doc, pending.sheet, *parent, edge.clone(), s.draft.flip)
        }
        _ => return,
    };
    match made {
        Ok(p) => {
            s.draft.pending = Some(p);
            s.say("Flipped: the view looks the other way");
        }
        Err(e) => s.say(e),
    }
}

/// Closes the profile of the detail being drawn.
pub fn close_profile(state: &mut AppState) {
    let doc = &state.doc;
    let Some(s) = state.drawing.session.as_mut() else { return };
    if s.tool != DrawingTool::Detail || !s.draft.profile || s.draft.points.len() < 3 {
        return;
    }
    let mut actions = Vec::new();
    finish_detail(s, doc, &mut |a| actions.push(a));
    for a in actions {
        state.push(crate::state::AppAction::Drawing(a));
    }
}

/// The dimension set the picks make, placed at a point of the view: ordinate and
/// baseline sets measure from the first pick, a chain from pick to pick.
fn build_set(s: &DrawingSession, kind: SetKind, placement: Point2) -> Annotation {
    let anchors = s.draft.anchors.clone();
    let view = s.draft.view.unwrap_or(SHEET);
    let sheet = s.sheet();
    let scale = sheet.view(view).map(|v| project::effective_scale(sheet, v)).unwrap_or(1.0);
    let (us, vs): (Vec<f64>, Vec<f64>) = anchors.iter().map(|a| (a.cached.u, a.cached.v)).unzip();
    let spread = |c: &[f64]| {
        c.iter().cloned().fold(f64::NEG_INFINITY, f64::max) - c.iter().cloned().fold(f64::INFINITY, f64::min)
    };
    // Points on one vertical line can only be dimensioned vertically, and the other
    // way round; otherwise the panel's choice holds.
    let orientation = if spread(&us) < 1e-6 && spread(&vs) > 1e-6 {
        LinearOrientation::Vertical
    } else if spread(&vs) < 1e-6 && spread(&us) > 1e-6 {
        LinearOrientation::Horizontal
    } else {
        s.draft.orientation
    };
    let along: Vec<f64> = match orientation {
        LinearOrientation::Vertical => vs.clone(),
        _ => us.clone(),
    };
    let values: Vec<f64> = match kind {
        SetKind::Chain => along.windows(2).map(|w| (w[1] - w[0]).abs() / scale).collect(),
        _ => along[1..].iter().map(|c| (c - along[0]).abs() / scale).collect(),
    };
    let values: Vec<f64> = values.into_iter().map(|v| (v * 1000.0).round() / 1000.0).collect();
    let precision = values.iter().map(|v| precision_for(*v)).max().unwrap_or(0);
    let first = anchors[0].cached;
    let placement = match orientation {
        LinearOrientation::Vertical => Point2::new(placement.u, first.v),
        _ => Point2::new(first.u, placement.v),
    };
    Annotation {
        id: s.doc.next_annotation_id(),
        view,
        body: AnnotationKind::DimensionSet(DimensionSet {
            kind,
            orientation,
            values,
            precision,
            dim_type: DimType::Projected,
        }),
        anchors,
        placement,
        style: StyleOverrides::default(),
        state: AnnotationState::Ok,
    }
}

/// Ordinate, baseline and chain dimensions: picks on the curves of one view, then a
/// click away from them places the set.
#[allow(clippy::too_many_arguments)]
pub(super) fn set_tool(
    s: &mut DrawingSession,
    kind: SetKind,
    sheet2: &Sheet2<'_>,
    lctx: &layout::Context<'_>,
    hit: Option<Hit>,
    pointer: Option<Point2>,
    clicked: bool,
    push: &mut impl FnMut(DrawingAction),
) {
    // The picks so far, numbered.
    if let Some(view) = s.draft.view
        && let Some(v) = s.doc.drawing.view(view)
    {
        for (i, a) in s.draft.anchors.iter().enumerate() {
            let at = sheet2.s(Point2::new(a.cached.u + v.position.u, a.cached.v + v.position.v));
            sheet2.painter.circle_filled(at, 4.5, theme::ACCENT);
            sheet2.painter.text(
                at + vec2(6.0, -6.0),
                egui::Align2::LEFT_BOTTOM,
                if i == 0 { "0".to_owned() } else { i.to_string() },
                theme::font_medium(11.0),
                theme::ACCENT,
            );
        }
    }
    let Some(p) = pointer else { return };
    let on_curve = matches!(hit, Some(Hit { owner: Owner::Curve(..), .. }));
    if s.draft.anchors.len() >= 2
        && !on_curve
        && let Some(view) = s.draft.view
    {
        let a = build_set(s, kind, to_view(s, view, p));
        ghost_annotation(s, sheet2, lctx, &a);
    }
    if !clicked {
        return;
    }
    match (on_curve, hit.and_then(|h| anchor_from_hit(s, h))) {
        (true, Some((view, anchor))) => {
            if s.draft.view.is_some_and(|v| v != view) {
                s.say("Every point must be in the same view");
                return;
            }
            if s.draft
                .anchors
                .iter()
                .any(|a| (a.cached.u - anchor.cached.u).abs() < 1e-6 && (a.cached.v - anchor.cached.v).abs() < 1e-6)
            {
                s.say("That point is picked already");
                return;
            }
            s.draft.view = Some(view);
            s.draft.anchors.push(anchor);
            let n = s.draft.anchors.len();
            s.say(if n == 1 {
                "Origin picked · pick the points to dimension".to_owned()
            } else {
                format!(
                    "{} point{} · pick more, or click away from the edges to place",
                    n - 1,
                    if n == 2 { "" } else { "s" }
                )
            });
        }
        _ => {
            if s.draft.anchors.len() < 2 {
                s.say("Pick the origin and at least one point first");
                return;
            }
            let Some(view) = s.draft.view else { return };
            let a = build_set(s, kind, to_view(s, view, p));
            let id = a.id;
            push(DrawingAction::Command(Command::AddAnnotation { annotation: a }));
            push(DrawingAction::Select(Some(DrawingSel::Annotation(id))));
            s.draft.anchors.clear();
            s.draft.view = None;
            s.say("Dimensions placed · pick the next origin, or Esc");
        }
    }
}

/// The body of the symbol a tool places, from the draft and the drawing's counters.
fn symbol_body(s: &DrawingSession) -> AnnotationKind {
    match s.tool {
        DrawingTool::Datum => AnnotationKind::Datum {
            letter: if s.draft.datum.trim().is_empty() {
                s.doc.drawing.next_datum_label.clone()
            } else {
                s.draft.datum.trim().to_uppercase()
            },
        },
        DrawingTool::SurfaceFinish => s.draft.finish.body(),
        DrawingTool::RevisionSymbol => AnnotationKind::RevisionSymbol {
            revision: if s.draft.revision.trim().is_empty() {
                s.doc.drawing.meta.revision.clone()
            } else {
                s.draft.revision.trim().to_owned()
            },
        },
        _ => s.draft.gdt.body(),
    }
}

fn symbol_annotation(s: &DrawingSession, view: ViewId, anchors: Vec<Anchor>, placement: Point2) -> Annotation {
    Annotation {
        id: s.doc.next_annotation_id(),
        view,
        body: symbol_body(s),
        anchors,
        placement,
        style: StyleOverrides::default(),
        state: AnnotationState::Ok,
    }
}

/// Geometric tolerance, datum, surface finish and revision symbols: a click on an edge
/// anchors the symbol, the next click places it. A geometric tolerance or a revision
/// symbol clicked away from the edges stands free where it was clicked.
#[allow(clippy::too_many_arguments)]
pub(super) fn symbol_tool(
    s: &mut DrawingSession,
    sheet2: &Sheet2<'_>,
    lctx: &layout::Context<'_>,
    hit: Option<Hit>,
    pointer: Option<Point2>,
    clicked: bool,
    push: &mut impl FnMut(DrawingAction),
) {
    let Some(p) = pointer else { return };
    if let (Some(view), Some(anchor)) = (s.draft.view, s.draft.anchors.first().cloned()) {
        let Some(v) = s.doc.drawing.view(view) else { return };
        let at = sheet2.s(Point2::new(anchor.cached.u + v.position.u, anchor.cached.v + v.position.v));
        sheet2.painter.circle_filled(at, 4.5, theme::ACCENT);
        let a = symbol_annotation(s, view, vec![anchor], to_view(s, view, p));
        ghost_annotation(s, sheet2, lctx, &a);
        if clicked {
            let id = a.id;
            push(DrawingAction::Command(Command::AddAnnotation { annotation: a }));
            push(DrawingAction::SetTool(DrawingTool::Select));
            push(DrawingAction::Select(Some(DrawingSel::Annotation(id))));
            s.say("Placed · its properties edit the symbol");
        }
        return;
    }
    if !clicked {
        return;
    }
    match hit.and_then(|h| anchor_from_hit(s, h)) {
        Some((view, anchor)) => {
            s.draft.view = Some(view);
            s.draft.anchors = vec![anchor];
            s.say("Click where the symbol goes");
        }
        None if matches!(s.tool, DrawingTool::Gdt | DrawingTool::RevisionSymbol) => {
            let view = view_at(s, p).unwrap_or(SHEET);
            let a = symbol_annotation(s, view, Vec::new(), to_view(s, view, p));
            let id = a.id;
            let cmd = if view == SHEET {
                Command::AddSheetAnnotation { sheet: s.sheet, annotation: a }
            } else {
                Command::AddAnnotation { annotation: a }
            };
            push(DrawingAction::Command(cmd));
            push(DrawingAction::SetTool(DrawingTool::Select));
            push(DrawingAction::Select(Some(DrawingSel::Annotation(id))));
        }
        None => s.say("Click an edge of the feature first"),
    }
}
