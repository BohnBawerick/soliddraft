//! Catallax stage 3 and 4 features through the kernel interface the application uses.

use std::f64::consts::PI;

use soliddraft_kernel_api::*;
use soliddraft_kernel_catallax::CatallaxKernel;

/// A `w` by `h` block centred on the origin in XY, from `z0` up by `d`.
fn block(k: &CatallaxKernel, tag: &str, w: f64, h: f64, z0: f64, d: f64) -> BodyId {
    let plane = Plane::XY.with_origin(Vec3::new(0.0, 0.0, z0));
    let sheet = k.create_body_from_profile(&OpTag::new(tag), &Profile::rectangle(plane, Point2::new(0.0, 0.0), w, h));
    let sheet = sheet.unwrap().body;
    let body = k.extrude(&OpTag::new(tag), sheet, &Extrude { distance: d, direction: None }).unwrap().body;
    k.release(sheet).unwrap();
    body
}

fn volume(k: &CatallaxKernel, body: BodyId) -> f64 {
    k.mass_properties(body).unwrap().volume
}

fn roles(r: &OpResult) -> Vec<FaceRole> {
    r.record.face_roles.iter().map(|o| o.role).collect()
}

fn down(point: Vec3) -> HolePlacement {
    HolePlacement { point, direction: -Vec3::Z, on_face: None }
}

#[test]
fn holes_drill_with_standard_angles_and_name_their_parts() {
    let k = CatallaxKernel::new();
    let plate = block(&k, "plate", 60.0, 40.0, 0.0, 10.0);
    let before = volume(&k, plate);
    let through = Hole {
        placements: vec![down(Vec3::new(-20.0, 0.0, 10.0)), down(Vec3::new(20.0, 0.0, 10.0))],
        diameter: 4.0,
        depth: HoleDepth::Through,
        tip: HoleTip::Flat,
        head: Some(HoleHead::Countersink { diameter: 8.0, angle_deg: 90.0 }),
    };
    let drilled = k.hole(&OpTag::new("bolts"), plate, &through).unwrap();
    assert_eq!(drilled.record.op, OpKind::Hole);
    let made = roles(&drilled);
    for placement in [0, 1] {
        for part in [HolePart::Countersink, HolePart::Wall] {
            assert!(made.contains(&FaceRole::Hole { placement, part }), "{placement} {part:?} in {made:?}");
        }
    }
    // Each hole removes a Ø4 bore through 10 mm and a 90° countersink 2 mm deep.
    let bore = PI * 4.0 * 10.0;
    let sink = PI * 2.0 / 3.0 * (16.0 + 8.0 + 4.0) - PI * 4.0 * 2.0;
    assert!((before - volume(&k, drilled.body) - 2.0 * (bore + sink)).abs() < 1e-6);
    assert!(k.validate(drilled.body).unwrap().valid);

    for (angle, head) in [(118.0, None), (118.0, Some(82.0)), (118.0, Some(100.0))] {
        let blind = Hole {
            placements: vec![down(Vec3::new(0.0, 0.0, 10.0))],
            diameter: 5.0,
            depth: HoleDepth::Blind { depth: 6.0 },
            tip: HoleTip::Cone { angle_deg: angle },
            head: head.map(|angle_deg| HoleHead::Countersink { diameter: 9.0, angle_deg }),
        };
        let r = k.hole(&OpTag::new("blind"), plate, &blind).unwrap();
        assert!(roles(&r).contains(&FaceRole::Hole { placement: 0, part: HolePart::Tip }), "{angle} {head:?}");
        assert!(volume(&k, r.body) < before);
    }
    // The input body is untouched.
    assert!((volume(&k, plate) - before).abs() < 1e-9);
}

#[test]
fn patterns_and_mirrors_copy_the_body_with_copy_roles() {
    let k = CatallaxKernel::new();
    let cube = block(&k, "cube", 10.0, 10.0, 0.0, 10.0);
    let faces: Vec<FaceId> = k.topology(cube).unwrap().faces.iter().map(|f| f.id).collect();
    let row = k.pattern(&OpTag::new("row"), cube, &PatternLayout::Linear { step: Vec3::new(20.0, 0.0, 0.0), count: 3 });
    let row = row.unwrap();
    assert!((volume(&k, row.body) - 3000.0).abs() < 1e-6);
    for o in &row.record.face_roles {
        let FaceRole::Copy { instance, face } = o.role else { panic!("{:?}", o.role) };
        assert!((1..3).contains(&instance) && faces.contains(&face));
    }
    assert_eq!(row.record.face_roles.len(), 12);

    let turned = k
        .pattern(
            &OpTag::new("turn"),
            cube,
            &PatternLayout::Circular {
                axis_origin: Vec3::new(-20.0, 0.0, 0.0),
                axis_direction: Vec3::Z,
                count: 4,
                angle_deg: 90.0,
            },
        )
        .unwrap();
    assert!((volume(&k, turned.body) - 4000.0).abs() < 1e-6);

    let mirrored = k
        .mirror(
            &OpTag::new("mirror"),
            cube,
            &Mirror { plane_origin: Vec3::new(10.0, 0.0, 0.0), plane_normal: Vec3::X, keep_original: true },
        )
        .unwrap();
    assert!((volume(&k, mirrored.body) - 2000.0).abs() < 1e-6);
    assert!(roles(&mirrored).iter().all(|r| matches!(r, FaceRole::Copy { instance: 1, .. })));
    let only = k
        .mirror(
            &OpTag::new("image"),
            cube,
            &Mirror { plane_origin: Vec3::new(10.0, 0.0, 0.0), plane_normal: Vec3::X, keep_original: false },
        )
        .unwrap();
    let centroid = k.mass_properties(only.body).unwrap().centroid;
    assert!((centroid.x - 20.0).abs() < 1e-9);
}

#[test]
fn fillets_round_plane_edges_and_name_their_blends() {
    let k = CatallaxKernel::new();
    let b = block(&k, "block", 40.0, 30.0, 0.0, 10.0);
    let topology = k.topology(b).unwrap();
    let edge = topology
        .edges
        .iter()
        .find(|e| e.curve == CurveKind::Line && e.midpoint.distance(Vec3::new(0.0, 15.0, 10.0)) < 1e-9)
        .expect("the top edge along x at y = 15")
        .id;
    let rounded = k.fillet(&OpTag::new("round"), b, &[edge], 2.0).unwrap();
    assert_eq!(rounded.record.op, OpKind::Fillet);
    assert!(roles(&rounded).contains(&FaceRole::Blend { edge }), "{:?}", roles(&rounded));
    let removed = (1.0 - PI / 4.0) * 4.0 * 40.0;
    assert!((volume(&k, b) - volume(&k, rounded.body) - removed).abs() < 1e-6);
    assert!(k.topology(rounded.body).unwrap().faces.iter().any(|f| f.surface == SurfaceKind::Cylinder));
    assert!(matches!(k.fillet(&OpTag::new("none"), b, &[], 2.0), Err(KernelError::InvalidInput(_))));
}

#[test]
fn chamfers_bevel_plane_edges_and_circular_rims() {
    let k = CatallaxKernel::new();
    let b = block(&k, "block", 40.0, 30.0, 0.0, 10.0);
    let edge = k
        .topology(b)
        .unwrap()
        .edges
        .iter()
        .find(|e| e.curve == CurveKind::Line && e.midpoint.distance(Vec3::new(0.0, 15.0, 10.0)) < 1e-9)
        .expect("the top edge along x at y = 15")
        .id;
    let bevelled = k.chamfer(&OpTag::new("bevel"), b, &[edge], 2.0).unwrap();
    assert_eq!(bevelled.record.op, OpKind::Chamfer);
    assert!(roles(&bevelled).contains(&FaceRole::Blend { edge }), "{:?}", roles(&bevelled));
    assert!((volume(&k, b) - volume(&k, bevelled.body) - 0.5 * 4.0 * 40.0).abs() < 1e-6);
    assert!(k.validate(bevelled.body).unwrap().valid);

    // The P2 case: a plane meeting a cylinder along a ring makes a cone.
    let cylinder = contract::cylinder(&k, Vec3::ZERO, 12.0, 12.0);
    let rim = k
        .topology(cylinder)
        .unwrap()
        .edges
        .iter()
        .find(|e| e.curve == CurveKind::Circle && e.midpoint.z > 6.0)
        .expect("the top rim")
        .id;
    let ring = k.chamfer(&OpTag::new("rim"), cylinder, &[rim], 1.0).unwrap();
    // A 1 x 1 triangle swept around a centroid circle of radius 12 - 1/3.
    let removed = 0.5 * 2.0 * PI * (12.0 - 1.0 / 3.0);
    assert!((volume(&k, cylinder) - volume(&k, ring.body) - removed).abs() < 1e-6);
    assert!(k.topology(ring.body).unwrap().faces.iter().any(|f| f.surface == SurfaceKind::Cone));
    assert!(matches!(k.chamfer(&OpTag::new("none"), b, &[], 1.0), Err(KernelError::InvalidInput(_))));
}

#[test]
fn drafts_tilt_planar_faces_about_the_neutral_plane() {
    let k = CatallaxKernel::new();
    let b = block(&k, "block", 40.0, 30.0, 0.0, 20.0);
    let topology = k.topology(b).unwrap();
    let side = topology.faces.iter().find(|f| f.normal.is_some_and(|n| n.distance(Vec3::X) < 1e-12)).unwrap().id;
    let params = Draft { pull: Vec3::Z, neutral_origin: Vec3::ZERO, neutral_normal: Vec3::Z, angle_deg: 5.0 };
    let drafted = k.draft(&OpTag::new("draft"), b, &[side], &params).unwrap();
    assert_eq!(drafted.record.op, OpKind::Draft);
    // The tilted face keeps its place at z = 0 and leans by 20 tan 5° at the top.
    let wedge = 0.5 * 20.0 * 20.0 * (5.0f64).to_radians().tan() * 30.0;
    assert!(((volume(&k, b) - volume(&k, drafted.body)).abs() - wedge).abs() < 1e-6);
    let history = k.history(drafted.body).unwrap();
    let record = history.last().unwrap();
    assert!(record.faces.split.iter().any(|s| s.from == side), "the drafted face descends from the side");
    assert!(k.validate(drafted.body).unwrap().valid);
}

#[test]
fn a_hole_on_a_tilted_face_keeps_its_exact_centre_and_direction() {
    let k = CatallaxKernel::new();
    // A 40 by 30 rectangle along the 3-4-5 direction (0.8, 0.6), with exact corners.
    let corners = [(0.0, 0.0), (32.0, 24.0), (14.0, 48.0), (-18.0, 24.0)].map(|(u, v)| Point2::new(u, v));
    let outer = (0..4).map(|i| Segment::Line { from: corners[i], to: corners[(i + 1) % 4] }).collect();
    let profile = Profile { plane: Plane::XY, outer: Loop(outer), holes: vec![] };
    let sheet = k.create_body_from_profile(&OpTag::new("p"), &profile).unwrap().body;
    let body = k.extrude(&OpTag::new("b"), sheet, &Extrude { distance: 30.0, direction: None }).unwrap().body;
    let topology = k.topology(body).unwrap();
    let outward = Vec3::new(-0.6, 0.8, 0.0);
    let face = topology.faces.iter().find(|f| f.normal.is_some_and(|n| n.distance(outward) < 1e-12)).unwrap();
    // On this wall the frame's origin is (-18, 24, 0), X is (-0.8, -0.6, 0) and Y is Z, so
    // the local point (-20, 15) is the wall's centre (-2, 36, 15).
    let frame = soliddraft_model::plane_on_face(face.normal.unwrap(), face.centroid);
    let (u, v) = (-20.0, 15.0);
    let placement = HolePlacement {
        point: frame.origin + frame.x_dir * u + frame.y_dir() * v,
        direction: -frame.normal,
        on_face: Some(FacePoint { face: face.id, u, v }),
    };
    assert!(placement.point.distance(Vec3::new(-2.0, 36.0, 15.0)) < 1e-9, "the frames agree");
    let params = Hole {
        placements: vec![placement],
        diameter: 4.0,
        depth: HoleDepth::Blind { depth: 2.0 },
        tip: HoleTip::Flat,
        head: None,
    };
    let drilled = k.hole(&OpTag::new("hole"), body, &params).unwrap().body;
    let recipe = k.body_recipe(drilled).unwrap();
    let steps = recipe["steps"].as_array().unwrap();
    let hole = steps.iter().find(|s| s["native"]["op"] == "hole").unwrap();
    assert_eq!(hole["native"]["at"][0]["point"], serde_json::json!(["-2", "36", "15"]));
    assert_eq!(hole["native"]["at"][0]["direction"], serde_json::json!(["0.6", "-0.8", "0"]));
    assert!(k.validate(drilled).unwrap().valid);
}

#[test]
fn a_drafted_face_reports_its_own_plane() {
    let k = CatallaxKernel::new();
    let b = block(&k, "block", 40.0, 30.0, 0.0, 20.0);
    let topology = k.topology(b).unwrap();
    let side = topology.faces.iter().find(|f| f.normal.is_some_and(|n| n.distance(Vec3::X) < 1e-12)).unwrap().id;
    for angle_deg in [3.0f64, 5.0] {
        let params = Draft { pull: Vec3::Z, neutral_origin: Vec3::ZERO, neutral_normal: Vec3::Z, angle_deg };
        let tag = OpTag::new(format!("draft-{angle_deg}"));
        let drafted = k.draft(&tag, b, &[side], &params).unwrap().body;
        let (sin, cos) = angle_deg.to_radians().sin_cos();
        let topology = k.topology(drafted).unwrap();
        let face = topology.faces.iter().find(|f| f.normal.is_some_and(|n| n.x > 0.5)).unwrap();
        assert!(face.normal.unwrap().distance(Vec3::new(cos, 0.0, sin)) < 1e-12, "{:?}", face.normal);
        // The wall leans in from x = 20 at z = 0, so its centre is 10 tan a in at z = 10.
        let centre = Vec3::new(20.0 - 10.0 * sin / cos, 0.0, 10.0);
        assert!(face.centroid.distance(centre) < 1e-9, "{:?}", face.centroid);
    }
}

#[test]
fn unchanged_drafted_entities_keep_their_ids_after_their_roots_are_refined() {
    let k = CatallaxKernel::new();
    let b = block(&k, "block", 40.0, 30.0, 0.0, 20.0);
    let sides: Vec<_> = k
        .topology(b)
        .unwrap()
        .faces
        .iter()
        .filter(|f| f.normal.is_some_and(|n| n.distance(Vec3::X) < 1e-12 || n.distance(-Vec3::X) < 1e-12))
        .map(|f| f.id)
        .collect();
    let params = Draft { pull: Vec3::Z, neutral_origin: Vec3::ZERO, neutral_normal: Vec3::Z, angle_deg: 3.0 };
    let drafted = k.draft(&OpTag::new("draft"), b, &sides, &params).unwrap().body;
    let before = k.topology(drafted).unwrap();
    // A measurement and a finer mesh refine the drafted corners' exact roots between
    // the parent's build and the child's, as background display refinement can.
    k.measured_topology(drafted).unwrap();
    k.tessellate(drafted, &TessellationParams { chord_height: 0.01, ..TessellationParams::default() }).unwrap();
    let circle = Profile::circle(Plane::XY.with_origin(Vec3::new(0.0, 0.0, 20.0)), Point2::new(0.0, 0.0), 5.0);
    let sheet = k.create_body_from_profile(&OpTag::new("boss"), &circle).unwrap().body;
    let boss = k.extrude(&OpTag::new("boss"), sheet, &Extrude { distance: 5.0, direction: None }).unwrap().body;
    let joined = k.boolean(&OpTag::new("join"), BooleanOp::Union, drafted, boss).unwrap();
    let after = k.topology(joined.body).unwrap();
    // The boss only touches the top, so everything below it is the parent's.
    let below = |z: f64| z < 20.0 - 1e-9;
    for face in before.faces.iter().filter(|f| below(f.centroid.z)) {
        assert!(after.faces.iter().any(|g| g.id == face.id), "face {:?} at {:?} lost its id", face.id, face.centroid);
    }
    for edge in before.edges.iter().filter(|e| below(e.midpoint.z)) {
        assert!(after.edges.iter().any(|g| g.id == edge.id), "edge {:?} at {:?} lost its id", edge.id, edge.midpoint);
    }
    for vertex in before.vertices.iter().filter(|v| below(v.position.z)) {
        assert!(after.vertices.iter().any(|g| g.id == vertex.id), "vertex {:?} lost its id", vertex.position);
    }
    let changed = &joined.record.faces;
    let gone: Vec<_> = changed.deleted.iter().chain(changed.split.iter().map(|s| &s.from)).collect();
    assert!(before.faces.iter().filter(|f| below(f.centroid.z)).all(|f| !gone.contains(&&f.id)), "{changed:?}");
}

#[test]
fn extrusions_stop_at_the_next_boundary_or_a_face() {
    let k = CatallaxKernel::new();
    let plate = block(&k, "plate", 60.0, 40.0, 0.0, 5.0);
    let shelf = block(&k, "shelf", 60.0, 40.0, 35.0, 5.0);
    let part = k.boolean(&OpTag::new("part"), BooleanOp::Union, plate, shelf).unwrap().body;
    let circle = Profile::circle(Plane::XY.with_origin(Vec3::new(0.0, 0.0, 5.0)), Point2::new(0.0, 0.0), 6.0);
    let sheet = k.create_body_from_profile(&OpTag::new("post"), &circle).unwrap().body;
    let next = k.extrude_to(&OpTag::new("next"), sheet, part, None, &ExtrudeLimit::UpToNext).unwrap();
    assert_eq!(next.record.op, OpKind::ExtrudeTo);
    assert!((volume(&k, next.body) - PI * 36.0 * 30.0).abs() < 1e-6);
    let underside = k
        .topology(part)
        .unwrap()
        .faces
        .iter()
        .find(|f| f.normal.is_some_and(|n| n.distance(-Vec3::Z) < 1e-12) && (f.centroid.z - 35.0).abs() < 1e-9)
        .unwrap()
        .id;
    let to = k.extrude_to(&OpTag::new("to"), sheet, part, None, &ExtrudeLimit::UpToFace { face: underside }).unwrap();
    assert!((volume(&k, to.body) - PI * 36.0 * 30.0).abs() < 1e-6);
    // The swept solid's history does not claim the target's faces.
    assert!(to.record.faces.deleted.is_empty());
}

#[test]
fn measured_topology_replaces_display_estimates() {
    let k = CatallaxKernel::new();
    let b = block(&k, "block", 40.0, 30.0, 0.0, 10.0);
    // Ordinary topology estimates from the display, so rebuilds never wait for a proof.
    let estimated = k.topology(b).unwrap();
    assert!(estimated.display_chord_height.is_some());
    // A planar face's estimated centroid still lies on its exact plane, where a sketch
    // on that face is placed.
    for f in estimated.faces.iter().filter(|f| f.normal.is_some_and(|n| n.distance(Vec3::Z) < 1e-12)) {
        assert_eq!(f.centroid.z, 10.0);
    }
    let topology = k.measured_topology(b).unwrap();
    assert!(topology.display_chord_height.is_none());
    assert!(topology.bbox.min.distance(Vec3::new(-20.0, -15.0, 0.0)) < 1e-9);
    assert!(topology.bbox.max.distance(Vec3::new(20.0, 15.0, 10.0)) < 1e-9);
    let top = topology.faces.iter().find(|f| f.normal.is_some_and(|n| n.distance(Vec3::Z) < 1e-12)).unwrap();
    assert!(top.centroid.distance(Vec3::new(0.0, 0.0, 10.0)) < 1e-9);
    for e in &topology.edges {
        assert!([40.0, 30.0, 10.0].iter().any(|l| (e.length - l).abs() < 1e-9), "{}", e.length);
    }
    let m = k.mass_properties(b).unwrap();
    let inertia = m.inertia.expect("Catallax proves inertia");
    let v = 40.0 * 30.0 * 10.0;
    assert!((inertia[0][0] - v * (30.0f64.powi(2) + 10.0f64.powi(2)) / 12.0).abs() < 1e-3);
    assert!((inertia[2][2] - v * (40.0f64.powi(2) + 30.0f64.powi(2)) / 12.0).abs() < 1e-3);
    assert!(inertia[0][1].abs() < 1e-6);

    let cylinder = contract::cylinder(&k, Vec3::ZERO, 10.0, 20.0);
    let rim = k.measured_topology(cylinder).unwrap();
    let circle = rim.edges.iter().find(|e| e.curve == CurveKind::Circle).unwrap();
    assert!((circle.length - 2.0 * PI * 10.0).abs() < 1e-9);
    let params = TessellationParams {
        chord_height: 0.05,
        curve_angle_deg: 0.0,
        surface_angle_deg: 5.0,
        max_edge_length: 4.0,
        max_aspect_ratio: 0.0,
    };
    let mesh = k.tessellate(cylinder, &params).unwrap();
    assert!(mesh.unavailable_controls.iter().all(|c| !c.starts_with("curve_angle_deg")));
    let loose = k.tessellate(cylinder, &TessellationParams { chord_height: 0.05, ..TessellationParams::default() });
    assert!(mesh.triangle_count() > loose.unwrap().triangle_count());
}
