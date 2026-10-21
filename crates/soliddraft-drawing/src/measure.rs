//! Dimension measurement: values come from the exact 3D curves of the referenced
//! entities, projected into the view for projected dimensions, so a diameter is read
//! from the circle, never from line work.

use soliddraft_kernel_api::drawing::Curve3;
use soliddraft_kernel_api::{Point2, Vec3};

use kurbo::{BezPath, ParamCurve, PathEl, Shape};

use crate::annotation::{
    Anchor, Attach, DimType, DimValue, Dimension, DimensionKind, DimensionSet, LinearOrientation, SetKind,
};
use crate::ids::ViewId;
use crate::project::ViewTransform;
use crate::view::View;

/// One anchor's geometry for measuring: the entity's exact curve (or, for a face, its
/// fingerprint point) and where on it the anchor attaches.
#[derive(Clone, Debug)]
pub struct AnchorGeometry<'a> {
    /// The edge's curve, or `None` for a face or vertex.
    pub curve: Option<&'a Curve3>,
    /// A point on the entity when there is no curve.
    pub point: Vec3,
    /// Where it attaches.
    pub attach: Attach,
}

/// The model point an anchor stands for. `across` is the view direction an extension
/// line runs in; [`Attach::Min`] and [`Attach::Max`] pick the extreme points of a curve
/// along it.
pub fn anchor_point(a: &AnchorGeometry, xf: &ViewTransform, across: Point2) -> Vec3 {
    let Some(curve) = a.curve else {
        return match a.attach {
            Attach::Point { at } => at,
            _ => a.point,
        };
    };
    match a.attach {
        Attach::Start => curve.start(),
        Attach::End => curve.end(),
        Attach::Mid | Attach::Silhouette => curve.point_at(0.5),
        Attach::Centre => match curve {
            Curve3::Arc { center, .. } | Curve3::Circle { center, .. } => *center,
            _ => curve.point_at(0.5),
        },
        Attach::Min | Attach::Max => {
            let key = |p: &Vec3| {
                let q = xf.vector(*p);
                q.u * across.u + q.v * across.v
            };
            let samples = curve.sample(0.0);
            let pick = if matches!(a.attach, Attach::Max) {
                samples.into_iter().max_by(|p, q| key(p).total_cmp(&key(q)))
            } else {
                samples.into_iter().min_by(|p, q| key(p).total_cmp(&key(q)))
            };
            pick.unwrap_or_else(|| curve.point_at(0.5))
        }
        Attach::Point { at } => at,
    }
}

/// The direction extension lines of a dimension run in, in the view: across the
/// measuring direction.
pub fn extension_direction(kind: &DimensionKind, a: Point2, b: Point2) -> Point2 {
    match kind {
        DimensionKind::Linear { orientation: LinearOrientation::Horizontal } => Point2::new(0.0, 1.0),
        DimensionKind::Linear { orientation: LinearOrientation::Vertical } => Point2::new(1.0, 0.0),
        _ => {
            let (dx, dy) = (b.u - a.u, b.v - a.v);
            let len = dx.hypot(dy);
            if len > 0.0 { Point2::new(-dy / len, dx / len) } else { Point2::new(0.0, 1.0) }
        }
    }
}

/// Measures a dimension of kind `kind` and type `dim_type` from its anchors in a view
/// with transform `xf`. `None` when the anchors do not fit the kind, such as a diameter
/// on a line.
pub fn measure(kind: &DimensionKind, dim_type: DimType, anchors: &[AnchorGeometry], xf: &ViewTransform) -> Option<f64> {
    match kind {
        DimensionKind::Linear { orientation } => {
            let (p, q) = match anchors {
                [one] => {
                    let c = one.curve?;
                    (c.start(), c.end())
                }
                [a, b, ..] => {
                    let across = extension_direction(kind, Point2::default(), Point2::default());
                    (anchor_point(a, xf, across), anchor_point(b, xf, across))
                }
                [] => return None,
            };
            let d = q - p;
            let frame = &xf.frame;
            Some(match (orientation, dim_type) {
                (LinearOrientation::Horizontal, _) => d.dot(frame.right).abs(),
                (LinearOrientation::Vertical, _) => d.dot(frame.up).abs(),
                (LinearOrientation::Aligned, DimType::Projected) => d.dot(frame.right).hypot(d.dot(frame.up)),
                (LinearOrientation::Aligned, DimType::True) => d.length(),
            })
        }
        DimensionKind::Diameter | DimensionKind::Radius => {
            let radius = match anchors.first()?.curve? {
                Curve3::Arc { radius, .. } | Curve3::Circle { radius, .. } => *radius,
                _ => return None,
            };
            Some(if matches!(kind, DimensionKind::Diameter) { 2.0 * radius } else { radius })
        }
        DimensionKind::Angular => {
            let dir = |a: &AnchorGeometry| {
                let c = a.curve?;
                let v = xf.vector(c.end() - c.start());
                let len = v.u.hypot(v.v);
                (len > 0.0).then(|| Point2::new(v.u / len, v.v / len))
            };
            let (a, b) = (dir(anchors.first()?)?, dir(anchors.get(1)?)?);
            let cos = (a.u * b.u + a.v * b.v).clamp(-1.0, 1.0);
            Some(cos.acos().to_degrees())
        }
        DimensionKind::Ordinate | DimensionKind::Chamfer => None,
    }
}

/// The distances of a dimension set from its anchors' geometry, one per anchor after the
/// first, along the set's direction in the view: from anchor 0 for ordinate and baseline
/// sets, from the previous anchor for a chain. `None` with fewer than two anchors.
pub fn measure_set(set: &DimensionSet, anchors: &[AnchorGeometry], xf: &ViewTransform) -> Option<Vec<f64>> {
    if anchors.len() < 2 {
        return None;
    }
    let (axis, across) = match set.orientation {
        LinearOrientation::Vertical => (xf.frame.up, Point2::new(1.0, 0.0)),
        _ => (xf.frame.right, Point2::new(0.0, 1.0)),
    };
    let at: Vec<f64> = anchors.iter().map(|g| anchor_point(g, xf, across).dot(axis)).collect();
    Some(match set.kind {
        SetKind::Chain => at.windows(2).map(|w| (w[1] - w[0]).abs()).collect(),
        SetKind::Ordinate | SetKind::Baseline => at[1..].iter().map(|c| (c - at[0]).abs()).collect(),
    })
}

/// A value at a precision, without a negative zero: `format_value(10.0, 0)` is "10".
pub fn format_value(value: f64, precision: u8) -> String {
    let s = format!("{value:.*}", usize::from(precision));
    if s.trim_start_matches('-').chars().all(|c| c == '0' || c == '.') {
        s.trim_start_matches('-').to_owned()
    } else {
        s
    }
}

/// The displayed value of a dimension, without prefix or tolerance: what "changed"
/// compares.
pub fn value_text(d: &Dimension) -> String {
    match &d.value {
        DimValue::Measured { value } => format_value(*value, d.precision),
        DimValue::Driving { parameter } => format!("{}.{}", parameter.feature, parameter.name),
    }
}

/// The full text of a dimension in the ISO style: see [`display_text`].
pub fn dimension_text(d: &Dimension) -> String {
    display_text(d, &crate::library::style("iso").symbols)
}

/// Decimal places that show `value` exactly when it is a whole number or has one or two
/// decimals, otherwise two.
pub fn precision_for(value: f64) -> u8 {
    for p in 0..2u8 {
        let k = 10f64.powi(i32::from(p));
        if ((value * k).round() - value * k).abs() < 1e-6 {
            return p;
        }
    }
    2
}

/// What a view curve is, geometrically, in view millimetres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CurveShape {
    /// A straight segment.
    Line {
        /// Start.
        a: Point2,
        /// End.
        b: Point2,
    },
    /// A full circle.
    Circle {
        /// Centre.
        centre: Point2,
        /// Radius.
        radius: f64,
    },
    /// A circular arc.
    Arc {
        /// Centre.
        centre: Point2,
        /// Radius.
        radius: f64,
        /// Start.
        a: Point2,
        /// End.
        b: Point2,
    },
    /// Anything else, by its ends.
    Other {
        /// Start.
        a: Point2,
        /// End.
        b: Point2,
    },
}

impl CurveShape {
    /// Where the curve starts.
    pub fn start(&self) -> Point2 {
        match *self {
            CurveShape::Line { a, .. } | CurveShape::Arc { a, .. } | CurveShape::Other { a, .. } => a,
            CurveShape::Circle { centre, radius } => Point2::new(centre.u + radius, centre.v),
        }
    }

    /// Where the curve ends.
    pub fn end(&self) -> Point2 {
        match *self {
            CurveShape::Line { b, .. } | CurveShape::Arc { b, .. } | CurveShape::Other { b, .. } => b,
            CurveShape::Circle { centre, radius } => Point2::new(centre.u + radius, centre.v),
        }
    }

    /// The middle of the curve, or a round's centre.
    pub fn mid(&self) -> Point2 {
        let (a, b) = (self.start(), self.end());
        match *self {
            CurveShape::Circle { centre, .. } => centre,
            _ => Point2::new((a.u + b.u) / 2.0, (a.v + b.v) / 2.0),
        }
    }

    /// True for circles and arcs.
    pub fn is_round(&self) -> bool {
        matches!(self, CurveShape::Circle { .. } | CurveShape::Arc { .. })
    }
}

fn pt(p: kurbo::Point) -> Point2 {
    Point2::new(p.x, p.y)
}

/// Classifies a path: a single straight segment is a line, a closed path of curves a
/// circle, an open path of curves whose points lie at one distance from a centre an arc.
pub fn shape_of(path: &BezPath) -> CurveShape {
    let els = path.elements();
    let first = match els.first() {
        Some(PathEl::MoveTo(p)) => *p,
        _ => return CurveShape::Other { a: Point2::new(0.0, 0.0), b: Point2::new(0.0, 0.0) },
    };
    if els.len() == 2
        && let PathEl::LineTo(b) = els[1]
    {
        return CurveShape::Line { a: pt(first), b: pt(b) };
    }
    let closed = els.iter().any(|e| matches!(e, PathEl::ClosePath));
    let curved = els.iter().any(|e| matches!(e, PathEl::CurveTo(..) | PathEl::QuadTo(..)));
    let bbox = path.bounding_box();
    let centre = bbox.center();
    let mut last = first;
    for e in els {
        match e {
            PathEl::LineTo(p) | PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => last = *p,
            _ => {}
        }
    }
    if curved && closed && (bbox.width() - bbox.height()).abs() < 1e-3 * bbox.width().max(1.0) {
        return CurveShape::Circle { centre: pt(centre), radius: bbox.width() / 2.0 };
    }
    if curved && !closed {
        // An arc: fit a centre from three points.
        let samples: Vec<kurbo::Point> =
            path.segments().flat_map(|s| [s.eval(0.0), s.eval(0.5), s.eval(1.0)]).collect();
        if samples.len() >= 3
            && let Some(c) = circumcentre(samples[0], samples[samples.len() / 2], samples[samples.len() - 1])
        {
            let r = c.distance(samples[0]);
            if samples.iter().all(|p| (c.distance(*p) - r).abs() < 1e-3 * r.max(1.0)) {
                return CurveShape::Arc { centre: pt(c), radius: r, a: pt(first), b: pt(last) };
            }
        }
    }
    if !curved && els.len() > 2 && !closed {
        // A sampled curve: treat as an arc when it is one, otherwise by its ends.
        let pts: Vec<kurbo::Point> = els
            .iter()
            .filter_map(|e| match e {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => Some(*p),
                _ => None,
            })
            .collect();
        if pts.len() >= 3
            && let Some(c) = circumcentre(pts[0], pts[pts.len() / 2], pts[pts.len() - 1])
        {
            let r = c.distance(pts[0]);
            if r.is_finite() && pts.iter().all(|p| (c.distance(*p) - r).abs() < 1e-2 * r.max(1.0)) {
                return CurveShape::Arc { centre: pt(c), radius: r, a: pt(first), b: pt(last) };
            }
        }
    }
    CurveShape::Other { a: pt(first), b: pt(last) }
}

fn circumcentre(a: kurbo::Point, b: kurbo::Point, c: kurbo::Point) -> Option<kurbo::Point> {
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    if d.abs() < 1e-9 {
        return None;
    }
    let (a2, b2, c2) = (a.x * a.x + a.y * a.y, b.x * b.x + b.y * b.y, c.x * c.x + c.y * c.y);
    let ux = (a2 * (b.y - c.y) + b2 * (c.y - a.y) + c2 * (a.y - b.y)) / d;
    let uy = (a2 * (c.x - b.x) + b2 * (a.x - c.x) + c2 * (b.x - a.x)) / d;
    Some(kurbo::Point::new(ux, uy))
}

/// One Smart dimension pick: a curve of a view, or one of its ends.
#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    /// The view.
    pub view: ViewId,
    /// Index of the curve in the view's geometry.
    pub curve: usize,
    /// `Some` when the click landed on an end of the curve: which one.
    pub end: Option<Attach>,
}

/// What Smart dimension proposes from its picks, before placement decides the
/// orientation.
#[derive(Clone, Debug, PartialEq)]
pub struct Proposal {
    /// The view.
    pub view: ViewId,
    /// What it would measure.
    pub kind: DimensionKind,
    /// Its anchors so far.
    pub anchors: Vec<Anchor>,
    /// For a linear proposal between two points: the two points in view millimetres.
    pub points: Vec<Point2>,
    /// Radius in view millimetres for diameter and radius dimensions.
    pub radius: f64,
    /// Angle in degrees for angular dimensions.
    pub angle: f64,
    /// Centre of the round for diameter and radius dimensions.
    pub centre: Point2,
    /// Whether the proposal is complete or wants another pick.
    pub complete: bool,
}

fn anchor(curve: &crate::Curve2, attach: Attach, at: Point2) -> Anchor {
    Anchor { entity: curve.source.clone(), attach, cached: at }
}

/// Reads one or two picks the way SolidWorks does: a line gives its length, a circle a
/// diameter, an arc a radius, two parallel lines a distance, two other lines an angle,
/// a round and a line the distance from the centre, two ends a point-to-point distance.
pub fn propose(view: &View, picks: &[Pick]) -> Option<Proposal> {
    let curve = |i: usize| view.geometry.curves.get(picks.get(i)?.curve);
    let first = curve(0)?;
    let s1 = shape_of(&first.path);
    let base = Proposal {
        view: view.id,
        kind: DimensionKind::Linear { orientation: LinearOrientation::Horizontal },
        anchors: Vec::new(),
        points: Vec::new(),
        radius: 0.0,
        angle: 0.0,
        centre: Point2::new(0.0, 0.0),
        complete: true,
    };
    if picks.len() == 1 {
        let p = &picks[0];
        return Some(match (s1, p.end) {
            (CurveShape::Circle { centre, radius }, _) => Proposal {
                kind: DimensionKind::Diameter,
                anchors: vec![anchor(first, Attach::Centre, centre)],
                radius,
                centre,
                ..base
            },
            (CurveShape::Arc { centre, radius, .. }, None) => Proposal {
                kind: DimensionKind::Radius,
                anchors: vec![anchor(first, Attach::Centre, centre)],
                radius,
                centre,
                ..base
            },
            (CurveShape::Line { a, b }, None) => Proposal {
                anchors: vec![anchor(first, Attach::Start, a), anchor(first, Attach::End, b)],
                points: vec![a, b],
                ..base
            },
            (shape, Some(end)) => {
                let at = if end == Attach::Start { shape.start() } else { shape.end() };
                Proposal { anchors: vec![anchor(first, end, at)], points: vec![at], complete: false, ..base }
            }
            (CurveShape::Other { a, b }, None) => Proposal {
                anchors: vec![anchor(first, Attach::Start, a), anchor(first, Attach::End, b)],
                points: vec![a, b],
                ..base
            },
        });
    }
    let second = curve(1)?;
    let s2 = shape_of(&second.path);
    let (p1, p2) = (&picks[0], &picks[1]);
    // Two ends, or an end and a curve: point to point or point to the curve's reference.
    let point_of = |pick: &Pick, shape: CurveShape| -> (Attach, Point2) {
        match pick.end {
            Some(Attach::Start) => (Attach::Start, shape.start()),
            Some(_) => (Attach::End, shape.end()),
            None => match shape {
                CurveShape::Circle { centre, .. } | CurveShape::Arc { centre, .. } => (Attach::Centre, centre),
                _ => (Attach::Mid, shape.mid()),
            },
        }
    };
    if let (CurveShape::Line { a: a1, b: b1 }, CurveShape::Line { a: a2, b: b2 }) = (s1, s2)
        && p1.end.is_none()
        && p2.end.is_none()
    {
        let d1 = kurbo::Vec2::new(b1.u - a1.u, b1.v - a1.v);
        let d2 = kurbo::Vec2::new(b2.u - a2.u, b2.v - a2.v);
        let cross = d1.x * d2.y - d1.y * d2.x;
        if cross.abs() < 1e-6 * d1.length() * d2.length() {
            // Parallel: the distance between them, measured across.
            let m1 = s1.mid();
            let m2 = s2.mid();
            return Some(Proposal {
                anchors: vec![anchor(first, Attach::Mid, m1), anchor(second, Attach::Mid, m2)],
                points: vec![m1, m2],
                ..base
            });
        }
        let angle = (d1.dot(d2) / (d1.length() * d2.length())).clamp(-1.0, 1.0).acos().to_degrees();
        let angle = if angle > 90.0 { 180.0 - angle } else { angle };
        return Some(Proposal {
            kind: DimensionKind::Angular,
            anchors: vec![anchor(first, Attach::Mid, s1.mid()), anchor(second, Attach::Mid, s2.mid())],
            points: vec![s1.mid(), s2.mid()],
            angle,
            ..base
        });
    }
    let (at1, q1) = point_of(p1, s1);
    let (at2, q2) = point_of(p2, s2);
    Some(Proposal { anchors: vec![anchor(first, at1, q1), anchor(second, at2, q2)], points: vec![q1, q2], ..base })
}

/// The orientation a placement gives a linear dimension between two points: above or
/// below the span measures horizontally, beside it vertically, inside it along the line.
pub fn orientation_for(points: &[Point2], placement: Point2) -> LinearOrientation {
    let [a, b] = [points[0], points[1]];
    let (du, dv) = ((b.u - a.u).abs(), (b.v - a.v).abs());
    if dv < 1e-6 {
        return LinearOrientation::Horizontal;
    }
    if du < 1e-6 {
        return LinearOrientation::Vertical;
    }
    let (u0, u1) = (a.u.min(b.u), a.u.max(b.u));
    let (v0, v1) = (a.v.min(b.v), a.v.max(b.v));
    let outside_u = placement.u < u0 || placement.u > u1;
    let outside_v = placement.v < v0 || placement.v > v1;
    match (outside_u, outside_v) {
        (true, false) => LinearOrientation::Vertical,
        (false, true) => LinearOrientation::Horizontal,
        _ => LinearOrientation::Aligned,
    }
}

/// The measured value of a linear dimension between two view points, in model units.
pub fn linear_value(orientation: LinearOrientation, a: Point2, b: Point2, scale: f64) -> f64 {
    let d = match orientation {
        LinearOrientation::Horizontal => (b.u - a.u).abs(),
        LinearOrientation::Vertical => (b.v - a.v).abs(),
        LinearOrientation::Aligned => ((b.u - a.u).powi(2) + (b.v - a.v).powi(2)).sqrt(),
    };
    d / scale
}

/// The text a dimension shows in a dimension style: `<DIM>` in its text replaced by
/// the value with the style's prefix, suffix and tolerance, in parentheses when the
/// style marks reference dimensions so.
pub fn display_text(dim: &Dimension, symbols: &crate::library::Symbols) -> String {
    let value = match &dim.value {
        crate::DimValue::Measured { value } => *value,
        crate::DimValue::Driving { .. } => 0.0,
    };
    let number = format_value(value, dim.precision);
    let prefix = match dim.kind {
        DimensionKind::Diameter => symbols.diameter.as_str(),
        DimensionKind::Radius => symbols.radius.as_str(),
        _ => "",
    };
    let suffix = match dim.kind {
        DimensionKind::Angular => symbols.degree.as_str(),
        _ => "",
    };
    let core = format!("{prefix}{number}{suffix}");
    let with_tol = match dim.tolerance {
        crate::Tolerance::None => core,
        crate::Tolerance::Symmetric { value } => {
            format!("{core} ±{}", format_value(value, dim.precision.max(1)))
        }
        crate::Tolerance::Deviation { upper, lower } => {
            format!(
                "{core} +{}/{}",
                format_value(upper, dim.precision.max(1)),
                format_value(lower, dim.precision.max(1))
            )
        }
        crate::Tolerance::Limits { upper, lower } => {
            format!(
                "{} / {}",
                format_value(value + upper, dim.precision.max(1)),
                format_value(value + lower, dim.precision.max(1))
            )
        }
    };
    let text = if dim.text.is_empty() { Dimension::VALUE_TEXT.to_owned() } else { dim.text.clone() };
    let text = text.replace(Dimension::VALUE_TEXT, &with_tol);
    if symbols.reference_parentheses && matches!(dim.value, crate::DimValue::Measured { .. }) {
        format!("({text})")
    } else {
        text
    }
}

/// The radius, in model millimetres, of a round edge.
pub fn radius_of(curve: &Curve3) -> Option<f64> {
    match curve {
        Curve3::Circle { radius, .. } | Curve3::Arc { radius, .. } => Some(*radius),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::Tolerance;
    use soliddraft_kernel_api::drawing::{StandardView, ViewSpec};

    fn xf() -> ViewTransform {
        ViewTransform {
            frame: ViewSpec::standard(StandardView::Front).frame().unwrap(),
            scale: 0.5,
            centre: Point2::new(0.0, 0.0),
        }
    }

    #[test]
    fn linear_dimensions_measure_model_lengths_along_the_view() {
        let a = Curve3::Line { a: Vec3::new(0.0, 0.0, 0.0), b: Vec3::new(0.0, 0.0, 60.0) };
        let b = Curve3::Line { a: Vec3::new(120.0, 5.0, 0.0), b: Vec3::new(120.0, 5.0, 10.0) };
        let anchors = [
            AnchorGeometry { curve: Some(&a), point: Vec3::ZERO, attach: Attach::Start },
            AnchorGeometry { curve: Some(&b), point: Vec3::ZERO, attach: Attach::End },
        ];
        let h = DimensionKind::Linear { orientation: LinearOrientation::Horizontal };
        let v = DimensionKind::Linear { orientation: LinearOrientation::Vertical };
        let aligned = DimensionKind::Linear { orientation: LinearOrientation::Aligned };
        assert_eq!(measure(&h, DimType::Projected, &anchors, &xf()), Some(120.0), "the view's scale does not matter");
        assert_eq!(measure(&v, DimType::Projected, &anchors, &xf()), Some(10.0));
        let projected = measure(&aligned, DimType::Projected, &anchors, &xf()).unwrap();
        let true_length = measure(&aligned, DimType::True, &anchors, &xf()).unwrap();
        assert!((projected - 120.0f64.hypot(10.0)).abs() < 1e-9 && true_length > projected);
        assert_eq!(measure(&v, DimType::Projected, &anchors[1..], &xf()), Some(10.0), "one edge gives its length");
    }

    #[test]
    fn diameters_come_from_the_circle() {
        let c = Curve3::Circle { center: Vec3::new(70.0, 45.0, 30.0), normal: Vec3::Z, radius: 12.0 };
        let anchors = [AnchorGeometry { curve: Some(&c), point: Vec3::ZERO, attach: Attach::Centre }];
        assert_eq!(measure(&DimensionKind::Diameter, DimType::Projected, &anchors, &xf()), Some(24.0));
        assert_eq!(measure(&DimensionKind::Radius, DimType::Projected, &anchors, &xf()), Some(12.0));
        let max = anchor_point(&anchors[0].clone(), &xf(), Point2::new(1.0, 0.0));
        let right = AnchorGeometry { attach: Attach::Max, ..anchors[0].clone() };
        assert!(anchor_point(&right, &xf(), Point2::new(1.0, 0.0)).distance(Vec3::new(82.0, 45.0, 30.0)) < 1e-6);
        assert_eq!(max, Vec3::new(70.0, 45.0, 30.0));
    }

    #[test]
    fn values_format_at_their_precision() {
        assert_eq!(format_value(10.0, 0), "10");
        assert_eq!(format_value(-0.0001, 2), "0.00");
        assert_eq!(format_value(12.345, 1), "12.3");
        assert_eq!(
            (precision_for(10.0), precision_for(4.5), precision_for(1.25), precision_for(1.0 / 3.0)),
            (0, 1, 2, 2)
        );
        let mut d = Dimension::measured(DimensionKind::Diameter, 9.0, 0);
        d.text = "4X <DIM> THRU".into();
        assert_eq!(dimension_text(&d), "4X Ø9 THRU");
        d.tolerance = Tolerance::Symmetric { value: 0.1 };
        assert_eq!(dimension_text(&d), "4X Ø9 ±0.1 THRU");
    }
}
