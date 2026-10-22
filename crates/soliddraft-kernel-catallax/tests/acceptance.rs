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
        assert!(kernel.mass_properties(body).unwrap().inertia.is_none());
        let before = summarize(&kernel, body).unwrap();
        assert!(
            matches!(kernel.fillet(&OpTag::new("finish"), body, &[], 1.0), Err(KernelError::Unsupported(s)) if s.contains("stage 4"))
        );
        assert_eq!(before, summarize(&kernel, body).unwrap());
        kernel.release(body).unwrap();
        assert!(matches!(kernel.topology(body), Err(KernelError::UnknownBody(_))));
    }
    assert!((volumes[1] - volumes[0] - 10800.0).abs() < 1e-4);
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
    assert!(
        matches!(k.chamfer(&OpTag::new("bevel"), b, &[], 1.0), Err(KernelError::Unsupported(s)) if s.contains("stage 3"))
    );
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
