//! Canonical public P4 dependency probe on the checked-in application kernel pin.
use soliddraft_kernel_api::{BooleanOp, Extrude, Kernel, OpTag, Plane, Point2, Profile, StepOptions, Vec3};
use soliddraft_kernel_catallax::CatallaxKernel;

#[test]
fn canonical_p4_final_intersection_at_both_wall_thicknesses() {
    let k = CatallaxKernel::new();
    let blank_profile = k
        .create_body_from_profile(
            &OpTag::new("blank-profile"),
            &Profile::rectangle(Plane::XY, Point2::new(0.0, 0.0), 40.0, 30.0),
        )
        .unwrap()
        .body;
    let blank =
        k.extrude(&OpTag::new("blank"), blank_profile, &Extrude { distance: 12.0, direction: None }).unwrap().body;
    let top = k.topology(blank).unwrap().faces.iter().find(|f| f.normal == Some(Vec3::Z)).unwrap().id;
    let round_profile = k
        .create_body_from_profile(
            &OpTag::new("rounding-profile"),
            &Profile::circle(Plane::XY.with_origin(Vec3::new(0.0, 0.0, -1.0)), Point2::new(0.0, 0.0), 23.0),
        )
        .unwrap()
        .body;
    let rounding =
        k.extrude(&OpTag::new("rounding"), round_profile, &Extrude { distance: 14.0, direction: None }).unwrap().body;
    let mut failures = Vec::new();
    for thickness in [2.0, 1.5, 2.0, 1.5] {
        let tray = k.shell(&OpTag::new("tray"), blank, &[top], thickness).unwrap().body;
        let start = std::time::Instant::now();
        let result = k.boolean(&OpTag::new("rounded"), BooleanOp::Intersect, tray, rounding);
        eprintln!(
            "P4 wall={thickness}, kernel intersection elapsed={:?}, answered={}",
            start.elapsed(),
            result.is_ok()
        );
        match result {
            Ok(result) => {
                assert!(k.validate(result.body).unwrap().valid);
                let topology = k.topology(result.body).unwrap();
                assert_eq!(topology.faces.len(), 15);
                let primitive = |x: f64| 0.5 * (x * (529.0 - x * x).sqrt() + 529.0 * (x / 23.0).asin());
                let start_x = 304.0_f64.sqrt();
                let removed_corner = 15.0 * (20.0 - start_x) - (primitive(20.0) - primitive(start_x));
                let expected = (1200.0 - 4.0 * removed_corner) * 12.0
                    - (40.0 - 2.0 * thickness) * (30.0 - 2.0 * thickness) * (12.0 - thickness);
                assert!((k.mass_properties(result.body).unwrap().volume - expected).abs() < 1e-6);
                // The inner floor faces the cavity; the outside floor faces away.
                assert!(
                    topology.faces.iter().any(|f| f.normal == Some(Vec3::Z) && (f.centroid.z - thickness).abs() < 1e-9)
                );
                assert!(topology.faces.iter().any(|f| f.normal == Some(-Vec3::Z) && f.centroid.z.abs() < 1e-9));
                let options = StepOptions::default();
                let step = k.export_step(result.body, &options).unwrap();
                assert!(step.text.starts_with("ISO-10303-21;"));
                assert!(step.report.within_tolerance);
                assert!(step.report.max_deviation_mm <= options.tolerance);
                assert_eq!(step.report.faces, topology.faces.len());
                k.release(result.body).unwrap();
            }
            Err(error) => failures.push((thickness, error)),
        }
        k.release(tray).unwrap();
    }
    for body in [blank_profile, blank, round_profile, rounding] {
        k.release(body).unwrap();
    }
    assert!(failures.is_empty(), "canonical P4 is blocked: {failures:?}");
}
