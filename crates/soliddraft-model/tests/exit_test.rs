//! Phase 2 exit test, headless: the sample L-bracket built through the command layer,
//! saved, reopened, its base thickness changed, and every downstream feature either
//! rebuilt, waited for its Catallax stage, or stopped with a repair.
//!
//! The bracket (sample dimensions): a 120 x 90 x 10 base with 8 mm corner radii, a 60 mm
//! flange above it with rounded top corners, an 8 mm inside fillet where they meet, a
//! 24 mm boss with a 12 mm bore, four 9 mm holes on a 90 x 44 pattern and a 40 x 12 slot
//! through the flange.

use std::f64::consts::PI;
use std::sync::Arc;

use soliddraft_kernel_api::{Kernel, Vec3};
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::sketch::{
    Align, ConstraintKind, DistanceKind, EntityKind, P2, PointRef, Sketch, SolveStatus, solve::solve,
};
use soliddraft_model::{
    BlendFeature, Command, DatumPlane, Document, EndCondition, ExtrudeFeature, ExtrudeOp, FeatureId, FeatureKind,
    FeatureStatus, ProfileSource, RepairReason, SketchFeature, SketchPlane,
};

fn kernel() -> Arc<dyn Kernel> {
    Arc::new(CatallaxKernel::new())
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
    let boss = PI * 144.0 * 14.0;
    let bore = PI * 36.0 * (t + 14.0);
    let holes = 4.0 * PI * 20.25 * t;
    let slot = (28.0 * 12.0 + PI * 36.0) * 10.0;
    base + flange - overlap + boss - bore - holes - slot
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
        doc.features().iter().all(|f| {
            if matches!(f.kind, FeatureKind::Fillet(_)) {
                matches!(doc.status(f.id), Some(FeatureStatus::Failed(error)) if error.contains("Catallax stage 3") && error.contains("Catallax stage 4"))
                    && doc.body_after(f.id) == doc.body_before(f.id)
            } else {
                doc.status(f.id).unwrap().is_ok()
            }
        }),
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
            matches!(status, FeatureStatus::Ok | FeatureStatus::RepairNeeded(_))
                || (f.id == ids.fillet
                    && matches!(status, FeatureStatus::Failed(error) if error.contains("Catallax stage"))),
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
    // waiting fillet keeps the preceding body at z = 12.
    let boss_plane = reopened.built_sketch(ids.boss_sketch).unwrap().plane;
    assert!((boss_plane.origin.z - 12.0).abs() < 1e-9, "boss sketch plane at {:?}", boss_plane.origin);
    assert!(matches!(reopened.status(ids.fillet), Some(FeatureStatus::Failed(error))
        if error.contains("Catallax stage 3") && error.contains("Catallax stage 4")));
    assert_eq!(reopened.body_after(ids.fillet), reopened.body_before(ids.fillet));

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
        for f in doc.features().iter().filter(|f| !except.contains(&f.id) && f.id != ids.fillet) {
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

/// Rebuild from the first changed feature: an edit builds from the feature it changed,
/// a rename builds nothing, a value put back reuses the bodies built for it, and every
/// result is the one a full replay of the saved part gives.
mod rebuild {
    use std::collections::{BTreeSet, HashMap};

    use soliddraft_kernel_api::counting::{Counting, Work};
    use soliddraft_kernel_api::{BodyId, FaceId, TessellationParams};
    use soliddraft_model::feature_of_tag;

    use super::*;

    fn counting() -> Arc<Counting> {
        Arc::new(Counting::new(kernel()))
    }

    /// The features whose operations ran, and how many meshes were made.
    fn recomputed(work: &[Work]) -> (BTreeSet<FeatureId>, usize) {
        let features = work
            .iter()
            .filter_map(|w| match w {
                Work::Operation(tag) => Some(feature_of_tag(tag).unwrap_or_else(|| panic!("untagged operation {tag}"))),
                Work::Mesh(_) => None,
            })
            .collect();
        (features, work.iter().filter(|w| matches!(w, Work::Mesh(_))).count())
    }

    /// The part without kernel ids, one line per fact: every feature's status, its
    /// faces' names and its body's face count; then the final body's faces and edges by
    /// name with their measures, its mass properties, its validation and its mesh.
    fn picture(doc: &Document) -> Vec<String> {
        let k = doc.kernel();
        let mut out = Vec::new();
        for f in doc.features() {
            let names: Vec<String> =
                doc.generated_faces(f.id).iter().map(|n| format!("{:?} {}", n.generator, n.ordinal)).collect();
            let faces = doc.body_after(f.id).map(|b| k.topology(b).unwrap().faces.len());
            out.push(format!("{} {}: {:?}, faces {names:?}, body faces {faces:?}", f.id, f.name, doc.status(f.id)));
        }
        let body = doc.body().unwrap();
        let t = k.topology(body).unwrap();
        let labels: HashMap<FaceId, String> = doc
            .face_labels(body)
            .into_iter()
            .map(|(id, l)| (id, l.map_or_else(|| "unnamed".to_owned(), |l| l.text)))
            .collect();
        for f in &t.faces {
            out.push(format!(
                "face {}: {:?} area {:?} centroid {:?} normal {:?} {} edges",
                labels[&f.id],
                f.surface,
                f.area,
                f.centroid,
                f.normal,
                f.edges.len()
            ));
        }
        for e in &t.edges {
            let faces: Vec<&String> = e.faces.iter().map(|f| &labels[f]).collect();
            out.push(format!("edge {:?} length {:?} midpoint {:?} between {faces:?}", e.curve, e.length, e.midpoint));
        }
        out.push(format!("{} vertices, box {:?}", t.vertex_count, t.bbox));
        out.push(format!("mass {:?}", k.mass_properties(body).unwrap()));
        out.push(format!("validation {:?}", k.validate(body).unwrap()));
        let mesh = k.tessellate(body, &TessellationParams::default()).unwrap();
        out.push(format!("mesh positions {:?}", mesh.positions));
        out.push(format!("mesh normals {:?}", mesh.normals));
        out.push(format!("mesh indices {:?}", mesh.indices));
        let faces: Vec<&String> = mesh.triangle_faces.iter().map(|f| &labels[f]).collect();
        out.push(format!("mesh faces {faces:?}, uncertified controls {:?}", mesh.unavailable_controls));
        out
    }

    /// Asserts that `doc` is what replaying its saved file from nothing gives.
    fn assert_matches_full_replay(doc: &Document, when: &str) {
        let replay = Document::from_bytes(&doc.to_bytes().unwrap(), kernel()).unwrap();
        let (got, want) = (picture(doc), picture(&replay));
        assert_eq!(got.len(), want.len(), "{when}: the incremental part and the full replay differ in size");
        for (g, w) in got.iter().zip(&want) {
            assert!(
                g == w,
                "{when}: the incremental part differs from a full replay\n  got:  {g:.400}\n  want: {w:.400}"
            );
        }
    }

    fn bodies(doc: &Document) -> Vec<Option<BodyId>> {
        doc.features().iter().map(|f| doc.body_after(f.id)).collect()
    }

    fn cut(sketch: FeatureId, end: EndCondition) -> FeatureKind {
        extrude(sketch, ExtrudeOp::Cut, end, true)
    }

    #[test]
    fn editing_the_last_feature_rebuilds_only_that_feature() {
        let kernel = counting();
        let mut doc = Document::new("Mount bracket", kernel.clone());
        let ids = build_bracket(&mut doc);
        let slot = doc.features().last().unwrap().id;
        let through = doc.feature(slot).unwrap().kind.clone();
        assert_eq!(through, cut(ids.slot_sketch, EndCondition::ThroughAll));
        kernel.take();
        let built = bodies(&doc);
        let last = built.len() - 1;

        // A pocket instead of a slot through the flange: only the slot is built again,
        // on the body the features above it left, and nothing is tessellated.
        doc.execute(Command::EditFeature { id: slot, kind: cut(ids.slot_sketch, blind(5.0)) }).unwrap();
        assert_eq!(recomputed(&kernel.take()), ([slot].into(), 0), "the edit built only the slot");
        let pocket = bodies(&doc);
        assert_eq!(pocket[..last], built[..last], "the features above the slot kept their bodies");
        assert_ne!(pocket[last], built[last]);
        assert_all_ok(&doc);
        assert_matches_full_replay(&doc, "after the edit");
        kernel.take();

        // A depth one unit in the last place away is another input, never rounded to the
        // first one.
        let nudged = cut(ids.slot_sketch, blind(5.0 + 4.0 * f64::EPSILON));
        doc.execute(Command::EditFeature { id: slot, kind: nudged }).unwrap();
        assert_eq!(recomputed(&kernel.take()).0, [slot].into(), "a different depth is built");
        doc.undo().unwrap();
        assert_eq!(bodies(&doc), pocket);
        kernel.take();

        // Putting the slot back through, as a new edit, reuses the bodies built for it.
        doc.execute(Command::EditFeature { id: slot, kind: through.clone() }).unwrap();
        assert_eq!(kernel.take(), [], "the restored value built nothing");
        assert_eq!(bodies(&doc), built);

        // Undo and redo build nothing either.
        doc.undo().unwrap();
        assert_eq!(kernel.take(), [], "undo built nothing");
        assert_eq!(bodies(&doc), pocket);
        assert_matches_full_replay(&doc, "after undo");
        kernel.take();
        doc.redo().unwrap();
        assert_eq!(kernel.take(), [], "redo built nothing");
        assert_eq!(bodies(&doc), built);

        // Renaming builds nothing, and the new names show in labels as a replay's do.
        for (id, name) in [(ids.slot_sketch, "Slot outline"), (slot, "Slot cut"), (ids.base, "Plate")] {
            doc.execute(Command::RenameFeature { id, name: name.into() }).unwrap();
        }
        assert_eq!(kernel.take(), [], "renames built nothing");
        assert_eq!(bodies(&doc), built);
        let labels = doc.face_labels(doc.body().unwrap());
        assert!(
            labels
                .iter()
                .any(|(_, l)| l.as_ref().is_some_and(|l| l.feature == slot && l.text.starts_with("Slot cut · ")))
        );
        assert_matches_full_replay(&doc, "after the renames");
        kernel.take();

        // Save and reopen: the reopened part replays in full and matches; editing its
        // last feature again builds only that feature, and undo builds nothing.
        let dir = std::env::temp_dir().join(format!("soliddraft-rebuild-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bracket.dwprt");
        doc.save(&path).unwrap();
        let mut reopened = Document::open(&path, kernel.clone()).unwrap();
        assert_eq!(picture(&reopened), picture(&doc), "the reopened part is the edited one");
        kernel.take();
        let reopened_built = bodies(&reopened);
        reopened.execute(Command::EditFeature { id: slot, kind: cut(ids.slot_sketch, blind(5.0)) }).unwrap();
        assert_eq!(recomputed(&kernel.take()), ([slot].into(), 0), "the reopened part built only the slot");
        assert_eq!(bodies(&reopened)[..last], reopened_built[..last]);
        assert_matches_full_replay(&reopened, "after editing the reopened part");
        kernel.take();
        reopened.undo().unwrap();
        assert_eq!(kernel.take(), [], "undo in the reopened part built nothing");
        assert_eq!(bodies(&reopened), reopened_built);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_suppressed_feature_comes_back_without_building() {
        let kernel = counting();
        let mut doc = Document::new("Mount bracket", kernel.clone());
        let ids = build_bracket(&mut doc);
        let built = bodies(&doc);
        kernel.take();

        // Suppressing the boss builds everything below it on the plate without a boss.
        doc.execute(Command::SetSuppressed { id: ids.boss, suppressed: true }).unwrap();
        let (features, meshes) = recomputed(&kernel.take());
        assert!(!features.contains(&ids.base) && !features.contains(&ids.boss) && meshes == 0, "{features:?}");
        assert_matches_full_replay(&doc, "with the boss suppressed");
        kernel.take();

        // Unsuppressing, and rolling back and forward, reuse what was built.
        doc.execute(Command::SetSuppressed { id: ids.boss, suppressed: false }).unwrap();
        assert_eq!(kernel.take(), [], "unsuppressing built nothing");
        assert_eq!(bodies(&doc), built);
        doc.execute(Command::RollTo { before: Some(ids.bore_sketch) }).unwrap();
        doc.execute(Command::RollTo { before: None }).unwrap();
        assert_eq!(kernel.take(), [], "rolling back and forward built nothing");
        assert_eq!(bodies(&doc), built);
        assert_matches_full_replay(&doc, "after rolling forward");
    }

    #[test]
    fn a_fork_builds_only_what_changes_in_it() {
        let kernel = counting();
        let mut doc = Document::new("Mount bracket", kernel.clone());
        let ids = build_bracket(&mut doc);
        let slot = doc.features().last().unwrap().id;
        let built = bodies(&doc);
        kernel.take();

        // A dry run on a fork, as an assistant proposal makes, builds only the slot.
        let mut fork = doc.fork();
        assert_eq!(bodies(&fork), built);
        fork.execute(Command::EditFeature { id: slot, kind: cut(ids.slot_sketch, blind(5.0)) }).unwrap();
        assert_eq!(recomputed(&kernel.take()), ([slot].into(), 0), "the fork built only the slot");
        assert_matches_full_replay(&fork, "in the fork");
        kernel.take();

        // Dropping the fork leaves the part's bodies alive.
        drop(fork);
        assert_eq!(bodies(&doc), built);
        assert!(built.iter().flatten().all(|b| doc.keeps(*b) && doc.kernel().topology(*b).is_ok()));
        assert!(doc.kernel().validate(doc.body().unwrap()).unwrap().valid);
        assert_matches_full_replay(&doc, "after the fork was dropped");
    }

    #[test]
    fn the_cache_keeps_to_its_budget() {
        let kernel = counting();
        let mut doc = Document::new("Mount bracket", kernel.clone());
        let ids = build_bracket(&mut doc);
        let slot = doc.features().last().unwrap().id;
        let built = bodies(&doc);
        doc.execute(Command::EditFeature { id: slot, kind: cut(ids.slot_sketch, blind(5.0)) }).unwrap();
        let pocket = doc.body().unwrap();
        let (retained, budget) = doc.rebuild_cache_usage();
        assert!(retained > 0 && retained <= budget, "{retained} of {budget} bytes");
        assert!(doc.keeps(pocket) && doc.keeps(built.last().unwrap().unwrap()));

        // With no budget only the current tree's builds stay: the through slot is
        // released, so undo builds it again, and the result is still a replay's.
        doc.set_rebuild_budget(0);
        assert!(doc.keeps(pocket), "the current tree is always kept");
        assert!(!doc.keeps(built.last().unwrap().unwrap()), "the slot through the flange was released");
        assert!(doc.rebuild_cache_usage().0 < retained);
        kernel.take();
        doc.undo().unwrap();
        assert_eq!(recomputed(&kernel.take()).0, [slot].into(), "the released build was made again");
        assert!(!doc.keeps(pocket), "the pocket left the tree and the budget");
        assert_eq!(bodies(&doc)[..built.len() - 1], built[..built.len() - 1]);
        assert_matches_full_replay(&doc, "after rebuilding a released build");
    }
}
