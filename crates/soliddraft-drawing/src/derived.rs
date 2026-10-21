//! Making derived views: sections, details and auxiliary views of a view already on the
//! drawing, with their line work, and a free place on a sheet to put them.
//!
//! Each `new_*` function returns the view ready to place and its context (its
//! transform, the circles it sees). Nothing is added to the drawing: a tool previews the
//! view, then places it with [`Command::PlaceView`](crate::Command::PlaceView), which
//! also moves the drawing's next view letter on.

use soliddraft_kernel_api::{Point2, Vec3};

use crate::entity::EntityRef;
use crate::ids::ViewId;
use crate::part::{Part, PartIndex, round};
use crate::project::{self, Contexts, ViewContext, view_bounds};
use crate::sheet::Projection;
use crate::view::{AlignAxis, Alignment, HiddenLines, View, ViewDisplay, ViewGeometry, ViewKind, ViewScale};
use crate::{Drawing, DrawingError, library};

/// Room kept between a derived view and the views around it, in sheet millimetres.
const GAP: f64 = 20.0;

fn blank(drawing: &Drawing, kind: ViewKind, display: ViewDisplay) -> View {
    View {
        id: drawing.next_view_id(),
        kind,
        position: Point2::new(0.0, 0.0),
        alignment: None,
        scale: ViewScale::Parent,
        display,
        excluded_from_update: false,
        geometry: ViewGeometry::default(),
        label: None,
        crop: None,
        broken_out: Vec::new(),
    }
}

/// The context of a view already on the drawing, computed fresh.
pub fn context(drawing: &Drawing, part: &Part, index: &PartIndex, id: ViewId) -> Result<ViewContext, DrawingError> {
    ViewContext::compute(drawing, part, index, id)
}

/// A cutting line across the whole of view `parent` through the model point `through`:
/// horizontal or vertical on the sheet, reaching 6 mm past the view's line work.
pub fn cutting_line(
    drawing: &Drawing,
    part: &Part,
    index: &PartIndex,
    parent: ViewId,
    through: Vec3,
    horizontal: bool,
) -> Result<[Point2; 2], DrawingError> {
    let ctx = context(drawing, part, index, parent)?;
    let p = ctx.xf.point(through);
    let [x0, y0, x1, y1] = drawing.view(parent).ok_or(DrawingError::UnknownView(parent))?.geometry.bbox;
    let m = 6.0;
    Ok(if horizontal {
        [Point2::new(round(x0 - m), p.v), Point2::new(round(x1 + m), p.v)]
    } else {
        [Point2::new(p.u, round(y0 - m)), Point2::new(p.u, round(y1 + m))]
    })
}

/// A section of view `parent` along `line` (two points in the parent's view
/// millimetres), placed on sheet `sheet`: aligned with its parent on the side the sheet's
/// projection angle puts it when both are on one sheet, otherwise at a free place.
/// `label` defaults to the drawing's next view letter. Hidden lines are left out.
#[allow(clippy::too_many_arguments)]
pub fn new_section(
    drawing: &Drawing,
    part: &Part,
    index: &PartIndex,
    sheet: usize,
    parent: ViewId,
    line: [Point2; 2],
    flip: bool,
    label: Option<String>,
) -> Result<(View, ViewContext), DrawingError> {
    let pctx = context(drawing, part, index, parent)?;
    let model_line = line.iter().map(|p| pctx.xf.unproject(*p)).collect();
    let kind = ViewKind::Section {
        parent,
        line: line.to_vec(),
        flip,
        label: label.unwrap_or_else(|| drawing.next_view_label.clone()),
        model_line,
        hatch: None,
    };
    let display = ViewDisplay { hidden: HiddenLines::Removed, ..ViewDisplay::ORTHOGRAPHIC };
    let mut view = blank(drawing, kind, display);
    let ctx = compute(drawing, part, index, &view, sheet)?;
    view.geometry = ctx.projected.geometry.clone();
    // The view goes where the eye is (third angle) or where the arrows point (first
    // angle), in line with its parent.
    let arrow =
        project::section_arrow(&line, flip).ok_or(DrawingError::BadRequest("the cutting line has no length".into()))?;
    let projection = drawing.sheets.get(sheet).map_or(Projection::FirstAngle, |s| s.projection);
    let side = match projection {
        Projection::ThirdAngle => Point2::new(-arrow.u, -arrow.v),
        Projection::FirstAngle => arrow,
    };
    place_beside(drawing, sheet, &mut view, parent, side);
    Ok((view, ctx))
}

/// A detail of view `parent` inside the circle at `center` with `radius` (parent view
/// millimetres), or inside `profile` when it has three points or more, magnified
/// `factor` times, placed at a free place on sheet `sheet` near its parent.
#[allow(clippy::too_many_arguments)]
pub fn new_detail(
    drawing: &Drawing,
    part: &Part,
    index: &PartIndex,
    sheet: usize,
    parent: ViewId,
    center: Point2,
    radius: f64,
    profile: Vec<Point2>,
    factor: f64,
    label: Option<String>,
) -> Result<(View, ViewContext), DrawingError> {
    if !(factor > 0.0 && radius > 0.0) {
        return Err(DrawingError::BadRequest("a detail needs a positive radius and factor".into()));
    }
    let pctx = context(drawing, part, index, parent)?;
    let kind = ViewKind::Detail {
        parent,
        center,
        radius,
        factor,
        label: label.unwrap_or_else(|| drawing.next_view_label.clone()),
        profile,
        model_center: Some(pctx.xf.unproject(center)),
    };
    let display = drawing.view(parent).map_or(ViewDisplay::ORTHOGRAPHIC, |p| p.display);
    let mut view = blank(drawing, kind, display);
    let ctx = compute(drawing, part, index, &view, sheet)?;
    view.geometry = ctx.projected.geometry.clone();
    let near = drawing.view(parent).filter(|_| project::sheet_of(drawing, parent) == Some(sheet)).map(|p| p.position);
    place_free(drawing, sheet, &mut view, near);
    Ok((view, ctx))
}

/// An auxiliary view of view `parent` square to `edge`, placed beside its parent on
/// the side it looks from.
#[allow(clippy::too_many_arguments)]
pub fn new_auxiliary(
    drawing: &Drawing,
    part: &Part,
    index: &PartIndex,
    sheet: usize,
    parent: ViewId,
    edge: EntityRef,
    flip: bool,
    label: Option<String>,
) -> Result<(View, ViewContext), DrawingError> {
    let kind =
        ViewKind::Auxiliary { parent, edge, flip, label: label.unwrap_or_else(|| drawing.next_view_label.clone()) };
    let display = ViewDisplay { hidden: HiddenLines::Removed, ..ViewDisplay::ORTHOGRAPHIC };
    let mut view = blank(drawing, kind, display);
    let ctx = compute(drawing, part, index, &view, sheet)?;
    view.geometry = ctx.projected.geometry.clone();
    let pctx = context(drawing, part, index, parent)?;
    // The side it looks from, in the parent's view.
    let d = ctx.xf.frame.direction;
    let side = Point2::new(d.dot(pctx.xf.frame.right), d.dot(pctx.xf.frame.up));
    let projection = drawing.sheets.get(sheet).map_or(Projection::FirstAngle, |s| s.projection);
    let side = match projection {
        Projection::ThirdAngle => side,
        Projection::FirstAngle => Point2::new(-side.u, -side.v),
    };
    let near = drawing.view(parent).filter(|_| project::sheet_of(drawing, parent) == Some(sheet)).map(|p| {
        let len = side.u.hypot(side.v).max(1e-9);
        let (w, h) = size(drawing.view(parent).expect("found"));
        let (cw, ch) = size(&view);
        let reach = (w + cw) / 2.0 * (side.u / len).abs() + (h + ch) / 2.0 * (side.v / len).abs() + GAP;
        Point2::new(p.position.u + side.u / len * reach, p.position.v + side.v / len * reach)
    });
    place_free(drawing, sheet, &mut view, near);
    Ok((view, ctx))
}

fn compute(
    drawing: &Drawing,
    part: &Part,
    index: &PartIndex,
    view: &View,
    sheet: usize,
) -> Result<ViewContext, DrawingError> {
    let mut memo = Contexts::new();
    project::compute_view(drawing, part, index, view, sheet, &mut memo)
}

fn size(view: &View) -> (f64, f64) {
    let [x0, y0, x1, y1] = view.geometry.bbox;
    (x1 - x0, y1 - y0)
}

/// Puts a view in line with its parent on `side` (a unit vector in sheet axes) when the
/// parent is on the same sheet and the side is along a sheet axis, keeping the
/// alignment; otherwise at a free place.
fn place_beside(drawing: &Drawing, sheet: usize, view: &mut View, parent: ViewId, side: Point2) {
    let on_sheet = project::sheet_of(drawing, parent) == Some(sheet);
    let Some(p) = drawing.view(parent).filter(|_| on_sheet) else {
        place_free(drawing, sheet, view, None);
        return;
    };
    let axis = if side.u.abs() > 1.0 - 1e-9 {
        Some(AlignAxis::Horizontal)
    } else if side.v.abs() > 1.0 - 1e-9 {
        Some(AlignAxis::Vertical)
    } else {
        None
    };
    let (w, h) = size(p);
    let (cw, ch) = size(view);
    let reach = (w + cw) / 2.0 * side.u.abs() + (h + ch) / 2.0 * side.v.abs() + GAP;
    let want = Point2::new(p.position.u + side.u * reach, p.position.v + side.v * reach);
    match axis {
        Some(axis) => {
            let spot = free_spot(drawing, sheet, view, want, Some((axis, p.position)));
            match spot {
                Some(at) => {
                    view.position = at;
                    view.alignment = Some(Alignment { parent, axis });
                }
                None => place_free(drawing, sheet, view, Some(want)),
            }
        }
        None => place_free(drawing, sheet, view, Some(want)),
    }
}

/// Puts a view at the free place nearest `near` (or the drawing area's centre).
fn place_free(drawing: &Drawing, sheet: usize, view: &mut View, near: Option<Point2>) {
    let format = library::template(drawing.sheets.get(sheet).map_or("iso-a3", |s| s.template.as_str()));
    let [l, b, r, t] = format.drawing_area();
    let want = near.unwrap_or(Point2::new((l + r) / 2.0, (b + t) / 2.0));
    view.position = free_spot(drawing, sheet, view, want, None).unwrap_or(want);
}

/// The position nearest `want` where `view` fits on sheet `sheet` without covering
/// other views, the title block or a table. Positions are tried on a 5 mm grid with
/// 10 mm kept clear around the views, then closer and finer when nothing fits. With
/// `aligned`, only positions that keep the alignment with a parent at that position
/// count.
pub fn free_spot(
    drawing: &Drawing,
    sheet: usize,
    view: &View,
    want: Point2,
    aligned: Option<(AlignAxis, Point2)>,
) -> Option<Point2> {
    [(GAP / 2.0, 5.0), (GAP / 4.0, 2.5), (1.0, 1.0)]
        .into_iter()
        .find_map(|(margin, step)| free_spot_with(drawing, sheet, view, want, aligned, margin, step))
}

#[allow(clippy::too_many_arguments)]
fn free_spot_with(
    drawing: &Drawing,
    sheet: usize,
    view: &View,
    want: Point2,
    aligned: Option<(AlignAxis, Point2)>,
    margin: f64,
    step: f64,
) -> Option<Point2> {
    let s = drawing.sheets.get(sheet)?;
    let format = library::template(s.template.as_str());
    let [l, b, r, t] = format.frame();
    let band = format.zones.band;
    let (x0, y0, x1, y1) = (l + band, b + band, r - band, t - band);
    let o = format.title_block_origin();
    let mut blocked: Vec<[f64; 4]> =
        vec![[o.u, o.v, o.u + format.title_block.width, o.v + format.title_block.height + margin / 2.0]];
    for v in s.views.iter().filter(|v| v.id != view.id) {
        let [a, c, e, g] = view_bounds(v);
        blocked.push([a - margin, c - margin, e + margin, g + margin]);
    }
    for [a, c, e, g] in crate::display::table_bounds(s) {
        blocked.push([a - margin, c - margin, e + margin, g + margin]);
    }
    let [vx0, vy0, vx1, vy1] = view.geometry.bbox;
    let fits = |p: Point2| {
        let rect = [p.u + vx0 - margin, p.v + vy0 - margin, p.u + vx1 + margin, p.v + vy1 + margin];
        rect[0] >= x0
            && rect[1] >= y0
            && rect[2] <= x1
            && rect[3] <= y1
            && blocked.iter().all(|k| rect[2] <= k[0] || k[2] <= rect[0] || rect[3] <= k[1] || k[3] <= rect[1])
    };
    let mut best: Option<(f64, Point2)> = None;
    let snap = |v: f64| (v / step).round() * step;
    let mut consider = |p: Point2| {
        if fits(p) {
            let d = (p.u - want.u).hypot(p.v - want.v);
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, p));
            }
        }
    };
    match aligned {
        Some((AlignAxis::Horizontal, at)) => {
            let mut x = snap(x0);
            while x <= x1 {
                consider(Point2::new(x, at.v));
                x += step;
            }
        }
        Some((AlignAxis::Vertical, at)) => {
            let mut y = snap(y0);
            while y <= y1 {
                consider(Point2::new(at.u, y));
                y += step;
            }
        }
        None => {
            let mut x = snap(x0);
            while x <= x1 {
                let mut y = snap(y0);
                while y <= y1 {
                    consider(Point2::new(x, y));
                    y += step;
                }
                x += step;
            }
        }
    }
    best.map(|(_, p)| Point2::new(round(p.u), round(p.v)))
}
