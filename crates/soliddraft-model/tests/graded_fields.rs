//! Graded terminal fields in the document: the V7d states D0-D2 and G0-G2 as a part
//! holds them, their saved form, upstream edits that leave the anchors where they are,
//! undo and reopen, the outputs a field part has, and the terminal-field policy.

use std::sync::Arc;

use soliddraft_kernel_api::*;
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::sketch::{P2, Sketch, solve::solve};
use soliddraft_model::{
    Command, CommandError, DatumPlane, Document, EndCondition, ExtrudeFeature, ExtrudeOp, FeatureId, FeatureKind,
    FeatureStatus, GyroidFeature, LatticeFeature, ProfileSource, SketchFeature, SketchPlane,
};

fn kernel() -> Arc<CatallaxKernel> {
    Arc::new(CatallaxKernel::new())
}

fn add(doc: &mut Document, kind: FeatureKind) -> FeatureId {
    doc.execute(Command::AddFeature { kind, name: None, position: None, via_assistant: false })
        .unwrap()
        .created
        .unwrap()
}

fn edit(doc: &mut Document, id: FeatureId, kind: FeatureKind) -> Result<(), CommandError> {
    doc.execute(Command::EditFeature { id, kind }).map(|_| ())
}

fn status(d: &Document, id: FeatureId) -> FeatureStatus {
    d.statuses().into_iter().find(|(i, _)| *i == id).unwrap().1.clone()
}

/// A `w` by `h` rectangle centred on the axis, on the top plane at `z`.
fn sketch(w: f64, h: f64, z: f64) -> FeatureKind {
    let mut s = Sketch::new();
    s.rectangle(P2::new(0.0, 0.0), w, h).unwrap();
    solve(&mut s, None, true);
    FeatureKind::Sketch(SketchFeature { plane: SketchPlane::Datum { datum: DatumPlane::Top, offset: z }, sketch: s })
}

fn boss(sketch: FeatureId, depth: f64) -> FeatureKind {
    FeatureKind::Extrude(ExtrudeFeature {
        profile: ProfileSource::Sketch { feature: sketch },
        operation: ExtrudeOp::Boss,
        end: EndCondition::Blind { depth },
        reverse: false,
    })
}

fn linear(from: f64, to: f64, start: f64, end: f64) -> Grade {
    Grade::Linear(LinearGrade { axis: GradeAxis::Z, from, to, start, end })
}

fn lattice(radius: Grade) -> FeatureKind {
    FeatureKind::Lattice(LatticeFeature {
        cell_size_mm: Some(100.0),
        radius_mm: Some(radius),
        accuracy_mm: 0.1,
        min_detail_mm: 0.1,
        ..Default::default()
    })
}

fn gyroid(level: Grade) -> FeatureKind {
    FeatureKind::Gyroid(GyroidFeature { level, accuracy_mm: 0.1, min_detail_mm: 0.1, ..Default::default() })
}

/// D0: the 6 by 6 by 10 region at z = 20 and its radius 1 to 2 mm along z.
fn d0() -> (Document, FeatureId, FeatureId) {
    let mut d = Document::new("D0", kernel());
    let s = add(&mut d, sketch(6.0, 6.0, 20.0));
    let b = add(&mut d, boss(s, 10.0));
    let l = add(&mut d, lattice(linear(20.0, 30.0, 1.0, 2.0)));
    assert!(status(&d, l).is_ok(), "{:?}", status(&d, l));
    (d, b, l)
}

/// G0: the 2 mm cube about the origin and its level -1/10 to 1/10 along z.
fn g0() -> (Document, FeatureId, FeatureId) {
    let mut d = Document::new("G0", kernel());
    let s = add(&mut d, sketch(2.0, 2.0, -1.0));
    let b = add(&mut d, boss(s, 2.0));
    let g = add(&mut d, gyroid(linear(-1.0, 1.0, -0.1, 0.1)));
    assert!(status(&d, g).is_ok(), "{:?}", status(&d, g));
    (d, b, g)
}

/// The native field step the document's body holds.
fn field_step(d: &Document, kernel: &CatallaxKernel) -> serde_json::Value {
    let recipe = kernel.body_recipe(d.body().unwrap()).unwrap();
    let result = recipe["result"].clone();
    recipe["steps"].as_array().unwrap().iter().find(|s| s["id"] == result).unwrap().clone()
}

#[test]
fn grades_save_as_named_functions_and_constants_stay_bare_numbers() {
    // The saved kind of the last feature.
    let saved = |d: &Document| -> serde_json::Value {
        let part: serde_json::Value = serde_json::from_str(&d.part_json()).unwrap();
        part["features"].as_array().unwrap().last().unwrap()["kind"].clone()
    };
    let (d, _, l) = d0();
    assert_eq!(
        saved(&d)["radius_mm"],
        serde_json::json!({"linear":{"axis":"z","from":20.0,"to":30.0,"start":1.0,"end":2.0}})
    );
    let reopened = Document::from_bytes(&d.to_bytes().unwrap(), kernel()).unwrap();
    assert_eq!(reopened.feature(l).unwrap().kind, d.feature(l).unwrap().kind);
    assert!(status(&reopened, l).is_ok());
    assert_eq!(reopened.part_json(), d.part_json(), "saving an opened file reproduces it");

    let (g, _, gid) = g0();
    assert_eq!(
        saved(&g)["level"],
        serde_json::json!({"linear":{"axis":"z","from":-1.0,"to":1.0,"start":-0.1,"end":0.1}})
    );
    let reopened = Document::from_bytes(&g.to_bytes().unwrap(), kernel()).unwrap();
    assert_eq!(reopened.feature(gid).unwrap().kind, g.feature(gid).unwrap().kind);

    // A constant keeps the form every earlier file has: a bare number.
    let mut c = Document::new("constant", kernel());
    let s = add(&mut c, sketch(20.0, 20.0, -5.0));
    add(&mut c, boss(s, 10.0));
    add(&mut c, gyroid(Grade::Constant(-0.5)));
    assert_eq!(saved(&c)["level"], serde_json::json!(-0.5));
    let mut c = Document::new("constant", kernel());
    let s = add(&mut c, sketch(20.0, 10.0, 0.0));
    add(&mut c, boss(s, 5.0));
    add(&mut c, FeatureKind::Lattice(LatticeFeature::default()));
    assert_eq!(saved(&c)["radius_mm"], serde_json::json!(1.0));
}

#[test]
fn unknown_grade_data_refuses_the_file_instead_of_becoming_a_constant() {
    let (d, _, _) = d0();
    let saved: serde_json::Value = serde_json::from_str(&d.part_json()).unwrap();
    let field = |part: &mut serde_json::Value| -> serde_json::Value {
        let features = part["features"].as_array_mut().unwrap();
        features.last_mut().unwrap()["kind"].take()
    };
    let replaced = |change: &dyn Fn(&mut serde_json::Value)| {
        let mut part = saved.clone();
        let mut kind = field(&mut part);
        change(&mut kind);
        part["features"].as_array_mut().unwrap().last_mut().unwrap()["kind"] = kind;
        part.to_string()
    };
    let future = [
        replaced(&|k| k["radius_mm"] = serde_json::json!({"radial":{"axis":"z","from":0,"to":1,"start":1,"end":2}})),
        replaced(&|k| k["radius_mm"]["linear"]["span"] = serde_json::json!(1)),
        replaced(&|k| k["radius_mm"] = serde_json::json!("sin(z)")),
        replaced(&|k| {
            k.as_object_mut().unwrap().remove("radius_mm");
            k["radius_distance"] = serde_json::json!({"near":0.1,"far":0.2,"span":1});
        }),
    ];
    for text in future {
        assert!(Document::from_part_json(&text, kernel()).is_err(), "{text}");
    }
    // An old two-setting file still opens, with its missing input reported.
    let legacy = replaced(&|k| {
        k.as_object_mut().unwrap().retain(|key, _| ["type", "accuracy_mm", "min_detail_mm"].contains(&key.as_str()))
    });
    let opened = Document::from_part_json(&legacy, kernel()).unwrap();
    let FeatureKind::Lattice(l) = opened.features().last().unwrap().kind else { panic!("lattice") };
    assert_eq!(l.radius_mm, None);
}

#[test]
fn grade_values_are_checked_before_the_document_changes() {
    let (mut d, _, l) = d0();
    let (body, saved, depth) = (d.body(), d.part_json(), d.undo_depth());
    for radius in [
        linear(20.0, 30.0, 0.0, 2.0),
        linear(20.0, 30.0, 1.0, -0.001),
        linear(20.0, 20.0, 1.0, 2.0),
        linear(20.0, f64::NAN, 1.0, 2.0),
        linear(20.0, 30.0, f64::INFINITY, 2.0),
    ] {
        assert!(matches!(edit(&mut d, l, lattice(radius)), Err(CommandError::Invalid(_))), "{radius:?}");
        assert_eq!((d.body(), d.part_json(), d.undo_depth()), (body, saved.clone(), depth));
    }
    let (mut g, _, gid) = g0();
    for level in [linear(1.0, 1.0, -0.1, 0.1), linear(-1.0, 1.0, f64::NAN, 0.1)] {
        assert!(matches!(edit(&mut g, gid, gyroid(level)), Err(CommandError::Invalid(_))), "{level:?}");
    }
    // Negative and zero levels are valid; the kernel decides what it can certify.
    edit(&mut g, gid, gyroid(linear(-1.0, 1.0, -0.1, 0.0))).unwrap();
    assert!(status(&g, gid).is_ok(), "{:?}", status(&g, gid));
    // Out of the certified range: the feature keeps the edit and shows the coverage code.
    edit(&mut g, gid, gyroid(linear(-1.0, 1.0, -0.1, 0.100001))).unwrap();
    let FeatureStatus::Failed(message) = status(&g, gid) else { panic!("{:?}", status(&g, gid)) };
    assert!(message.contains("gyroid.grade-range-unsupported"), "{message}");
    g.undo().unwrap();
    assert!(status(&g, gid).is_ok());
}

#[test]
fn upstream_edits_keep_the_anchors_and_rebuild_the_field() {
    let k = kernel();
    let mut d = Document::new("D0", k.clone());
    let s = add(&mut d, sketch(6.0, 6.0, 20.0));
    let b = add(&mut d, boss(s, 10.0));
    let l = add(&mut d, lattice(linear(20.0, 30.0, 1.0, 2.0)));
    let d0_step = field_step(&d, &k);
    let d0_kind = d.feature(l).unwrap().kind.clone();

    // D1: the region grows to z = 32; the grade still ends at 30 and clamps above it.
    edit(&mut d, b, boss(s, 12.0)).unwrap();
    assert!(status(&d, l).is_ok());
    assert_eq!(d.feature(l).unwrap().kind, d0_kind, "the anchors do not follow the size");
    assert_eq!(field_step(&d, &k)["radius"], d0_step["radius"]);
    let d1_bytes = d.export(soliddraft_exchange::Format::Stl).unwrap().bytes;

    // Back to D0, then D2's reversed grade.
    d.undo().unwrap();
    assert_eq!(field_step(&d, &k), d0_step);
    edit(&mut d, l, lattice(linear(20.0, 30.0, 2.0, 1.0))).unwrap();
    assert!(status(&d, l).is_ok());
    d.undo().unwrap();
    assert_eq!(d.feature(l).unwrap().kind, d0_kind);
    d.redo().unwrap();
    assert_eq!(d.feature(l).unwrap().kind, lattice(linear(20.0, 30.0, 2.0, 1.0)));

    // A rename upstream changes no geometry.
    let before = field_step(&d, &k);
    d.execute(Command::RenameFeature { id: b, name: "Region".into() }).unwrap();
    assert_eq!(field_step(&d, &k), before);

    // Returning before the field restores CAD; rolling forward rebuilds the same field.
    d.return_before_lattice().unwrap();
    assert_eq!(status(&d, l), FeatureStatus::RolledBack);
    assert!(d.cad_after_field().is_none());
    d.execute(Command::RollTo { before: None }).unwrap();
    assert!(status(&d, l).is_ok());
    assert_eq!(field_step(&d, &k), before);

    // The saved part reopens on a fresh kernel to the same native field and the same bytes.
    edit(&mut d, b, boss(s, 12.0)).unwrap();
    let fresh = kernel();
    let reopened = Document::from_bytes(&d.to_bytes().unwrap(), fresh.clone()).unwrap();
    assert_eq!(field_step(&reopened, &fresh)["radius"], field_step(&d, &k)["radius"]);
    let reversed = reopened.export(soliddraft_exchange::Format::Stl).unwrap().bytes;
    assert_ne!(reversed, d1_bytes, "the reversed grade is another body");
    assert_eq!(reversed, d.export(soliddraft_exchange::Format::Stl).unwrap().bytes);
}

#[test]
fn a_field_part_writes_its_labelled_mesh_or_its_explicit_stand_in_and_nothing_else() {
    use soliddraft_exchange::{Format, Report};
    let (mut d, _, l) = g0();
    let mesh = d.export(Format::Stl).unwrap();
    let Report::FieldMesh(r) = &mesh.report else { panic!("labelled field mesh") };
    assert_eq!((r.label.as_str(), r.minimum_detail_mm.as_str()), ("catallax-print-stl/1", "1/10"));
    for format in [Format::Step, Format::ThreeMf, Format::Obj] {
        let refused = d.export(format).map(|_| ()).unwrap_err();
        assert!(refused.contains("field parts export as meshes"), "{refused}");
    }
    let stand_in = d.export(Format::StandIn).unwrap();
    let Report::StandIn(r) = &stand_in.report else { panic!("stand-in") };
    assert_eq!(r.label, "mass stand-in — not field boundary geometry");
    assert!(std::str::from_utf8(&stand_in.bytes).unwrap().contains("SHELL_BASED_SURFACE_MODEL"));
    let mass = d.field_mass(&FieldMassAccuracy::default()).unwrap();
    assert_eq!(mass.volume_mm3, r.field.volume_mm3, "one definition, one answer");

    // Returning before the field: an exact STEP again, and no stand-in of a B-rep.
    d.return_before_lattice().unwrap();
    assert_eq!(status(&d, l), FeatureStatus::RolledBack);
    assert!(matches!(d.export(Format::Step).unwrap().report, Report::Step(_)));
    assert!(d.export(Format::StandIn).map(|_| ()).unwrap_err().contains("needs an active lattice or gyroid"));
    assert!(d.field_mass(&FieldMassAccuracy::default()).is_err());
}

#[test]
fn no_cad_follows_an_active_field() {
    let (mut d, b, l) = d0();
    let refusal = d.cad_after_field().unwrap();
    assert_eq!(refusal, CommandError::AfterLattice(l));
    assert_eq!(refusal.kernel_code(), Some("lattice.cad-after-lattice"));
    let (count, depth) = (d.features().len(), d.undo_depth());
    let added = d.execute(Command::AddFeature {
        kind: sketch(2.0, 2.0, 22.0),
        name: None,
        position: None,
        via_assistant: false,
    });
    assert_eq!(added.map(|_| ()), Err(CommandError::AfterLattice(l)));
    assert_eq!((d.features().len(), d.undo_depth()), (count, depth));
    let (g, _, gid) = g0();
    assert_eq!(g.cad_after_field(), Some(CommandError::AfterGyroid(gid)));

    // A saved file with CAD after the field is damaged, not another part.
    let mut part: serde_json::Value = serde_json::from_str(&d.part_json()).unwrap();
    let features = part["features"].as_array_mut().unwrap();
    let field = features.pop().unwrap();
    features.insert(features.iter().position(|f| f["id"] == serde_json::json!(b.0)).unwrap(), field);
    let error = Document::from_part_json(&part.to_string(), kernel()).map(|_| ()).unwrap_err().to_string();
    assert!(error.contains("must stay last"), "{error}");
}
