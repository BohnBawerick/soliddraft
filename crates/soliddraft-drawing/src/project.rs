//! Projection: from a view's place in the drawing to the kernel's view request, and from
//! the kernel's 3D curves to the view's 2D line work.
//!
//! A base view's orientation is stored; a projected view's comes from its parent's,
//! folded towards the side it sits on, with the sheet's projection angle deciding which
//! face that side shows. A section view turns its parent's frame a quarter turn about
//! the cutting line, and an auxiliary view about its reference edge; a detail view keeps
//! its parent's frame and magnifies it. Scales chain the same way. Curves are projected
//! with the kernel's [`ViewFrame`], scaled, and moved so the centre of their bounding box
//! is the view's origin (for a detail, the centre of its boundary). Lines stay lines and
//! circular arcs become cubic Béziers, which a parallel projection maps exactly, so a
//! circle seen at an angle is an exact ellipse.
//!
//! [`compute_view`] makes any view's line work, parents first; crops and broken-out
//! sections are applied last, by clipping the line work with
//! [`clip_path`].

use std::collections::BTreeMap;

use kurbo::{BezPath, ParamCurve, PathSeg, Point};
use soliddraft_kernel_api::drawing::{
    Curve3, CurveSource, CutRegion, SectionSpec, ViewCurve, ViewCurves, ViewFrame, ViewSpec, mesh_hlr,
};
use soliddraft_kernel_api::{KernelError, Point2, Vec3};

use soliddraft_kernel_api::drawing::StandardView;

use crate::annotation::{Anchor, Annotation, AnnotationKind, AnnotationState, Attach, StyleOverrides};
use crate::entity::{EntityKind, EntityRef};
use crate::ids::{AnnotationId, ViewId};
use crate::part::{Entity, Part, PartIndex, Resolution, round};
use crate::sheet::{Projection, Sheet};
use crate::view::{
    Accuracy, Boundary, BrokenOut, Curve2, CurveKind, FoldDirection, Polygon, View, ViewDisplay, ViewGeometry,
    ViewKind, ViewScale,
};
use crate::{Drawing, DrawingError};

/// Model space to view millimetres: project with `frame`, scale, then move `centre` (in
/// projected, scaled millimetres) to the origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewTransform {
    /// The kernel's view frame.
    pub frame: ViewFrame,
    /// Sheet millimetres per model millimetre.
    pub scale: f64,
    /// The bounding box centre in projected, scaled millimetres.
    pub centre: Point2,
}

impl ViewTransform {
    /// A model point projected and scaled, not yet centred.
    pub fn raw(&self, p: Vec3) -> Point {
        let q = self.frame.project(p);
        Point::new(q.u * self.scale, q.v * self.scale)
    }

    /// View millimetres of a model point.
    pub fn point(&self, p: Vec3) -> Point2 {
        let q = self.frame.project(p);
        Point2::new(round(q.u * self.scale - self.centre.u), round(q.v * self.scale - self.centre.v))
    }

    /// A model direction in the view, scaled.
    pub fn vector(&self, d: Vec3) -> Point2 {
        Point2::new(d.dot(self.frame.right) * self.scale, d.dot(self.frame.up) * self.scale)
    }

    /// The model point in the plane through the model origin facing the eye that a view
    /// point shows. Any point along the view direction from it shows at the same place.
    pub fn unproject(&self, p: Point2) -> Vec3 {
        self.frame.right * ((p.u + self.centre.u) / self.scale) + self.frame.up * ((p.v + self.centre.v) / self.scale)
    }

    fn kpoint(&self, p: Vec3) -> Point {
        let q = self.point(p);
        Point::new(q.u, q.v)
    }
}

/// A view with what was computed for it: the kernel's curves and the transform into the
/// view's millimetres. Model items and the update work on these.
#[derive(Clone, Debug)]
pub struct ViewContext {
    /// The view.
    pub id: ViewId,
    /// The kernel's curves, with kernel ids. For a section, the cut body's.
    pub curves: ViewCurves,
    /// Model space to view millimetres.
    pub xf: ViewTransform,
    /// True for a pictorial view: one that looks along no model axis.
    pub pictorial: bool,
    /// The curves projected into the view.
    pub projected: Projected,
}

/// Views computed so far, by id: the parents derived views are made from.
pub type Contexts = BTreeMap<ViewId, ViewContext>;

impl ViewContext {
    /// Computes a view's curves and transform at its current scale, with its parents.
    pub fn compute(drawing: &Drawing, part: &Part, index: &PartIndex, id: ViewId) -> Result<ViewContext, DrawingError> {
        let mut memo = Contexts::new();
        compute_in(drawing, part, index, id, &mut memo)?;
        Ok(memo.remove(&id).expect("just computed"))
    }

    /// The kernel request the view was made with.
    pub fn spec(&self) -> ViewSpec {
        let f = self.xf.frame;
        ViewSpec::new(f.direction, f.up)
    }
}

/// Computes view `id` of the drawing into `memo`, its parents first, unless it is there.
pub fn compute_in(
    drawing: &Drawing,
    part: &Part,
    index: &PartIndex,
    id: ViewId,
    memo: &mut Contexts,
) -> Result<(), DrawingError> {
    if memo.contains_key(&id) {
        return Ok(());
    }
    let sheet = sheet_of(drawing, id).ok_or(DrawingError::UnknownView(id))?;
    let view = drawing.view(id).ok_or(DrawingError::UnknownView(id))?;
    let ctx = compute_view(drawing, part, index, view, sheet, memo)?;
    memo.insert(id, ctx);
    Ok(())
}

/// Computes the line work of `view` as if it were on sheet `sheet`, at its scale. The
/// view need not be on the drawing yet, so a tool can preview it; its parent must be.
/// Parents go into `memo`.
pub fn compute_view(
    drawing: &Drawing,
    part: &Part,
    index: &PartIndex,
    view: &View,
    sheet: usize,
    memo: &mut Contexts,
) -> Result<ViewContext, DrawingError> {
    compute_view_at(drawing, part, index, view, sheet, memo, 0)
}

fn compute_view_at(
    drawing: &Drawing,
    part: &Part,
    index: &PartIndex,
    view: &View,
    sheet: usize,
    memo: &mut Contexts,
    depth: u8,
) -> Result<ViewContext, DrawingError> {
    if depth > 16 {
        return Err(DrawingError::BadView(view.id));
    }
    if let Some(parent) = view.parent()
        && !memo.contains_key(&parent)
    {
        let p = drawing.view(parent).ok_or(DrawingError::UnknownView(parent))?;
        let ps = sheet_of(drawing, parent).unwrap_or(sheet);
        let ctx = compute_view_at(drawing, part, index, p, ps, memo, depth + 1)?;
        memo.insert(parent, ctx);
    }
    let scale = scale_in(drawing, sheet, view);
    let mut ctx = match &view.kind {
        ViewKind::Base { .. } | ViewKind::Projected { .. } => {
            let spec = spec_in(drawing, sheet, view).ok_or(DrawingError::BadView(view.id))?;
            let curves = view_curves(part, &spec)?;
            let xf = transform(&curves, &spec, scale)?;
            let projected = project(index, &curves, &xf, part.digest.as_str());
            ViewContext { id: view.id, curves, xf, pictorial: is_pictorial(&spec), projected }
        }
        ViewKind::Section { parent, line, flip, model_line, .. } => {
            let pctx = &memo[parent];
            let (direction, up) = section_axes(&pctx.xf.frame, line, *flip).ok_or(DrawingError::BadView(view.id))?;
            let spec = with_display(ViewSpec::new(direction, up), &view.display);
            let origin = model_line.first().copied().unwrap_or_else(|| pctx.xf.unproject(line[0]));
            let section = part
                .kernel
                .section_view(part.body, &SectionSpec { view: ViewSpec { preview: part.preview, ..spec }, origin })?;
            let xf = transform(&section.curves, &spec, scale)?;
            let mut projected = project(index, &section.curves, &xf, part.digest.as_str());
            projected.geometry.hatch = section.regions.iter().map(|r| region_polygon(r, &xf, None)).collect();
            ViewContext { id: view.id, curves: section.curves, xf, pictorial: false, projected }
        }
        ViewKind::Detail { parent, center, radius, profile, model_center, .. } => {
            let pctx = &memo[parent];
            let ratio = scale / pctx.xf.scale;
            let model = model_center.unwrap_or_else(|| pctx.xf.unproject(*center));
            let probe = ViewTransform { frame: pctx.xf.frame, scale, centre: Point2::new(0.0, 0.0) };
            let c = probe.raw(model);
            let xf = ViewTransform { frame: pctx.xf.frame, scale, centre: Point2::new(c.x, c.y) };
            // Parent view millimetres to detail view millimetres: p -> (p + pc) * ratio - dc.
            let (pc, dc) = (pctx.xf.centre, xf.centre);
            let (du, dv) = (pc.u - dc.u / ratio, pc.v - dc.v / ratio);
            let boundary = if profile.len() >= 3 {
                Boundary::Polygon { points: profile.clone() }
            } else {
                Boundary::Circle { center: *center, radius: *radius }
            };
            let boundary = boundary.mapped(du, dv, ratio);
            // The parent's finished line work, magnified: what the parent shows, cut or
            // broken out, the detail shows too.
            let map = |p: &Point2| Point2::new(round((p.u + du) * ratio), round((p.v + dv) * ratio));
            let kmap = |p: Point| Point::new(round((p.x + du) * ratio), round((p.y + dv) * ratio));
            let parent = &pctx.projected;
            let curves = parent
                .geometry
                .curves
                .iter()
                .map(|c| Curve2 { path: c.path.elements().iter().map(|el| map_el(*el, kmap)).collect(), ..c.clone() })
                .collect();
            let hatch = parent
                .geometry
                .hatch
                .iter()
                .map(|h| {
                    let outer: Vec<Point2> = h.outer.iter().map(map).collect();
                    let holes: Vec<Vec<Point2>> = h.holes.iter().map(|l| l.iter().map(map).collect()).collect();
                    match &h.clip {
                        // A region clipped already, as by a broken-out section: cut its
                        // outline to the detail and keep the first clip.
                        Some(c) => {
                            let window = boundary.polygon(96);
                            Polygon {
                                outer: crate::display::clip_polygon(&outer, &window),
                                holes: holes.iter().map(|l| crate::display::clip_polygon(l, &window)).collect(),
                                clip: Some(c.mapped(du, dv, ratio)),
                            }
                        }
                        None => Polygon { outer, holes, clip: Some(boundary.clone()) },
                    }
                })
                .filter(|h| h.outer.len() >= 3)
                .collect();
            let circles = parent
                .circles
                .iter()
                .map(|c| CircleInfo { centre: map(&c.centre), radius: c.radius * ratio, ..c.clone() })
                .collect();
            let geometry = ViewGeometry {
                source_digest: part.digest.to_string(),
                curves,
                hatch,
                outline: None,
                bbox: parent.geometry.bbox,
                unproved: parent.geometry.unproved,
            };
            let mut projected = Projected { geometry, transform: xf, circles };
            clip_projected(&mut projected, &boundary, true);
            projected.geometry.bbox = boundary.bounds().map(round);
            projected.geometry.outline = Some(boundary);
            ViewContext { id: view.id, curves: pctx.curves.clone(), xf, pictorial: pctx.pictorial, projected }
        }
        ViewKind::Auxiliary { parent, edge, flip, .. } => {
            let pctx = &memo[parent];
            let (direction, up) = auxiliary_axes(index, pctx, edge, *flip)?;
            let spec = with_display(ViewSpec::new(direction, up), &view.display);
            let curves = view_curves(part, &spec)?;
            let xf = transform(&curves, &spec, scale)?;
            let projected = project(index, &curves, &xf, part.digest.as_str());
            ViewContext { id: view.id, curves, xf, pictorial: false, projected }
        }
    };
    for b in &view.broken_out {
        broken_out(part, index, &mut ctx, b)?;
    }
    if let Some(crop) = &view.crop {
        clip_projected(&mut ctx.projected, crop, true);
        let mut bbox = curves_bounds(&ctx.projected.geometry.curves).unwrap_or(ctx.projected.geometry.bbox);
        for h in &ctx.projected.geometry.hatch {
            for p in &h.outer {
                bbox = [bbox[0].min(p.u), bbox[1].min(p.v), bbox[2].max(p.u), bbox[3].max(p.v)];
            }
        }
        let c = crop.bounds();
        ctx.projected.geometry.bbox =
            [bbox[0].max(c[0]), bbox[1].max(c[1]), bbox[2].min(c[2]), bbox[3].min(c[3])].map(round);
    }
    Ok(ctx)
}

fn with_display(mut spec: ViewSpec, display: &ViewDisplay) -> ViewSpec {
    spec.smooth_edges = display.tangent != crate::view::TangentEdges::Removed;
    spec.seams = display.seams;
    spec
}

/// Cuts a broken-out section into a view: the material in front of the plane `depth`
/// behind the part's nearest point is removed inside the boundary.
fn broken_out(part: &Part, index: &PartIndex, ctx: &mut ViewContext, b: &BrokenOut) -> Result<(), DrawingError> {
    let d = ctx.xf.frame.direction;
    let bbox = part.kernel.topology(part.body)?.bbox;
    let front = (0..8)
        .map(|i| {
            Vec3::new(
                if i & 1 == 0 { bbox.min.x } else { bbox.max.x },
                if i & 2 == 0 { bbox.min.y } else { bbox.max.y },
                if i & 4 == 0 { bbox.min.z } else { bbox.max.z },
            )
            .dot(d)
        })
        .fold(f64::NEG_INFINITY, f64::max);
    let mut spec = ctx.spec();
    spec.smooth_edges = ctx.projected.geometry.curves.iter().any(|c| c.tangent);
    spec.preview = part.preview;
    let section = part.kernel.section_view(part.body, &SectionSpec { view: spec, origin: d * (front - b.depth) })?;
    let mut cut = project(index, &section.curves, &ctx.xf, part.digest.as_str());
    clip_projected(&mut ctx.projected, &b.boundary, false);
    clip_projected(&mut cut, &b.boundary, true);
    ctx.projected.geometry.curves.extend(cut.geometry.curves);
    ctx.projected.geometry.unproved |= cut.geometry.unproved;
    ctx.projected
        .geometry
        .hatch
        .extend(section.regions.iter().map(|r| region_polygon(r, &ctx.xf, Some(b.boundary.clone()))));
    Ok(())
}

/// A cut region as a polygon in view millimetres.
pub fn region_polygon(r: &CutRegion, xf: &ViewTransform, clip: Option<Boundary>) -> Polygon {
    let ring = |curves: &[ViewCurve]| {
        let mut out: Vec<Point2> = Vec::new();
        for c in curves {
            for p in c.curve.sample(SHEET_CHORD_MM / xf.scale.max(1e-9)) {
                let q = xf.point(p);
                if out.last().is_none_or(|l| (l.u - q.u).abs() > 1e-9 || (l.v - q.v).abs() > 1e-9) {
                    out.push(q);
                }
            }
        }
        if out.len() > 1 && out.first() == out.last() {
            out.pop();
        }
        out
    };
    Polygon { outer: ring(&r.outer), holes: r.holes.iter().map(|h| ring(h)).collect(), clip }
}

/// Keeps the part of every curve, hatch and face-on circle inside `boundary` (or
/// outside it, when `inside` is false).
pub fn clip_projected(p: &mut Projected, boundary: &Boundary, inside: bool) {
    let test = |q: Point| boundary.contains(Point2::new(q.x, q.y)) == inside;
    let curves = std::mem::take(&mut p.geometry.curves);
    for mut c in curves {
        c.path = clip_path(&c.path, &test);
        if c.path.elements().len() >= 2 {
            p.geometry.curves.push(c);
        }
    }
    if inside {
        for h in &mut p.geometry.hatch {
            if h.clip.is_none() {
                h.clip = Some(boundary.clone());
            }
        }
    }
    p.circles.retain(|c| boundary.contains(c.centre) == inside);
}

/// The bounds of the curves' control points, or `None` when there are none.
fn curves_bounds(curves: &[Curve2]) -> Option<[f64; 4]> {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for c in curves {
        let r = kurbo::Shape::bounding_box(&c.path);
        b = [b[0].min(r.x0), b[1].min(r.y0), b[2].max(r.x1), b[3].max(r.y1)];
    }
    b[0].is_finite().then_some(b)
}

/// The parts of `path` where `keep` holds, exactly: each segment is cut where it crosses
/// the boundary of the kept region, found by sampling and bisection, and its pieces are
/// the segment's own subsegments.
pub fn clip_path(path: &BezPath, keep: &dyn Fn(Point) -> bool) -> BezPath {
    let mut out = BezPath::new();
    let mut pen: Option<Point> = None;
    for seg in path.segments() {
        let length = kurbo::ParamCurveArclen::arclen(&seg, 1e-3);
        let n = ((length / 0.25).ceil() as usize).clamp(32, 4000);
        let mut cuts = vec![0.0];
        let mut was = keep(seg.eval(0.0));
        for i in 1..=n {
            let t = i as f64 / n as f64;
            let now = keep(seg.eval(t));
            if now != was {
                let (mut lo, mut hi) = ((i - 1) as f64 / n as f64, t);
                for _ in 0..48 {
                    let mid = 0.5 * (lo + hi);
                    if keep(seg.eval(mid)) == was {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                cuts.push(0.5 * (lo + hi));
            }
            was = now;
        }
        cuts.push(1.0);
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            if b - a < 1e-12 || !keep(seg.eval(0.5 * (a + b))) {
                pen = None;
                continue;
            }
            let piece = seg.subsegment(a..b);
            let start = piece.eval(0.0);
            if pen.is_none_or(|q| q.distance(start) > 1e-9) {
                out.move_to(start);
            }
            match piece {
                PathSeg::Line(l) => out.line_to(l.p1),
                PathSeg::Quad(q) => out.quad_to(q.p1, q.p2),
                PathSeg::Cubic(c) => out.curve_to(c.p1, c.p2, c.p3),
            }
            pen = Some(piece.eval(1.0));
        }
    }
    out
}

/// True when `spec` looks along no model axis.
pub fn is_pictorial(spec: &ViewSpec) -> bool {
    let d = spec.direction.normalized().unwrap_or(Vec3::Z);
    [d.x, d.y, d.z].iter().filter(|c| c.abs() > 1e-9).count() > 1
}

/// The kernel request for a view: its orientation, followed through its parents.
pub fn view_spec(drawing: &Drawing, id: ViewId) -> Result<ViewSpec, DrawingError> {
    let sheet = sheet_of(drawing, id).ok_or(DrawingError::UnknownView(id))?;
    let view = drawing.view(id).ok_or(DrawingError::UnknownView(id))?;
    spec_in(drawing, sheet, view).ok_or(DrawingError::BadView(id))
}

/// The kernel request for a view of `sheet`, or `None` when its orientation is
/// degenerate or a parent is missing. Parents are looked for on `sheet` only; see
/// [`spec_in`] for views whose parents are on other sheets. An auxiliary view's own
/// direction needs the part, so here it looks the way its parent does.
pub fn spec_of(sheet: &Sheet, view: &View) -> Option<ViewSpec> {
    let find = |id: ViewId| sheet.view(id).map(|v| (v, sheet.projection));
    let (direction, up) = view_axes(&find, view, sheet.projection, 0)?;
    Some(with_display(ViewSpec::new(direction, up), &view.display))
}

/// The kernel request for `view` as if on sheet `sheet` of `drawing`, its parents looked
/// for on every sheet. An auxiliary view looks the way its parent does here; its own
/// direction needs the part ([`compute_view`]).
pub fn spec_in(drawing: &Drawing, sheet: usize, view: &View) -> Option<ViewSpec> {
    let find = |id: ViewId| drawing.sheets.iter().find_map(|s| s.view(id).map(|v| (v, s.projection)));
    let projection = drawing.sheets.get(sheet).map_or(Projection::FirstAngle, |s| s.projection);
    let (direction, up) = view_axes(&find, view, projection, 0)?;
    Some(with_display(ViewSpec::new(direction, up), &view.display))
}

type Find<'a> = dyn Fn(ViewId) -> Option<(&'a View, Projection)> + 'a;

/// A view's direction towards the eye and up vector, following its parents back to a
/// base view. `projection` is the angle of the sheet the view is on.
fn view_axes<'a>(find: &Find<'a>, view: &View, projection: Projection, depth: u8) -> Option<(Vec3, Vec3)> {
    if depth > 16 {
        return None;
    }
    let parent = |id: ViewId| {
        let (p, angle) = find(id)?;
        view_axes(find, p, angle, depth + 1)
    };
    match &view.kind {
        ViewKind::Base { orientation } => {
            let s = orientation.spec(&view.display);
            Some((s.direction, s.up))
        }
        ViewKind::Projected { parent: p, direction } => {
            let (pd, pu) = parent(*p)?;
            let frame = ViewSpec::new(pd, pu).frame()?;
            Some(fold(&frame, *direction, projection))
        }
        ViewKind::Section { parent: p, line, flip, .. } => {
            let (pd, pu) = parent(*p)?;
            section_axes(&ViewSpec::new(pd, pu).frame()?, line, *flip)
        }
        ViewKind::Detail { parent: p, .. } | ViewKind::Auxiliary { parent: p, .. } => parent(*p),
    }
}

/// The way the arrows of a cutting line point on the parent, the way the section looks,
/// as a unit vector in the parent's view: to the left of the line run from its leftmost
/// (or, when vertical, lowest) point, the other way when `flip` is set. `None` when the
/// line has no length.
pub fn section_arrow(line: &[Point2], flip: bool) -> Option<Point2> {
    let (a, b) = (line.first()?, line.last()?);
    let (mut dx, mut dy) = (b.u - a.u, b.v - a.v);
    let len = dx.hypot(dy);
    if len < 1e-9 {
        return None;
    }
    (dx, dy) = (dx / len, dy / len);
    if dx < -1e-9 || (dx.abs() <= 1e-9 && dy < 0.0) {
        (dx, dy) = (-dx, -dy);
    }
    let arrow = Point2::new(-dy, dx);
    Some(if flip { Point2::new(-arrow.u, -arrow.v) } else { arrow })
}

/// The frame turned a quarter turn about the in-view direction `axis` so that the eye
/// moves to the side `eye` (both unit vectors in the parent's view): what a section or
/// auxiliary view looks along. Returns (direction towards the eye, up).
pub fn fold_about(parent: &ViewFrame, axis: Point2, eye: Point2) -> (Vec3, Vec3) {
    let e = parent.right * axis.u + parent.up * axis.v;
    let n = parent.right * eye.u + parent.up * eye.v;
    let d = parent.direction;
    let turn = |v: Vec3| e * v.dot(e) - d * v.dot(n) + n * v.dot(d);
    (n, turn(parent.up))
}

/// The direction towards the eye and up vector of a section through a parent with frame
/// `parent` along `line`.
pub fn section_axes(parent: &ViewFrame, line: &[Point2], flip: bool) -> Option<(Vec3, Vec3)> {
    let arrow = section_arrow(line, flip)?;
    let axis = Point2::new(arrow.v, -arrow.u);
    Some(fold_about(parent, axis, Point2::new(-arrow.u, -arrow.v)))
}

/// The direction an auxiliary view looks from and its up vector: square to the
/// reference edge in the parent's view, from the side of the edge away from the
/// parent's centre unless `flip`.
fn auxiliary_axes(
    index: &PartIndex,
    pctx: &ViewContext,
    edge: &EntityRef,
    flip: bool,
) -> Result<(Vec3, Vec3), DrawingError> {
    let id = match index.resolve(edge) {
        Resolution::Found(Entity::Edge(id)) => id,
        Resolution::Found(_) => return Err(DrawingError::BadRequest("an auxiliary view needs an edge".into())),
        Resolution::Lost(why) => return Err(DrawingError::LostReference(why)),
    };
    let curve = index.edge_curve(id).ok_or(DrawingError::LostReference("the edge has no curve".into()))?;
    let along = pctx.xf.vector(curve.end() - curve.start());
    let len = along.u.hypot(along.v);
    if len < 1e-6 {
        return Err(DrawingError::BadRequest("the parent sees the reference edge end on".into()));
    }
    let axis = Point2::new(along.u / len, along.v / len);
    let mut eye = Point2::new(-axis.v, axis.u);
    let mid = pctx.xf.point(curve.point_at(0.5));
    if mid.u * eye.u + mid.v * eye.v < 0.0 {
        eye = Point2::new(-eye.u, -eye.v);
    }
    if flip {
        eye = Point2::new(-eye.u, -eye.v);
    }
    Ok(fold_about(&pctx.xf.frame, axis, eye))
}

/// The direction towards the eye and the up vector of a view folded off a parent with
/// frame `parent` towards `side`.
///
/// Third angle places a view on the side of the face it shows; first angle on the
/// opposite side. The diagonals give pictorial views from the corner between the two
/// faces the neighbouring orthographic folds show.
pub fn fold(parent: &ViewFrame, side: FoldDirection, projection: Projection) -> (Vec3, Vec3) {
    let (r, u, d) = (parent.right, parent.up, parent.direction);
    let (sx, sy): (f64, f64) = match side {
        FoldDirection::Left => (-1.0, 0.0),
        FoldDirection::Right => (1.0, 0.0),
        FoldDirection::Up => (0.0, 1.0),
        FoldDirection::Down => (0.0, -1.0),
        FoldDirection::UpLeft => (-1.0, 1.0),
        FoldDirection::UpRight => (1.0, 1.0),
        FoldDirection::DownLeft => (-1.0, -1.0),
        FoldDirection::DownRight => (1.0, -1.0),
    };
    let k = match projection {
        Projection::ThirdAngle => 1.0,
        Projection::FirstAngle => -1.0,
    };
    let (sx, sy) = (sx * k, sy * k);
    if side.is_diagonal() {
        let direction = (r * sx + u * sy + d).normalized().unwrap_or(d);
        return (direction, u);
    }
    if sy == 0.0 {
        (r * sx, u)
    } else {
        // Folding up or down: the old direction towards the eye becomes the new down or up.
        (u * sy, d * -sy)
    }
}

/// Sheet millimetres per model millimetre of a view, following parents on any sheet.
pub fn view_scale(drawing: &Drawing, id: ViewId) -> Result<f64, DrawingError> {
    let sheet = sheet_of(drawing, id).ok_or(DrawingError::UnknownView(id))?;
    Ok(scale_in(drawing, sheet, drawing.view(id).ok_or(DrawingError::UnknownView(id))?))
}

/// Sheet millimetres per model millimetre of a view of `sheet`, following parents on
/// that sheet. See [`scale_in`] for views whose parents are on other sheets.
pub fn effective_scale(sheet: &Sheet, view: &View) -> f64 {
    let find = |id: ViewId| sheet.view(id).map(|v| (v, sheet.scale.factor()));
    scale_of(&find, sheet.scale.factor(), view, 0)
}

/// Sheet millimetres per model millimetre of `view` as if on sheet `sheet` of
/// `drawing`, following parents on every sheet.
pub fn scale_in(drawing: &Drawing, sheet: usize, view: &View) -> f64 {
    let find = |id: ViewId| drawing.sheets.iter().find_map(|s| s.view(id).map(|v| (v, s.scale.factor())));
    let own = drawing.sheets.get(sheet).map_or(1.0, |s| s.scale.factor());
    scale_of(&find, own, view, 0)
}

type FindScale<'a> = dyn Fn(ViewId) -> Option<(&'a View, f64)> + 'a;

/// A detail view that follows its parent's scale magnifies it by its factor.
fn scale_of<'a>(find: &FindScale<'a>, sheet_scale: f64, view: &View, depth: u8) -> f64 {
    match view.scale {
        ViewScale::Sheet => sheet_scale,
        ViewScale::Custom { factor } => factor,
        ViewScale::Parent => {
            let factor = match view.kind {
                ViewKind::Detail { factor, .. } if factor > 0.0 => factor,
                _ => 1.0,
            };
            match view.parent().and_then(find) {
                Some((p, own)) if depth < 16 => scale_of(find, own, p, depth + 1) * factor,
                _ => sheet_scale * factor,
            }
        }
    }
}

/// The kernel's curves for a view, with the mesh method when the backend has no exact
/// hidden-line removal.
pub fn view_curves(part: &Part, spec: &ViewSpec) -> Result<ViewCurves, DrawingError> {
    let spec = &ViewSpec { preview: part.preview, ..*spec };
    match part.kernel.view_curves(part.body, spec) {
        Err(KernelError::Unsupported(_)) => Ok(mesh_hlr::view_curves(part.kernel, part.body, spec)?),
        other => Ok(other?),
    }
}

/// The transform that centres `curves` at `scale`.
pub fn transform(curves: &ViewCurves, spec: &ViewSpec, scale: f64) -> Result<ViewTransform, DrawingError> {
    let frame = spec.frame().ok_or_else(|| DrawingError::Kernel(KernelError::InvalidInput("view direction".into())))?;
    let [x0, y0, x1, y1] = curves.bbox_2d;
    let centre = Point2::new((x0 + x1) / 2.0 * scale, (y0 + y1) / 2.0 * scale);
    Ok(ViewTransform { frame, scale, centre })
}

/// A circular edge seen face-on in a view: what centre marks and hole callouts attach
/// to.
#[derive(Clone, Debug, PartialEq)]
pub struct CircleInfo {
    /// The edge.
    pub entity: EntityRef,
    /// Centre in view millimetres.
    pub centre: Point2,
    /// Radius in view millimetres.
    pub radius: f64,
    /// Centre in model space.
    pub model_centre: Vec3,
    /// Radius in model millimetres.
    pub model_radius: f64,
}

/// One view's projection: its line work, its transform and the circles it sees
/// face-on.
#[derive(Clone, Debug, PartialEq)]
pub struct Projected {
    /// The line work.
    pub geometry: ViewGeometry,
    /// Model space to view millimetres.
    pub transform: ViewTransform,
    /// Circles seen face-on, one per edge.
    pub circles: Vec<CircleInfo>,
}

impl Projected {
    /// Width and height in view millimetres.
    pub fn size(&self) -> (f64, f64) {
        let [x0, y0, x1, y1] = self.geometry.bbox;
        (x1 - x0, y1 - y0)
    }

    /// Changes the scale without going back to the kernel.
    pub fn rescale(&mut self, scale: f64) {
        let k = scale / self.transform.scale;
        if (k - 1.0).abs() < 1e-12 {
            return;
        }
        let s = |p: Point| Point::new(round(p.x * k), round(p.y * k));
        for c in &mut self.geometry.curves {
            c.path = c.path.elements().iter().map(|el| map_el(*el, s)).collect();
        }
        self.geometry.bbox = self.geometry.bbox.map(|v| round(v * k));
        self.transform.centre = Point2::new(self.transform.centre.u * k, self.transform.centre.v * k);
        self.transform.scale = scale;
        for c in &mut self.circles {
            c.centre = Point2::new(round(c.centre.u * k), round(c.centre.v * k));
            c.radius *= k;
        }
    }
}

/// Projects the kernel's curves into a view's line work.
pub fn project(index: &PartIndex, curves: &ViewCurves, xf: &ViewTransform, digest: &str) -> Projected {
    let mut out = Vec::with_capacity(curves.curves.len());
    let mut circles: Vec<CircleInfo> = Vec::new();
    for c in &curves.curves {
        let path = path_of(&c.curve, xf);
        if path.elements().len() < 2 {
            continue;
        }
        let (source, kind) = source_ref(index, c);
        // Visibility fragments keep the complete edge's exact circle metadata.
        let geometry = match c.source {
            CurveSource::Edge(edge) => index.edge_curve(edge).unwrap_or(&c.curve),
            _ => &c.curve,
        };
        if let Curve3::Circle { center, normal, radius } = geometry
            && normal.cross(xf.frame.direction).length() < 1e-6
            && matches!(c.source, CurveSource::Edge(_))
            && !circles.iter().any(|k| k.entity.kernel_hint == source.kernel_hint)
        {
            circles.push(CircleInfo {
                entity: source.clone(),
                centre: xf.point(*center),
                radius: radius * xf.scale,
                model_centre: *center,
                model_radius: *radius,
            });
        }
        out.push(Curve2 { path, source, visible: c.visible, kind, tangent: c.smooth, accuracy: accuracy_of(&c.curve) });
    }
    let [x0, y0, x1, y1] = curves.bbox_2d;
    let (lo, hi) = (
        Point2::new(round(x0 * xf.scale - xf.centre.u), round(y0 * xf.scale - xf.centre.v)),
        Point2::new(round(x1 * xf.scale - xf.centre.u), round(y1 * xf.scale - xf.centre.v)),
    );
    let geometry = ViewGeometry {
        source_digest: digest.to_owned(),
        curves: out,
        hatch: Vec::new(),
        outline: None,
        bbox: [lo.u, lo.v, hi.u, hi.v],
        unproved: curves.unproved,
    };
    Projected { geometry, transform: *xf, circles }
}

/// Projects `part` for a view with orientation `spec` at `scale`.
pub fn project_part(part: &Part, index: &PartIndex, spec: &ViewSpec, scale: f64) -> Result<Projected, DrawingError> {
    let curves = view_curves(part, spec)?;
    let xf = transform(&curves, spec, scale)?;
    Ok(project(index, &curves, &xf, part.digest.as_str()))
}

/// Centre marks for the circles a view sees face-on, one per centre (the largest circle
/// there), with ids from `first_id`.
pub fn centre_marks(projected: &Projected, view: ViewId, first_id: u32) -> Vec<Annotation> {
    let mut by_centre: Vec<&CircleInfo> = Vec::new();
    for c in &projected.circles {
        match by_centre
            .iter()
            .position(|k| (k.centre.u - c.centre.u).abs() < 1e-3 && (k.centre.v - c.centre.v).abs() < 1e-3)
        {
            Some(i) if c.radius > by_centre[i].radius => by_centre[i] = c,
            Some(_) => {}
            None => by_centre.push(c),
        }
    }
    by_centre
        .iter()
        .enumerate()
        .map(|(i, c)| Annotation {
            id: AnnotationId(first_id + i as u32),
            view,
            body: AnnotationKind::CentreMark,
            anchors: vec![Anchor { entity: c.entity.clone(), attach: Attach::Centre, cached: c.centre }],
            placement: c.centre,
            style: StyleOverrides::default(),
            state: AnnotationState::Ok,
        })
        .collect()
}

/// The standard view with these axes, if any.
pub fn standard_of(direction: Vec3, up: Vec3) -> Option<StandardView> {
    let want = ViewSpec::new(direction, up).frame()?;
    StandardView::ALL.into_iter().find(|v| {
        ViewSpec::standard(*v)
            .frame()
            .is_some_and(|f| f.direction.distance(want.direction) < 1e-6 && f.up.distance(want.up) < 1e-6)
    })
}

/// The caption of a standard view, such as "Front view".
pub fn standard_label(view: StandardView) -> &'static str {
    match view {
        StandardView::Front => "Front view",
        StandardView::Back => "Back view",
        StandardView::Top => "Top view",
        StandardView::Bottom => "Bottom view",
        StandardView::Right => "Right view",
        StandardView::Left => "Left view",
        StandardView::Isometric => "Isometric view",
    }
}

/// A view's name in the tree: its own label, or what it shows.
pub fn view_label(sheet: &Sheet, view: &View) -> String {
    if let Some(l) = &view.label {
        return l.clone();
    }
    let find = |id: ViewId| sheet.view(id).map(|v| (v, sheet.projection));
    match &view.kind {
        ViewKind::Section { label, .. } => format!("Section {label}-{label}"),
        ViewKind::Detail { label, .. } => format!("Detail {label}"),
        ViewKind::Auxiliary { label, .. } if !label.is_empty() => format!("View {label}"),
        ViewKind::Auxiliary { .. } => "Auxiliary view".into(),
        _ => match view_axes(&find, view, sheet.projection, 0).and_then(|(d, u)| standard_of(d, u)) {
            Some(s) => standard_label(s).to_owned(),
            None if matches!(view.kind, ViewKind::Base { .. }) => "Model view".into(),
            None => "Pictorial view".into(),
        },
    }
}

/// The side a projected standard view folds to from the front view under a projection
/// angle, or `None` for views that do not fold from the front.
pub fn standard_fold(projection: Projection, view: StandardView) -> Option<FoldDirection> {
    Some(match (projection, view) {
        (Projection::FirstAngle, StandardView::Top) => FoldDirection::Down,
        (Projection::FirstAngle, StandardView::Bottom) => FoldDirection::Up,
        (Projection::FirstAngle, StandardView::Left) => FoldDirection::Right,
        (Projection::FirstAngle, StandardView::Right) => FoldDirection::Left,
        (Projection::ThirdAngle, StandardView::Top) => FoldDirection::Up,
        (Projection::ThirdAngle, StandardView::Bottom) => FoldDirection::Down,
        (Projection::ThirdAngle, StandardView::Left) => FoldDirection::Left,
        (Projection::ThirdAngle, StandardView::Right) => FoldDirection::Right,
        _ => return None,
    })
}

/// The display a standard view starts with in the app: hidden lines shown on
/// orthographic views, removed on pictorial ones, tangent edges phantom on pictorials.
pub fn default_display(view: StandardView) -> ViewDisplay {
    match view {
        StandardView::Isometric => ViewDisplay::PICTORIAL,
        _ => ViewDisplay { hidden: crate::view::HiddenLines::Visible, ..ViewDisplay::ORTHOGRAPHIC },
    }
}

/// Where a view's geometry sits on the sheet: `[left, bottom, right, top]`.
pub fn view_bounds(view: &View) -> [f64; 4] {
    let [x0, y0, x1, y1] = view.geometry.bbox;
    [x0 + view.position.u, y0 + view.position.v, x1 + view.position.u, y1 + view.position.v]
}

/// The views aligned to `parent`, directly or through other children.
pub fn descendants(sheet: &Sheet, parent: ViewId) -> Vec<ViewId> {
    let mut out = Vec::new();
    let mut stack = vec![parent];
    while let Some(p) = stack.pop() {
        for v in &sheet.views {
            if v.alignment.is_some_and(|a| a.parent == p) && !out.contains(&v.id) {
                out.push(v.id);
                stack.push(v.id);
            }
        }
    }
    out
}

/// Index of the sheet a view is on.
pub fn sheet_of(drawing: &Drawing, view: ViewId) -> Option<usize> {
    drawing.sheets.iter().position(|s| s.view(view).is_some())
}

/// The name of a view curve's source.
fn source_ref(index: &PartIndex, c: &ViewCurve) -> (EntityRef, CurveKind) {
    let unknown = || EntityRef::new(EntityKind::Edge, "", "unknown", 0, Vec3::ZERO, 0.0);
    match c.source {
        CurveSource::Edge(id) => (index.edge_ref(id).unwrap_or_else(unknown), CurveKind::Edge),
        CurveSource::Silhouette(id) => (index.face_ref(id).unwrap_or_else(unknown), CurveKind::Silhouette),
        CurveSource::Seam(id) => (index.face_ref(id).unwrap_or_else(unknown), CurveKind::Seam),
        CurveSource::Section(id) => (index.face_ref(id).unwrap_or_else(unknown), CurveKind::Section),
        CurveSource::Unknown => (unknown(), CurveKind::Edge),
    }
}

/// Chord tolerance on the sheet for curves that are neither lines nor arcs.
const SHEET_CHORD_MM: f64 = 0.01;

/// Whether a kernel curve is the model's own curve or a polyline from its mesh.
///
/// The kernel keeps a whole line or an untrimmed circle after polygon visibility and
/// answers a polyline for everything else; a NURBS curve is sampled on the sheet, so it
/// is labelled by what the path holds, not by what the model has.
pub fn accuracy_of(curve: &Curve3) -> Accuracy {
    match curve {
        Curve3::Line { .. } | Curve3::Arc { .. } | Curve3::Circle { .. } => Accuracy::Exact,
        Curve3::Polyline { .. } | Curve3::Nurbs { .. } => Accuracy::Polygon,
    }
}

/// A model curve as a path in view millimetres.
pub fn path_of(curve: &Curve3, xf: &ViewTransform) -> BezPath {
    let mut path = BezPath::new();
    match curve {
        Curve3::Line { a, b } => {
            path.move_to(xf.kpoint(*a));
            path.line_to(xf.kpoint(*b));
        }
        Curve3::Arc { center, normal, x_axis, radius, start_deg, end_deg } => {
            arc_path(&mut path, xf, *center, *normal, *x_axis, *radius, *start_deg, *end_deg);
        }
        Curve3::Circle { center, normal, radius } => {
            let x_axis = soliddraft_kernel_api::drawing::perpendicular(*normal);
            arc_path(&mut path, xf, *center, *normal, x_axis, *radius, 0.0, 360.0);
            path.close_path();
        }
        Curve3::Nurbs { .. } => {
            let simple = curve.simplified(1e-6);
            if !matches!(simple, Curve3::Nurbs { .. }) {
                return path_of(&simple, xf);
            }
            polyline(&mut path, xf, &curve.sample(SHEET_CHORD_MM / xf.scale.max(1e-9)));
        }
        Curve3::Polyline { points } => polyline(&mut path, xf, points),
    }
    path
}

fn polyline(path: &mut BezPath, xf: &ViewTransform, points: &[Vec3]) {
    for (i, p) in points.iter().enumerate() {
        if i == 0 {
            path.move_to(xf.kpoint(*p));
        } else {
            path.line_to(xf.kpoint(*p));
        }
    }
}

/// Cubic Béziers for a circular arc, at most a quarter turn each, built in model space
/// and projected. Parallel projection is affine, so the projected cubics approximate the
/// projected arc exactly as well as the model ones approximate the arc.
#[allow(clippy::too_many_arguments)]
fn arc_path(
    path: &mut BezPath,
    xf: &ViewTransform,
    center: Vec3,
    normal: Vec3,
    x_axis: Vec3,
    radius: f64,
    start_deg: f64,
    end_deg: f64,
) {
    let y_axis = normal.cross(x_axis);
    let at = |t: f64| center + (x_axis * t.cos() + y_axis * t.sin()) * radius;
    let tangent = |t: f64| (x_axis * -t.sin() + y_axis * t.cos()) * radius;
    let (a0, a1) = (start_deg.to_radians(), end_deg.to_radians());
    let pieces = ((end_deg - start_deg).abs() / 90.0).ceil().max(1.0) as usize;
    let step = (a1 - a0) / pieces as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    path.move_to(xf.kpoint(at(a0)));
    for i in 0..pieces {
        let (s, e) = (a0 + step * i as f64, a0 + step * (i + 1) as f64);
        let (p0, p3) = (at(s), at(e));
        let p1 = p0 + tangent(s) * k;
        let p2 = p3 - tangent(e) * k;
        path.curve_to(xf.kpoint(p1), xf.kpoint(p2), xf.kpoint(p3));
    }
}

/// Re-projects a view at a new scale without the kernel: its geometry and the cached
/// points of its annotations scale about the view's origin.
pub fn rescale_view(view: &mut View, ratio: f64) {
    let s = |p: Point| Point::new(round(p.x * ratio), round(p.y * ratio));
    for c in &mut view.geometry.curves {
        c.path = c.path.elements().iter().map(|el| map_el(*el, s)).collect();
    }
    for poly in &mut view.geometry.hatch {
        for p in poly.outer.iter_mut().chain(poly.holes.iter_mut().flatten()) {
            *p = Point2::new(round(p.u * ratio), round(p.v * ratio));
        }
        if let Some(c) = &mut poly.clip {
            *c = c.mapped(0.0, 0.0, ratio);
        }
    }
    view.geometry.bbox = view.geometry.bbox.map(|v| round(v * ratio));
    if let Some(c) = &mut view.crop {
        *c = c.mapped(0.0, 0.0, ratio);
    }
    if let Some(c) = &mut view.geometry.outline {
        *c = c.mapped(0.0, 0.0, ratio);
    }
    for b in &mut view.broken_out {
        b.boundary = b.boundary.mapped(0.0, 0.0, ratio);
    }
}

pub(crate) fn map_el(el: kurbo::PathEl, f: impl Fn(Point) -> Point) -> kurbo::PathEl {
    use kurbo::PathEl;
    match el {
        PathEl::MoveTo(p) => PathEl::MoveTo(f(p)),
        PathEl::LineTo(p) => PathEl::LineTo(f(p)),
        PathEl::QuadTo(a, b) => PathEl::QuadTo(f(a), f(b)),
        PathEl::CurveTo(a, b, c) => PathEl::CurveTo(f(a), f(b), f(c)),
        PathEl::ClosePath => PathEl::ClosePath,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soliddraft_kernel_api::drawing::StandardView;

    fn frame(v: StandardView) -> ViewFrame {
        ViewSpec::standard(v).frame().unwrap()
    }

    fn same(a: Vec3, b: Vec3) -> bool {
        a.distance(b) < 1e-12
    }

    #[test]
    fn first_angle_folds_put_views_opposite_the_face_they_show() {
        let front = frame(StandardView::Front);
        let (d, u) = fold(&front, FoldDirection::Down, Projection::FirstAngle);
        let top = frame(StandardView::Top);
        assert!(same(d, top.direction) && same(u, top.up), "below front is the top view");
        let (d, u) = fold(&front, FoldDirection::Right, Projection::FirstAngle);
        let left = frame(StandardView::Left);
        assert!(same(d, left.direction) && same(u, left.up), "right of front is the left view");
    }

    #[test]
    fn third_angle_folds_put_views_on_the_side_of_the_face_they_show() {
        let front = frame(StandardView::Front);
        let (d, u) = fold(&front, FoldDirection::Up, Projection::ThirdAngle);
        let top = frame(StandardView::Top);
        assert!(same(d, top.direction) && same(u, top.up));
        let (d, _) = fold(&front, FoldDirection::Right, Projection::ThirdAngle);
        assert!(same(d, frame(StandardView::Right).direction));
        let (d, u) = fold(&front, FoldDirection::Down, Projection::ThirdAngle);
        let bottom = frame(StandardView::Bottom);
        assert!(same(d, bottom.direction) && same(u, bottom.up));
        let (d, _) = fold(&front, FoldDirection::UpRight, Projection::ThirdAngle);
        assert!(same(d, frame(StandardView::Isometric).direction), "up and right of front is the isometric");
    }

    #[test]
    fn lines_arcs_and_circles_are_exact_and_mesh_polylines_are_polygon() {
        let a = Vec3::ZERO;
        let b = Vec3::new(10.0, 0.0, 0.0);
        assert_eq!(accuracy_of(&Curve3::Line { a, b }), Accuracy::Exact);
        assert_eq!(accuracy_of(&Curve3::Circle { center: a, normal: Vec3::Z, radius: 5.0 }), Accuracy::Exact);
        let arc =
            Curve3::Arc { center: a, normal: Vec3::Z, x_axis: Vec3::X, radius: 5.0, start_deg: 0.0, end_deg: 90.0 };
        assert_eq!(accuracy_of(&arc), Accuracy::Exact);
        assert_eq!(accuracy_of(&Curve3::Polyline { points: vec![a, b] }), Accuracy::Polygon);
        assert_eq!(Accuracy::default(), Accuracy::Polygon, "a file without the label makes the weaker claim");
    }

    #[test]
    fn circles_project_to_exact_ellipses() {
        let spec = ViewSpec::standard(StandardView::Isometric);
        let xf = ViewTransform { frame: spec.frame().unwrap(), scale: 2.0, centre: Point2::new(0.0, 0.0) };
        let circle = Curve3::Circle { center: Vec3::ZERO, normal: Vec3::Z, radius: 10.0 };
        let path = path_of(&circle, &xf);
        // Every point of the projected path, taken back to the circle's plane, lies on the
        // circle within the cubic approximation's error, about 2.7e-4 of the radius.
        let f = xf.frame;
        let (a, b, c, d) = (f.right.x * 2.0, f.right.y * 2.0, f.up.x * 2.0, f.up.y * 2.0);
        let det = a * d - b * c;
        let mut worst: f64 = 0.0;
        kurbo::flatten(path.iter(), 1e-4, |el| {
            if let kurbo::PathEl::LineTo(p) | kurbo::PathEl::MoveTo(p) = el {
                let (x, y) = ((d * p.x - b * p.y) / det, (-c * p.x + a * p.y) / det);
                worst = worst.max((x.hypot(y) - 10.0).abs());
            }
        });
        assert!(worst < 10.0 * 3e-4, "{worst}");
    }
}
