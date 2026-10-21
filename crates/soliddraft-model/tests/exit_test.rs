//! Phase 2 exit test, headless: the sample L-bracket built through the command layer,
//! saved, reopened, its base thickness changed, and every downstream feature either
//! rebuilt or stopped with a repair.
//!
//! The bracket (sample dimensions): a 120 x 90 x 10 base with 8 mm corner radii, a 60 mm
//! flange above it with rounded top corners, an 8 mm inside fillet where they meet, a
//! 24 mm boss with a 12 mm bore, four 9 mm holes on a 90 x 44 pattern and a 40 x 12 slot
//! through the flange.

use std::f64::consts::PI;
use std::sync::Arc;

use soliddraft_kernel_api::{Kernel, SurfaceKind, Vec3};
use soliddraft_kernel_smlib::SmlibKernel;
use soliddraft_model::sketch::{
    Align, ConstraintKind, DistanceKind, EntityKind, P2, PointRef, Sketch, SolveStatus, solve::solve,
};
use soliddraft_model::{
    BlendFeature, Command, DatumPlane, Document, EndCondition, ExtrudeFeature, ExtrudeOp, FeatureId, FeatureKind,
    FeatureStatus, Generator, ProfileSource, RepairReason, SketchFeature, SketchPlane,
};

fn kernel() -> Arc<dyn Kernel> {
    Arc::new(SmlibKernel::new())
}

fn p(u: f64, v: f64) -> P2 {
    P2::new(u, v)
}

fn solved(mut sketch: Sketch) -> Sketch {
    solve(&mut sketch, None, true);
    assert_eq!(sketch.status, SolveStatus::FullyDefined, "sketch not fully defined; free {:?}", sketch.free);
    sketch
}

/// The flange outline: a rectangle `width` wide from v = 0 up to `height`, its top
/// corners rounded with `radius`. Fully constrained.
fn flange_outline(width: f64, height: f64, radius: f64) -> Sketch {
    let (hw, h, r) = (width / 2.0, height, radius);
    let mut s = Sketch::new();
    let line = |s: &mut Sketch, a: P2, b: P2| s.add(EntityKind::Line { a, b }, false);
    let bottom = line(&mut s, p(-hw, 0.0), p(hw, 0.0));
    let right = line(&mut s, p(hw, 0.0), p(hw, h - r));
    let a1 = s.add(EntityKind::Arc { center: p(hw - r, h - r), start: p(hw, h - r), end: p(hw - r, h) }, false);
    let top = line(&mut s, p(hw - r, h), p(-hw + r, h));
    let a2 = s.add(EntityKind::Arc { center: p(-hw + r, h - r), start: p(-hw + r, h), end: p(-hw, h - r) }, false);
    let left = line(&mut s, p(-hw, h - r), p(-hw, 0.0));
    // Lines run from point 0 to point 1; arcs have their centre at 0, start at 1, end at 2.
    let pr = PointRef::new;
    for (a, b) in [
        (pr(bottom, 1), pr(right, 0)),
        (pr(right, 1), pr(a1, 1)),
        (pr(a1, 2), pr(top, 0)),
        (pr(top, 1), pr(a2, 1)),
        (pr(a2, 2), pr(left, 0)),
        (pr(left, 1), pr(bottom, 0)),
    ] {
        s.constrain(ConstraintKind::Coincident { a, b });
    }
    for c in [
        ConstraintKind::Fix { point: pr(bottom, 0), at: p(-hw, 0.0) },
        ConstraintKind::Horizontal { target: Align::Line(bottom) },
        ConstraintKind::Vertical { target: Align::Line(right) },
        ConstraintKind::Horizontal { target: Align::Line(top) },
        ConstraintKind::Vertical { target: Align::Line(left) },
        ConstraintKind::Tangent { a: right, b: a1 },
        ConstraintKind::Tangent { a: a1, b: top },
        ConstraintKind::Tangent { a: top, b: a2 },
        ConstraintKind::Tangent { a: a2, b: left },
        ConstraintKind::Equal { a: a1, b: a2 },
        ConstraintKind::Radius { entity: a1, value: radius, diameter: false, label: p(hw, h + 5.0) },
        ConstraintKind::Distance {
            a: pr(bottom, 0),
            b: pr(bottom, 1),
            kind: DistanceKind::Horizontal,
            value: width,
            label: p(0.0, -8.0),
        },
        ConstraintKind::Distance {
            a: pr(bottom, 0),
            b: pr(top, 0),
            kind: DistanceKind::Vertical,
            value: height,
            label: p(hw + 8.0, h / 2.0),
        },
    ] {
        s.constrain(c);
    }
    solved(s)
}

fn add(doc: &mut Document, name: &str, kind: FeatureKind) -> FeatureId {
    let done = doc.execute(Command::AddFeature { kind, name: Some(name.into()), position: None, via_assistant: false });
    done.unwrap_or_else(|e| panic!("adding {name}: {e}")).created.unwrap()
}

fn sketch_on(plane: SketchPlane, sketch: Sketch) -> FeatureKind {
    FeatureKind::Sketch(SketchFeature { plane, sketch })
}

fn extrude(sketch: FeatureId, operation: ExtrudeOp, end: EndCondition, reverse: bool) -> FeatureKind {
    FeatureKind::Extrude(ExtrudeFeature { profile: ProfileSource::Sketch { feature: sketch }, operation, end, reverse })
}

fn blind(depth: f64) -> EndCondition {
    EndCondition::Blind { depth }
}

/// A face of the final body as a user would click it: the planar face whose centroid is
/// nearest `near` and whose normal is `normal`.
fn pick_face(doc: &Document, near: Vec3, normal: Vec3) -> SketchPlane {
    let t = doc.kernel().topology(doc.body().unwrap()).unwrap();
    let face = t
        .faces
        .iter()
        .filter(|f| f.normal.is_some_and(|n| n.distance(normal) < 1e-9))
        .min_by(|a, b| a.centroid.distance(near).total_cmp(&b.centroid.distance(near)))
        .expect("a planar face with that normal");
    SketchPlane::Face { face: doc.face_ref(face.id).unwrap_or_else(|e| panic!("naming the face near {near:?}: {e}")) }
}

/// Feature ids of the bracket, in tree order.
struct Bracket {
    base: FeatureId,
    flange: FeatureId,
    fillet: FeatureId,
    boss_sketch: FeatureId,
    boss: FeatureId,
    bore_sketch: FeatureId,
    bore: FeatureId,
    slot_sketch: FeatureId,
}

/// Builds the bracket the way a user would, one command per feature.
fn build_bracket(doc: &mut Document) -> Bracket {
    let mut base_sketch = Sketch::new();
    base_sketch.rounded_rectangle(p(0.0, 0.0), 120.0, 90.0, 8.0).unwrap();
    let s = add(
        doc,
        "Base sketch",
        sketch_on(SketchPlane::Datum { datum: DatumPlane::Top, offset: 0.0 }, solved(base_sketch)),
    );
    let base = add(doc, "Base", extrude(s, ExtrudeOp::Boss, blind(10.0), false));

    // The flange stands on the front edge of the base: sketched on the Front plane moved
    // 45 mm forward to y = -45 and extruded back into the part.
    let front = SketchPlane::Datum { datum: DatumPlane::Front, offset: 45.0 };
    let s = add(doc, "Flange sketch", sketch_on(front, flange_outline(120.0, 70.0, 8.0)));
    let flange = add(doc, "Flange", extrude(s, ExtrudeOp::Boss, blind(10.0), true));

    // The inside corner: the edge where the base's top meets the flange's back, picked
    // in the viewport.
    let t = doc.kernel().topology(doc.body().unwrap()).unwrap();
    let corner = t
        .edges
        .iter()
        .min_by(|a, b| {
            let d = |e: &soliddraft_kernel_api::EdgeInfo| e.midpoint.distance(Vec3::new(0.0, -35.0, 10.0));
            d(a).total_cmp(&d(b))
        })
        .unwrap();
    assert!(
        corner.midpoint.distance(Vec3::new(0.0, -35.0, 10.0)) < 1e-6,
        "inside corner edge at {:?}",
        corner.midpoint
    );
    let edge = doc.edge_ref(corner.id).expect("naming the inside corner");
    let fillet = add(doc, "Inside fillet", FeatureKind::Fillet(BlendFeature { edges: vec![edge], size: 8.0 }));

    let top = pick_face(doc, Vec3::new(0.0, 0.0, 10.0), Vec3::Z);
    let mut boss = Sketch::new();
    boss.circle_at(p(20.0, 0.0), 24.0).unwrap();
    let boss_sketch = add(doc, "Boss sketch", sketch_on(top.clone(), solved(boss)));
    let boss = add(doc, "Boss", extrude(boss_sketch, ExtrudeOp::Boss, blind(14.0), false));

    let boss_top = pick_face(doc, Vec3::new(20.0, 0.0, 24.0), Vec3::Z);
    let mut bore = Sketch::new();
    bore.circle_at(p(20.0, 0.0), 12.0).unwrap();
    let bore_sketch = add(doc, "Bore sketch", sketch_on(boss_top, solved(bore)));
    let bore = add(doc, "Bore", extrude(bore_sketch, ExtrudeOp::Cut, EndCondition::ThroughAll, true));

    let mut holes = Sketch::new();
    for (x, y) in [(-45.0, -22.0), (45.0, -22.0), (-45.0, 22.0), (45.0, 22.0)] {
        holes.circle_at(p(x, y), 9.0).unwrap();
    }
    let s = add(doc, "Hole pattern sketch", sketch_on(top, solved(holes)));
    add(doc, "Hole pattern", extrude(s, ExtrudeOp::Cut, EndCondition::ThroughAll, true));

    // The slot is sketched on the flange's front face, 50 mm up, and cut through. The
    // union merged that face with the base's front face, which is coplanar, and the
    // merged face kept the base's name.
    let flange_front = pick_face(doc, Vec3::new(0.0, -45.0, 40.0), -Vec3::Y);
    let mut slot = Sketch::new();
    slot.slot(p(0.0, 50.0), 40.0, 12.0).unwrap();
    let slot_sketch = add(doc, "Slot sketch", sketch_on(flange_front, solved(slot)));
    add(doc, "Slot", extrude(slot_sketch, ExtrudeOp::Cut, EndCondition::ThroughAll, true));

    let label =
        doc.face_label(doc.resolve_face(slot_face(doc, slot_sketch), doc.body_before(slot_sketch).unwrap()).unwrap());
    assert!(label.unwrap().text.starts_with("Base · Side of line"), "the flange front is the base's front face");
    Bracket { base, flange, fillet, boss_sketch, boss, bore_sketch, bore, slot_sketch }
}

fn slot_face(doc: &Document, sketch: FeatureId) -> &soliddraft_model::FaceRef {
    match &doc.feature(sketch).unwrap().kind {
        FeatureKind::Sketch(SketchFeature { plane: SketchPlane::Face { face }, .. }) => face,
        other => panic!("{sketch} is not a sketch on a face: {other:?}"),
    }
}

/// The bracket's volume in closed form, for a base `t` thick.
fn bracket_volume(t: f64) -> f64 {
    let corner = (1.0 - PI / 4.0) * 64.0; // area an 8 mm corner rounding removes
    let base = (120.0 * 90.0 - 4.0 * corner) * t;
    let flange = (120.0 * 70.0 - 2.0 * corner) * 10.0;
    let overlap = (120.0 * 10.0 - 2.0 * corner) * t; // flange inside the base
    let fillet = corner * 120.0;
    let boss = PI * 144.0 * 14.0;
    let bore = PI * 36.0 * (t + 14.0);
    let holes = 4.0 * PI * 20.25 * t;
    let slot = (28.0 * 12.0 + PI * 36.0) * 10.0;
    base + flange - overlap + fillet + boss - bore - holes - slot
}

fn volume(doc: &Document) -> f64 {
    doc.kernel().mass_properties(doc.body().unwrap()).unwrap().volume
}

fn report(doc: &Document) -> String {
    doc.features()
        .iter()
        .map(|f| format!("  {} {}: {:?}", f.id, f.name, doc.status(f.id).unwrap()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_all_ok(doc: &Document) {
    assert!(
        doc.features().iter().all(|f| doc.status(f.id).unwrap().is_ok()),
        "not every feature built:\n{}",
        report(doc)
    );
}

fn assert_close(got: f64, want: f64, what: &str) {
    assert!((got - want).abs() <= 1e-3 * want, "{what}: volume {got}, expected {want}");
}

#[test]
fn bracket_survives_save_reopen_and_a_thicker_base() {
    let kernel = kernel();
    let mut doc = Document::new("Mount bracket", kernel.clone());
    let ids = build_bracket(&mut doc);
    assert_eq!(doc.features().len(), 13);
    assert_all_ok(&doc);
    assert!(kernel.validate(doc.body().unwrap()).unwrap().valid);
    assert_close(volume(&doc), bracket_volume(10.0), "as built");
    let built_volume = volume(&doc);

    // Save, reopen in a new document, and get the same part back.
    let dir = std::env::temp_dir().join(format!("soliddraft-exit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bracket.dwprt");
    doc.save(&path).unwrap();
    let mut reopened = Document::open(&path, kernel.clone()).unwrap();
    assert_eq!(reopened.part_json(), doc.part_json(), "the features round-trip");
    assert_eq!(reopened.id(), doc.id(), "the part keeps its id");
    assert_eq!(
        reopened.to_bytes().unwrap(),
        std::fs::read(&path).unwrap(),
        "saving the reopened part gives the same file"
    );
    assert_all_ok(&reopened);
    assert!((volume(&reopened) - built_volume).abs() <= 1e-9 * built_volume, "the reopened part has the same volume");
    drop(doc);

    // Thicken the base from 10 to 12 mm. Everything downstream rebuilds on the new base.
    let before: Vec<_> = reopened.features().iter().map(|f| reopened.body_after(f.id)).collect();
    let thicker = extrude(reopened_sketch(&reopened, ids.base), ExtrudeOp::Boss, blind(12.0), false);
    reopened.execute(Command::EditFeature { id: ids.base, kind: thicker }).unwrap();
    for f in reopened.features() {
        let status = reopened.status(f.id).unwrap();
        assert!(
            matches!(status, FeatureStatus::Ok | FeatureStatus::RepairNeeded(_)),
            "{} neither rebuilt nor asked for a repair: {status:?}",
            f.name
        );
    }
    assert_all_ok(&reopened);
    let after: Vec<_> = reopened.features().iter().map(|f| reopened.body_after(f.id)).collect();
    for (i, f) in reopened.features().iter().enumerate().skip(1) {
        if matches!(f.kind, FeatureKind::Sketch(_)) {
            continue;
        }
        assert_ne!(before[i], after[i], "{} was rebuilt", f.name);
    }
    assert_close(volume(&reopened), bracket_volume(12.0), "with a 12 mm base");

    // The references followed the base: the boss sketch now lies on z = 12 and the
    // fillet rounds the corner at z = 12.
    let boss_plane = reopened.built_sketch(ids.boss_sketch).unwrap().plane;
    assert!((boss_plane.origin.z - 12.0).abs() < 1e-9, "boss sketch plane at {:?}", boss_plane.origin);
    let t = kernel.topology(reopened.body().unwrap()).unwrap();
    let named = &reopened.generated_faces(ids.fillet)[0];
    assert_eq!(named.generator, Generator::Blend { edge: 0 });
    let round = t.face(named.face).expect("the fillet face is in the final body");
    // SMLib types its circular fillet surface as a cone; what matters is that it is curved.
    assert_ne!(round.surface, SurfaceKind::Plane, "the fillet face is round");
    assert!(
        round.bbox.min.y > -35.0 - 1e-6 && round.bbox.max.y < -27.0 + 1e-6,
        "fillet spans y -35..-27: {:?}",
        round.bbox
    );
    assert!(
        round.bbox.min.z > 12.0 - 1e-6 && round.bbox.max.z < 20.0 + 1e-6,
        "fillet spans z 12..20: {:?}",
        round.bbox
    );

    // Undo puts the 10 mm base back; redo makes it 12 again.
    assert!(reopened.undo().unwrap().starts_with("edit Base"));
    assert_close(volume(&reopened), bracket_volume(10.0), "after undo");
    reopened.redo().unwrap();
    assert_close(volume(&reopened), bracket_volume(12.0), "after redo");
    std::fs::remove_dir_all(&dir).ok();
}

fn reopened_sketch(doc: &Document, extrusion: FeatureId) -> FeatureId {
    match &doc.feature(extrusion).unwrap().kind {
        FeatureKind::Extrude(ExtrudeFeature { profile: ProfileSource::Sketch { feature }, .. }) => *feature,
        other => panic!("{extrusion} is not an extrusion: {other:?}"),
    }
}

#[test]
fn removing_features_asks_for_repairs_instead_of_guessing() {
    let mut doc = Document::new("Mount bracket", kernel());
    let ids = build_bracket(&mut doc);
    assert_all_ok(&doc);
    let others_ok = |doc: &Document, except: &[FeatureId]| {
        for f in doc.features().iter().filter(|f| !except.contains(&f.id)) {
            assert!(doc.status(f.id).unwrap().is_ok(), "{}: {:?}\n{}", f.name, doc.status(f.id), report(doc));
        }
    };

    // The fillet's edge lies on the flange. Without the flange it stops with a repair
    // that says what broke and offers candidates; every other feature still builds.
    doc.execute(Command::SetSuppressed { id: ids.flange, suppressed: true }).unwrap();
    let fillet = doc.status(ids.fillet).unwrap().clone();
    let FeatureStatus::RepairNeeded(repairs) = fillet else { panic!("fillet: {fillet:?}\n{}", report(&doc)) };
    assert_eq!(repairs.len(), 1);
    assert_eq!(repairs[0].reason, RepairReason::Missing, "{:?}", repairs[0]);
    assert!(!repairs[0].candidates.is_empty(), "a repair offers candidates");
    others_ok(&doc, &[ids.flange, ids.fillet]);
    assert!(doc.kernel().validate(doc.body().unwrap()).unwrap().valid, "the part without its flange is valid");
    doc.undo().unwrap();
    assert_all_ok(&doc);

    // The bore is sketched on the boss's top face. Without the boss the sketch's plane
    // needs a repair and the bore waits for it.
    doc.execute(Command::SetSuppressed { id: ids.boss, suppressed: true }).unwrap();
    let sketch = doc.status(ids.bore_sketch).unwrap();
    assert!(
        matches!(sketch, FeatureStatus::RepairNeeded(r) if r[0].slot == soliddraft_model::ReferenceSlot::SketchPlane),
        "{sketch:?}"
    );
    assert_eq!(doc.status(ids.bore), Some(&FeatureStatus::Blocked(ids.bore_sketch)));
    others_ok(&doc, &[ids.boss, ids.bore_sketch, ids.bore]);
    doc.undo().unwrap();
    assert_all_ok(&doc);
    let _ = ids.slot_sketch;
}
