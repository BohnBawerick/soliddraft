//! Annotations made from model points rather than picks: what the command line and the
//! assistant use to place dimension sets and symbols without a window.
//!
//! An anchor goes on the edge nearest the point, at the edge's end, middle or centre
//! when the point is one of those, otherwise at the point itself, so it follows the
//! part through updates like a picked anchor.

use soliddraft_kernel_api::{Point2, Vec3};

use crate::annotation::{
    Anchor, Annotation, AnnotationKind, AnnotationState, Attach, DimType, DimensionSet, LinearOrientation, SetKind,
    StyleOverrides,
};
use crate::ids::ViewId;
use crate::measure::{AnchorGeometry, measure_set, precision_for};
use crate::part::{PartIndex, round};
use crate::project::{ViewContext, ViewTransform};
use crate::{Drawing, DrawingError};

/// The anchor on the edge nearest `at`, with its point in the view of `xf`.
pub fn anchor_near(index: &PartIndex, xf: &ViewTransform, at: Vec3) -> Option<Anchor> {
    let (id, curve, _) = index
        .edges()
        .map(|(id, c)| {
            let d = c.sample(0.0).windows(2).map(|w| segment_distance(at, w[0], w[1])).fold(f64::INFINITY, f64::min);
            (id, c, d)
        })
        .min_by(|a, b| a.2.total_cmp(&b.2))?;
    let entity = index.edge_ref(id)?;
    let mut candidates =
        vec![(Attach::Start, curve.start()), (Attach::End, curve.end()), (Attach::Mid, curve.point_at(0.5))];
    if let soliddraft_kernel_api::drawing::Curve3::Arc { center, .. }
    | soliddraft_kernel_api::drawing::Curve3::Circle { center, .. } = curve
    {
        candidates.push((Attach::Centre, *center));
    }
    let (attach, point) = candidates
        .into_iter()
        .min_by(|a, b| a.1.distance(at).total_cmp(&b.1.distance(at)))
        .filter(|(_, p)| p.distance(at) <= 1e-3)
        .unwrap_or((Attach::Point { at }, at));
    Some(Anchor { entity, attach, cached: xf.point(point) })
}

fn segment_distance(p: Vec3, a: Vec3, b: Vec3) -> f64 {
    let ab = b - a;
    let len2 = ab.dot(ab);
    let t = if len2 > 0.0 { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
    p.distance(a + ab * t)
}

fn new_annotation(
    drawing: &Drawing,
    view: ViewId,
    body: AnnotationKind,
    anchors: Vec<Anchor>,
    placement: Point2,
) -> Annotation {
    Annotation {
        id: drawing.next_annotation_id(),
        view,
        body,
        anchors,
        placement,
        style: StyleOverrides::default(),
        state: AnnotationState::Ok,
    }
}

/// Ordinate, baseline or chain dimensions of view `ctx` through the model points
/// `points`, the first being the origin, measured along `orientation` (horizontal or
/// vertical). The values or their dimension lines sit `offset` sheet millimetres beyond
/// the outermost point across the measuring direction: above or right for a positive
/// offset, below or left for a negative one.
pub fn dimension_set(
    drawing: &Drawing,
    index: &PartIndex,
    ctx: &ViewContext,
    kind: SetKind,
    orientation: LinearOrientation,
    points: &[Vec3],
    offset: f64,
) -> Result<Annotation, DrawingError> {
    if points.len() < 2 {
        return Err(DrawingError::BadRequest("a dimension set needs an origin and at least one point".into()));
    }
    if orientation == LinearOrientation::Aligned {
        return Err(DrawingError::BadRequest("dimension sets are horizontal or vertical".into()));
    }
    let anchors: Vec<Anchor> = points
        .iter()
        .map(|p| anchor_near(index, &ctx.xf, *p))
        .collect::<Option<_>>()
        .ok_or(DrawingError::BadRequest("the part has no edges".into()))?;
    let geometry: Vec<AnchorGeometry> = anchors
        .iter()
        .map(|a| {
            let curve = match index.resolve(&a.entity) {
                crate::Resolution::Found(crate::Entity::Edge(id)) => index.edge_curve(id),
                _ => None,
            };
            AnchorGeometry { curve, point: a.entity.fingerprint.point, attach: a.attach }
        })
        .collect();
    let mut set = DimensionSet { kind, orientation, values: Vec::new(), precision: 0, dim_type: DimType::Projected };
    let values =
        measure_set(&set, &geometry, &ctx.xf).ok_or(DrawingError::BadRequest("the points do not measure".into()))?;
    set.precision = values.iter().map(|v| precision_for(*v)).max().unwrap_or(0);
    set.values = values.into_iter().map(crate::model_items::round_value).collect();
    let across: Vec<f64> = anchors
        .iter()
        .map(|a| if orientation == LinearOrientation::Horizontal { a.cached.v } else { a.cached.u })
        .collect();
    let level = if offset >= 0.0 {
        across.iter().copied().fold(f64::NEG_INFINITY, f64::max) + offset
    } else {
        across.iter().copied().fold(f64::INFINITY, f64::min) + offset
    };
    let first = anchors[0].cached;
    let placement = if orientation == LinearOrientation::Horizontal {
        Point2::new(first.u, round(level))
    } else {
        Point2::new(round(level), first.v)
    };
    Ok(new_annotation(drawing, ctx.id, AnnotationKind::DimensionSet(set), anchors, placement))
}

/// An annotation of view `ctx` attached to the edge nearest the model point `at`, with
/// its text or symbol `offset` sheet millimetres from the anchor: a geometric tolerance,
/// a datum, a surface finish or a revision symbol.
pub fn attached(
    drawing: &Drawing,
    index: &PartIndex,
    ctx: &ViewContext,
    at: Vec3,
    body: AnnotationKind,
    offset: Point2,
) -> Result<Annotation, DrawingError> {
    let anchor = anchor_near(index, &ctx.xf, at).ok_or(DrawingError::BadRequest("the part has no edges".into()))?;
    let placement = Point2::new(round(anchor.cached.u + offset.u), round(anchor.cached.v + offset.v));
    Ok(new_annotation(drawing, ctx.id, body, vec![anchor], placement))
}
