//! Reference survival on Catallax origins, headless: the bracket's plate thickness and
//! boss radius change, features are renamed, a later cut splits the plate's top face, a
//! tab merges with it, a feature is suppressed and restored, commands are undone and
//! redone, and the part is saved and reopened. After every step each reference resolves
//! to a face that carries the origin it names, or raises the repair prompt. The same
//! steps run through MCP in `ci/scenarios/references.json`.

use std::collections::BTreeMap;
use std::sync::Arc;

use soliddraft_kernel_api::{FaceId, Topology};
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::sketch::{ConstraintId, P2, Sketch, SolveStatus, solve::solve};
use soliddraft_model::{
    Command, DatumPlane, Document, EndCondition, ExtrudeFeature, ExtrudeOp, FaceRef, FeatureId, FeatureKind,
    FeatureStatus, Generator, ProfileSource, RepairReason, SketchFeature, SketchPlane,
};

fn add(doc: &mut Document, name: &str, kind: FeatureKind) -> FeatureId {
    let done = doc.execute(Command::AddFeature { kind, name: Some(name.into()), position: None, via_assistant: false });
    done.unwrap_or_else(|e| panic!("adding {name}: {e}")).created.unwrap()
}

fn sketch(plane: SketchPlane, build: impl FnOnce(&mut Sketch)) -> FeatureKind {
    let mut s = Sketch::new();
    build(&mut s);
    solve(&mut s, None, true);
    assert_eq!(s.status, SolveStatus::FullyDefined);
    FeatureKind::Sketch(SketchFeature { plane, sketch: s })
}

fn top() -> SketchPlane {
    SketchPlane::Datum { datum: DatumPlane::Top, offset: 0.0 }
}

fn extrude(sketch: FeatureId, operation: ExtrudeOp, end: EndCondition) -> FeatureKind {
    // A cut goes into the face it is sketched on, against the sketch normal.
    let reverse = operation == ExtrudeOp::Cut;
    FeatureKind::Extrude(ExtrudeFeature { profile: ProfileSource::Sketch { feature: sketch }, operation, end, reverse })
}

fn blind(depth: f64) -> EndCondition {
    EndCondition::Blind { depth }
}

/// The faces of `body` that carry the origin a feature's name stands for.
fn carriers(doc: &Document, body: soliddraft_kernel_api::BodyId, r: &FaceRef) -> (Topology, Vec<FaceId>) {
    let t = doc.kernel().topology(body).unwrap();
    let names = doc.generated_faces(r.feature);
    let faces = names
        .iter()
        .find(|n| n.generator == r.generator && n.ordinal == r.ordinal)
        .map(|n| t.faces.iter().filter(|f| f.origins.contains(&n.origin)).map(|f| f.id).collect())
        .unwrap_or_default();
    (t, faces)
}

/// A sketch plane on a face `feature` generated, picked the way a user clicks it: the
/// first face of the part carrying that origin.
fn on(doc: &Document, feature: FeatureId, generator: Generator) -> SketchPlane {
    let name = doc.generated_faces(feature).iter().find(|n| n.generator == generator).expect("the feature names it");
    let t = doc.kernel().topology(doc.body().unwrap()).unwrap();
    let face = t.faces.iter().find(|f| f.origins.contains(&name.origin)).expect("a face carries it").id;
    SketchPlane::Face { face: doc.plane_ref(face).unwrap_or_else(|e| panic!("sketching on {face}: {e}")) }
}

fn face_ref(doc: &Document, sketch: FeatureId) -> FaceRef {
    match &doc.feature(sketch).unwrap().kind {
        FeatureKind::Sketch(SketchFeature { plane: SketchPlane::Face { face }, .. }) => face.clone(),
        other => panic!("{sketch} is not on a face: {other:?}"),
    }
}

fn report(doc: &Document) -> String {
    let line = |f: &soliddraft_model::Feature| format!("  {} {}: {:?}", f.id, f.name, doc.status(f.id).unwrap());
    doc.features().iter().map(line).collect::<Vec<_>>().join("\n")
}

/// Every sketch on a face names the origin it named when it was made, and either lies
/// on a face of its input body that carries that origin or, when `repair` lists it,
/// raises the repair prompt. Everything else builds, or waits for a repair.
fn check(doc: &Document, origins: &mut BTreeMap<FeatureId, FaceRef>, repair: &[FeatureId], step: &str) {
    for f in doc.features() {
        let status = doc.status(f.id).unwrap();
        let waits_for_repair = matches!(status, FeatureStatus::Blocked(input) if repair.contains(input));
        let FeatureKind::Sketch(SketchFeature { plane: SketchPlane::Face { face }, .. }) = &f.kind else {
            let built = status.is_ok() || *status == FeatureStatus::Suppressed || waits_for_repair;
            assert!(built, "{step}: {} is {status:?}\n{}", f.name, report(doc));
            continue;
        };
        let was = origins.entry(f.id).or_insert_with(|| face.clone());
        assert_eq!((was.feature, was.generator, was.ordinal), (face.feature, face.generator, face.ordinal));
        if repair.contains(&f.id) {
            let FeatureStatus::RepairNeeded(items) = status else {
                panic!("{step}: {} should raise the repair prompt, it is {status:?}\n{}", f.name, report(doc))
            };
            assert_eq!(items[0].reason, RepairReason::Missing, "{step}: {}", items[0].message);
            continue;
        }
        assert!(status.is_ok(), "{step}: {} is {status:?}\n{}", f.name, report(doc));
        let plane = doc.built_sketch(f.id).unwrap().plane;
        let (t, faces) = carriers(doc, doc.body_before(f.id).unwrap(), face);
        assert!(!faces.is_empty(), "{step}: no face carries the origin of {}", f.name);
        for id in faces {
            let info = t.face(id).unwrap();
            assert_eq!(info.normal, Some(plane.normal), "{step}: {} faces the other way", f.name);
            let off = (info.centroid - plane.origin).dot(plane.normal);
            assert!(off.abs() < 1e-9, "{step}: {} lies {off} mm off its face", f.name);
        }
    }
}

fn faces_carrying(doc: &Document, r: &FaceRef) -> usize {
    carriers(doc, doc.body().unwrap(), r).1.len()
}

fn volume(doc: &Document) -> f64 {
    doc.kernel().mass_properties(doc.body().unwrap()).unwrap().volume
}

fn diameter(doc: &Document, sketch: FeatureId) -> ConstraintId {
    let FeatureKind::Sketch(s) = &doc.feature(sketch).unwrap().kind else { panic!("not a sketch") };
    s.sketch.constraints.iter().find(|c| c.is_dimension()).unwrap().id
}

#[test]
fn references_survive_edits_renames_splits_merges_suppression_undo_and_reopening() {
    let mut doc = Document::new("Bracket", Arc::new(CatallaxKernel::new()));
    let mut origins = BTreeMap::new();
    let pi = std::f64::consts::PI;

    let s = add(&mut doc, "Plate sketch", sketch(top(), |s| {
        s.rectangle(P2::new(60.0, 45.0), 120.0, 90.0).unwrap();
    }));
    let plate = add(&mut doc, "Plate", extrude(s, ExtrudeOp::Boss, blind(10.0)));
    let s = add(&mut doc, "Flange sketch", sketch(top(), |s| {
        s.rectangle(P2::new(5.0, 45.0), 10.0, 90.0).unwrap();
    }));
    add(&mut doc, "Flange", extrude(s, ExtrudeOp::Boss, blind(60.0)));
    let plane = on(&doc, plate, Generator::EndCap);
    let boss_sketch = add(&mut doc, "Boss sketch", sketch(plane, |s| {
        s.circle_at(P2::new(70.0, 45.0), 24.0).unwrap();
    }));
    let boss = add(&mut doc, "Boss", extrude(boss_sketch, ExtrudeOp::Boss, blind(20.0)));
    let plane = on(&doc, boss, Generator::EndCap);
    let bore_sketch = add(&mut doc, "Bore sketch", sketch(plane, |s| {
        s.circle_at(P2::new(70.0, 45.0), 12.0).unwrap();
    }));
    add(&mut doc, "Bore", extrude(bore_sketch, ExtrudeOp::Cut, EndCondition::ThroughAll));
    check(&doc, &mut origins, &[], "bracket");
    let plate_top = face_ref(&doc, boss_sketch);
    assert_eq!(faces_carrying(&doc, &plate_top), 1);

    // A groove across the plate splits its top in two; a pin hole is sketched on one piece.
    let plane = on(&doc, plate, Generator::EndCap);
    let s = add(&mut doc, "Groove sketch", sketch(plane, |s| {
        s.rectangle(P2::new(100.0, 45.0), 6.0, 100.0).unwrap();
    }));
    add(&mut doc, "Groove", extrude(s, ExtrudeOp::Cut, blind(3.0)));
    assert_eq!(faces_carrying(&doc, &plate_top), 2, "the groove splits the plate's top");
    let plane = on(&doc, plate, Generator::EndCap);
    let pin_sketch = add(&mut doc, "Pin sketch", sketch(plane, |s| {
        s.circle_at(P2::new(112.0, 45.0), 6.0).unwrap();
    }));
    add(&mut doc, "Pin hole", extrude(pin_sketch, ExtrudeOp::Cut, blind(5.0)));
    check(&doc, &mut origins, &[], "split");

    // A tab flush with the plate merges its top with one piece of the plate's top.
    let s = add(&mut doc, "Tab sketch", sketch(top(), |s| {
        s.rectangle(P2::new(130.0, 45.0), 20.0, 20.0).unwrap();
    }));
    let tab = add(&mut doc, "Tab", extrude(s, ExtrudeOp::Boss, blind(10.0)));
    let tab_top = doc.generated_faces(tab).iter().find(|n| n.generator == Generator::EndCap).unwrap().origin.clone();
    let t = doc.kernel().topology(doc.body().unwrap()).unwrap();
    let merged = t.faces.iter().find(|f| f.origins.contains(&tab_top)).unwrap();
    assert!(merged.origins.len() > 1, "the tab's top merged with the plate's: {:?}", merged.origins);
    let plane = on(&doc, tab, Generator::EndCap);
    let hole_sketch = add(&mut doc, "Tab hole sketch", sketch(plane, |s| {
        s.circle_at(P2::new(130.0, 45.0), 5.0).unwrap();
    }));
    add(&mut doc, "Tab hole", extrude(hole_sketch, ExtrudeOp::Cut, blind(4.0)));
    check(&doc, &mut origins, &[], "merge");
    let made = volume(&doc);
    let expected = 120.0 * 90.0 * 10.0 + 10.0 * 90.0 * 50.0 + pi * 144.0 * 20.0 - pi * 36.0 * 30.0 - 6.0 * 90.0 * 3.0
        - pi * 9.0 * 5.0
        + 4000.0
        - pi * 6.25 * 4.0;
    assert!((made - expected).abs() < 1e-3, "volume {made}, expected {expected}");

    // The plate gets thicker and the boss wider: every reference follows.
    let FeatureKind::Extrude(mut e) = doc.feature(plate).unwrap().kind.clone() else { panic!() };
    e.end = blind(12.0);
    doc.execute(Command::EditFeature { id: plate, kind: FeatureKind::Extrude(e) }).unwrap();
    check(&doc, &mut origins, &[], "plate thickness");
    let constraint = diameter(&doc, boss_sketch);
    doc.execute(Command::SetSketchDimension { id: boss_sketch, constraint, value: 30.0 }).unwrap();
    check(&doc, &mut origins, &[], "boss radius");
    assert!((volume(&doc) - made).abs() > 1.0, "the edits changed the part");

    // Renames change labels, never what a reference names.
    for (id, name) in [(plate, "Base plate"), (boss, "Round boss"), (boss_sketch, "Boss circle")] {
        doc.execute(Command::RenameFeature { id, name: name.into() }).unwrap();
        check(&doc, &mut origins, &[], "rename");
    }

    // Suppressing the boss loses the bore's face, which raises the repair prompt; the
    // restore brings it back. Undo and redo walk the same states.
    doc.execute(Command::SetSuppressed { id: boss, suppressed: true }).unwrap();
    check(&doc, &mut origins, &[bore_sketch], "suppress");
    doc.execute(Command::SetSuppressed { id: boss, suppressed: false }).unwrap();
    check(&doc, &mut origins, &[], "restore");
    doc.undo().unwrap();
    check(&doc, &mut origins, &[bore_sketch], "undo");
    doc.redo().unwrap();
    check(&doc, &mut origins, &[], "redo");

    // Save and reopen: the names are in the file, the kernel ids are not.
    let bytes = doc.to_bytes().unwrap();
    let before = volume(&doc);
    let mut doc = Document::from_bytes(&bytes, Arc::new(CatallaxKernel::new())).unwrap();
    check(&doc, &mut origins, &[], "reopen");
    assert!((volume(&doc) - before).abs() < 1e-9);

    // A genuinely lost reference: the boss is deleted, and the bore's sketch asks for a
    // repair instead of picking another face.
    doc.execute(Command::DeleteFeature { id: boss }).unwrap();
    check(&doc, &mut origins, &[bore_sketch], "delete");
    let FeatureStatus::RepairNeeded(items) = doc.status(bore_sketch).unwrap() else { unreachable!() };
    assert!(items[0].message.contains("not built before this feature"), "{}", items[0].message);
}
