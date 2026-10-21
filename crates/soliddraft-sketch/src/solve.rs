//! The constraint solver behind the sketch: a thin bridge from the sketch model
//! to ezpz. Everything the rest of the app knows about solving is [`solve`] and
//! the [`SolveStatus`] it leaves on the sketch, so another solver can take
//! ezpz's place behind this one function.

use crate::{
    Align, ConstraintId, ConstraintKind, DistanceKind, Entity, EntityId, EntityKind, ORIGIN, P2, PointRef, Sketch,
    SolveStatus,
};
use ezpz::datatypes::inputs::{DatumCircle, DatumCircularArc, DatumDistance, DatumLineSegment, DatumPoint};
use ezpz::datatypes::{Angle, AngleKind};
use ezpz::{CircleSide, Config, Constraint, ConstraintRequest, LineSide};
use std::collections::HashMap;
use std::ops::{Add, Sub};

/// A point the user is dragging: solved as a low-priority wish, so real
/// constraints win and the rest of the sketch follows.
#[derive(Clone, Copy, Debug)]
pub struct Drag {
    /// The point under the cursor.
    pub point: PointRef,
    /// Where the cursor is.
    pub to: P2,
}

struct Vars {
    ids: ezpz::IdGenerator,
    points: HashMap<PointRef, DatumPoint>,
    radii: HashMap<EntityId, DatumDistance>,
    guesses: Vec<(ezpz::Id, f64)>,
}

impl Vars {
    fn point(&mut self, r: PointRef, at: P2) -> DatumPoint {
        if let Some(p) = self.points.get(&r) {
            return *p;
        }
        let p = DatumPoint::new(&mut self.ids);
        self.guesses.push((p.id_x(), at.u));
        self.guesses.push((p.id_y(), at.v));
        self.points.insert(r, p);
        p
    }

    fn radius(&mut self, e: EntityId, r: f64) -> DatumDistance {
        if let Some(d) = self.radii.get(&e) {
            return *d;
        }
        let d = DatumDistance::new(self.ids.next_id());
        self.guesses.push((d.id, r));
        self.radii.insert(e, d);
        d
    }
}

/// Everything ezpz needs for one entity, built from the current geometry.
#[derive(Clone, Copy)]
enum Datum {
    Point(DatumPoint),
    Line(DatumLineSegment),
    Arc(DatumCircularArc, DatumCircle),
    Circle(DatumCircle),
}

impl Datum {
    fn line(self) -> Option<DatumLineSegment> {
        match self {
            Datum::Line(l) => Some(l),
            _ => None,
        }
    }

    fn circle(self) -> Option<DatumCircle> {
        match self {
            Datum::Arc(_, c) | Datum::Circle(c) => Some(c),
            _ => None,
        }
    }

    fn center(self) -> Option<DatumPoint> {
        self.circle().map(|c| c.center)
    }
}

/// Signed value of a dimension as ezpz measures it, so the user's positive
/// number keeps the sketch on the side it is on now.
fn signed(value: f64, current: f64) -> f64 {
    if current < 0.0 { -value.abs() } else { value.abs() }
}

fn line_equation(a: P2, b: P2) -> (f64, f64, f64) {
    let (ax, ay, bx, by) = (a.u, a.v, b.u, b.v);
    (ay - by, bx - ax, ax * by - bx * ay)
}

/// Solves the sketch in place. `analyse` also computes which entities are
/// still free, which is the expensive part; skip it while dragging.
pub fn solve(sketch: &mut Sketch, drag: Option<Drag>, analyse: bool) {
    if sketch.is_empty() {
        sketch.status = SolveStatus::Empty;
        sketch.free.clear();
        return;
    }
    let mut vars =
        Vars { ids: ezpz::IdGenerator::default(), points: HashMap::new(), radii: HashMap::new(), guesses: Vec::new() };
    let mut datums: HashMap<EntityId, Datum> = HashMap::new();
    let mut requests: Vec<ConstraintRequest> = Vec::new();
    // Which sketch constraint each request belongs to; None for internal ones.
    let mut owners: Vec<Option<ConstraintId>> = Vec::new();
    let push = |req: ConstraintRequest,
                owner: Option<ConstraintId>,
                requests: &mut Vec<ConstraintRequest>,
                owners: &mut Vec<Option<ConstraintId>>| {
        requests.push(req);
        owners.push(owner);
    };

    for e in &sketch.entities {
        let d = match e.kind {
            EntityKind::Point { p } => Datum::Point(vars.point(PointRef::new(e.id, 0), p)),
            EntityKind::Line { a, b } => Datum::Line(DatumLineSegment::new(
                vars.point(PointRef::new(e.id, 0), a),
                vars.point(PointRef::new(e.id, 1), b),
            )),
            EntityKind::Arc { center, start, end } => {
                let c = vars.point(PointRef::new(e.id, 0), center);
                let s = vars.point(PointRef::new(e.id, 1), start);
                let t = vars.point(PointRef::new(e.id, 2), end);
                let r = vars.radius(e.id, center.dist(start));
                // The arc's ends stay on its circle and the radius variable follows them.
                push(
                    ConstraintRequest::highest_priority(Constraint::Arc(DatumCircularArc {
                        center: c,
                        start: s,
                        end: t,
                    })),
                    None,
                    &mut requests,
                    &mut owners,
                );
                push(
                    ConstraintRequest::highest_priority(Constraint::DistanceVar(c, s, r)),
                    None,
                    &mut requests,
                    &mut owners,
                );
                Datum::Arc(DatumCircularArc { center: c, start: s, end: t }, DatumCircle { center: c, radius: r })
            }
            EntityKind::Circle { center, radius } => {
                let c = vars.point(PointRef::new(e.id, 0), center);
                let r = vars.radius(e.id, radius);
                Datum::Circle(DatumCircle { center: c, radius: r })
            }
        };
        datums.insert(e.id, d);
    }
    // The origin never moves.
    if let Some(Datum::Point(o)) = datums.get(&ORIGIN) {
        push(
            ConstraintRequest::highest_priority(Constraint::Fixed(o.id_x(), 0.0)).with_weight(10.0),
            None,
            &mut requests,
            &mut owners,
        );
        push(
            ConstraintRequest::highest_priority(Constraint::Fixed(o.id_y(), 0.0)).with_weight(10.0),
            None,
            &mut requests,
            &mut owners,
        );
    }

    let pt = |vars: &Vars, r: PointRef| vars.points.get(&r).copied();
    for c in &sketch.constraints {
        let mut reqs: Vec<Constraint> = Vec::new();
        match c.kind {
            ConstraintKind::Coincident { a, b } => {
                if let (Some(p), Some(q)) = (pt(&vars, a), pt(&vars, b)) {
                    reqs.push(Constraint::PointsCoincident(p, q));
                }
            }
            ConstraintKind::PointOn { point, entity } => {
                if let (Some(p), Some(d)) = (pt(&vars, point), datums.get(&entity)) {
                    match d {
                        Datum::Line(l) => reqs.push(Constraint::PointLineDistance(p, *l, 0.0)),
                        Datum::Arc(_, circle) | Datum::Circle(circle) => {
                            reqs.push(Constraint::DistanceVar(circle.center, p, circle.radius))
                        }
                        Datum::Point(q) => reqs.push(Constraint::PointsCoincident(p, *q)),
                    }
                }
            }
            ConstraintKind::Horizontal { target } | ConstraintKind::Vertical { target } => {
                let seg = match target {
                    Align::Line(l) => datums.get(&l).and_then(|d| d.line()),
                    Align::Points(a, b) => match (pt(&vars, a), pt(&vars, b)) {
                        (Some(p), Some(q)) => Some(DatumLineSegment::new(p, q)),
                        _ => None,
                    },
                };
                if let Some(seg) = seg {
                    reqs.push(if matches!(c.kind, ConstraintKind::Horizontal { .. }) {
                        Constraint::Horizontal(seg)
                    } else {
                        Constraint::Vertical(seg)
                    });
                }
            }
            ConstraintKind::Parallel { a, b } | ConstraintKind::Perpendicular { a, b } => {
                if let (Some(l0), Some(l1)) =
                    (datums.get(&a).and_then(|d| d.line()), datums.get(&b).and_then(|d| d.line()))
                {
                    let kind = if matches!(c.kind, ConstraintKind::Parallel { .. }) {
                        AngleKind::Parallel
                    } else {
                        AngleKind::Perpendicular
                    };
                    reqs.push(Constraint::LinesAtAngle(l0, l1, kind));
                }
            }
            ConstraintKind::Tangent { a, b } => {
                if let Some(r) = tangent(sketch, &datums, a, b) {
                    reqs.push(r);
                }
            }
            ConstraintKind::Equal { a, b } => match (datums.get(&a), datums.get(&b)) {
                (Some(Datum::Line(l0)), Some(Datum::Line(l1))) => reqs.push(Constraint::LinesEqualLength(*l0, *l1)),
                (Some(x), Some(y)) => {
                    if let (Some(c0), Some(c1)) = (x.circle(), y.circle()) {
                        reqs.push(Constraint::ScalarEqual(c0.radius.id, c1.radius.id));
                    }
                }
                _ => {}
            },
            ConstraintKind::Midpoint { point, line } => {
                if let (Some(p), Some(l)) = (pt(&vars, point), datums.get(&line).and_then(|d| d.line())) {
                    reqs.push(Constraint::Midpoint(l, p));
                }
            }
            ConstraintKind::Concentric { a, b } => {
                if let (Some(p), Some(q)) =
                    (datums.get(&a).and_then(|d| d.center()), datums.get(&b).and_then(|d| d.center()))
                {
                    reqs.push(Constraint::PointsCoincident(p, q));
                }
            }
            ConstraintKind::Fix { point, at } => {
                if let Some(p) = pt(&vars, point) {
                    reqs.push(Constraint::Fixed(p.id_x(), at.u));
                    reqs.push(Constraint::Fixed(p.id_y(), at.v));
                }
            }
            ConstraintKind::Distance { a, b, kind, value, .. } => {
                if let (Some(p), Some(q), Some(pa), Some(pb)) =
                    (pt(&vars, a), pt(&vars, b), sketch.point(a), sketch.point(b))
                {
                    reqs.push(match kind {
                        DistanceKind::Aligned => Constraint::Distance(p, q, value.abs()),
                        DistanceKind::Horizontal => Constraint::HorizontalDistance(p, q, signed(value, pa.u - pb.u)),
                        DistanceKind::Vertical => Constraint::VerticalDistance(p, q, signed(value, pa.v - pb.v)),
                    });
                }
            }
            ConstraintKind::PointLineDistance { point, line, value, .. } => {
                if let (Some(p), Some(l), Some(pp), Some(le)) = (
                    pt(&vars, point),
                    datums.get(&line).and_then(|d| d.line()),
                    sketch.point(point),
                    sketch.entity(line),
                ) && let EntityKind::Line { a, b } = le.kind
                {
                    let (ea, eb, ec) = line_equation(a, b);
                    let side = ea * pp.u + eb * pp.v + ec;
                    reqs.push(Constraint::PointLineDistance(p, l, signed(value, side)));
                }
            }
            ConstraintKind::Angle { a, b, value, .. } => {
                if let (Some(l0), Some(l1)) =
                    (datums.get(&a).and_then(|d| d.line()), datums.get(&b).and_then(|d| d.line()))
                {
                    reqs.push(Constraint::LinesAtAngle(l0, l1, AngleKind::Other(Angle::from_degrees(value))));
                }
            }
            ConstraintKind::Radius { entity, value, diameter, .. } => {
                if let Some(circle) = datums.get(&entity).and_then(|d| d.circle()) {
                    let r = if diameter { value / 2.0 } else { value };
                    reqs.push(Constraint::Fixed(circle.radius.id, r.abs()));
                }
            }
        }
        for r in reqs {
            push(ConstraintRequest::highest_priority(r), Some(c.id), &mut requests, &mut owners);
        }
    }
    if let Some(d) = drag
        && let Some(p) = pt(&vars, d.point)
    {
        push(ConstraintRequest::new(Constraint::Fixed(p.id_x(), d.to.u), 1), None, &mut requests, &mut owners);
        push(ConstraintRequest::new(Constraint::Fixed(p.id_y(), d.to.v), 1), None, &mut requests, &mut owners);
    }

    // ezpz's default stops once every residual is below 1e-8, which leaves coordinates a
    // few nanometres off. A tighter tolerance costs one or two more iterations and lands
    // dimensioned corners within 1e-9.
    let config = Config::default().with_max_iterations(60).with_convergence_tolerance(1e-11);
    let guesses = vars.guesses.clone();
    let (outcome, under) = if analyse {
        match ezpz::solve_analysis(&requests, guesses, config) {
            Ok(r) => (r.outcome, Some(r.analysis.into_underconstrained())),
            Err(e) => {
                sketch.status = SolveStatus::Failed(e.error.to_string());
                return;
            }
        }
    } else {
        match ezpz::solve(&requests, guesses, config) {
            Ok(o) => (o, None),
            Err(e) => {
                sketch.status = SolveStatus::Failed(e.error.to_string());
                return;
            }
        }
    };

    // Write the solution back, unless the solver diverged.
    let values = outcome.final_values();
    if values.iter().any(|v| !v.is_finite()) {
        sketch.status = SolveStatus::Failed("the solver returned non-finite values".into());
        return;
    }
    let at = |id: ezpz::Id| values.get(id as usize).copied();
    for e in &mut sketch.entities {
        // The origin stays exactly where it is rather than within the tolerance.
        if e.id == ORIGIN {
            continue;
        }
        for i in 0..e.point_count() {
            if let Some(p) = vars.points.get(&PointRef::new(e.id, i))
                && let (Some(x), Some(y)) = (at(p.id_x()), at(p.id_y()))
            {
                e.set_point(i, P2::new(x, y));
            }
        }
        if let EntityKind::Circle { radius, .. } = &mut e.kind
            && let Some(r) = vars.radii.get(&e.id).and_then(|d| at(d.id))
        {
            *radius = r.abs();
        }
    }
    // Keep arc ends exactly on their circle whatever the residual was.
    for e in &mut sketch.entities {
        if let EntityKind::Arc { center, start, end } = &mut e.kind {
            let r = (center.dist(*start) + center.dist(*end)) / 2.0;
            if r > 1e-9 {
                *start = center.add(start.sub(*center).normalized().scale(r));
                *end = center.add(end.sub(*center).normalized().scale(r));
            }
        }
    }

    let mut conflicting: Vec<ConstraintId> =
        outcome.unsatisfied().iter().filter_map(|&i| owners.get(i).copied().flatten()).collect();
    conflicting.sort_unstable();
    conflicting.dedup();
    if !outcome.converged() && conflicting.is_empty() {
        // Nothing singled out but the system did not settle: blame every dimension so the user sees it.
        conflicting = sketch.constraints.iter().filter(|c| c.is_dimension()).map(|c| c.id).collect();
    }
    if let Some(under) = under {
        let mut free: Vec<EntityId> = Vec::new();
        for (r, p) in &vars.points {
            if under.contains(&p.id_x()) || under.contains(&p.id_y()) {
                free.push(r.entity);
            }
        }
        for (e, d) in &vars.radii {
            if under.contains(&d.id) {
                free.push(*e);
            }
        }
        free.sort_unstable();
        free.dedup();
        sketch.free = free;
    }
    sketch.status = if !conflicting.is_empty() {
        SolveStatus::OverDefined { conflicting }
    } else if sketch.free.is_empty() {
        SolveStatus::FullyDefined
    } else {
        SolveStatus::UnderDefined
    };
}

/// Tangency between a line and a curve, or two curves. At a shared end the
/// relation is the exact one: the line is perpendicular to the radius there, and
/// two arcs have parallel radii. ezpz's distance form has a zero derivative along
/// the circle at the touching point, so a shared end could slide without the
/// solver noticing. Elsewhere it is ezpz's distance form.
fn tangent(sketch: &Sketch, datums: &HashMap<EntityId, Datum>, a: EntityId, b: EntityId) -> Option<Constraint> {
    let (da, db) = (*datums.get(&a)?, *datums.get(&b)?);
    let shared = shared_point(sketch, a, b);
    match (da, db) {
        (Datum::Line(l), other) | (other, Datum::Line(l)) if other.circle().is_some() => {
            let circle = other.circle()?;
            if let Some((pa, pb)) = shared {
                let on_curve = if matches!(da, Datum::Line(_)) { pb } else { pa };
                let p = curve_point(datums, on_curve)?;
                return Some(Constraint::LinesAtAngle(
                    l,
                    DatumLineSegment::new(circle.center, p),
                    AngleKind::Perpendicular,
                ));
            }
            Some(Constraint::LineTangentToCircle(l, circle, LineSide::Undefined))
        }
        (x, y) => {
            let (cx, cy) = (x.circle()?, y.circle()?);
            if let Some((pa, _)) = shared {
                let p = curve_point(datums, pa)?;
                return Some(Constraint::LinesAtAngle(
                    DatumLineSegment::new(cx.center, p),
                    DatumLineSegment::new(cy.center, p),
                    AngleKind::Parallel,
                ));
            }
            Some(Constraint::CircleTangentToCircle(cx, cy, CircleSide::Undefined))
        }
    }
}

/// An end of `a` and an end of `b` joined by coincident constraints, in that order.
fn shared_point(sketch: &Sketch, a: EntityId, b: EntityId) -> Option<(PointRef, PointRef)> {
    let ea = sketch.entity(a)?;
    let eb = sketch.entity(b)?;
    let ends = |e: &Entity| -> Vec<PointRef> {
        match e.kind {
            EntityKind::Line { .. } => vec![PointRef::new(e.id, 0), PointRef::new(e.id, 1)],
            EntityKind::Arc { .. } => vec![PointRef::new(e.id, 1), PointRef::new(e.id, 2)],
            _ => Vec::new(),
        }
    };
    for pa in ends(ea) {
        let group = sketch.coincident_group(pa);
        for pb in ends(eb) {
            if group.contains(&pb) {
                return Some((pa, pb));
            }
        }
    }
    None
}

fn curve_point(datums: &HashMap<EntityId, Datum>, r: PointRef) -> Option<DatumPoint> {
    match datums.get(&r.entity)? {
        Datum::Arc(arc, _) => match r.index {
            1 => Some(arc.start),
            2 => Some(arc.end),
            _ => Some(arc.center),
        },
        Datum::Line(l) => Some(if r.index == 0 { l.p0 } else { l.p1 }),
        Datum::Point(p) => Some(*p),
        Datum::Circle(c) => Some(c.center),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rectangle(s: &mut Sketch, w: f64, h: f64) -> [EntityId; 4] {
        let c = [P2::new(0.0, 0.0), P2::new(w, 0.0), P2::new(w, h), P2::new(0.0, h)];
        let ids: Vec<EntityId> =
            (0..4).map(|i| s.add(EntityKind::Line { a: c[i], b: c[(i + 1) % 4] }, false)).collect();
        for i in 0..4 {
            s.constrain(ConstraintKind::Coincident {
                a: PointRef::new(ids[i], 1),
                b: PointRef::new(ids[(i + 1) % 4], 0),
            });
        }
        s.constrain(ConstraintKind::Horizontal { target: Align::Line(ids[0]) });
        s.constrain(ConstraintKind::Horizontal { target: Align::Line(ids[2]) });
        s.constrain(ConstraintKind::Vertical { target: Align::Line(ids[1]) });
        s.constrain(ConstraintKind::Vertical { target: Align::Line(ids[3]) });
        [ids[0], ids[1], ids[2], ids[3]]
    }

    #[test]
    fn a_dimensioned_rectangle_on_the_origin_is_fully_defined() {
        let mut s = Sketch::new();
        let ids = rectangle(&mut s, 50.0, 30.0);
        solve(&mut s, None, true);
        assert_eq!(s.status, SolveStatus::UnderDefined);
        s.constrain(ConstraintKind::Coincident { a: PointRef::new(ids[0], 0), b: PointRef::ORIGIN });
        s.constrain(ConstraintKind::Distance {
            a: PointRef::new(ids[0], 0),
            b: PointRef::new(ids[0], 1),
            kind: DistanceKind::Horizontal,
            value: 120.0,
            label: P2::ZERO,
        });
        s.constrain(ConstraintKind::Distance {
            a: PointRef::new(ids[1], 0),
            b: PointRef::new(ids[1], 1),
            kind: DistanceKind::Vertical,
            value: 90.0,
            label: P2::ZERO,
        });
        solve(&mut s, None, true);
        assert_eq!(s.status, SolveStatus::FullyDefined, "{:?}", s.free);
        let far = s.point(PointRef::new(ids[2], 0)).unwrap();
        assert!((far.u - 120.0).abs() < 1e-6 && (far.v - 90.0).abs() < 1e-6, "{far:?}");
    }

    #[test]
    fn conflicting_dimensions_are_reported() {
        let mut s = Sketch::new();
        let ids = rectangle(&mut s, 50.0, 30.0);
        s.constrain(ConstraintKind::Coincident { a: PointRef::new(ids[0], 0), b: PointRef::ORIGIN });
        let d1 = s.constrain(ConstraintKind::Distance {
            a: PointRef::new(ids[0], 0),
            b: PointRef::new(ids[0], 1),
            kind: DistanceKind::Aligned,
            value: 50.0,
            label: P2::ZERO,
        });
        let d2 = s.constrain(ConstraintKind::Distance {
            a: PointRef::new(ids[2], 0),
            b: PointRef::new(ids[2], 1),
            kind: DistanceKind::Aligned,
            value: 70.0,
            label: P2::ZERO,
        });
        solve(&mut s, None, true);
        match &s.status {
            SolveStatus::OverDefined { conflicting } => {
                assert!(conflicting.contains(&d1) || conflicting.contains(&d2), "{conflicting:?}")
            }
            other => panic!("expected over defined, got {other:?}"),
        }
    }

    #[test]
    fn a_slot_keeps_its_tangent_ends_while_dragged() {
        let mut s = Sketch::new();
        let (c0, c1, r) = (P2::new(0.0, 0.0), P2::new(40.0, 0.0), 6.0);
        let top = s.add(EntityKind::Line { a: P2::new(0.0, r), b: P2::new(40.0, r) }, false);
        let bottom = s.add(EntityKind::Line { a: P2::new(40.0, -r), b: P2::new(0.0, -r) }, false);
        // Arcs are counter-clockwise: the right cap runs from -v to +v through +u.
        let right = s.add(EntityKind::Arc { center: c1, start: P2::new(40.0, -r), end: P2::new(40.0, r) }, false);
        let left = s.add(EntityKind::Arc { center: c0, start: P2::new(0.0, r), end: P2::new(0.0, -r) }, false);
        for (a, b) in [
            (PointRef::new(top, 1), PointRef::new(right, 2)),
            (PointRef::new(right, 1), PointRef::new(bottom, 0)),
            (PointRef::new(bottom, 1), PointRef::new(left, 2)),
            (PointRef::new(left, 1), PointRef::new(top, 0)),
        ] {
            s.constrain(ConstraintKind::Coincident { a, b });
        }
        s.constrain(ConstraintKind::Tangent { a: top, b: right });
        s.constrain(ConstraintKind::Tangent { a: bottom, b: right });
        s.constrain(ConstraintKind::Tangent { a: top, b: left });
        s.constrain(ConstraintKind::Tangent { a: bottom, b: left });
        s.constrain(ConstraintKind::Equal { a: left, b: right });
        solve(&mut s, None, true);
        assert!(matches!(s.status, SolveStatus::UnderDefined), "{:?}", s.status);
        solve(&mut s, Some(Drag { point: PointRef::new(right, 0), to: P2::new(60.0, 5.0) }), false);
        let c = s.point(PointRef::new(right, 0)).unwrap();
        assert!((c.u - 60.0).abs() < 1e-3 && (c.v - 5.0).abs() < 1e-3, "{c:?}");
        // Nothing fixes the radius, but the two caps stay equal and the ends stay joined.
        let (er, el) = (s.entity(right).unwrap(), s.entity(left).unwrap());
        assert!((er.radius().unwrap() - el.radius().unwrap()).abs() < 1e-6);
        assert!((er.radius().unwrap() - r).abs() < 0.5, "{}", er.radius().unwrap());
        let gap = s.point(PointRef::new(top, 1)).unwrap().dist(s.point(PointRef::new(right, 2)).unwrap());
        assert!(gap < 1e-6, "gap {gap}");
    }
}
