//! Document behaviour on the Catallax backend: commands, undo and redo, reorder, suppress,
//! rollback, references and files.

use std::sync::Arc;

use soliddraft_kernel_api::{Kernel, Vec3};
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::sketch::{P2, Sketch, SolveStatus, solve::solve};
use soliddraft_model::{
    BlendFeature, BuiltinShape, Command, CommandError, DatumPlane, Document, EndCondition, ExtrudeFeature, ExtrudeOp,
    FeatureId, FeatureKind, FeatureStatus, FileError, Generator, ProfileSource, SketchFeature, SketchPlane,
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

fn top_sketch(build: impl FnOnce(&mut Sketch)) -> FeatureKind {
    let mut s = Sketch::new();
    build(&mut s);
    solve(&mut s, None, true);
    assert_eq!(s.status, SolveStatus::FullyDefined);
    FeatureKind::Sketch(SketchFeature { plane: SketchPlane::Datum { datum: DatumPlane::Top, offset: 0.0 }, sketch: s })
}

fn boss(sketch: FeatureId, depth: f64) -> FeatureKind {
    FeatureKind::Extrude(ExtrudeFeature {
        profile: ProfileSource::Sketch { feature: sketch },
        operation: ExtrudeOp::Boss,
        end: EndCondition::Blind { depth },
        reverse: false,
    })
}

fn volume(doc: &Document) -> f64 {
    doc.kernel().mass_properties(doc.body().unwrap()).unwrap().volume
}

/// A 40 x 30 x 10 block: a sketch and an extrusion.
fn block(doc: &mut Document) -> (FeatureId, FeatureId) {
    let s = add(
        doc,
        top_sketch(|s| {
            s.rectangle(P2::new(0.0, 0.0), 40.0, 30.0).unwrap();
        }),
    );
    let e = add(doc, boss(s, 10.0));
    (s, e)
}

fn assert_waiting(doc: &Document, feature: FeatureId) {
    assert!(
        matches!(doc.status(feature), Some(FeatureStatus::Failed(error))
        if error.contains("Catallax stage 3") && error.contains("Catallax stage 4")),
        "{:?}",
        doc.status(feature)
    );
}

/// The face of the final body that carries a name's origin.
fn carrier(doc: &Document, name: &soliddraft_model::NamedFace) -> soliddraft_kernel_api::FaceId {
    let t = doc.kernel().topology(doc.body().unwrap()).unwrap();
    let mut faces = t.faces.iter().filter(|f| f.origins.contains(&name.origin));
    let face = faces.next().expect("a face carries the origin").id;
    assert!(faces.next().is_none(), "one face carries the origin");
    face
}

fn top_edges(doc: &Document) -> Vec<soliddraft_model::EdgeRef> {
    let t = doc.kernel().topology(doc.body().unwrap()).unwrap();
    t.edges.iter().filter(|e| e.midpoint.z > 10.0 - 1e-6).map(|e| doc.edge_ref(e.id).unwrap()).collect()
}

#[test]
fn features_get_names_and_faces_get_labels() {
    let mut d = doc();
    let (s, e) = block(&mut d);
    assert_eq!(d.feature(s).unwrap().name, "Sketch1");
    assert_eq!(d.feature(e).unwrap().name, "Boss-Extrude1");
    assert!((volume(&d) - 12000.0).abs() < 1e-6);
    let generators: Vec<Generator> = d.generated_faces(e).iter().map(|n| n.generator).collect();
    assert!(generators.contains(&Generator::StartCap) && generators.contains(&Generator::EndCap));
    assert_eq!(generators.iter().filter(|g| matches!(g, Generator::Side { .. })).count(), 4);
    let end = carrier(&d, d.generated_faces(e).iter().find(|n| n.generator == Generator::EndCap).unwrap());
    assert_eq!(d.face_label(end).unwrap().text, "Boss-Extrude1 · End cap");
    let side =
        carrier(&d, d.generated_faces(e).iter().find(|n| matches!(n.generator, Generator::Side { .. })).unwrap());
    assert!(d.face_label(side).unwrap().text.starts_with("Boss-Extrude1 · Side of line "));
}

#[test]
fn undo_and_redo_walk_the_command_stack() {
    let mut d = doc();
    let (_, e) = block(&mut d);
    let edges = top_edges(&d);
    let f = add(&mut d, FeatureKind::Fillet(BlendFeature { edges, size: 2.0 }));
    let filleted = volume(&d);
    assert!((filleted - 12000.0).abs() < 1e-6);
    assert_waiting(&d, f);
    assert_eq!(d.undo_label(), Some("add Fillet1"));
    d.undo().unwrap();
    assert!(d.feature(f).is_none());
    assert!((volume(&d) - 12000.0).abs() < 1e-6);
    d.redo().unwrap();
    assert_eq!(d.feature(f).unwrap().name, "Fillet1", "redo restores the same feature id");
    assert!((volume(&d) - filleted).abs() < 1e-9 * filleted);
    // A new command clears the redo stack.
    d.undo().unwrap();
    d.execute(Command::RenameFeature { id: e, name: "Block".into() }).unwrap();
    assert!(matches!(d.redo(), Err(CommandError::Empty("redo"))));
    // Undo all the way back to an empty part.
    while d.undo().is_ok() {}
    assert!(d.features().is_empty());
    assert!(d.body().is_none());
}

#[test]
fn edits_rebuild_only_from_the_first_changed_feature() {
    let mut d = doc();
    let (s, e) = block(&mut d);
    let edges = top_edges(&d);
    let f = add(&mut d, FeatureKind::Fillet(BlendFeature { edges, size: 2.0 }));
    let (block_body, fillet_body) = (d.body_after(e), d.body_after(f));
    d.execute(Command::RenameFeature { id: f, name: "Round".into() }).unwrap();
    assert_eq!(d.body_after(e), block_body, "a rename rebuilds nothing");
    d.execute(Command::EditFeature {
        id: f,
        kind: FeatureKind::Fillet(BlendFeature { edges: top_edges_of(&d, e), size: 3.0 }),
    })
    .unwrap();
    assert_eq!(d.body_after(e), block_body, "editing the fillet keeps the block");
    assert_eq!(d.body_after(f), fillet_body, "an unsupported fillet passes the same body through");
    assert_waiting(&d, f);
    d.execute(Command::EditFeature { id: e, kind: boss(s, 20.0) }).unwrap();
    assert_ne!(d.body_after(e), block_body, "editing the block rebuilds it");
    assert_waiting(&d, f);
    assert_eq!(d.body_after(f), d.body_after(e));
}

fn top_edges_of(d: &Document, feature: FeatureId) -> Vec<soliddraft_model::EdgeRef> {
    let body = d.body_after(feature).unwrap();
    let t = d.kernel().topology(body).unwrap();
    let z = t.edges.iter().map(|e| e.midpoint.z).fold(f64::NEG_INFINITY, f64::max);
    t.edges.iter().filter(|e| e.midpoint.z > z - 1e-6).map(|e| d.edge_ref(e.id).unwrap()).collect()
}

#[test]
fn sketch_dimensions_drive_the_part() {
    let mut d = doc();
    let mut sketch = Sketch::new();
    let shape = sketch.rectangle(P2::new(0.0, 0.0), 40.0, 30.0).unwrap();
    solve(&mut sketch, None, true);
    assert_eq!(sketch.status, SolveStatus::FullyDefined);
    let s = add(
        &mut d,
        FeatureKind::Sketch(SketchFeature {
            plane: SketchPlane::Datum { datum: DatumPlane::Top, offset: 0.0 },
            sketch,
        }),
    );
    add(&mut d, boss(s, 10.0));
    let width = shape.dimensions[0];
    d.execute(Command::SetSketchDimension { id: s, constraint: width, value: 50.0 }).unwrap();
    assert_eq!(d.last_solve(), Some(&SolveStatus::FullyDefined));
    assert!((volume(&d) - 15000.0).abs() < 1e-6, "a 50 mm wide block");
    d.undo().unwrap();
    assert!((volume(&d) - 12000.0).abs() < 1e-6);
}

#[test]
fn reorder_keeps_inputs_above_their_readers() {
    let mut d = doc();
    let (s, e) = block(&mut d);
    let s2 = add(
        &mut d,
        top_sketch(|s| {
            s.circle_at(P2::new(0.0, 0.0), 10.0).unwrap();
        }),
    );
    let cut = add(
        &mut d,
        FeatureKind::Extrude(ExtrudeFeature {
            profile: ProfileSource::Sketch { feature: s2 },
            operation: ExtrudeOp::Cut,
            end: EndCondition::ThroughAll,
            reverse: false,
        }),
    );
    assert!(d.status(cut).unwrap().is_ok(), "{:?}", d.status(cut));
    assert!(matches!(d.execute(Command::MoveFeature { id: e, position: 0 }), Err(CommandError::Order(_, _))));
    assert!(matches!(d.execute(Command::MoveFeature { id: cut, position: 2 }), Err(CommandError::Order(_, _))));
    // The second sketch reads nothing, so it may move to the top.
    d.execute(Command::MoveFeature { id: s2, position: 0 }).unwrap();
    assert_eq!(d.features().iter().map(|f| f.id).collect::<Vec<_>>(), [s2, s, e, cut]);
    assert!(d.features().iter().all(|f| d.status(f.id).unwrap().is_ok()));
    d.undo().unwrap();
    assert_eq!(d.features().iter().map(|f| f.id).collect::<Vec<_>>(), [s, e, s2, cut]);
}

#[test]
fn suppress_and_rollback_pass_the_body_through() {
    let mut d = doc();
    let (_, e) = block(&mut d);
    let edges = top_edges(&d);
    let f = add(&mut d, FeatureKind::Fillet(BlendFeature { edges, size: 2.0 }));
    let filleted = volume(&d);
    d.execute(Command::SetSuppressed { id: f, suppressed: true }).unwrap();
    assert_eq!(d.status(f), Some(&FeatureStatus::Suppressed));
    assert!((volume(&d) - 12000.0).abs() < 1e-6);
    d.undo().unwrap();
    assert!((volume(&d) - filleted).abs() < 1e-9 * filleted);

    d.execute(Command::RollTo { before: Some(f) }).unwrap();
    assert_eq!(d.status(f), Some(&FeatureStatus::RolledBack));
    assert_eq!(d.active_count(), 2);
    assert!((volume(&d) - 12000.0).abs() < 1e-6);
    // A feature added while rolled back goes above the bar.
    let s = add(
        &mut d,
        top_sketch(|s| {
            s.circle_at(P2::new(0.0, 0.0), 4.0).unwrap();
        }),
    );
    assert_eq!(d.index_of(s), Some(2));
    assert_eq!(d.active_count(), 3);
    d.execute(Command::RollTo { before: None }).unwrap();
    assert_waiting(&d, f);
    assert!(d.body_after(e).is_some());
}

#[test]
fn deleting_a_sketch_fails_its_extrusion_and_undo_restores_it() {
    let mut d = doc();
    let (s, e) = block(&mut d);
    d.execute(Command::DeleteFeature { id: s }).unwrap();
    assert!(matches!(d.status(e), Some(FeatureStatus::Failed(m)) if m.contains("missing")), "{:?}", d.status(e));
    assert!(d.body().is_none());
    d.undo().unwrap();
    assert_eq!(d.index_of(s), Some(0));
    assert!(d.status(e).unwrap().is_ok());
}

#[test]
fn a_batch_is_one_undo_entry_and_all_or_nothing() {
    let mut d = doc();
    let (_, e) = block(&mut d);
    let bad = Command::Batch {
        label: "rename and break".into(),
        commands: vec![
            Command::RenameFeature { id: e, name: "Plate".into() },
            Command::DeleteFeature { id: FeatureId(99) },
        ],
    };
    assert!(matches!(d.execute(bad), Err(CommandError::UnknownFeature(FeatureId(99)))));
    assert_eq!(d.feature(e).unwrap().name, "Boss-Extrude1", "a refused batch changes nothing");
    let good = Command::Batch {
        label: "rename twice".into(),
        commands: vec![
            Command::RenameFeature { id: e, name: "Plate".into() },
            Command::RenameFeature { id: e, name: "Base".into() },
        ],
    };
    d.execute(good).unwrap();
    assert_eq!(d.feature(e).unwrap().name, "Base");
    assert_eq!(d.undo().unwrap(), "rename twice");
    assert_eq!(d.feature(e).unwrap().name, "Boss-Extrude1");
}

#[test]
fn grouped_commands_undo_redo_and_abandon_as_one() {
    let mut d = doc();
    let depth = d.undo_depth();
    let (s, e) = block(&mut d);
    d.execute(Command::RenameFeature { id: e, name: "Plate".into() }).unwrap();
    d.group_undo(depth, "assistant proposal");
    assert_eq!(d.undo_depth(), depth + 1);
    assert_eq!(d.undo_label(), Some("assistant proposal"));
    assert_eq!(d.undo().unwrap(), "assistant proposal");
    assert!(d.features().is_empty(), "one undo reverts the whole group");
    d.redo().unwrap();
    assert_eq!(d.feature(e).unwrap().name, "Plate");
    assert!(d.feature(s).is_some());

    let depth = d.undo_depth();
    d.execute(Command::RenameFeature { id: e, name: "Base".into() }).unwrap();
    d.execute(Command::DeleteFeature { id: e }).unwrap();
    d.abandon(depth).unwrap();
    assert_eq!(d.feature(e).unwrap().name, "Plate", "abandon puts the part back");
    assert_eq!(d.undo_depth(), depth);
    assert_eq!(d.redo_label(), None, "and leaves nothing to redo");
    d.log_note("asked by the assistant");
    assert_eq!(d.log().last().map(String::as_str), Some("asked by the assistant"));
}

#[test]
fn invalid_commands_are_refused() {
    let mut d = doc();
    let (s, _) = block(&mut d);
    let zero = FeatureKind::Fillet(BlendFeature { edges: top_edges(&d), size: 0.0 });
    assert!(matches!(
        d.execute(Command::AddFeature { kind: zero, name: None, position: None, via_assistant: false }),
        Err(CommandError::Invalid(_))
    ));
    assert!(matches!(
        d.execute(Command::AddFeature {
            kind: boss(FeatureId(42), 5.0),
            name: None,
            position: None,
            via_assistant: false
        }),
        Err(CommandError::UnknownFeature(FeatureId(42)))
    ));
    assert!(matches!(
        d.execute(Command::AddFeature { kind: boss(s, 5.0), name: None, position: Some(0), via_assistant: false }),
        Err(CommandError::Order(_, _))
    ));
    assert!(matches!(d.execute(Command::RenameFeature { id: s, name: " ".into() }), Err(CommandError::Invalid(_))));
    assert_eq!(d.features().len(), 2);
}

#[test]
fn a_kernel_failure_marks_the_feature_and_keeps_the_body() {
    let mut d = doc();
    let (_, e) = block(&mut d);
    let edges = top_edges(&d);
    let f = add(&mut d, FeatureKind::Fillet(BlendFeature { edges, size: 50.0 }));
    assert!(matches!(d.status(f), Some(FeatureStatus::Failed(_))), "{:?}", d.status(f));
    assert_eq!(d.body(), d.body_after(e), "the body passes through the failed fillet");
}

#[test]
fn files_round_trip_and_refuse_what_they_cannot_read() {
    let kernel: Arc<dyn Kernel> = Arc::new(CatallaxKernel::new());
    let mut d = Document::new("Block", kernel.clone());
    block(&mut d);
    let edges = top_edges(&d);
    add(&mut d, FeatureKind::Fillet(BlendFeature { edges, size: 2.0 }));
    let bytes = d.to_bytes().unwrap();
    assert_eq!(d.to_bytes().unwrap(), bytes, "saving is deterministic");
    let back = Document::from_bytes(&bytes, kernel.clone()).unwrap();
    assert_eq!(back.name(), "Block");
    assert_eq!(back.id(), d.id(), "the id survives reopening");
    assert_eq!(back.semantic_hash(), d.semantic_hash(), "so does the revision");
    assert_eq!(back.part_json(), d.part_json());
    assert_eq!(back.to_bytes().unwrap(), bytes);

    let newer = d.part_json().replace("soliddraft-part/2", "soliddraft-part/3");
    assert!(matches!(Document::from_part_json(&newer, kernel.clone()), Err(FileError::Format { .. })));
    // A truth without a format tag says so, rather than naming a blank one.
    let Err(untagged) = Document::from_part_json("[1, 2]", kernel.clone()) else { panic!("a list is not a part") };
    assert!(matches!(untagged, FileError::Format { .. }));
    assert!(untagged.to_string().starts_with("the file has no format tag, this version reads"), "{untagged}");
    assert!(
        Document::from_part_json(&newer, kernel.clone())
            .unwrap_err()
            .to_string()
            .starts_with("the file is soliddraft-part/3,")
    );
    let broken = d.part_json().replace("\"next_feature\": 4", "\"next_feature\": 2");
    assert!(matches!(Document::from_part_json(&broken, kernel.clone()), Err(FileError::Invalid(_))), "{broken}");
    assert!(matches!(Document::from_bytes(b"not a zip", kernel), Err(FileError::Archive(_))));
}

/// A part file as phase D2 wrote it: `recipe.json` (format 1) and `meta.json`.
fn legacy_part_file(d: &Document) -> Vec<u8> {
    use std::io::Write;
    let recipe = d.part_json().replace("soliddraft-part/2", "draftwell-part/1");
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default();
    zip.start_file("recipe.json", o).unwrap();
    zip.write_all(recipe.as_bytes()).unwrap();
    zip.start_file("meta.json", o).unwrap();
    zip.write_all(br#"{"application": "draftwell 0.1.0", "kernel": {"name": "smlib", "version": "x"}}"#).unwrap();
    zip.finish().unwrap().into_inner()
}

#[test]
fn draftwell_parts_open_and_save_with_soliddraft_prefixes() {
    use soliddraft_document::manifest::{Entry, write_package};
    use soliddraft_document::{Package, Section};
    let kernel: Arc<dyn Kernel> = Arc::new(CatallaxKernel::new());
    let mut d = Document::new("Old block", kernel.clone());
    block(&mut d);
    let custom = soliddraft_tessellation::Settings {
        viewport: soliddraft_tessellation::Quality::Slider { value: 8 },
        ..Default::default()
    };
    d.set_settings(custom).unwrap();
    let mut m = d.manifest();
    m.envelope = "draftwell-document/1".into();
    m.format = "draftwell-part/2".into();
    m.writer = "draftwell 0.1.0".into();
    m.sections = vec![Section::required("part.json"), Section::optional("settings.json")];
    let truth = d.part_json().replace("soliddraft-part/2", "draftwell-part/2");
    let settings = d.settings().to_json().replace("soliddraft-settings/1", "draftwell-settings/1");
    let bytes = write_package(
        &m,
        &[Entry::new("part.json", truth.as_bytes()), Entry::new("settings.json", settings.as_bytes())],
    )
    .unwrap();
    let dir = std::env::temp_dir().join(format!("soliddraft-legacy-part-{}", std::process::id()));
    let path = dir.join("old.dwprt");
    soliddraft_document::save_atomic(&path, &bytes).unwrap();
    let back = Document::open(&path, kernel.clone()).unwrap();
    assert_eq!(back.id(), d.id());
    assert_eq!(back.semantic_hash(), d.semantic_hash());
    assert_eq!(back.part_json(), d.part_json());
    assert_eq!(back.settings(), d.settings());
    assert_eq!(back.body().is_some(), d.body().is_some());
    back.save(&path).unwrap();
    let saved = std::fs::read(&path).unwrap();
    let p = Package::from_bytes(&saved).unwrap();
    assert_eq!(p.manifest().unwrap().envelope, "soliddraft-document/1");
    assert_eq!(p.manifest().unwrap().format, "soliddraft-part/2");
    assert_eq!(Document::open(&path, kernel).unwrap().part_json(), d.part_json());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn format_1_parts_open_with_a_stable_id_and_migrate_on_save() {
    let kernel: Arc<dyn Kernel> = Arc::new(CatallaxKernel::new());
    let mut d = Document::new("Block", kernel.clone());
    block(&mut d);
    let old = legacy_part_file(&d);
    let a = Document::from_bytes(&old, kernel.clone()).unwrap();
    let b = Document::from_bytes(&old, kernel.clone()).unwrap();
    assert_eq!(a.id(), b.id(), "every open of the same old file agrees on its id");
    assert!(a.id().is_legacy() && a.id() != d.id());
    assert_eq!(a.id(), soliddraft_document::identify(std::io::Cursor::new(&old)).unwrap().id, "as the index sees it");
    assert_eq!(a.part_json(), d.part_json(), "the same features, now format 2");
    assert_eq!(a.semantic_hash(), d.semantic_hash(), "migration does not change the revision");
    assert_eq!(a.body().is_some(), d.body().is_some());

    // Saving writes format 2 with a manifest and keeps the derived id.
    let saved = a.to_bytes().unwrap();
    let mut p = soliddraft_document::Package::from_bytes(&saved).unwrap();
    let m = p.manifest().unwrap().clone();
    assert_eq!((m.id, m.kind, m.format.as_str()), (a.id(), soliddraft_document::DocKind::Part, "soliddraft-part/2"));
    assert_eq!(m.semantic_hash, a.semantic_hash());
    assert!(p.read("part.json").unwrap().is_some() && p.read("recipe.json").unwrap().is_none());
    let reopened = Document::from_bytes(&saved, kernel.clone()).unwrap();
    assert_eq!(reopened.id(), a.id());
    assert_eq!(reopened.semantic_hash(), a.semantic_hash());

    // A format 1 recipe inside the envelope is not a thing; a newer part is refused.
    let newer = legacy_part_file(&d);
    let newer = String::from_utf8_lossy(&newer).replace("soliddraft-part/1", "soliddraft-part/3");
    assert!(Document::from_bytes(newer.as_bytes(), kernel).is_err());
}

#[test]
fn hashes_follow_the_design_and_ids_follow_the_document() {
    let kernel: Arc<dyn Kernel> = Arc::new(CatallaxKernel::new());
    let mut d = Document::new("Block", kernel.clone());
    let (s, e) = block(&mut d);
    let (rev, shape) = (d.semantic_hash(), d.geometry_hash());
    // A rename is a new revision with the same shape.
    d.execute(Command::RenameFeature { id: s, name: "Base sketch".into() }).unwrap();
    assert_ne!(d.semantic_hash(), rev);
    assert_eq!(d.geometry_hash(), shape);
    // Undo comes back to the same revision.
    d.undo().unwrap();
    assert_eq!(d.semantic_hash(), rev);
    // A suppressed feature changes the shape.
    d.execute(Command::SetSuppressed { id: e, suppressed: true }).unwrap();
    assert_ne!(d.geometry_hash(), shape);
    // Two parts with the same features are two documents with one revision.
    let mut twin = Document::new("Block", kernel);
    block(&mut twin);
    assert_ne!(twin.id(), d.id());
    assert_eq!(twin.semantic_hash(), rev);
    let id = twin.id();
    twin.new_identity();
    assert_ne!(twin.id(), id, "Save as copy mints a new id");
}

#[test]
fn files_with_unknown_required_sections_are_refused() {
    use soliddraft_document::manifest::{Entry, Section, write_package};
    let kernel: Arc<dyn Kernel> = Arc::new(CatallaxKernel::new());
    let mut d = Document::new("Block", kernel.clone());
    block(&mut d);
    let mut m = d.manifest();
    m.sections.push(Section::optional("cache/"));
    let json = d.part_json();
    let with_cache =
        write_package(&m, &[Entry::new("part.json", json.as_bytes()), Entry::new("cache/mesh.bin", b"..".as_slice())]);
    assert!(Document::from_bytes(&with_cache.unwrap(), kernel.clone()).is_ok(), "unknown caches are dropped");
    m.sections.push(Section::required("mates.json"));
    let bytes = write_package(&m, &[Entry::new("part.json", json.as_bytes())]).unwrap();
    let err = Document::from_bytes(&bytes, kernel).unwrap_err();
    assert!(err.to_string().contains("newer SolidDraft"), "{err}");
}

#[test]
fn sketches_on_faces_follow_the_face() {
    let mut d = doc();
    let (s, e) = block(&mut d);
    let t = d.kernel().topology(d.body().unwrap()).unwrap();
    let top = t.faces.iter().find(|f| f.normal == Some(Vec3::Z)).unwrap();
    let face = d.face_ref(top.id).unwrap();
    let mut circle = Sketch::new();
    circle.circle_at(P2::new(5.0, 0.0), 6.0).unwrap();
    solve(&mut circle, None, true);
    assert_eq!(circle.status, SolveStatus::FullyDefined);
    let s2 = add(&mut d, FeatureKind::Sketch(SketchFeature { plane: SketchPlane::Face { face }, sketch: circle }));
    let pin = add(&mut d, boss(s2, 5.0));
    let pin_volume = std::f64::consts::PI * 9.0 * 5.0;
    assert!((volume(&d) - 12000.0 - pin_volume).abs() < 1e-3 * pin_volume, "{}", volume(&d));
    d.execute(Command::EditFeature { id: e, kind: boss(s, 25.0) }).unwrap();
    assert!((d.built_sketch(s2).unwrap().plane.origin.z - 25.0).abs() < 1e-9);
    assert!(d.status(pin).unwrap().is_ok());
    assert!((volume(&d) - 30000.0 - pin_volume).abs() < 1e-3 * pin_volume);
}

#[test]
fn existing_chamfer_waits_for_stage_three_and_keeps_the_body_after_reopen() {
    let mut d = doc();
    let (_, block) = block(&mut d);
    let edges = top_edges(&d);
    let c = add(&mut d, FeatureKind::Chamfer(BlendFeature { edges, size: 1.0 }));
    assert_eq!(d.feature(c).unwrap().name, "Chamfer1");
    let check = |d: &Document| {
        assert!(
            matches!(d.status(c), Some(FeatureStatus::Failed(error)) if error.contains("Catallax stage 3")),
            "{:?}",
            d.status(c)
        );
        assert_eq!(d.body_after(c), d.body_after(block));
        assert!(d.generated_faces(c).is_empty());
        assert!((volume(d) - 12000.0).abs() < 1e-6);
    };
    check(&d);
    let back = Document::from_bytes(&d.to_bytes().unwrap(), Arc::new(CatallaxKernel::new())).unwrap();
    check(&back);
}

#[test]
fn built_in_shapes_extrude_without_a_sketch() {
    let mut d = doc();
    let shape = |datum, shape, depth| {
        FeatureKind::Extrude(ExtrudeFeature {
            profile: ProfileSource::Shape { datum, shape },
            operation: ExtrudeOp::Boss,
            end: EndCondition::Blind { depth },
            reverse: false,
        })
    };
    let plate = add(&mut d, shape(DatumPlane::Top, BuiltinShape::Rectangle { width: 40.0, height: 30.0 }, 10.0));
    assert!((volume(&d) - 12000.0).abs() < 1e-6);
    let sides = d.generated_faces(plate).iter().filter(|n| matches!(n.generator, Generator::Side { .. })).count();
    assert_eq!(sides, 4, "each rectangle line names a side");
    let side = d.generated_faces(plate).iter().find(|n| matches!(n.generator, Generator::Side { .. })).unwrap();
    assert!(d.face_label(carrier(&d, side)).unwrap().text.starts_with("Boss-Extrude1 · Side of line "));
    // A boss through the plate's middle, from the Front plane: it joins the plate.
    let pin = add(&mut d, shape(DatumPlane::Front, BuiltinShape::Circle { diameter: 6.0 }, 20.0));
    assert!(d.status(pin).unwrap().is_ok(), "{:?}", d.status(pin));
    assert!(volume(&d) > 12000.0);
    // The same shape gives the same names after an edit.
    let before: Vec<Generator> = d.generated_faces(plate).iter().map(|n| n.generator).collect();
    d.execute(Command::EditFeature {
        id: plate,
        kind: shape(DatumPlane::Top, BuiltinShape::Rectangle { width: 50.0, height: 30.0 }, 10.0),
    })
    .unwrap();
    let after: Vec<Generator> = d.generated_faces(plate).iter().map(|n| n.generator).collect();
    assert_eq!(before, after);
}
