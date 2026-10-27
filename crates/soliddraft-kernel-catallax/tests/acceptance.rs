//! Catallax acceptance through the same trait and recipe runner the application uses.

use soliddraft_kernel_api::*;
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_recipe::{Command, StepOutcome, build, parse, replay, summarize};

const BRACKET: &str = include_str!("../../../recipes/bracket.json");

#[test]
fn phase_zero_repeatability_and_failure_preserves_the_previous_body() {
    for text in [
        include_str!("../../../recipes/box.json"),
        include_str!("../../../recipes/cylinder.json"),
        include_str!("../../../recipes/union.json"),
        include_str!("../../../recipes/subtract.json"),
        include_str!("../../../recipes/failing-fillet.json"),
    ] {
        let recipe = parse(text).unwrap();
        let first = replay(&CatallaxKernel::new(), &recipe).unwrap();
        for _ in 0..2 {
            assert_eq!(first, replay(&CatallaxKernel::new(), &recipe).unwrap());
        }
        for step in &first.steps {
            if let StepOutcome::Profile { profile } = &step.outcome {
                assert!(profile.closed);
                assert!(profile.loops > 0);
                assert_eq!(profile.tags, [OpTag::new(&step.id)]);
            }
        }
        if recipe.name == "failing-fillet" {
            assert!(matches!(first.steps.last().unwrap().outcome, StepOutcome::ExpectedError { .. }));
            let plain =
                replay(&CatallaxKernel::new(), &parse(include_str!("../../../recipes/box.json")).unwrap()).unwrap();
            assert_eq!(first.result, plain.result);
        }
    }
}

fn descendants(history: &[HistoryRecord], start: FaceId) -> Vec<FaceId> {
    let mut faces = vec![start];
    for record in history {
        for split in &record.faces.split {
            if faces.contains(&split.from) {
                faces.retain(|f| *f != split.from);
                faces.extend(&split.into);
            }
        }
        faces.retain(|f| !record.faces.deleted.contains(f));
    }
    faces
}

#[test]
fn bracket_builds_displays_picks_named_faces_and_edits_through_the_trait() {
    let mut recipe = parse(BRACKET).unwrap();
    let kernel = CatallaxKernel::new();
    let mut volumes = Vec::new();
    for height in [60.0, 72.0] {
        let Command::Extrude { distance, .. } =
            &mut recipe.steps.iter_mut().find(|s| s.id == "flange").unwrap().command
        else {
            panic!()
        };
        *distance = height;
        let (report, body) = build(&kernel, &recipe).unwrap();
        volumes.push(report.result.volume);
        assert!(report.result.valid);
        let topology = kernel.topology(body).unwrap();
        assert!(topology.display_chord_height.is_some());
        let mesh = kernel.tessellate(body, &TessellationParams::default()).unwrap();
        assert!(mesh.triangle_count() > 0);
        assert!(mesh.triangle_faces.iter().all(|f| topology.face(*f).is_some()));
        assert!(mesh.normals.iter().all(|n| (Vec3::from(n.map(f64::from)).length() - 1.0).abs() < 1e-5));
        assert!(!kernel.tessellate_edges(body, &TessellationParams::default()).unwrap().is_empty());
        let history = kernel.history(body).unwrap();
        for maker in [
            "plate",
            "flange",
            "boss",
            "bore_tool",
            "hole1_tool",
            "hole2_tool",
            "hole3_tool",
            "hole4_tool",
            "slot_tool",
        ] {
            let record = history.iter().find(|r| r.tag.as_str() == maker).unwrap();
            let named: Vec<_> = record.faces.created.iter().flat_map(|f| descendants(&history, *f)).collect();
            assert!(mesh.triangle_faces.iter().any(|f| named.contains(f)), "{maker} has no named pickable triangle");
        }
        let flange = history.iter().find(|r| r.tag.as_str() == "flange").unwrap();
        let cap = flange.face_roles.iter().find(|f| f.role == FaceRole::EndCap).unwrap().face;
        assert!(
            descendants(&history, cap)
                .iter()
                .any(|f| topology.face(*f).is_some_and(|f| (f.centroid.z - height).abs() < 1e-8))
        );
        assert!(kernel.mass_properties(body).unwrap().inertia.is_some());
        let before = summarize(&kernel, body).unwrap();
        assert!(matches!(kernel.fillet(&OpTag::new("finish"), body, &[], 1.0), Err(KernelError::InvalidInput(_))));
        assert_eq!(before, summarize(&kernel, body).unwrap());
        kernel.release(body).unwrap();
        assert!(matches!(kernel.topology(body), Err(KernelError::UnknownBody(_))));
    }
    assert!((volumes[1] - volumes[0] - 10800.0).abs() < 1e-4);
}

#[test]
fn faces_report_catallax_origins_through_merges_and_splits() {
    contract::faces_keep_their_origins(&CatallaxKernel::new());
}

#[test]
fn supported_queries_keep_the_existing_contract() {
    let k = CatallaxKernel::new();
    contract::mass_properties_match_closed_form(&k);
    let body = contract::cylinder(&k, Vec3::ZERO, 10.0, 20.0);
    let levels: Vec<_> =
        [0.1, 0.02, 0.005].map(|chord_height| TessellationParams { chord_height, ..Default::default() }).into();
    let meshes = k.tessellate_levels(body, &levels).unwrap();
    for (mesh, params) in meshes.iter().zip(&levels) {
        assert_eq!(*mesh, k.tessellate(body, params).unwrap());
    }
    assert!(meshes[0].triangle_count() < meshes[2].triangle_count());
    let topology = k.topology(body).unwrap();
    assert!(meshes.iter().all(|m| m.triangle_faces.iter().all(|f| topology.face(*f).is_some())));
}

#[test]
fn refused_extrusion_does_not_change_its_profile_and_handles_stay_owned() {
    let k = CatallaxKernel::new();
    let p = k
        .create_body_from_profile(&OpTag::new("profile"), &Profile::circle(Plane::XY, Point2::new(0.0, 0.0), 2.0))
        .unwrap()
        .body;
    let history = k.history(p).unwrap();
    assert!(k.extrude(&OpTag::new("invalid"), p, &Extrude { distance: 0.0, direction: None }).is_err());
    assert_eq!(k.history(p).unwrap(), history);
    let b = k.extrude(&OpTag::new("valid"), p, &Extrude { distance: 5.0, direction: None }).unwrap().body;
    k.release(p).unwrap();
    let before = summarize(&k, b).unwrap();
    assert!(
        matches!(k.boolean(&OpTag::new("empty"), BooleanOp::Subtract, b, b), Err(KernelError::Unsupported(message)) if message.contains("empty results"))
    );
    assert_eq!(before, summarize(&k, b).unwrap());
    assert!(k.validate(b).unwrap().valid);
    assert!(matches!(k.chamfer(&OpTag::new("bevel"), b, &[], 1.0), Err(KernelError::InvalidInput(_))));
    assert!(
        matches!(k.shell(&OpTag::new("shell"), b, &[], 1.0), Err(KernelError::Unsupported(s)) if s.contains("stage 3"))
    );
}

#[test]
fn polygon_views_and_boolean_sections_keep_sources_on_the_original_body() {
    use soliddraft_kernel_api::drawing::{Curve3, CurveSource, StandardView, ViewSpec};

    let k = CatallaxKernel::new();
    let (_, body) = build(&k, &parse(BRACKET).unwrap()).unwrap();
    let topology = k.topology(body).unwrap();
    let before = summarize(&k, body).unwrap();
    for direction in [StandardView::Front, StandardView::Top, StandardView::Right, StandardView::Isometric] {
        let spec = ViewSpec::standard(direction);
        let view = k.view_curves(body, &spec).unwrap();
        let again = k.view_curves(body, &spec).unwrap();
        assert_eq!(view, again, "polygon output must be deterministic");
        assert!(view.counts().0 > 0);
        assert!(view.curves.iter().all(|c| match c.source {
            CurveSource::Edge(e) => topology.edge(e).is_some(),
            CurveSource::Silhouette(f) | CurveSource::Seam(f) => topology.face(f).is_some(),
            _ => false,
        }));
        if direction == StandardView::Top {
            assert!(
                view.curves
                    .iter()
                    .any(|c| matches!(c.curve, Curve3::Circle { radius, .. } if (radius - 6.0).abs() < 1e-8)),
                "the bore remains dimensionable"
            );
        }
        if direction == StandardView::Isometric {
            assert!(
                view.curves.iter().any(|c| matches!(c.source, CurveSource::Silhouette(_))),
                "shared triangle vertices must produce silhouettes"
            );
        }
    }
    let section = k.section_view(body, &contract::bracket_section_spec()).unwrap();
    assert_eq!(section.region_areas(&contract::bracket_section_spec().view.frame().unwrap()), [910.0, 560.0, 230.0]);
    for curve in section
        .curves
        .curves
        .iter()
        .chain(section.regions.iter().flat_map(|r| r.outer.iter().chain(r.holes.iter().flatten())))
    {
        assert!(
            match curve.source {
                CurveSource::Edge(e) => topology.edge(e).is_some(),
                CurveSource::Silhouette(f) | CurveSource::Seam(f) | CurveSource::Section(f) =>
                    topology.face(f).is_some(),
                CurveSource::Unknown => false,
            },
            "section curve lost its original source: {:?}",
            curve.source
        );
    }
    assert_eq!(summarize(&k, body).unwrap(), before, "a section must not change the part");
    k.release(body).unwrap();
    contract::box_section_view_hatches_the_cut(&k);
    contract::section_planes_off_the_body(&k);
}

#[test]
fn preview_sections_are_cut_from_the_mesh_and_match_the_boolean_ones() {
    use soliddraft_kernel_api::drawing::{SectionSpec, ViewSpec};

    let k = CatallaxKernel::with_operation_log();
    contract::mesh_sections_match_boolean_sections(&k);
    // The backend routes a preview section to the mesh slice: no boolean runs inside it.
    let (_, body) = build(&k, &parse(BRACKET).unwrap()).unwrap();
    k.take_diagnostics().unwrap();
    let spec = contract::bracket_section_spec();
    let preview = SectionSpec { view: ViewSpec { preview: true, ..spec.view }, ..spec };
    let section = k.section_view(body, &preview).unwrap();
    assert!(section.curves.unproved);
    let calls = k.take_diagnostics().unwrap();
    let calls = calls["calls"].as_array().unwrap();
    assert!(calls.iter().any(|c| c["operation"] == "section_view" && c["nested"] == false), "{calls:?}");
    assert!(calls.iter().all(|c| c["operation"] != "boolean"), "a preview section ran a boolean: {calls:?}");
    let certified = k.section_view(body, &spec).unwrap();
    assert!(!certified.curves.unproved);
    let calls = k.take_diagnostics().unwrap();
    assert!(calls["calls"].as_array().unwrap().iter().any(|c| c["operation"] == "boolean" && c["nested"] == true));
    k.release(body).unwrap();
}

#[test]
fn polygon_face_loops_keep_holes_and_coedge_order() {
    use soliddraft_kernel_api::drawing::{SectionSpec, StandardView, ViewSpec};

    let k = CatallaxKernel::new();
    let outer = contract::cylinder(&k, Vec3::ZERO, 10.0, 20.0);
    let inner = contract::cylinder(&k, Vec3::new(0.0, 0.0, -1.0), 4.0, 22.0);
    let body = k.boolean(&OpTag::new("bore"), BooleanOp::Subtract, outer, inner).unwrap().body;
    let spec = SectionSpec { view: ViewSpec::standard(StandardView::Top), origin: Vec3::new(0.0, 0.0, 10.0) };
    let section = k.section_view(body, &spec).unwrap();
    assert_eq!(section.regions.len(), 1);
    assert_eq!(section.regions[0].holes.len(), 1);
    let area = section.regions[0].area(&spec.view.frame().unwrap());
    assert!((area - std::f64::consts::PI * 84.0).abs() < 0.2, "annular area {area}");
    for boundary in std::iter::once(&section.regions[0].outer).chain(&section.regions[0].holes) {
        for i in 0..boundary.len() {
            assert!(boundary[i].curve.end().distance(boundary[(i + 1) % boundary.len()].curve.start()) < 1e-8);
        }
    }
    for id in [body, outer, inner] {
        k.release(id).unwrap();
    }
}

#[test]
fn axis_aligned_front_extrusions_keep_the_exact_frame_in_both_directions() {
    let k = CatallaxKernel::with_operation_log();
    let profile =
        Profile::circle(Plane { origin: Vec3::ZERO, normal: -Vec3::Y, x_dir: Vec3::X }, Point2::new(0.0, 0.0), 3.0);
    let sheet = k.create_body_from_profile(&OpTag::new("front"), &profile).unwrap();
    for direction in [-Vec3::Y, Vec3::Y] {
        let body = k
            .extrude(&OpTag::new("pin"), sheet.body, &Extrude { distance: 20.0, direction: Some(direction) })
            .unwrap()
            .body;
        let mass = k.mass_properties(body).unwrap();
        assert!((mass.volume - 180.0 * std::f64::consts::PI).abs() < 1e-4);
        assert!(mass.centroid.distance(direction * 10.0) < 1e-8);
        let topology = k.topology(body).unwrap();
        let normals: Vec<_> = topology.faces.iter().filter_map(|f| f.normal).collect();
        assert!(normals.contains(&Vec3::Y) && normals.contains(&-Vec3::Y), "{normals:?}");
        let evidence = k.take_diagnostics().unwrap();
        let questions = evidence["questions"].as_array().unwrap();
        let recipe: catallax_recipe::Recipe =
            serde_json::from_value(questions.last().unwrap()["problem"].clone()).unwrap();
        catallax_app::build(&recipe, &catallax_mesh::MeshOptions::default(), &mut catallax_numbers::Decider::default())
            .expect("captured question must replay the exact frame used by the application");
        k.release(body).unwrap();
    }
    k.release(sheet.body).unwrap();
}

#[test]
fn face_frame_normal_sweeps_use_the_certified_frame_without_changing_other_directions() {
    let k = CatallaxKernel::with_operation_log();
    let plane = Plane {
        origin: Vec3::new(-3.5164312091753774e-36, -2.1476647763447118e-17, 0.0),
        normal: Vec3::new(-1.6373277840689282e-19, -1.0, 0.0),
        x_dir: Vec3::new(1.0, -1.6373277840689282e-19, 0.0),
    };
    let profile = Profile::circle(plane, Point2::new(0.0, 0.0), 3.0);
    let sheet = k.create_body_from_profile(&OpTag::new("front_face"), &profile).unwrap().body;
    for direction in [plane.normal, -plane.normal] {
        let body =
            k.extrude(&OpTag::new("pin"), sheet, &Extrude { distance: 20.0, direction: Some(direction) }).unwrap().body;
        let mass = k.mass_properties(body).unwrap();
        assert!((mass.volume - 180.0 * std::f64::consts::PI).abs() < 1e-4);
        assert!(mass.centroid.distance(plane.origin + direction * 10.0) < 1e-8);
        let evidence = k.take_diagnostics().unwrap();
        let recipe: catallax_recipe::Recipe =
            serde_json::from_value(evidence["questions"].as_array().unwrap().last().unwrap()["problem"].clone())
                .unwrap();
        catallax_app::build(&recipe, &catallax_mesh::MeshOptions::default(), &mut catallax_numbers::Decider::default())
            .expect("the captured normal sweep must replay");
        k.release(body).unwrap();
    }
    // A direction independent of the face frame keeps its exact components. Catallax
    // builds it since its exact algebraic extrusion directions.
    let body = k
        .extrude(
            &OpTag::new("independent_direction"),
            sheet,
            &Extrude { distance: 20.0, direction: Some(Vec3::new(2e-19, 1.0, 0.0)) },
        )
        .unwrap()
        .body;
    assert!((k.mass_properties(body).unwrap().volume - 180.0 * std::f64::consts::PI).abs() < 1e-4);
    k.release(body).unwrap();
    let evidence = k.take_diagnostics().unwrap();
    let question = evidence["questions"].as_array().unwrap().last().unwrap();
    let recipe: catallax_recipe::Recipe = serde_json::from_value(question["problem"].clone()).unwrap();
    let catallax_recipe::Operation::Extrude { direction: Some(direction), .. } = &recipe.steps.last().unwrap().op
    else {
        panic!()
    };
    assert_eq!(direction.0[0], catallax_recipe::Number::parse("2e-19").unwrap());
    k.release(sheet).unwrap();
}

#[test]
fn certified_application_display_serves_matching_requests_and_preserves_finer_meshing() {
    let k = CatallaxKernel::with_operation_log();
    let body = contract::cylinder(&k, Vec3::ZERO, 3.0, 5.0);
    k.take_diagnostics().unwrap();
    let topology = k.topology(body).unwrap();
    assert_eq!(topology.display_chord_height, Some(0.01));
    // Ordinary topology proves face areas only, and never asks for a new display.
    let log = k.take_diagnostics().unwrap();
    assert!(log["calls"].as_array().unwrap().iter().all(|call| {
        call["operation_log"]["operations"].as_array().unwrap().iter().all(|op| op["operation"] == "face_area")
    }));

    // Half the requested chord is reserved for the unchanged f32 rounding check. Without
    // angle controls the request matches the build's own display.
    let params =
        TessellationParams { chord_height: 0.02, curve_angle_deg: 0.0, surface_angle_deg: 0.0, ..Default::default() };
    let original = k.tessellate(body, &params).unwrap();
    let edges = k.tessellate_edges(body, &params).unwrap();
    let log = k.take_diagnostics().unwrap();
    assert_eq!(log["calls"].as_array().unwrap().len(), 2);
    assert!(
        log["calls"]
            .as_array()
            .unwrap()
            .iter()
            .all(|call| call["operation_log"]["operations"].as_array().unwrap().is_empty())
    );

    let finer = TessellationParams { chord_height: 0.005, ..params };
    let refined = k.tessellate(body, &finer).unwrap();
    assert!(refined.triangle_count() > original.triangle_count());
    let log = k.take_diagnostics().unwrap();
    assert_eq!(log["calls"][0]["operation_log"]["operations"][0]["operation"], "tessellate");

    assert_eq!(k.tessellate(body, &params).unwrap(), original);
    assert_eq!(k.tessellate_edges(body, &params).unwrap(), edges);
    assert_eq!(k.topology(body).unwrap(), topology);
    let log = k.take_diagnostics().unwrap();
    assert_eq!(log["calls"].as_array().unwrap().len(), 3);
    assert!(
        log["calls"]
            .as_array()
            .unwrap()
            .iter()
            .all(|call| call["operation_log"]["operations"].as_array().unwrap().is_empty())
    );
    k.release(body).unwrap();
}

#[test]
fn previews_are_labelled_unproved_and_never_replace_the_certified_mesh() {
    let k = CatallaxKernel::with_operation_log();
    let body = contract::cylinder(&k, Vec3::ZERO, 3.0, 5.0);
    let topology = k.topology(body).unwrap();
    let params = TessellationParams { chord_height: 0.002, ..Default::default() };
    k.take_diagnostics().unwrap();

    // The screen's mesh: faces and edges named like the body's, and no certificate.
    let preview = k.preview(body, &params).unwrap();
    let edges = k.preview_edges(body, &params).unwrap();
    assert!(preview.unproved);
    assert!(preview.triangle_count() > 0);
    assert_eq!(preview.positions.len(), preview.normals.len());
    assert!(preview.triangle_faces.iter().all(|f| topology.face(*f).is_some()));
    assert!(topology.faces.iter().all(|f| preview.triangle_faces.contains(&f.id)), "every face is pickable");
    assert_eq!(edges.len(), topology.edges.len());
    let log = k.take_diagnostics().unwrap();
    let operations: Vec<_> = log["calls"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|call| {
            call["operation_log"]["operations"].as_array().unwrap().iter().map(|op| op["operation"].clone())
        })
        .collect();
    assert_eq!(operations, ["preview"], "one preview serves the mesh and its edges, with no certified meshing");

    // Exports and proven results still get the certified mesh.
    let certified = k.tessellate(body, &params).unwrap();
    assert!(!certified.unproved);
    assert_eq!(k.preview(body, &params).unwrap(), preview);
    let log = k.take_diagnostics().unwrap();
    assert_eq!(log["calls"][0]["operation_log"]["operations"][0]["operation"], "tessellate");
    assert!(log["calls"][1]["operation_log"]["operations"].as_array().unwrap().is_empty(), "the preview is kept");
    k.release(body).unwrap();
}

#[test]
fn alternating_display_levels_reuse_their_exact_meshes_and_edges() {
    let k = CatallaxKernel::with_operation_log();
    let body = contract::cylinder(&k, Vec3::ZERO, 3.0, 5.0);
    let params =
        [0.16, 0.04, 0.01, 0.002].map(|chord_height| TessellationParams { chord_height, ..Default::default() });
    let meshes = params.map(|p| k.tessellate(body, &p).unwrap());
    let edges = params.map(|p| k.tessellate_edges(body, &p).unwrap());
    k.take_diagnostics().unwrap();

    // Viewport levels, file display data and drawing queries alternate on one body.
    // A return to any kept tolerance must use the same approximation without meshing.
    for _ in 0..2 {
        for i in 0..params.len() {
            assert_eq!(k.tessellate(body, &params[i]).unwrap(), meshes[i]);
            assert_eq!(k.tessellate_edges(body, &params[i]).unwrap(), edges[i]);
        }
    }
    let log = k.take_diagnostics().unwrap();
    for call in log["calls"].as_array().unwrap() {
        assert!(
            call["operation_log"]["operations"].as_array().unwrap().is_empty(),
            "an unchanged body was remeshed after switching tolerances: {call}"
        );
    }
    k.release(body).unwrap();
}

#[test]
fn captured_manual_slot_question_builds_without_changing_its_geometry() {
    let question: serde_json::Value = serde_json::from_str(include_str!("fixtures/slot-native-question.json")).unwrap();
    let recipe: catallax_recipe::Recipe = serde_json::from_value(question["problem"].clone()).unwrap();
    let body =
        catallax_app::build(&recipe, &catallax_mesh::MeshOptions::default(), &mut catallax_numbers::Decider::default())
            .expect("the captured manual slot must build with its original squared radii");
    assert!(body.built.model.check(body.built.result).is_empty());
    assert!(!body.display.value.triangles.is_empty());
}

#[test]
fn bracket_step_export_reports_its_proven_deviation() {
    contract::step_export_reports_what_it_wrote(&CatallaxKernel::new());
}

fn longest_edges(mesh: &Mesh, polylines: &[EdgePolyline]) -> (f64, f64) {
    let position = |i: u32| mesh.positions[i as usize].map(f64::from);
    let distance = |a: [f64; 3], b: [f64; 3]| (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt();
    let triangle = mesh
        .indices
        .chunks(3)
        .flat_map(|t| (0..3).map(|i| distance(position(t[i]), position(t[(i + 1) % 3]))))
        .fold(0.0, f64::max);
    let polyline = polylines
        .iter()
        .flat_map(|e| e.points.windows(2).map(|w| distance(w[0].into(), w[1].into())))
        .fold(0.0, f64::max);
    (triangle, polyline)
}

#[test]
fn maximum_edge_length_bounds_returned_triangles_and_polylines_across_request_orders() {
    let limit = 1.0;
    let chord = TessellationParams { chord_height: 0.02, ..Default::default() };
    let bounded = TessellationParams { max_edge_length: limit, ..chord };
    let looser = TessellationParams { max_edge_length: 2.0, ..chord };
    // Cold and warm requests in every order must agree with a body used for one request.
    let orders: [&[&TessellationParams]; 3] =
        [&[&chord, &bounded, &looser, &bounded, &chord], &[&bounded, &chord, &bounded], &[&looser, &bounded, &chord]];
    let reference = {
        let k = CatallaxKernel::new();
        let body = contract::cylinder(&k, Vec3::ZERO, 3.0, 5.0);
        (
            k.tessellate(body, &chord).unwrap(),
            k.tessellate(body, &bounded).unwrap(),
            k.tessellate_edges(body, &bounded).unwrap(),
        )
    };
    let (plain, expected, expected_edges) = reference;
    let (plain_longest, _) = longest_edges(&plain, &[]);
    assert!(plain_longest > limit, "the chord-only mesh must be coarser than the bound");
    assert!(expected.triangle_count() > plain.triangle_count());
    for order in orders {
        let k = CatallaxKernel::new();
        let body = contract::cylinder(&k, Vec3::ZERO, 3.0, 5.0);
        for params in order {
            let mesh = k.tessellate(body, params).unwrap();
            let edges = k.tessellate_edges(body, params).unwrap();
            if params.max_edge_length == 0.0 {
                assert_eq!(mesh, plain);
                assert!(mesh.unavailable_controls.iter().all(|c| !c.starts_with("max_edge_length")));
                continue;
            }
            let (triangle, polyline) = longest_edges(&mesh, &edges);
            let bound = params.max_edge_length;
            assert!(triangle <= bound && polyline <= bound, "{triangle} {polyline} over {bound}");
            assert!(mesh.unavailable_controls.iter().all(|c| !c.starts_with("max_edge_length")));
            if **params == bounded {
                assert_eq!(mesh, expected);
                assert_eq!(edges, expected_edges);
            }
        }
    }
}

#[test]
fn maximum_edge_length_is_refused_when_float_rounding_exceeds_it() {
    let k = CatallaxKernel::new();
    let body = contract::cylinder(&k, Vec3::ZERO, 3.0, 5.0);
    let params = TessellationParams { max_edge_length: 1e-7, ..Default::default() };
    assert!(matches!(k.tessellate(body, &params), Err(KernelError::Unsupported(_))));
    assert!(matches!(k.tessellate_edges(body, &params), Err(KernelError::Unsupported(_))));
    // The refusal leaves the default mesh and other bounds usable.
    assert!(k.tessellate(body, &TessellationParams::default()).is_ok());
    let usable = TessellationParams { max_edge_length: 2.0, ..Default::default() };
    assert!(k.tessellate(body, &usable).is_ok());
}

#[test]
fn other_unsupported_controls_stay_labelled_with_an_edge_length() {
    let k = CatallaxKernel::new();
    let body = contract::cylinder(&k, Vec3::ZERO, 3.0, 5.0);
    let params = TessellationParams { max_edge_length: 2.0, max_aspect_ratio: 3.0, ..Default::default() };
    let mesh = k.tessellate(body, &params).unwrap();
    assert!(mesh.unavailable_controls.iter().any(|c| c.starts_with("max_aspect_ratio")));
    assert!(mesh.unavailable_controls.iter().any(|c| c.starts_with("curve_angle_deg")));
    assert!(mesh.unavailable_controls.iter().all(|c| !c.starts_with("max_edge_length")));
}
