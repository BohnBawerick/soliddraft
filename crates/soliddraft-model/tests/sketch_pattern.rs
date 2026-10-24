//! Circular sketch patterns must remain valid at the Catallax conversion boundary.

use std::sync::Arc;

use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::sketch::{EntityKind, P2, Sketch, solve, tools::Operation};
use soliddraft_model::{
    Command, DatumPlane, Document, EndCondition, ExtrudeFeature, ExtrudeOp, FeatureKind, FeatureStatus, ProfileSource,
    SketchFeature, SketchPlane,
};

#[test]
fn right_angle_arc_patterns_extrude_through_catallax() {
    for (angle_degrees, count) in [(90.0, 4), (-90.0, 4), (180.0, 2)] {
        let mut sketch = Sketch::new();
        let arc = sketch.add(
            EntityKind::Arc { center: P2::new(10.0, 0.0), start: P2::new(12.0, 0.0), end: P2::new(8.0, 0.0) },
            false,
        );
        let line = sketch.add(EntityKind::Line { a: P2::new(8.0, 0.0), b: P2::new(12.0, 0.0) }, false);
        sketch
            .apply_operation(&Operation::CircularPattern {
                entities: vec![arc, line],
                center: P2::ZERO,
                angle_degrees,
                count,
            })
            .unwrap();
        solve::solve(&mut sketch, None, true);
        let mut doc = Document::new("Arc pattern", Arc::new(CatallaxKernel::new()));
        let sketch_id = doc
            .execute(Command::AddFeature {
                kind: FeatureKind::Sketch(SketchFeature {
                    plane: SketchPlane::Datum { datum: DatumPlane::Top, offset: 0.0 },
                    sketch,
                }),
                name: None,
                position: None,
                via_assistant: false,
            })
            .unwrap()
            .created
            .unwrap();
        let extrude_id = doc
            .execute(Command::AddFeature {
                kind: FeatureKind::Extrude(ExtrudeFeature {
                    profile: ProfileSource::Sketch { feature: sketch_id },
                    operation: ExtrudeOp::Boss,
                    end: EndCondition::Blind { depth: 2.0 },
                    reverse: false,
                }),
                name: None,
                position: None,
                via_assistant: false,
            })
            .unwrap()
            .created
            .unwrap();
        assert_eq!(doc.status(extrude_id), Some(&FeatureStatus::Ok), "angle {angle_degrees}");
        let actual = doc.kernel().mass_properties(doc.body().unwrap()).unwrap().volume;
        let expected = f64::from(count) * 4.0 * std::f64::consts::PI;
        assert!((actual - expected).abs() < 1e-5, "angle {angle_degrees}: {actual} != {expected}");
    }
}
