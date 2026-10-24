//! Catallax stage 3 and 4 features in the part document: each builds, edits and keeps
//! the references of the features after it.

use std::sync::Arc;

use soliddraft_kernel_api::{FaceId, HoleDepth, HoleHead, HoleTip, Vec3};
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::sketch::{P2, Sketch, solve::solve};
use soliddraft_model::{
    Command, DatumPlane, Document, DraftFeature, EndCondition, ExtrudeFeature, ExtrudeOp, FeatureId, FeatureKind,
    FeatureStatus, HoleFeature, MirrorFeature, PatternFeature, PatternSpec, PlaneRef, ProfileSource, SketchFeature,
    SketchPlane,
};

fn doc() -> Document {
    Document::new("Part1", Arc::new(CatallaxKernel::new()))
}

fn add(doc: &mut Document, kind: FeatureKind) -> FeatureId {
    doc.execute(Command::AddFeature { kind, name: None, position: None, via_assistant: false })
        .unwrap()
        .created
        .unwrap()
}

fn edit(doc: &mut Document, id: FeatureId, kind: FeatureKind) {
    doc.execute(Command::EditFeature { id, kind }).unwrap();
}

fn kind(doc: &Document, id: FeatureId) -> FeatureKind {
    doc.feature(id).unwrap().kind.clone()
}

fn sketch(plane: SketchPlane, build: impl FnOnce(&mut Sketch)) -> FeatureKind {
    let mut s = Sketch::new();
    build(&mut s);
    solve(&mut s, None, true);
    FeatureKind::Sketch(SketchFeature { plane, sketch: s })
}

fn top(offset: f64) -> SketchPlane {
    SketchPlane::Datum { datum: DatumPlane::Top, offset }
}

fn extrude(sketch: FeatureId, operation: ExtrudeOp, end: EndCondition, reverse: bool) -> FeatureKind {
    FeatureKind::Extrude(ExtrudeFeature { profile: ProfileSource::Sketch { feature: sketch }, operation, end, reverse })
}

/// A `w` by `h` block centred on the z axis from `z0` up by `depth`.
fn block(doc: &mut Document, w: f64, h: f64, z0: f64, depth: f64) -> FeatureId {
    let s = add(
        doc,
        sketch(top(z0), |s| {
            s.rectangle(P2::new(0.0, 0.0), w, h).unwrap();
        }),
    );
    add(doc, extrude(s, ExtrudeOp::Boss, EndCondition::Blind { depth }, false))
}

fn volume(doc: &Document) -> f64 {
    doc.kernel().mass_properties(doc.body().unwrap()).unwrap().volume
}

fn face_facing(doc: &Document, normal: Vec3, z: f64) -> FaceId {
    let t = doc.kernel().topology(doc.body().unwrap()).unwrap();
    let f = t
        .faces
        .iter()
        .find(|f| f.normal.is_some_and(|n| n.dot(normal) > 1.0 - 1e-9) && (f.centroid.z - z).abs() < 1e-6);
    f.unwrap().id
}

fn all_ok(doc: &Document) {
    for (id, status) in doc.statuses() {
        assert!(status.is_ok(), "{id}: {status:?}");
    }
}

/// A Ø6 boss sketched on the top face and extruded 5 mm: a downstream feature whose
/// sketch plane is a persistent face reference.
fn boss_on(doc: &mut Document, face: FaceId) -> FeatureId {
    let r = doc.face_ref(face).unwrap();
    let s = add(
        doc,
        sketch(SketchPlane::Face { face: r }, |s| {
            s.circle_at(P2::new(0.0, 0.0), 6.0).unwrap();
        }),
    );
    add(doc, extrude(s, ExtrudeOp::Boss, EndCondition::Blind { depth: 5.0 }, false))
}

#[test]
fn holes_build_edit_and_keep_later_references() {
    let mut d = doc();
    block(&mut d, 60.0, 40.0, 0.0, 10.0);
    let before = volume(&d);
    let face = d.face_ref(face_facing(&d, Vec3::Z, 10.0)).unwrap();
    let hole = add(
        &mut d,
        FeatureKind::Hole(HoleFeature {
            face,
            points: vec![P2::new(-20.0, -10.0), P2::new(20.0, 10.0)],
            diameter: 4.0,
            depth: HoleDepth::Through,
            tip: HoleTip::Flat,
            head: Some(HoleHead::Countersink { diameter: 8.0, angle_deg: 90.0 }),
        }),
    );
    all_ok(&d);
    assert!(volume(&d) < before);
    let face = face_facing(&d, Vec3::Z, 10.0);
    let boss = boss_on(&mut d, face);
    all_ok(&d);
    let FeatureKind::Hole(mut h) = kind(&d, hole) else { panic!() };
    // Blind and flat: two drill-point holes in one operation exhaust Catallax's number
    // budget, though one builds.
    h.diameter = 5.0;
    h.depth = HoleDepth::Blind { depth: 6.0 };
    edit(&mut d, hole, FeatureKind::Hole(h));
    all_ok(&d);
    assert_eq!(d.status(boss), Some(&FeatureStatus::Ok));
    // The hole's faces are named by placement and part.
    assert!(d.generated_faces(hole).iter().any(|n| n.generator.to_string() == "Hole 2 floor"));
}

#[test]
fn patterns_and_mirrors_build_edit_and_keep_later_references() {
    let mut d = doc();
    block(&mut d, 10.0, 10.0, 0.0, 10.0);
    let pattern = add(
        &mut d,
        FeatureKind::Pattern(PatternFeature {
            layout: PatternSpec::Linear { direction: Vec3::X, spacing: 20.0, count: 3 },
        }),
    );
    all_ok(&d);
    assert!((volume(&d) - 3000.0).abs() < 1e-6);
    // A boss on the last copy's top face follows the copy when the spacing changes.
    let tops: Vec<_> = {
        let t = d.kernel().topology(d.body().unwrap()).unwrap();
        t.faces
            .iter()
            .filter(|f| f.normal.is_some_and(|n| n.dot(Vec3::Z) > 1.0 - 1e-9))
            .map(|f| (f.centroid.x, f.id))
            .collect()
    };
    assert_eq!(tops.len(), 3);
    let last = tops.iter().max_by(|a, b| a.0.total_cmp(&b.0)).unwrap().1;
    let boss = boss_on(&mut d, last);
    all_ok(&d);
    let FeatureKind::Pattern(mut p) = kind(&d, pattern) else { panic!() };
    p.layout = PatternSpec::Linear { direction: Vec3::X, spacing: 25.0, count: 3 };
    edit(&mut d, pattern, FeatureKind::Pattern(p));
    all_ok(&d);
    assert_eq!(d.status(boss), Some(&FeatureStatus::Ok));

    let mut m = doc();
    block(&mut m, 10.0, 10.0, 0.0, 10.0);
    let mirror = add(
        &mut m,
        FeatureKind::Mirror(MirrorFeature {
            plane: PlaneRef::Datum { datum: DatumPlane::Right, offset: 10.0 },
            keep_original: true,
        }),
    );
    all_ok(&m);
    assert!((volume(&m) - 2000.0).abs() < 1e-6);
    edit(
        &mut m,
        mirror,
        FeatureKind::Mirror(MirrorFeature {
            plane: PlaneRef::Datum { datum: DatumPlane::Right, offset: 10.0 },
            keep_original: false,
        }),
    );
    all_ok(&m);
    assert!((volume(&m) - 1000.0).abs() < 1e-6);
}

#[test]
fn drafts_build_edit_and_keep_later_references() {
    let mut d = doc();
    block(&mut d, 40.0, 30.0, 0.0, 20.0);
    let t = d.kernel().topology(d.body().unwrap()).unwrap();
    let side = t.faces.iter().find(|f| f.normal.is_some_and(|n| n.dot(Vec3::X) > 1.0 - 1e-9)).unwrap().id;
    let side = d.face_ref(side).unwrap();
    let draft = add(
        &mut d,
        FeatureKind::Draft(DraftFeature {
            faces: vec![side],
            neutral: PlaneRef::Datum { datum: DatumPlane::Top, offset: 0.0 },
            angle: 3.0,
            reverse: false,
        }),
    );
    all_ok(&d);
    let face = face_facing(&d, Vec3::Z, 20.0);
    let boss = boss_on(&mut d, face);
    all_ok(&d);
    let FeatureKind::Draft(mut f) = kind(&d, draft) else { panic!() };
    f.angle = 5.0;
    edit(&mut d, draft, FeatureKind::Draft(f));
    all_ok(&d);
    assert_eq!(d.status(boss), Some(&FeatureStatus::Ok));
}

#[test]
fn extrusions_up_to_the_next_boundary_or_a_face_follow_their_limit() {
    let mut d = doc();
    block(&mut d, 60.0, 40.0, 0.0, 5.0);
    let shelf = block(&mut d, 60.0, 40.0, 35.0, 5.0);
    let underside = d.face_ref(face_facing(&d, -Vec3::Z, 35.0)).unwrap();
    let post_sketch = add(
        &mut d,
        sketch(top(5.0), |s| {
            s.circle_at(P2::new(0.0, 0.0), 12.0).unwrap();
        }),
    );
    let post = add(&mut d, extrude(post_sketch, ExtrudeOp::Boss, EndCondition::UpToFace { face: underside }, false));
    all_ok(&d);
    let plates = 2.0 * 60.0 * 40.0 * 5.0;
    assert!((volume(&d) - plates - std::f64::consts::PI * 36.0 * 30.0).abs() < 1e-6);
    // Moving the shelf up moves the face the post stops at.
    let FeatureKind::Sketch(mut s) = kind(&d, FeatureId(shelf.0 - 1)) else { panic!() };
    s.plane = top(45.0);
    edit(&mut d, FeatureId(shelf.0 - 1), FeatureKind::Sketch(s));
    all_ok(&d);
    assert!((volume(&d) - plates - std::f64::consts::PI * 36.0 * 40.0).abs() < 1e-6);
    edit(&mut d, post, extrude(post_sketch, ExtrudeOp::Boss, EndCondition::UpToNext, false));
    all_ok(&d);
    assert!((volume(&d) - plates - std::f64::consts::PI * 36.0 * 40.0).abs() < 1e-6);
}
