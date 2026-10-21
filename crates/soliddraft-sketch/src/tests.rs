use std::f64::consts::PI;

use soliddraft_kernel_api::{Plane, Segment};

use crate::solve::solve;
use crate::{
    Align, ConstraintId, ConstraintKind, DistanceKind, EntityId, EntityKind, ORIGIN, P2, PointRef, ProfileError,
    Region, Sketch, SketchError, SolveStatus,
};

fn p(u: f64, v: f64) -> P2 {
    P2::new(u, v)
}

fn assert_near(actual: f64, expected: f64, tol: f64) {
    assert!((actual - expected).abs() <= tol, "{actual} is not within {tol} of {expected}");
}

fn assert_point(actual: P2, expected: P2, tol: f64) {
    assert!(actual.dist(expected) <= tol, "{actual:?} is not within {tol} of {expected:?}");
}

/// Solves with analysis and checks every constraint holds.
fn solve_ok(s: &mut Sketch) {
    solve(s, None, true);
    assert!(matches!(s.status, SolveStatus::FullyDefined | SolveStatus::UnderDefined), "{:?}", s.status);
}

fn solve_full(s: &mut Sketch) {
    solve(s, None, true);
    assert_eq!(s.status, SolveStatus::FullyDefined, "free: {:?}", s.free);
    assert!(s.free.is_empty());
}

fn at(s: &Sketch, entity: EntityId, index: u8) -> P2 {
    s.point(PointRef::new(entity, index)).unwrap()
}

/// Moves one point without moving the points joined to it, so the solver has work to do.
fn nudge(s: &mut Sketch, entity: EntityId, index: u8, to: P2) {
    s.entity_mut(entity).unwrap().set_point(index, to);
}

fn line_ends(s: &Sketch, line: EntityId) -> (P2, P2) {
    match s.entity(line).unwrap().kind {
        EntityKind::Line { a, b } => (a, b),
        k => panic!("{line} is {k:?}"),
    }
}

fn dir(s: &Sketch, line: EntityId) -> P2 {
    let (a, b) = line_ends(s, line);
    (b - a).normalized()
}

/// Centre and radius of an arc or circle.
fn round(s: &Sketch, id: EntityId) -> (P2, f64) {
    let e = s.entity(id).unwrap();
    (e.point(0).unwrap(), e.radius().unwrap())
}

/// Unsigned distance from a point to the infinite line through a line entity.
fn point_line_distance(s: &Sketch, x: P2, line: EntityId) -> f64 {
    let (a, _) = line_ends(s, line);
    dir(s, line).cross(x - a).abs()
}

/// Checks each line joined to an arc end is perpendicular to the radius there and
/// touches the arc end.
fn assert_tangent_corners(s: &Sketch, lines: &[EntityId], arcs: &[EntityId]) {
    let mut joints = 0;
    for &arc in arcs {
        for end in [1, 2] {
            let group = s.coincident_group(PointRef::new(arc, end));
            for &line in lines {
                for li in [0, 1] {
                    if group.contains(&PointRef::new(line, li)) {
                        let radius = at(s, arc, end) - at(s, arc, 0);
                        assert_near(dir(s, line).dot(radius.normalized()), 0.0, 1e-9);
                        assert_point(at(s, line, li), at(s, arc, end), 1e-9);
                        joints += 1;
                    }
                }
            }
        }
    }
    assert_eq!(joints, 2 * arcs.len());
}

/// Checks every region passes the kernel check and every segment matches the
/// geometry of the entity it names.
fn assert_regions_sound(s: &Sketch, regions: &[Region]) {
    for region in regions {
        region.to_profile(Plane::XY).check().unwrap();
        assert!(region.outer.signed_area() > 0.0);
        let loops = std::iter::once(&region.outer).chain(&region.holes);
        for (li, lp) in loops.enumerate() {
            assert_eq!(lp.segments.len(), lp.entities.len());
            if li > 0 && !matches!(lp.segments[..], [Segment::Circle { .. }]) {
                assert!(lp.signed_area() < 0.0, "hole {li} runs counter-clockwise");
            }
            for (si, seg) in lp.segments.iter().enumerate() {
                let id = region.entity(li, si).unwrap();
                assert_eq!(id, lp.entities[si]);
                let e = s.entity(id).unwrap();
                let q = |x: soliddraft_kernel_api::Point2| P2::new(x.u, x.v);
                match (*seg, e.kind) {
                    (Segment::Line { from, to }, EntityKind::Line { a, b }) => {
                        let (f, t) = (q(from), q(to));
                        assert!(
                            (f.dist(a) < 1e-6 && t.dist(b) < 1e-6) || (f.dist(b) < 1e-6 && t.dist(a) < 1e-6),
                            "segment {si} of loop {li} is not line {id}"
                        );
                    }
                    (Segment::Arc { center, from, to, clockwise }, EntityKind::Arc { center: c, start, end }) => {
                        assert_point(q(center), c, 1e-6);
                        let (want_from, want_to) = if clockwise { (end, start) } else { (start, end) };
                        assert_point(q(from), want_from, 1e-6);
                        assert_point(q(to), want_to, 1e-6);
                    }
                    (Segment::Circle { center, radius }, EntityKind::Circle { center: c, radius: r }) => {
                        assert_point(q(center), c, 0.0);
                        assert_eq!(radius, r);
                    }
                    (seg, kind) => panic!("segment {seg:?} came from {kind:?}"),
                }
            }
        }
        assert_eq!(region.entity(region.holes.len() + 1, 0), None);
    }
}

fn dimension(s: &mut Sketch, id: ConstraintId, value: f64) {
    s.constraint_mut(id).unwrap().set_value(value);
}

#[test]
fn rectangle_solves_exactly_and_is_fully_defined() {
    let mut s = Sketch::new();
    let shape = s.rectangle(p(5.0, 5.0), 40.0, 30.0).unwrap();
    let sides = &shape.entities[..4];
    // Start from a sloppy drawing.
    nudge(&mut s, sides[1], 0, p(26.0, -9.0));
    nudge(&mut s, sides[3], 0, p(-14.0, 21.5));
    solve_full(&mut s);
    let expected = [p(-15.0, -10.0), p(25.0, -10.0), p(25.0, 20.0), p(-15.0, 20.0)];
    for (i, &id) in sides.iter().enumerate() {
        assert_point(at(&s, id, 0), expected[i], 1e-9);
        assert_point(at(&s, id, 1), expected[(i + 1) % 4], 1e-9);
    }
    assert_eq!(s.point(PointRef::ORIGIN), Some(P2::ZERO));
}

#[test]
fn editing_a_dimension_moves_the_geometry() {
    let mut s = Sketch::new();
    let shape = s.rectangle(p(5.0, 5.0), 40.0, 30.0).unwrap();
    let [width, height] = shape.dimensions[..] else { panic!("{shape:?}") };
    dimension(&mut s, width, 50.0);
    dimension(&mut s, height, 10.0);
    solve_full(&mut s);
    assert_point(at(&s, shape.entities[0], 0), p(-20.0, 0.0), 1e-9);
    assert_point(at(&s, shape.entities[2], 0), p(30.0, 10.0), 1e-9);
    assert_eq!(s.constraint(width).unwrap().value(), Some(50.0));
}

#[test]
fn rounded_rectangle_is_tangent_and_has_the_right_area() {
    let mut s = Sketch::new();
    let shape = s.rounded_rectangle(p(0.0, 0.0), 120.0, 90.0, 8.0).unwrap();
    let (lines, arcs) = (&shape.entities[..4], &shape.entities[4..8]);
    assert_eq!(shape.dimensions.len(), 3);
    // Nudge a line end and an arc end off their places.
    nudge(&mut s, lines[0], 1, p(51.5, -45.3));
    nudge(&mut s, arcs[0], 2, p(60.2, -36.5));
    solve_full(&mut s);
    assert_tangent_corners(&s, lines, arcs);
    for &arc in arcs {
        assert_near(round(&s, arc).1, 8.0, 1e-9);
    }

    let regions = s.regions().unwrap();
    assert_eq!(regions.len(), 1);
    let region = &regions[0];
    assert!(region.holes.is_empty());
    assert_eq!(region.outer.segments.len(), 8);
    assert_regions_sound(&s, &regions);
    let expected = 120.0 * 90.0 - (4.0 - PI) * 64.0;
    assert!((region.area() - expected).abs() <= 1e-9 * expected, "{} vs {expected}", region.area());
    // Loops start at their smallest entity, and every segment maps back to its entity.
    assert_eq!(region.entity(0, 0), Some(lines[0]));
    assert_eq!(region.outer.entities, vec![lines[0], arcs[0], lines[1], arcs[1], lines[2], arcs[2], lines[3], arcs[3]]);
    assert_eq!(region.entity(0, 8), None);
    assert_eq!(region.entity(1, 0), None);
    assert!(region.outer.segments.iter().all(|s| !matches!(s, Segment::Arc { clockwise: true, .. })));
}

#[test]
fn rounded_rectangle_with_five_holes_is_one_region_with_clockwise_holes() {
    let mut s = Sketch::new();
    let outer = s.rounded_rectangle(p(0.0, 0.0), 120.0, 90.0, 8.0).unwrap();
    let mut holes = Vec::new();
    for (u, v) in [(45.0, 30.0), (-45.0, 30.0), (-45.0, -30.0), (45.0, -30.0)] {
        holes.push(s.rectangle(p(u, v), 10.0, 8.0).unwrap());
    }
    holes.push(s.slot(p(0.0, 0.0), 40.0, 12.0).unwrap());
    solve_full(&mut s);
    let regions = s.regions().unwrap();
    assert_eq!(regions.len(), 1);
    let region = &regions[0];
    assert_eq!(region.outer.entities[0], outer.entities[0]);
    assert_eq!(region.holes.len(), 5);
    for (hole, shape) in region.holes.iter().zip(&holes) {
        assert!(hole.signed_area() < 0.0);
        assert_eq!(hole.entities[0], shape.entities[0]);
    }
    assert_regions_sound(&s, &regions);
    let expected = 120.0 * 90.0 - (4.0 - PI) * 64.0 - 4.0 * 80.0 - (28.0 * 12.0 + PI * 36.0);
    assert_near(region.area(), expected, 1e-9 * expected);
}

#[test]
fn rounded_rectangle_with_circle_holes() {
    let mut s = Sketch::new();
    s.rounded_rectangle(p(0.0, 0.0), 120.0, 90.0, 8.0).unwrap();
    for (u, v) in [(45.0, 22.0), (-45.0, 22.0), (-45.0, -22.0), (45.0, -22.0)] {
        s.circle_at(p(u, v), 9.0).unwrap();
    }
    s.circle_at(p(20.0, 0.0), 12.0).unwrap();
    solve_full(&mut s);
    let regions = s.regions().unwrap();
    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].holes.len(), 5);
    for hole in &regions[0].holes {
        assert!(matches!(hole.segments[..], [Segment::Circle { .. }]));
    }
    assert_regions_sound(&s, &regions);
    let expected = 120.0 * 90.0 - (4.0 - PI) * 64.0 - PI * (4.0 * 4.5 * 4.5 + 36.0);
    assert_near(regions[0].area(), expected, 1e-9 * expected);
}

#[test]
fn polygon_holes_run_clockwise() {
    let mut s = Sketch::new();
    s.rectangle(p(0.0, 0.0), 100.0, 100.0).unwrap();
    s.rectangle(p(10.0, 10.0), 20.0, 20.0).unwrap();
    // Drawn clockwise on purpose, so it has to be turned around.
    s.polyline_loop(&[p(-40.0, -10.0), p(-40.0, 10.0), p(-20.0, 10.0), p(-20.0, -10.0)]).unwrap();
    solve_ok(&mut s);
    let regions = s.regions().unwrap();
    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].holes.len(), 2);
    for hole in &regions[0].holes {
        assert_near(hole.signed_area(), -400.0, 1e-9);
    }
    assert_regions_sound(&s, &regions);
    assert_near(regions[0].area(), 100.0 * 100.0 - 800.0, 1e-9);
}

#[test]
fn four_circles_are_four_regions() {
    let mut s = Sketch::new();
    let shapes: Vec<_> = (0..4).map(|i| s.circle_at(p(20.0 * f64::from(i), 0.0), 10.0).unwrap()).collect();
    solve_full(&mut s);
    let regions = s.regions().unwrap();
    assert_eq!(regions.len(), 4);
    for (region, shape) in regions.iter().zip(&shapes) {
        assert_eq!(region.outer.entities, vec![shape.entities[0]]);
        assert_near(region.area(), 25.0 * PI, 1e-9);
    }
    assert_regions_sound(&s, &regions);
}

#[test]
fn slot_is_one_region_with_two_arcs() {
    let mut s = Sketch::new();
    let shape = s.slot(p(3.0, -2.0), 40.0, 12.0).unwrap();
    let (bottom, top) = (shape.entities[0], shape.entities[1]);
    nudge(&mut s, top, 0, p(17.5, 4.3));
    solve_full(&mut s);
    assert_point(at(&s, bottom, 0), p(3.0 - 14.0, -8.0), 1e-9);
    assert_point(at(&s, top, 0), p(3.0 + 14.0, 4.0), 1e-9);
    assert_tangent_corners(&s, &shape.entities[..2], &shape.entities[2..4]);
    let regions = s.regions().unwrap();
    assert_eq!(regions.len(), 1);
    let outer = &regions[0].outer;
    assert_eq!(outer.segments.iter().filter(|s| matches!(s, Segment::Arc { clockwise: false, .. })).count(), 2);
    assert_regions_sound(&s, &regions);
    assert_near(regions[0].area(), 28.0 * 12.0 + PI * 36.0, 1e-9);
}

#[test]
fn concave_arc_runs_clockwise_in_the_outer_loop() {
    let mut s = Sketch::new();
    let pts = [p(0.0, 0.0), p(40.0, 0.0), p(40.0, 15.0), p(20.0, 15.0), p(15.0, 20.0), p(15.0, 40.0), p(0.0, 40.0)];
    let mut chain = Vec::new();
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        chain.push(if i == 3 {
            // Counter-clockwise from (15, 20) to (20, 15) around (20, 20), which the
            // outer loop walks backwards.
            (s.add(EntityKind::Arc { center: p(20.0, 20.0), start: b, end: a }, false), true)
        } else {
            (s.add(EntityKind::Line { a, b }, false), false)
        });
    }
    let arc = chain[3].0;
    for i in 0..chain.len() {
        let ((e, _), (f, next_reversed)) = (chain[i], chain[(i + 1) % chain.len()]);
        // A line ends at point 1 and an arc walked backwards ends at its start, also
        // point 1; the reversed arc begins at its end, point 2.
        let start = if next_reversed { PointRef::new(f, 2) } else { PointRef::new(f, 0) };
        s.constrain(ConstraintKind::Coincident { a: PointRef::new(e, 1), b: start });
    }
    solve_ok(&mut s);
    let regions = s.regions().unwrap();
    assert_eq!(regions.len(), 1);
    let outer = &regions[0].outer;
    let i = outer.entities.iter().position(|&e| e == arc).unwrap();
    let Segment::Arc { from, to, clockwise, .. } = outer.segments[i] else { panic!("{:?}", outer.segments[i]) };
    assert!(clockwise);
    assert_point(P2::new(from.u, from.v), p(20.0, 15.0), 1e-9);
    assert_point(P2::new(to.u, to.v), p(15.0, 20.0), 1e-9);
    assert_regions_sound(&s, &regions);
    assert_near(regions[0].area(), 975.0 + 25.0 - 25.0 * PI / 4.0, 1e-9);
    // The same sketch always gives the same segments in the same order.
    assert_eq!(s.regions().unwrap(), regions);
    assert_eq!(s.clone().regions().unwrap(), regions);
}

#[test]
fn open_and_branched_chains_are_rejected() {
    let square = [p(0.0, 0.0), p(10.0, 0.0), p(10.0, 5.0), p(0.0, 5.0)];
    let mut s = Sketch::new();
    let shape = s.polyline_loop(&square).unwrap();
    s.remove_entity(shape.entities[3]);
    assert!(matches!(s.regions(), Err(ProfileError::OpenEnd(_))));
    assert!(crate::profile::profiles(&s, Plane::XY).is_err());

    let mut s = Sketch::new();
    let shape = s.polyline_loop(&square).unwrap();
    let spur = s.add(EntityKind::Line { a: p(10.0, 0.0), b: p(20.0, 0.0) }, false);
    s.constrain(ConstraintKind::Coincident { a: PointRef::new(shape.entities[0], 1), b: PointRef::new(spur, 0) });
    assert!(matches!(s.regions(), Err(ProfileError::Branch(_))));

    // A closed loop next to a stray line still gives its region.
    let mut s = Sketch::new();
    s.polyline_loop(&square).unwrap();
    s.add(EntityKind::Line { a: p(30.0, 0.0), b: p(40.0, 0.0) }, false);
    assert_eq!(s.regions().unwrap().len(), 1);

    assert_eq!(Sketch::new().regions(), Err(ProfileError::NoLoop));
}

#[test]
fn ends_within_tolerance_join_and_construction_is_ignored() {
    let mut s = Sketch::new();
    s.add(EntityKind::Line { a: p(0.0, 0.0), b: p(10.0, 0.0) }, false);
    s.add(EntityKind::Line { a: p(10.0, 0.0), b: p(10.0, 10.0) }, false);
    s.add(EntityKind::Line { a: p(10.0, 10.0 + 5e-7), b: p(0.0, 0.0) }, false);
    s.add(EntityKind::Line { a: p(0.0, 0.0), b: p(10.0, 10.0) }, true);
    s.add(EntityKind::Circle { center: p(50.0, 0.0), radius: 3.0 }, true);
    let regions = s.regions().unwrap();
    assert_eq!(regions.len(), 1);
    assert_regions_sound(&s, &regions);
    assert_near(regions[0].area(), 50.0, 1e-5);
}

#[test]
fn inconsistent_sketch_reports_its_constraints() {
    let mut s = Sketch::new();
    let a = s.add(EntityKind::Point { p: p(0.0, 0.0) }, false);
    let b = s.add(EntityKind::Point { p: p(10.0, 0.0) }, false);
    let (pa, pb) = (PointRef::new(a, 0), PointRef::new(b, 0));
    let fix = s.constrain(ConstraintKind::Fix { point: pa, at: p(0.0, 0.0) });
    let mut distance = |value| {
        s.constrain(ConstraintKind::Distance { a: pa, b: pb, kind: DistanceKind::Aligned, value, label: P2::ZERO })
    };
    let (d1, d2) = (distance(10.0), distance(20.0));
    solve(&mut s, None, true);
    let SolveStatus::OverDefined { conflicting } = &s.status else { panic!("{:?}", s.status) };
    assert!(!conflicting.is_empty());
    assert!(conflicting.iter().all(|c| [fix, d1, d2].contains(c)), "{conflicting:?}");
    assert!(conflicting.windows(2).all(|w| w[0] < w[1]));
    // Solving again from the compromise does not panic either.
    solve(&mut s, None, false);
}

/// A line with both ends held in place.
fn fixed_line(s: &mut Sketch, a: P2, b: P2) -> EntityId {
    let id = s.add(EntityKind::Line { a, b }, false);
    s.constrain(ConstraintKind::Fix { point: PointRef::new(id, 0), at: a });
    s.constrain(ConstraintKind::Fix { point: PointRef::new(id, 1), at: b });
    id
}

/// A circle whose centre is held in place.
fn fixed_circle(s: &mut Sketch, c: P2, r: f64) -> EntityId {
    let id = s.add(EntityKind::Circle { center: c, radius: r }, false);
    s.constrain(ConstraintKind::Fix { point: PointRef::new(id, 0), at: c });
    id
}

fn line(s: &mut Sketch, a: P2, b: P2) -> EntityId {
    s.add(EntityKind::Line { a, b }, false)
}

fn arc(s: &mut Sketch, center: P2, start: P2, end: P2) -> EntityId {
    s.add(EntityKind::Arc { center, start, end }, false)
}

fn point(s: &mut Sketch, at: P2) -> PointRef {
    PointRef::new(s.add(EntityKind::Point { p: at }, false), 0)
}

#[test]
fn horizontal_and_vertical() {
    let mut s = Sketch::new();
    let h = line(&mut s, p(1.0, 1.0), p(10.0, 3.0));
    let v = line(&mut s, p(2.0, 2.0), p(3.5, 9.0));
    let (a, b) = (point(&mut s, p(0.0, 5.0)), point(&mut s, p(7.0, 6.0)));
    let (c, d) = (point(&mut s, p(-3.0, 0.0)), point(&mut s, p(-2.0, 8.0)));
    s.constrain(ConstraintKind::Horizontal { target: Align::Line(h) });
    s.constrain(ConstraintKind::Vertical { target: Align::Line(v) });
    s.constrain(ConstraintKind::Horizontal { target: Align::Points(a, b) });
    s.constrain(ConstraintKind::Vertical { target: Align::Points(c, d) });
    solve_ok(&mut s);
    let (h0, h1) = line_ends(&s, h);
    assert_near(h0.v, h1.v, 1e-9);
    assert!((h0.u - h1.u).abs() > 1.0);
    let (v0, v1) = line_ends(&s, v);
    assert_near(v0.u, v1.u, 1e-9);
    assert_near(s.point(a).unwrap().v, s.point(b).unwrap().v, 1e-9);
    assert_near(s.point(c).unwrap().u, s.point(d).unwrap().u, 1e-9);
}

#[test]
fn coincident_and_point_on_lines_arcs_and_circles() {
    let mut s = Sketch::new();
    let l = fixed_line(&mut s, p(0.0, 0.0), p(10.0, 0.0));
    let circle = fixed_circle(&mut s, p(30.0, 0.0), 5.0);
    let a = arc(&mut s, p(0.0, 30.0), p(5.0, 30.0), p(0.0, 35.0));
    let (q1, q2) = (point(&mut s, p(4.0, 1.0)), point(&mut s, p(4.5, 2.0)));
    let on_line = point(&mut s, p(3.0, 1.5));
    let on_circle = point(&mut s, p(33.0, 1.0));
    let on_arc = point(&mut s, p(3.0, 33.0));
    s.constrain(ConstraintKind::Coincident { a: q1, b: q2 });
    s.constrain(ConstraintKind::PointOn { point: on_line, entity: l });
    s.constrain(ConstraintKind::PointOn { point: on_circle, entity: circle });
    s.constrain(ConstraintKind::PointOn { point: on_arc, entity: a });
    solve_ok(&mut s);
    assert_point(s.point(q1).unwrap(), s.point(q2).unwrap(), 1e-9);
    assert_near(point_line_distance(&s, s.point(on_line).unwrap(), l), 0.0, 1e-9);
    let (c, r) = round(&s, circle);
    assert_near(s.point(on_circle).unwrap().dist(c), r, 1e-9);
    let (c, r) = round(&s, a);
    assert_near(s.point(on_arc).unwrap().dist(c), r, 1e-9);
}

#[test]
fn tangent_pairs() {
    let mut s = Sketch::new();
    // Line and arc.
    let l1 = fixed_line(&mut s, p(0.0, 0.0), p(10.0, 0.0));
    let a1 = arc(&mut s, p(5.0, 3.5), p(9.0, 3.5), p(1.0, 3.5));
    // Line and circle.
    let l2 = fixed_line(&mut s, p(20.0, 0.0), p(30.0, 0.0));
    let c2 = fixed_circle(&mut s, p(25.0, 4.0), 3.0);
    // Arc and arc, arc and circle, circle and circle.
    let a3 = arc(&mut s, p(50.0, 0.0), p(55.0, 0.0), p(45.0, 0.0));
    let a4 = arc(&mut s, p(60.0, 0.0), p(64.0, 0.0), p(56.0, 0.0));
    let c5 = fixed_circle(&mut s, p(80.0, 0.0), 5.0);
    let c6 = fixed_circle(&mut s, p(91.0, 0.0), 5.0);
    // Two arcs meeting at a shared end, an S bend.
    let s1 = arc(&mut s, p(105.0, 0.0), p(105.0, 5.0), p(110.0, 0.5));
    let s2 = arc(&mut s, p(116.0, 1.0), p(116.0, -5.0), p(110.0, 0.5));
    s.constrain(ConstraintKind::Coincident { a: PointRef::new(s1, 2), b: PointRef::new(s2, 2) });
    for (a, b) in [(l1, a1), (c2, l2), (a3, a4), (a4, c5), (c5, c6), (s1, s2)] {
        s.constrain(ConstraintKind::Tangent { a, b });
    }
    solve_ok(&mut s);
    for (l, c) in [(l1, a1), (l2, c2)] {
        let (center, r) = round(&s, c);
        assert_near(point_line_distance(&s, center, l), r, 1e-9);
    }
    for (a, b) in [(a3, a4), (a4, c5), (c5, c6), (s1, s2)] {
        let ((ca, ra), (cb, rb)) = (round(&s, a), round(&s, b));
        let d = ca.dist(cb);
        assert!((d - (ra + rb)).abs() < 1e-9 || (d - (ra - rb).abs()).abs() < 1e-9, "{a} {b}: {d} {ra} {rb}");
    }
    assert_point(at(&s, s1, 2), at(&s, s2, 2), 1e-9);
}

#[test]
fn perpendicular_parallel_angle_and_equal() {
    let mut s = Sketch::new();
    let base = fixed_line(&mut s, p(0.0, 0.0), p(10.0, 0.0));
    let perp = line(&mut s, p(0.0, 1.0), p(1.0, 9.0));
    let par = line(&mut s, p(0.0, 5.0), p(9.0, 6.0));
    let angled = line(&mut s, p(20.0, 0.0), p(25.0, 5.0));
    let eq = line(&mut s, p(30.0, 0.0), p(33.0, 1.0));
    let (r1, r2) = (fixed_circle(&mut s, p(50.0, 0.0), 2.0), fixed_circle(&mut s, p(60.0, 0.0), 3.0));
    s.constrain(ConstraintKind::Radius { entity: r1, value: 2.0, diameter: false, label: P2::ZERO });
    s.constrain(ConstraintKind::Perpendicular { a: base, b: perp });
    s.constrain(ConstraintKind::Parallel { a: base, b: par });
    s.constrain(ConstraintKind::Angle { a: base, b: angled, value: 30.0, label: P2::ZERO });
    s.constrain(ConstraintKind::Equal { a: base, b: eq });
    s.constrain(ConstraintKind::Equal { a: r1, b: r2 });
    solve_ok(&mut s);
    let d = dir(&s, base);
    assert_near(d.dot(dir(&s, perp)), 0.0, 1e-9);
    assert_near(d.cross(dir(&s, par)), 0.0, 1e-9);
    // Counter-clockwise from the first line, modulo a half turn.
    let turn = (dir(&s, angled).angle() - d.angle()).rem_euclid(PI);
    assert_near(turn.to_degrees(), 30.0, 1e-7);
    assert_near(s.entity(eq).unwrap().length().unwrap(), 10.0, 1e-9);
    assert_near(round(&s, r2).1, 2.0, 1e-9);
}

#[test]
fn midpoint_concentric_radius_and_diameter() {
    let mut s = Sketch::new();
    let l = fixed_line(&mut s, p(0.0, 0.0), p(10.0, 4.0));
    let m = point(&mut s, p(3.0, 3.0));
    s.constrain(ConstraintKind::Midpoint { point: m, line: l });
    let c1 = fixed_circle(&mut s, p(20.0, 0.0), 2.0);
    let a2 = arc(&mut s, p(22.0, 1.0), p(26.0, 1.0), p(22.0, 5.0));
    s.constrain(ConstraintKind::Concentric { a: c1, b: a2 });
    s.constrain(ConstraintKind::Radius { entity: a2, value: 7.0, diameter: false, label: P2::ZERO });
    s.constrain(ConstraintKind::Radius { entity: c1, value: 6.0, diameter: true, label: P2::ZERO });
    solve_ok(&mut s);
    assert_point(s.point(m).unwrap(), p(5.0, 2.0), 1e-9);
    let ((cc, rc), (ca, ra)) = (round(&s, c1), round(&s, a2));
    assert_point(ca, cc, 1e-9);
    assert_near(rc, 3.0, 1e-9);
    assert_near(ra, 7.0, 1e-9);
    assert_near(at(&s, a2, 0).dist(at(&s, a2, 2)), 7.0, 1e-9);
}

#[test]
fn distances_keep_their_sides() {
    let mut s = Sketch::new();
    let a = point(&mut s, p(0.0, 0.0));
    s.constrain(ConstraintKind::Fix { point: a, at: p(0.0, 0.0) });
    let b = point(&mut s, p(3.0, 4.5));
    s.constrain(ConstraintKind::Distance { a, b, kind: DistanceKind::Aligned, value: 5.0, label: P2::ZERO });
    let l = fixed_line(&mut s, p(10.0, 0.0), p(20.0, 0.0));
    // Below the line, so the point must stay below it.
    let below = point(&mut s, p(15.0, -2.0));
    s.constrain(ConstraintKind::PointLineDistance { point: below, line: l, value: 3.0, label: P2::ZERO });
    let (c, d) = (point(&mut s, p(30.0, 0.0)), point(&mut s, p(29.0, 1.0)));
    s.constrain(ConstraintKind::Fix { point: c, at: p(30.0, 0.0) });
    // d is left of and above c, and stays there.
    s.constrain(ConstraintKind::Distance { a: c, b: d, kind: DistanceKind::Horizontal, value: 6.0, label: P2::ZERO });
    s.constrain(ConstraintKind::Distance { a: c, b: d, kind: DistanceKind::Vertical, value: 2.5, label: P2::ZERO });
    solve_ok(&mut s);
    assert_near(s.point(a).unwrap().dist(s.point(b).unwrap()), 5.0, 1e-9);
    assert_near(s.point(below).unwrap().v, -3.0, 1e-9);
    assert_point(s.point(d).unwrap(), p(24.0, 2.5), 1e-9);
}

#[test]
fn fixed_points_free_points_and_the_origin() {
    let mut s = Sketch::new();
    let a = point(&mut s, p(1.0, 1.0));
    s.constrain(ConstraintKind::Fix { point: a, at: p(-4.0, 7.0) });
    let b = point(&mut s, p(3.0, 3.0));
    s.constrain(ConstraintKind::Coincident { a: b, b: PointRef::ORIGIN });
    let free = point(&mut s, p(9.0, 9.0));
    solve(&mut s, None, true);
    assert_eq!(s.status, SolveStatus::UnderDefined);
    assert_eq!(s.free, vec![free.entity]);
    assert_point(s.point(a).unwrap(), p(-4.0, 7.0), 1e-9);
    assert_point(s.point(b).unwrap(), p(0.0, 0.0), 1e-9);
    assert_eq!(s.point(PointRef::ORIGIN), Some(P2::ZERO));
    assert_point(s.point(free).unwrap(), p(9.0, 9.0), 1e-12);
    assert!(!s.free.contains(&ORIGIN));
}

#[test]
fn unconstrained_circle_radius_is_free() {
    let mut s = Sketch::new();
    let circle = fixed_circle(&mut s, p(5.0, 5.0), 2.0);
    solve(&mut s, None, true);
    assert_eq!(s.free, vec![circle]);
    s.constrain(ConstraintKind::Radius { entity: circle, value: 3.0, diameter: false, label: P2::ZERO });
    solve_full(&mut s);
    assert_near(round(&s, circle).1, 3.0, 1e-9);
}

/// A shape builder call with its arguments filled in.
type Builder = fn(&mut Sketch) -> Result<crate::Shape, SketchError>;

#[test]
fn every_shape_is_fully_defined() {
    let builders: [Builder; 4] = [
        |s| s.rectangle(p(7.0, -3.0), 30.0, 20.0),
        |s| s.rounded_rectangle(p(-7.0, 3.0), 30.0, 20.0, 4.0),
        |s| s.circle_at(p(12.0, 4.0), 9.0),
        |s| s.slot(p(-2.0, 8.0), 30.0, 10.0),
    ];
    for build in builders {
        let mut s = Sketch::new();
        let shape = build(&mut s).unwrap();
        assert_eq!(shape.constraints.len(), s.constraints.len());
        assert!(shape.dimensions.iter().all(|d| shape.constraints.contains(d)));
        solve_full(&mut s);
        assert_regions_sound(&s, &s.regions().unwrap());
    }
    let mut s = Sketch::new();
    let shape = s.polyline_loop(&[p(0.0, 0.0), p(10.0, 0.0), p(5.0, 8.0)]).unwrap();
    assert!(shape.dimensions.is_empty());
    solve(&mut s, None, true);
    assert_eq!(s.status, SolveStatus::UnderDefined);
    assert_regions_sound(&s, &s.regions().unwrap());
}

#[test]
fn shape_builders_refuse_bad_sizes() {
    let mut s = Sketch::new();
    assert!(matches!(s.rectangle(P2::ZERO, 0.0, 1.0), Err(SketchError::NonPositive { what: "width", .. })));
    assert!(matches!(s.circle_at(P2::ZERO, f64::NAN), Err(SketchError::NonPositive { .. })));
    assert!(matches!(s.rounded_rectangle(P2::ZERO, 10.0, 10.0, 5.0), Err(SketchError::RadiusTooLarge { .. })));
    assert!(matches!(s.slot(P2::ZERO, 5.0, 6.0), Err(SketchError::SlotTooShort { .. })));
    assert_eq!(s.polyline_loop(&[P2::ZERO, p(1.0, 0.0)]), Err(SketchError::TooFewPoints(2)));
    assert!(s.is_empty());
}

#[test]
fn serde_round_trip_is_exact_and_ids_stay_fresh() {
    let mut s = Sketch::new();
    s.rounded_rectangle(p(1.0, 2.0), 50.0, 30.0, 4.0).unwrap();
    let slot = s.slot(p(0.0, 0.0), 20.0, 6.0).unwrap();
    nudge(&mut s, slot.entities[1], 0, p(7.1, -2.9));
    solve_full(&mut s);
    let text = serde_json::to_string(&s).unwrap();
    let mut back: Sketch = serde_json::from_str(&text).unwrap();
    // The solve results are not saved.
    assert_eq!(back.status, SolveStatus::Empty);
    back.status = s.status.clone();
    assert_eq!(back, s);
    assert_eq!(serde_json::to_string(&back).unwrap(), text);
    assert!(text.contains("\"next_entity\"") && text.contains("\"next_constraint\""));
    assert!(!text.contains("\"status\"") && !text.contains("\"free\""));

    // A file whose counters are stale gets fresh ids after normalise.
    let highest_entity = s.entities.iter().map(|e| e.id).max().unwrap();
    let highest_constraint = s.constraints.iter().map(|c| c.id).max().unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
    value["next_entity"] = 1.into();
    value["next_constraint"] = 1.into();
    let mut stale: Sketch = serde_json::from_value(value).unwrap();
    stale.normalise();
    assert!(stale.add(EntityKind::Point { p: P2::ZERO }, false) > highest_entity);
    let fresh = stale.constrain(ConstraintKind::Fix { point: PointRef::new(highest_entity + 1, 0), at: P2::ZERO });
    assert!(fresh > highest_constraint);
}

#[test]
fn json_shape_matches_the_app_files() {
    let mut s = Sketch::new();
    let l = s.add(EntityKind::Line { a: p(1.0, 2.0), b: p(3.0, 2.0) }, false);
    s.constrain(ConstraintKind::Horizontal { target: Align::Line(l) });
    assert_eq!(
        serde_json::to_string(&s).unwrap(),
        r#"{"entities":[{"id":0,"kind":{"type":"point","p":{"u":0.0,"v":0.0}},"construction":true},{"id":1,"kind":{"type":"line","a":{"u":1.0,"v":2.0},"b":{"u":3.0,"v":2.0}},"construction":false}],"constraints":[{"id":1,"kind":{"type":"horizontal","target":1}}],"next_entity":2,"next_constraint":2}"#
    );
}
