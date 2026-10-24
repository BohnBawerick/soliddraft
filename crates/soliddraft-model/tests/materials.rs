//! A part's material and appearances on the Catallax backend: assignment, precedence,
//! faces that split or disappear, configuration scopes, mass, undo and files.

use std::sync::Arc;

use soliddraft_document::{ConfigScope, ConfigurationId};
use soliddraft_kernel_api::{FaceId, Vec3};
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_materials::{Appearance, AppearanceId, Catalog, MaterialId, Srgb, Unknown};
use soliddraft_model::sketch::{P2, Sketch, SolveStatus, solve::solve};
use soliddraft_model::{
    AppearanceTarget, Command, CommandError, DatumPlane, Document, EndCondition, ExtrudeFeature, ExtrudeOp, FeatureId,
    FeatureKind, FileError, MassError, ProfileSource, RepairReason, SketchFeature, SketchPlane,
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

fn sketch_at(offset: f64, build: impl FnOnce(&mut Sketch)) -> FeatureKind {
    let mut s = Sketch::new();
    build(&mut s);
    solve(&mut s, None, true);
    assert_eq!(s.status, SolveStatus::FullyDefined);
    FeatureKind::Sketch(SketchFeature { plane: SketchPlane::Datum { datum: DatumPlane::Top, offset }, sketch: s })
}

fn extrude(sketch: FeatureId, operation: ExtrudeOp, depth: f64, reverse: bool) -> FeatureKind {
    FeatureKind::Extrude(ExtrudeFeature {
        profile: ProfileSource::Sketch { feature: sketch },
        operation,
        end: EndCondition::Blind { depth },
        reverse,
    })
}

/// A 40 x 30 x 10 block centred on the z axis: a sketch and an extrusion.
fn block(doc: &mut Document) -> FeatureId {
    let s = add(
        doc,
        sketch_at(0.0, |s| {
            s.rectangle(P2::new(0.0, 0.0), 40.0, 30.0).unwrap();
        }),
    );
    add(doc, extrude(s, ExtrudeOp::Boss, 10.0, false))
}

/// The faces of the part's body whose normal is `normal`.
fn faces_facing(doc: &Document, normal: Vec3) -> Vec<FaceId> {
    let t = doc.kernel().topology(doc.body().unwrap()).unwrap();
    t.faces.iter().filter(|f| f.normal.is_some_and(|n| n.dot(normal) > 1.0 - 1e-9)).map(|f| f.id).collect()
}

fn top_face_target(doc: &Document) -> AppearanceTarget {
    let top = faces_facing(doc, Vec3::Z);
    assert_eq!(top.len(), 1);
    AppearanceTarget::Face { face: doc.face_ref(top[0]).unwrap() }
}

fn appearance(catalog: &Catalog, id: &str) -> Appearance {
    catalog.appearance(&AppearanceId(id.into())).unwrap().1.clone()
}

fn paint(doc: &mut Document, target: AppearanceTarget, a: &Appearance) {
    doc.execute(Command::SetAppearance { target, appearance: Some(a.clone()), scope: ConfigScope::All }).unwrap();
}

#[test]
fn a_new_part_has_no_material_and_an_unknown_mass() {
    let mut d = doc();
    block(&mut d);
    assert!(d.material(None).is_none());
    assert_eq!(d.mass(None), Err(MassError::Unknown(Unknown::NoMaterial)));
    assert_eq!(d.part_appearance(None), Appearance::default_part());
    assert!(d.appearance_table(None).faces.is_empty());
    assert!(d.materials().is_empty());
}

#[test]
fn a_material_gives_mass_and_its_default_appearance() {
    let catalog = Catalog::built_in();
    let mut d = doc();
    block(&mut d);
    let id = MaterialId("aluminium-6061-t6".into());
    d.execute(Command::set_material_from(&catalog, &id).unwrap()).unwrap();
    let m = d.material(None).unwrap();
    assert_eq!(m.name, "Aluminium 6061-T6");
    let mass = d.mass(None).unwrap();
    // 12 000 mm³ at 2713 kg/m³.
    assert!((mass.mass_kg - 12000.0 * 2713.0 * 1e-9).abs() < 1e-9, "{}", mass.mass_kg);
    assert!(mass.centre_mm[0].abs() < 1e-6 && (mass.centre_mm[2] - 5.0).abs() < 1e-6, "{:?}", mass.centre_mm);
    assert_eq!(d.part_appearance(None).id.as_str(), "aluminium-brushed");
    assert_eq!(d.undo_label(), Some("set material Aluminium 6061-T6"));

    // A material without a density leaves the mass unknown.
    let mut no_density = m.clone();
    no_density.id = MaterialId("mystery".into());
    no_density.name = "Mystery alloy".into();
    no_density.density_kg_m3 = None;
    no_density.default_appearance = None;
    let set = Command::SetMaterial { material: Some(no_density), default_appearance: None, scope: ConfigScope::All };
    d.execute(set).unwrap();
    assert_eq!(d.mass(None).unwrap_err().to_string(), "Mystery alloy has no density");
    assert_eq!(d.materials().snapshots.materials.len(), 1, "the replaced material's copy goes");
    assert_eq!(d.part_appearance(None), Appearance::default_part());

    d.undo().unwrap();
    assert_eq!(d.material(None).unwrap().id, id);
    d.undo().unwrap();
    assert!(d.materials().is_empty());
    d.redo().unwrap();
    assert_eq!(d.part_appearance(None).id.as_str(), "aluminium-brushed");
}

#[test]
fn the_most_specific_appearance_wins() {
    let catalog = Catalog::built_in();
    let mut d = doc();
    let e = block(&mut d);
    let (red, blue, black) = (
        appearance(&catalog, "paint-red"),
        appearance(&catalog, "anodised-blue"),
        appearance(&catalog, "anodised-black"),
    );
    let top = top_face_target(&d);
    paint(&mut d, top.clone(), &red);
    paint(&mut d, AppearanceTarget::Feature { feature: e }, &blue);
    paint(&mut d, AppearanceTarget::Part, &black);
    let table = d.appearance_table(None);
    assert!(table.lost.is_empty());
    assert_eq!(table.appearances[0].id.as_str(), "anodised-black");
    let top_id = faces_facing(&d, Vec3::Z)[0];
    assert_eq!(table.of(top_id).id.as_str(), "paint-red", "the face beats its feature");
    let bottom = faces_facing(&d, -Vec3::Z)[0];
    assert_eq!(table.of(bottom).id.as_str(), "anodised-blue", "the feature beats the part");
    assert_eq!(table.faces.len(), 6, "every face of the block is the feature's");

    // Painting the same face again replaces the assignment rather than adding one.
    let face_now = AppearanceTarget::Face { face: d.face_ref(top_id).unwrap() };
    paint(&mut d, face_now, &blue);
    assert_eq!(d.materials().appearances.len(), 3);
    assert!(d.materials().snapshots.appearance(&red.id).is_none(), "the unused copy is dropped");
    d.execute(Command::SetAppearance { target: top, appearance: None, scope: ConfigScope::All }).unwrap();
    assert_eq!(d.materials().appearances.len(), 2);
    assert_eq!(d.undo_label().unwrap(), "remove appearance from Boss-Extrude1 · End cap");
}

#[test]
fn a_split_face_keeps_its_appearance_and_a_lost_one_is_reported() {
    let catalog = Catalog::built_in();
    let mut d = doc();
    block(&mut d);
    let red = appearance(&catalog, "paint-red");
    let top = top_face_target(&d);
    paint(&mut d, top, &red);
    // A slot across the top splits the top face in two.
    let s = add(
        &mut d,
        sketch_at(10.0, |s| {
            s.rectangle(P2::new(0.0, 0.0), 10.0, 40.0).unwrap();
        }),
    );
    let cut = add(&mut d, extrude(s, ExtrudeOp::Cut, 4.0, true));
    assert!(d.status(cut).unwrap().is_ok(), "{:?}", d.status(cut));
    let tops: Vec<FaceId> = faces_facing(&d, Vec3::Z)
        .into_iter()
        .filter(|f| d.kernel().topology(d.body().unwrap()).unwrap().face(*f).unwrap().centroid.z > 9.0)
        .collect();
    assert_eq!(tops.len(), 2);
    let table = d.appearance_table(None);
    assert!(table.lost.is_empty(), "{:?}", table.lost);
    for f in &tops {
        assert_eq!(table.of(*f).id.as_str(), "paint-red", "both pieces keep the colour");
    }
    let slot_floor =
        faces_facing(&d, Vec3::Z).into_iter().find(|f| !tops.contains(f)).expect("the slot's floor faces up too");
    assert_eq!(table.of(slot_floor), &Appearance::default_part(), "the cut's own faces are not the top");

    // A feature appearance on a feature that is deleted is lost, and comes back on undo.
    let blue = appearance(&catalog, "anodised-blue");
    paint(&mut d, AppearanceTarget::Feature { feature: cut }, &blue);
    assert_eq!(d.appearance_table(None).of(slot_floor).id.as_str(), "anodised-blue");
    d.execute(Command::DeleteFeature { id: cut }).unwrap();
    let lost = d.appearance_table(None).lost;
    assert_eq!(lost.len(), 1);
    assert_eq!(lost[0].reason, RepairReason::Missing);
    d.undo().unwrap();
    assert!(d.appearance_table(None).lost.is_empty());
}

#[test]
fn a_face_that_is_gone_is_reported_not_moved() {
    let catalog = Catalog::built_in();
    let mut d = doc();
    let e = block(&mut d);
    let s = match &d.feature(e).unwrap().kind {
        FeatureKind::Extrude(x) => match x.profile {
            ProfileSource::Sketch { feature } => feature,
            _ => unreachable!(),
        },
        _ => unreachable!(),
    };
    let top = top_face_target(&d);
    paint(&mut d, top, &appearance(&catalog, "paint-yellow"));
    // Extruding the other way makes the end cap face down: not the face that was painted.
    d.execute(Command::EditFeature { id: e, kind: extrude(s, ExtrudeOp::Boss, 10.0, true) }).unwrap();
    let table = d.appearance_table(None);
    assert_eq!(table.lost.len(), 1);
    assert!(matches!(table.lost[0].reason, RepairReason::Mismatch { .. }), "{:?}", table.lost);
    assert!(table.lost[0].message.starts_with("Boss-Extrude1 · End cap: "), "{}", table.lost[0].message);
    assert!(table.faces.is_empty(), "no other face takes the colour");
    d.undo().unwrap();
    let table = d.appearance_table(None);
    assert!(table.lost.is_empty());
    assert_eq!(table.of(faces_facing(&d, Vec3::Z)[0]).id.as_str(), "paint-yellow");

    // A feature target must exist when it is set.
    let paint_missing = Command::SetAppearance {
        target: AppearanceTarget::Feature { feature: FeatureId(99) },
        appearance: Some(appearance(&catalog, "paint-red")),
        scope: ConfigScope::All,
    };
    assert_eq!(d.execute(paint_missing), Err(CommandError::UnknownFeature(FeatureId(99))));
}

#[test]
fn configurations_choose_their_own_colour() {
    let catalog = Catalog::built_in();
    let mut d = doc();
    block(&mut d);
    let long = ConfigurationId("long".into());
    paint(&mut d, AppearanceTarget::Part, &appearance(&catalog, "paint-blue"));
    let red = appearance(&catalog, "paint-red");
    let only_long = ConfigScope::Only(vec![long.clone()]);
    d.execute(Command::SetAppearance { target: AppearanceTarget::Part, appearance: Some(red), scope: only_long })
        .unwrap();
    assert_eq!(d.part_appearance(None).id.as_str(), "paint-blue");
    assert_eq!(d.part_appearance(Some(&long)).id.as_str(), "paint-red");
    assert_eq!(d.part_appearance(Some(&ConfigurationId("short".into()))).id.as_str(), "paint-blue");
}

#[test]
fn copies_survive_files_and_follow_library_updates_only_on_request() {
    let catalog = Catalog::built_in();
    let mut d = doc();
    block(&mut d);
    let geometry = d.geometry_hash();
    let revision = d.semantic_hash();
    d.execute(Command::set_material_from(&catalog, &MaterialId("oak-white".into())).unwrap()).unwrap();
    let mut custom = Appearance::new("my-teal", "Teal", Srgb::new(0, 128, 128));
    custom.roughness = 0.3;
    let top = top_face_target(&d);
    paint(&mut d, top, &custom);
    assert_eq!(d.geometry_hash(), geometry, "colours and materials do not change the shape");
    assert_ne!(d.semantic_hash(), revision, "they are part of the revision");

    let json = d.part_json();
    assert!(json.contains("\"on\": \"face\"") && json.contains("\"scope\": \"all\""), "{json}");
    let back = Document::from_bytes(&d.to_bytes().unwrap(), Arc::new(CatallaxKernel::new())).unwrap();
    assert_eq!(back.materials(), d.materials());
    assert_eq!(back.part_json(), json, "a saved file round-trips exactly");
    assert_eq!(back.semantic_hash(), d.semantic_hash());
    let top = faces_facing(&back, Vec3::Z)[0];
    assert_eq!(back.appearance_table(None).of(top), &custom, "the copy, not a library, decides the look");

    // Updating a copy is a command; one the part does not use is refused.
    let mut lighter = custom.clone();
    lighter.base_color = Srgb::new(64, 160, 160);
    d.execute(Command::UpdateCopies { materials: Vec::new(), appearances: vec![lighter.clone()] }).unwrap();
    assert_eq!(d.materials().snapshots.appearance(&lighter.id), Some(&lighter));
    let stranger = appearance(&catalog, "chrome");
    let refused = d.execute(Command::UpdateCopies { materials: Vec::new(), appearances: vec![stranger] });
    assert!(matches!(refused, Err(CommandError::Invalid(_))));

    // A file whose assignment has no copy is refused rather than shown wrong.
    let broken = json.replace("\"id\": \"my-teal\"", "\"id\": \"my-other-teal\"");
    let err = Document::from_part_json(&broken, Arc::new(CatallaxKernel::new())).unwrap_err();
    assert!(matches!(err, FileError::Invalid(ref m) if m.contains("my-teal")), "{err}");
}

#[test]
fn commands_with_bad_values_are_refused() {
    let mut d = doc();
    block(&mut d);
    let mut bad = Appearance::new("bad", "Bad", Srgb::WHITE);
    bad.opacity = 2.0;
    let r = d.execute(Command::SetAppearance {
        target: AppearanceTarget::Part,
        appearance: Some(bad),
        scope: ConfigScope::All,
    });
    assert!(matches!(r, Err(CommandError::Invalid(m)) if m.contains("opacity")));
    let catalog = Catalog::built_in();
    let mut set = Command::set_material_from(&catalog, &MaterialId("abs".into())).unwrap();
    if let Command::SetMaterial { default_appearance, .. } = &mut set {
        *default_appearance = Some(appearance(&catalog, "chrome"));
    }
    assert!(matches!(d.execute(set), Err(CommandError::Invalid(_))), "only the material's own default");
    assert!(d.materials().is_empty());
    // Commands round-trip as JSON, for the assistant and scripts.
    let c = Command::set_material_from(&catalog, &MaterialId("abs".into())).unwrap();
    let text = serde_json::to_string(&c).unwrap();
    assert!(text.starts_with(r#"{"command":"set_material""#), "{text}");
    assert_eq!(serde_json::from_str::<Command>(&text).unwrap(), c);
}
