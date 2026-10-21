//! Model items: the part's feature dimensions brought into the drawing, each once, in the
//! view that shows it best, as reference dimensions anchored to real edges.
//!
//! The part's definition says what to dimension ([`ModelDimension`]: a width between
//! two planes, a diameter of a circle). The import finds, in each candidate view, the
//! edges that carry those planes or that circle, prefers views where the sketch is seen
//! true shape and the edges are visible, and places the dimension outside the view,
//! stacking parallel dimensions with the shorter ones nearer the view.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use soliddraft_kernel_api::drawing::{Curve3, CurveSource};
use soliddraft_kernel_api::{EdgeId, Point2, Vec3};

use crate::annotation::{
    Anchor, Annotation, AnnotationKind, AnnotationState, Attach, Dimension, DimensionKind, LinearOrientation,
    StyleOverrides,
};
use crate::ids::ViewId;
use crate::measure::{AnchorGeometry, anchor_point, measure, precision_for};
use crate::part::{Part, PartIndex};
use crate::project::ViewContext;
use crate::{Drawing, DrawingError};

/// A dimension of one of the part's features.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelDimension {
    /// The feature it belongs to, such as `"plate"`.
    pub feature: String,
    /// What it is in the feature, such as `"width"` or `"depth"`.
    pub name: String,
    /// What it measures.
    pub measure: ModelMeasure,
    /// How many identical features it stands for, such as 4 for a pattern of holes.
    #[serde(default = "one")]
    pub count: u32,
}

fn one() -> u32 {
    1
}

/// What a model dimension measures, in model space.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelMeasure {
    /// The distance along `axis` between the planes normal to it through `from` and `to`.
    Distance {
        /// Unit direction measured along.
        axis: Vec3,
        /// A point on the first plane.
        from: Vec3,
        /// A point on the second plane.
        to: Vec3,
        /// The normal of the sketch the dimension belongs to, if any: a view looking
        /// along it shows the sketch true shape and is preferred.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sketch_normal: Option<Vec3>,
    },
    /// The diameter of a circle.
    Diameter {
        /// A point on the circle's axis.
        center: Vec3,
        /// The circle's axis.
        normal: Vec3,
        /// Its radius.
        radius: f64,
    },
}

/// What [`import`] did.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ImportReport {
    /// Dimensions placed.
    pub placed: usize,
    /// Duplicates left out.
    pub duplicates: usize,
    /// Dimensions no view could carry, with the reason, as "feature width: reason".
    pub skipped: Vec<String>,
}

/// Lengths below this are zero, relative to the part's size.
const GEOM_TOL: f64 = 1e-6;

/// First dimension line from the view's outline, then the step between stacked ones, in
/// sheet millimetres (ISO 129 asks for at least 10 and 7).
const FIRST_OFFSET: f64 = 10.0;
const STACK_STEP: f64 = 8.0;

/// One planned dimension before it is placed.
struct Plan {
    view: ViewId,
    kind: DimensionKind,
    text: String,
    anchors: Vec<(EdgeId, Attach, Point2)>,
    value: f64,
    /// For linear dimensions: side (+1 or -1 along the extension direction) and the span.
    side: f64,
    span: f64,
    centre: Point2,
    radius: f64,
}

/// Brings `part.dimensions` into the views of sheet `sheet`, once each.
pub fn import(
    drawing: &mut Drawing,
    sheet: usize,
    part: &Part,
    index: &PartIndex,
    views: &[ViewContext],
) -> Result<ImportReport, DrawingError> {
    let mut report = ImportReport::default();
    let size = index_size(index);
    let tol = GEOM_TOL * size;
    let mut seen: HashSet<String> = HashSet::new();
    let template = drawing.sheets.get(sheet).map(|s| s.template.as_str()).unwrap_or("");
    let times = crate::library::template(template).style_file().symbols.times.clone();
    let mut plans = Vec::new();
    for d in &part.dimensions {
        if !seen.insert(dedupe_key(&d.measure)) {
            report.duplicates += 1;
            continue;
        }
        let candidates: Vec<&ViewContext> = views.iter().filter(|v| !v.pictorial).collect();
        let plan = match &d.measure {
            ModelMeasure::Distance { axis, from, to, sketch_normal } => {
                plan_distance(index, &candidates, *axis, *from, *to, *sketch_normal, tol)
            }
            ModelMeasure::Diameter { center, normal, radius } => {
                plan_diameter(index, &candidates, *center, *normal, *radius, tol)
            }
        };
        match plan {
            Ok(mut p) => {
                if d.count > 1 {
                    p.text = format!("{}{} {}", d.count, times, Dimension::VALUE_TEXT);
                }
                plans.push(p);
            }
            Err(why) => report.skipped.push(format!("{} {}: {why}", d.feature, d.name)),
        }
    }
    report.placed = plans.len();
    place(drawing, sheet, index, plans);
    Ok(report)
}

/// Brings `part.dimensions` into the views of sheet `sheet`, projecting each view
/// through the kernel first.
pub fn import_part(drawing: &mut Drawing, sheet: usize, part: &Part) -> Result<ImportReport, DrawingError> {
    let index = PartIndex::build(part)?;
    let ids: Vec<_> = drawing.sheets.get(sheet).map(|s| s.views.iter().map(|v| v.id).collect()).unwrap_or_default();
    let contexts =
        ids.into_iter().map(|id| ViewContext::compute(drawing, part, &index, id)).collect::<Result<Vec<_>, _>>()?;
    import(drawing, sheet, part, &index, &contexts)
}

fn index_size(index: &PartIndex) -> f64 {
    let mut lo = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut hi = Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for (_, c) in index.edges() {
        for p in [c.start(), c.end(), c.point_at(0.5)] {
            lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
            hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
        }
    }
    (hi - lo).length().max(1.0)
}

fn dedupe_key(m: &ModelMeasure) -> String {
    let r = |v: f64| format!("{:.4}", v + 0.0);
    match m {
        ModelMeasure::Distance { axis, from, to, .. } => {
            let a = canonical_axis(*axis);
            let (s, e) = (from.dot(a), to.dot(a));
            format!("d {} {} {} {} {}", r(a.x), r(a.y), r(a.z), r(s.min(e)), r(s.max(e)))
        }
        ModelMeasure::Diameter { center, normal, radius } => {
            let n = canonical_axis(*normal);
            // The centre's position across the axis identifies the circle's axis line.
            let c = *center - n * center.dot(n);
            format!("o {} {} {} {} {} {} {}", r(n.x), r(n.y), r(n.z), r(c.x), r(c.y), r(c.z), r(*radius))
        }
    }
}

/// The axis with its largest component positive, so opposite directions compare equal.
fn canonical_axis(a: Vec3) -> Vec3 {
    let a = a.normalized().unwrap_or(Vec3::Z);
    let big = [a.x, a.y, a.z].into_iter().max_by(|p, q| p.abs().total_cmp(&q.abs())).unwrap_or(1.0);
    if big < 0.0 { -a } else { a }
}

/// True when the view shows the curve of `edge` as visible line work.
fn visible_in(view: &ViewContext, edge: EdgeId) -> bool {
    view.curves.curves.iter().any(|c| c.visible && c.source == CurveSource::Edge(edge))
}

/// The best edge of the body lying in the plane through `p` normal to `axis` and not
/// seen end on in `view`: visible first, then the longest in the view, then the nearest
/// the eye.
fn plane_edge(index: &PartIndex, view: &ViewContext, p: Vec3, axis: Vec3, tol: f64) -> Option<(EdgeId, bool)> {
    let mut best: Option<((bool, f64, f64), EdgeId)> = None;
    for (id, curve) in index.edges() {
        let samples = curve.sample(0.0);
        if !samples.iter().all(|q| (*q - p).dot(axis).abs() <= tol.max(1e-7)) {
            continue;
        }
        let (mut lo, mut hi) =
            (Point2::new(f64::INFINITY, f64::INFINITY), Point2::new(f64::NEG_INFINITY, f64::NEG_INFINITY));
        for q in &samples {
            let s = view.xf.vector(*q);
            lo = Point2::new(lo.u.min(s.u), lo.v.min(s.v));
            hi = Point2::new(hi.u.max(s.u), hi.v.max(s.v));
        }
        let extent = (hi.u - lo.u).hypot(hi.v - lo.v);
        if extent <= tol * view.xf.scale {
            continue;
        }
        let depth = view.xf.frame.depth(curve.point_at(0.5));
        let key = (visible_in(view, id), extent, depth);
        let better = match &best {
            None => true,
            Some((k, _)) if key.0 != k.0 => key.0,
            Some((k, _)) if (key.1 - k.1).abs() > tol => key.1 > k.1,
            Some((k, _)) => key.2 > k.2 + tol,
        };
        if better {
            best = Some((key, id));
        }
    }
    best.map(|(k, id)| (id, k.0))
}

#[allow(clippy::too_many_arguments)]
fn plan_distance(
    index: &PartIndex,
    views: &[&ViewContext],
    axis: Vec3,
    from: Vec3,
    to: Vec3,
    sketch_normal: Option<Vec3>,
    tol: f64,
) -> Result<Plan, String> {
    let axis = axis.normalized().ok_or("zero axis")?;
    let mut best: Option<(i32, Plan)> = None;
    for (order, v) in views.iter().enumerate() {
        let f = &v.xf.frame;
        if axis.dot(f.direction).abs() > 1e-6 {
            continue;
        }
        let orientation = if axis.dot(f.right).abs() > 1.0 - 1e-6 {
            LinearOrientation::Horizontal
        } else if axis.dot(f.up).abs() > 1.0 - 1e-6 {
            LinearOrientation::Vertical
        } else {
            continue;
        };
        let (Some((a, va)), Some((b, vb))) =
            (plane_edge(index, v, from, axis, tol), plane_edge(index, v, to, axis, tol))
        else {
            continue;
        };
        if a == b {
            continue;
        }
        let kind = DimensionKind::Linear { orientation };
        let across = match orientation {
            LinearOrientation::Horizontal => Point2::new(0.0, 1.0),
            _ => Point2::new(1.0, 0.0),
        };
        // The side of the view the anchors lean to, along the extension direction.
        let (ca, cb) = (index.edge_curve(a).expect("indexed"), index.edge_curve(b).expect("indexed"));
        let lean = |c: &Curve3| {
            let at = match c {
                Curve3::Arc { center, .. } | Curve3::Circle { center, .. } => *center,
                _ => c.point_at(0.5),
            };
            let q = v.xf.point(at);
            q.u * across.u + q.v * across.v
        };
        let side = if lean(ca) + lean(cb) >= 0.0 { 1.0 } else { -1.0 };
        let attach = |c: &Curve3| match c {
            Curve3::Line { .. } => {
                let (s, e) = (v.xf.point(c.start()), v.xf.point(c.end()));
                let along = |q: Point2| (q.u * across.u + q.v * across.v) * side;
                if along(e) > along(s) { Attach::End } else { Attach::Start }
            }
            _ => {
                if side > 0.0 {
                    Attach::Max
                } else {
                    Attach::Min
                }
            }
        };
        let (aa, ab) = (attach(ca), attach(cb));
        let geo = [
            AnchorGeometry { curve: Some(ca), point: Vec3::ZERO, attach: aa },
            AnchorGeometry { curve: Some(cb), point: Vec3::ZERO, attach: ab },
        ];
        let Some(value) = measure(&kind, crate::annotation::DimType::Projected, &geo, &v.xf) else {
            continue;
        };
        let pa = v.xf.point(anchor_point(&geo[0], &v.xf, across));
        let pb = v.xf.point(anchor_point(&geo[1], &v.xf, across));
        let true_shape =
            sketch_normal.is_some_and(|n| n.normalized().is_some_and(|n| n.dot(f.direction).abs() > 1.0 - 1e-6));
        let score = 4 * i32::from(true_shape) + i32::from(va) + i32::from(vb);
        let plan = Plan {
            view: v.id,
            kind,
            text: Dimension::VALUE_TEXT.to_owned(),
            anchors: vec![(a, aa, pa), (b, ab, pb)],
            value,
            side,
            span: value * v.xf.scale,
            centre: Point2::default(),
            radius: 0.0,
        };
        // Earlier views win ties: the base view comes first.
        let score = score * 100 - order as i32;
        if best.as_ref().is_none_or(|(s, _)| score > *s) {
            best = Some((score, plan));
        }
    }
    best.map(|(_, p)| p).ok_or_else(|| "no view shows both planes as edges".to_owned())
}

fn plan_diameter(
    index: &PartIndex,
    views: &[&ViewContext],
    center: Vec3,
    normal: Vec3,
    radius: f64,
    tol: f64,
) -> Result<Plan, String> {
    let n = normal.normalized().ok_or("zero normal")?;
    let mut best: Option<((bool, i32, f64), Plan)> = None;
    for (order, v) in views.iter().enumerate() {
        if n.dot(v.xf.frame.direction).abs() < 1.0 - 1e-6 {
            continue;
        }
        for (id, curve) in index.edges() {
            let (c, cn, r) = match curve {
                Curve3::Circle { center, normal, radius } | Curve3::Arc { center, normal, radius, .. } => {
                    (*center, *normal, *radius)
                }
                _ => continue,
            };
            let off_axis = (c - center) - n * (c - center).dot(n);
            if (r - radius).abs() > tol.max(1e-7) || cn.cross(n).length() > 1e-6 || off_axis.length() > tol.max(1e-7) {
                continue;
            }
            // Visible first, then the earlier view, then the circle nearest the eye.
            let key = (visible_in(v, id), -(order as i32), v.xf.frame.depth(c));
            let better = match &best {
                None => true,
                Some((k, _)) if (k.0, k.1) != (key.0, key.1) => (key.0, key.1) > (k.0, k.1),
                Some((k, _)) => key.2 > k.2 + tol,
            };
            if !better {
                continue;
            }
            let at = v.xf.point(c);
            let plan = Plan {
                view: v.id,
                kind: DimensionKind::Diameter,
                text: Dimension::VALUE_TEXT.to_owned(),
                anchors: vec![(id, Attach::Centre, at)],
                value: 2.0 * r,
                side: 1.0,
                span: 0.0,
                centre: at,
                radius: r * v.xf.scale,
            };
            best = Some((key, plan));
        }
    }
    best.map(|(_, p)| p).ok_or_else(|| "no view shows the circle true shape".to_owned())
}

/// Places the planned dimensions: linear ones stacked outside their view, shorter spans
/// nearer the outline; diameters on a diagonal leader away from the view's centre, with
/// concentric circles taking different diagonals.
fn place(drawing: &mut Drawing, sheet: usize, index: &PartIndex, plans: Vec<Plan>) {
    let mut stacks: BTreeMap<(ViewId, u8, i8), Vec<usize>> = BTreeMap::new();
    for (i, p) in plans.iter().enumerate() {
        if let DimensionKind::Linear { orientation } = p.kind {
            let o = u8::from(orientation == LinearOrientation::Vertical);
            stacks.entry((p.view, o, p.side as i8)).or_default().push(i);
        }
    }
    let mut placement: Vec<Point2> = vec![Point2::default(); plans.len()];
    for ((view, vertical, side), mut members) in stacks {
        let Some(bbox) = drawing.view(view).map(|v| v.geometry.bbox) else { continue };
        members.sort_by(|a, b| plans[*a].span.total_cmp(&plans[*b].span));
        for (k, i) in members.into_iter().enumerate() {
            let offset = FIRST_OFFSET + STACK_STEP * k as f64;
            let p = &plans[i];
            let (a, b) = (p.anchors[0].2, p.anchors[1].2);
            let side = f64::from(side);
            placement[i] = if vertical == 1 {
                let edge = if side > 0.0 { bbox[2] } else { bbox[0] };
                Point2::new(crate::part::round(edge + side * offset), crate::part::round((a.v + b.v) / 2.0))
            } else {
                let edge = if side > 0.0 { bbox[3] } else { bbox[1] };
                Point2::new(crate::part::round((a.u + b.u) / 2.0), crate::part::round(edge + side * offset))
            };
        }
    }
    let mut used: BTreeMap<(ViewId, i64, i64), Vec<(f64, f64)>> = BTreeMap::new();
    for (i, p) in plans.iter().enumerate() {
        if p.kind != DimensionKind::Diameter {
            continue;
        }
        let key = (p.view, (p.centre.u * 1e3).round() as i64, (p.centre.v * 1e3).round() as i64);
        let taken = used.entry(key).or_default();
        let outward = (if p.centre.u >= 0.0 { 1.0 } else { -1.0 }, if p.centre.v >= 0.0 { 1.0 } else { -1.0 });
        let order = [outward, (outward.0, -outward.1), (-outward.0, outward.1), (-outward.0, -outward.1)];
        let dir = order.into_iter().find(|d| !taken.contains(d)).unwrap_or(outward);
        taken.push(dir);
        let reach = p.radius + 8.0;
        let s = std::f64::consts::FRAC_1_SQRT_2;
        placement[i] = Point2::new(
            crate::part::round(p.centre.u + dir.0 * reach * s),
            crate::part::round(p.centre.v + dir.1 * reach * s),
        );
    }
    let mut next = drawing.next_annotation_id();
    for (p, at) in plans.into_iter().zip(placement) {
        let mut dimension = Dimension::measured(p.kind, round_value(p.value), precision_for(p.value));
        dimension.text = p.text;
        let anchors = p
            .anchors
            .iter()
            .filter_map(|(id, attach, cached)| {
                Some(Anchor { entity: index.edge_ref(*id)?, attach: *attach, cached: *cached })
            })
            .collect();
        drawing.sheets[sheet].annotations.push(Annotation {
            id: next,
            view: p.view,
            body: AnnotationKind::Dimension(dimension),
            anchors,
            placement: at,
            style: StyleOverrides::default(),
            state: AnnotationState::Ok,
        });
        next = crate::ids::AnnotationId(next.0 + 1);
    }
}

/// Measured values to a nanometre, so files do not carry round-off.
pub(crate) fn round_value(v: f64) -> f64 {
    let r = (v * 1e6).round() / 1e6;
    if r == 0.0 { 0.0 } else { r }
}
