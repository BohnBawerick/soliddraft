use crate::tools::{End, Operation};
use crate::{ConstraintKind, EntityKind, P2, PointRef, Sketch, profile, solve};
use soliddraft_kernel_api::Plane;

fn p(u: f64, v: f64) -> P2 {
    P2::new(u, v)
}
fn line(s: &mut Sketch, a: P2, b: P2, construction: bool) -> u32 {
    s.add(EntityKind::Line { a, b }, construction)
}
fn area(s: &Sketch) -> f64 {
    profile::regions(s).unwrap().iter().map(|r| r.area()).sum()
}
fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-5, "{actual} != {expected}");
}
fn semicircle(s: &mut Sketch) -> [u32; 2] {
    let arc = s.add(EntityKind::Arc { center: P2::ZERO, start: p(5.0, 0.0), end: p(-5.0, 0.0) }, false);
    let base = line(s, p(-5.0, 0.0), p(5.0, 0.0), false);
    [arc, base]
}

#[test]
fn polygon_closes_and_survives_solving_and_serialization() {
    let mut s = Sketch::new();
    s.apply_operation(&Operation::Polygon { center: p(2.0, 3.0), vertex: p(7.0, 3.0), sides: 4 }).unwrap();
    solve::solve(&mut s, None, true);
    close(area(&s), 50.0);
    let mut back: Sketch = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    back.normalise();
    close(area(&back), 50.0);
    assert!(profile::profiles(&back, Plane::XY).is_ok());
}

#[test]
fn trim_splits_only_the_picked_interval_and_drops_stale_relations() {
    let mut s = Sketch::new();
    let target = line(&mut s, p(-10.0, 0.0), p(10.0, 0.0), false);
    line(&mut s, p(-3.0, -2.0), p(-3.0, 2.0), true);
    line(&mut s, p(4.0, -2.0), p(4.0, 2.0), true);
    s.constrain(ConstraintKind::Fix { point: PointRef::new(target, 1), at: p(10.0, 0.0) });
    let pieces = s.apply_operation(&Operation::Trim { entity: target, at: P2::ZERO }).unwrap();
    assert_eq!(s.entity(pieces[0]).unwrap().kind, EntityKind::Line { a: p(-10.0, 0.0), b: p(-3.0, 0.0) });
    assert_eq!(s.entity(pieces[1]).unwrap().kind, EntityKind::Line { a: p(4.0, 0.0), b: p(10.0, 0.0) });
    assert_eq!(s.constraints_of(target).count(), 0);
}

#[test]
fn trim_arc_uses_true_circle_intersections() {
    let mut s = Sketch::new();
    let ids = semicircle(&mut s);
    line(&mut s, p(0.0, -1.0), p(0.0, 8.0), true);
    s.apply_operation(&Operation::Trim { entity: ids[0], at: p(4.0, 3.0) }).unwrap();
    let EntityKind::Arc { center, start, end } = s.entity(ids[0]).unwrap().kind else { panic!() };
    assert_eq!(center, P2::ZERO);
    close(start.u, 0.0);
    close(start.v, 5.0);
    assert_eq!(end, p(-5.0, 0.0));
}

#[test]
fn extend_chooses_requested_end_and_respects_bounded_boundary() {
    let mut s = Sketch::new();
    let target = line(&mut s, p(0.0, 0.0), p(2.0, 0.0), false);
    let boundary = line(&mut s, p(5.0, -1.0), p(5.0, 1.0), true);
    let before = s.clone();
    assert!(s.apply_operation(&Operation::Extend { entity: target, end: End::Start, boundary }).is_err());
    assert_eq!(s, before);
    s.apply_operation(&Operation::Extend { entity: target, end: End::End, boundary }).unwrap();
    assert_eq!(s.entity(target).unwrap().kind, EntityKind::Line { a: P2::ZERO, b: p(5.0, 0.0) });
    let outside = line(&mut s, p(8.0, 2.0), p(8.0, 4.0), true);
    let before = s.clone();
    assert!(s.apply_operation(&Operation::Extend { entity: target, end: End::End, boundary: outside }).is_err());
    assert_eq!(s, before);
}

#[test]
fn extend_arc_past_angle_wrap_without_changing_radius() {
    let mut s = Sketch::new();
    let arc = s.add(EntityKind::Arc { center: P2::ZERO, start: p(0.0, -5.0), end: p(5.0, 0.0) }, false);
    let boundary = line(&mut s, p(-1.0, 5.0), p(1.0, 5.0), true);
    s.apply_operation(&Operation::Extend { entity: arc, end: End::End, boundary }).unwrap();
    let EntityKind::Arc { center, start, end } = s.entity(arc).unwrap().kind else { panic!() };
    assert_eq!((center, start, end), (P2::ZERO, p(0.0, -5.0), p(0.0, 5.0)));
}

#[test]
fn mirror_reverses_arc_handedness_and_preserves_construction() {
    let mut s = Sketch::new();
    let ids = semicircle(&mut s);
    let guide = line(&mut s, p(0.0, 1.0), p(1.0, 2.0), true);
    let copies = s
        .apply_operation(&Operation::Mirror {
            entities: vec![ids[0], ids[1], guide],
            axis_start: p(10.0, 0.0),
            axis_end: p(10.0, 1.0),
        })
        .unwrap();
    assert_eq!(
        s.entity(copies[0]).unwrap().kind,
        EntityKind::Arc { center: p(20.0, 0.0), start: p(25.0, 0.0), end: p(15.0, 0.0) }
    );
    assert!(s.entity(copies[2]).unwrap().construction);
    close(area(&s), 25.0 * std::f64::consts::PI);
}

#[test]
fn offset_rounded_chain_joins_lines_and_arcs_without_faceting() {
    let mut s = Sketch::new();
    let shape = s.rounded_rectangle(P2::ZERO, 20.0, 12.0, 2.0).unwrap();
    let ids = shape.entities;
    let order = vec![ids[0], ids[4], ids[1], ids[5], ids[2], ids[6], ids[3], ids[7]];
    let out = s.apply_operation(&Operation::Offset { entities: order, distance: -1.0 }).unwrap();
    for &id in &out {
        let e = s.entity(id).unwrap();
        if e.is_curve() {
            close(e.radius().unwrap(), 3.0);
        }
    }
    let outer = 22.0 * 14.0 - (4.0 - std::f64::consts::PI) * 9.0;
    let inner = 20.0 * 12.0 - (4.0 - std::f64::consts::PI) * 4.0;
    close(area(&s), outer - inner);
}

#[test]
fn offset_miters_a_square_and_rejects_consumed_edges_atomically() {
    let mut s = Sketch::new();
    let shape = s.rectangle(P2::ZERO, 10.0, 10.0).unwrap();
    let ids = shape.entities[..4].to_vec();
    s.apply_operation(&Operation::Offset { entities: ids.clone(), distance: -2.0 }).unwrap();
    close(area(&s), 196.0 - 100.0);
    let before = s.clone();
    assert!(s.apply_operation(&Operation::Offset { entities: ids, distance: 6.0 }).is_err());
    assert_eq!(s, before);
}

#[test]
fn patterns_copy_lines_and_arcs_with_seed_in_count() {
    let mut s = Sketch::new();
    let ids = semicircle(&mut s);
    s.apply_operation(&Operation::LinearPattern { entities: ids.to_vec(), step: p(15.0, 0.0), count: 3 }).unwrap();
    close(area(&s), 37.5 * std::f64::consts::PI);
    let mut s = Sketch::new();
    let shape = s.slot(p(10.0, 0.0), 6.0, 2.0).unwrap();
    s.apply_operation(&Operation::CircularPattern {
        entities: shape.entities[..4].to_vec(),
        center: P2::ZERO,
        angle_degrees: 90.0,
        count: 4,
    })
    .unwrap();
    close(area(&s), 4.0 * (8.0 + std::f64::consts::PI));
}

#[test]
fn invalid_tools_do_not_consume_ids_or_touch_constraints() {
    let mut s = Sketch::new();
    let shape = s.rectangle(P2::ZERO, 10.0, 8.0).unwrap();
    let ids = shape.entities[..4].to_vec();
    for op in [
        Operation::Polygon { center: P2::ZERO, vertex: P2::ZERO, sides: 6 },
        Operation::Mirror { entities: ids.clone(), axis_start: P2::ZERO, axis_end: P2::ZERO },
        Operation::LinearPattern { entities: ids.clone(), step: p(f64::NAN, 0.0), count: 2 },
        Operation::LinearPattern { entities: ids.clone(), step: p(10.0, 0.0), count: 0 },
        Operation::CircularPattern { entities: ids.clone(), center: P2::ZERO, angle_degrees: 1.0, count: u32::MAX },
        Operation::CircularPattern { entities: ids, center: P2::ZERO, angle_degrees: 180.0, count: 3 },
        Operation::Offset { entities: vec![999], distance: 1.0 },
        Operation::Trim { entity: shape.entities[0], at: p(-5.0, -4.0) },
    ] {
        let before = s.clone();
        assert!(s.apply_operation(&op).is_err(), "{op:?}");
        assert_eq!(s, before);
    }
}
