//! Deferred authored changes retain the previous body until a matching build is adopted.

use std::sync::Arc;

use soliddraft_exchange::Format;
use soliddraft_kernel_api::Kernel;
use soliddraft_kernel_api::counting::Counting;
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::{
    BuiltinShape, Command, DatumPlane, Document, EndCondition, ExtrudeFeature, ExtrudeOp, FeatureId, FeatureKind,
    ProfileSource,
};

fn block(depth: f64) -> FeatureKind {
    FeatureKind::Extrude(ExtrudeFeature {
        profile: ProfileSource::Shape {
            datum: DatumPlane::Top,
            shape: BuiltinShape::Rectangle { width: 40.0, height: 30.0 },
        },
        operation: ExtrudeOp::Boss,
        end: EndCondition::Blind { depth },
        reverse: false,
    })
}

fn add(doc: &mut Document, depth: f64) -> FeatureId {
    doc.execute(Command::AddFeature { kind: block(depth), name: None, position: None, via_assistant: false })
        .unwrap()
        .created
        .unwrap()
}

fn volume(doc: &Document) -> f64 {
    doc.kernel().mass_properties(doc.body().unwrap()).unwrap().volume
}

#[test]
fn deferred_history_and_zero_budget_adoption_build_the_edit_only_once() {
    let kernel = Arc::new(Counting::new(Arc::new(CatallaxKernel::new())));
    let mut doc = Document::new("Deferred plate", kernel.clone());
    let id = add(&mut doc, 10.0);
    let old = doc.body();
    let depth = doc.undo_depth();
    doc.set_rebuild_budget(0);
    kernel.take();
    let executed = doc.execute_deferred(Command::EditFeature { id, kind: block(14.0) }).unwrap();
    assert_eq!(doc.undo_depth(), depth + 1);
    assert_eq!(doc.undo_label(), Some(executed.label.as_str()));
    assert!(doc.is_dirty());
    assert_eq!(doc.body(), old);
    assert!(doc.export(Format::Step).unwrap_err().contains("not been rebuilt"));
    assert_eq!(kernel.take(), [], "a deferred edit ran kernel work");

    let mut completed = doc.fork();
    completed.rebuild();
    let proved = completed.body();
    assert_eq!(volume(&completed), 16800.0);
    kernel.take();
    doc.rebuild_reusing(&completed).unwrap();
    assert_eq!(kernel.take(), [], "adopting the completed build evaluated it again");
    assert!(!doc.is_dirty());
    assert_eq!(doc.body(), proved);
    assert_eq!(doc.undo_depth(), depth + 1);
    drop(completed);
    assert_eq!(volume(&doc), 16800.0, "adoption must own the body's lifetime");
    assert!(doc.export(Format::Step).is_ok());
    kernel.take();

    doc.undo_deferred().unwrap();
    assert!(doc.is_dirty());
    assert_eq!(doc.body(), proved);
    assert!(doc.export(Format::Step).is_err());
    assert_eq!(kernel.take(), [], "deferred undo ran kernel work");
    doc.rebuild();
    assert_eq!(volume(&doc), 12000.0);
    kernel.take();
    doc.redo_deferred().unwrap();
    assert!(doc.is_dirty());
    assert_eq!(kernel.take(), [], "deferred redo ran kernel work");
    doc.rebuild();
    assert_eq!(volume(&doc), 16800.0);
    assert_eq!(doc.undo_label(), Some(executed.label.as_str()));
}

#[test]
fn adoption_rejects_stale_unbuilt_and_foreign_completions() {
    let kernel: Arc<dyn Kernel> = Arc::new(CatallaxKernel::new());
    let mut doc = Document::new("Plate", kernel.clone());
    let id = add(&mut doc, 10.0);
    let mut completed = doc.fork();
    completed.execute(Command::EditFeature { id, kind: block(14.0) }).unwrap();
    doc.execute_deferred(Command::EditFeature { id, kind: block(18.0) }).unwrap();
    let old = doc.body();
    let history = doc.undo_depth();
    assert!(doc.rebuild_reusing(&completed).unwrap_err().contains("does not match"));
    assert_eq!(doc.body(), old);
    assert_eq!(doc.undo_depth(), history);
    assert!(doc.is_dirty());
    let unbuilt = doc.fork();
    assert!(doc.rebuild_reusing(&unbuilt).unwrap_err().contains("unbuilt definitions"));
    let foreign = Document::new("Other part", kernel);
    assert!(doc.rebuild_reusing(&foreign).unwrap_err().contains("another document"));
    doc.rebuild();
    assert_eq!(volume(&doc), 21600.0);
}

#[test]
fn deferred_add_delete_and_journal_replay_gate_the_current_definition() {
    let mut doc = Document::new("Plate", Arc::new(CatallaxKernel::new()));
    let add = Command::AddFeature { kind: block(10.0), name: None, position: None, via_assistant: false };
    doc.run_journal_command(serde_json::json!({"op":"execute_deferred", "command":add})).unwrap();
    let id = doc.features()[0].id;
    assert!(doc.is_dirty());
    assert!(doc.body().is_none());
    assert!(doc.export(Format::Step).unwrap_err().contains("not been rebuilt"));
    doc.run_journal_command(serde_json::json!({"op":"rebuild"})).unwrap();
    let body = doc.body();
    assert_eq!(volume(&doc), 12000.0);
    doc.execute_deferred(Command::DeleteFeature { id }).unwrap();
    assert!(doc.is_dirty(), "deleting the final feature invalidates the previous body");
    assert_eq!(doc.body(), body);
    assert!(doc.export(Format::Step).unwrap_err().contains("not been rebuilt"));
    doc.run_journal_command(serde_json::json!({"op":"undo_deferred"})).unwrap();
    doc.run_journal_command(serde_json::json!({"op":"rebuild"})).unwrap();
    assert_eq!(doc.body(), body);
    doc.run_journal_command(serde_json::json!({"op":"redo_deferred"})).unwrap();
    doc.rebuild();
    assert!(!doc.is_dirty());
    assert!(doc.body().is_none());
    assert!(doc.export(Format::Step).unwrap_err().contains("no body"));
}

#[test]
fn pending_save_keeps_the_authored_definition_without_mislabelling_the_previous_mesh() {
    let kernel = Arc::new(Counting::new(Arc::new(CatallaxKernel::new())));
    let mut doc = Document::new("Pending save", kernel.clone());
    let id = add(&mut doc, 10.0);
    let mut settings = doc.settings().clone();
    settings.save_display_data = true;
    doc.set_settings(settings).unwrap();
    doc.execute_deferred(Command::EditFeature { id, kind: block(14.0) }).unwrap();
    kernel.take();
    let bytes = doc.to_bytes().unwrap();
    assert_eq!(kernel.take(), [], "saving pending definitions must not mesh the preceding body");
    let archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
    assert!(
        archive.file_names().all(|name| !name.unwrap().starts_with("cache/mesh/")),
        "a previous revision's mesh was saved against the pending definition"
    );
    let reopened = Document::from_bytes(&bytes, Arc::new(CatallaxKernel::new())).unwrap();
    assert_eq!(volume(&reopened), 16800.0);
    assert_eq!(volume(&doc), 12000.0, "saving must not adopt an unrequested build");
    assert!(doc.is_dirty());
}

#[test]
fn a_refused_batch_restores_the_previous_rebuild_state() {
    for pending in [false, true] {
        let kernel = Arc::new(Counting::new(Arc::new(CatallaxKernel::new())));
        let mut doc = Document::new("Refused batch", kernel.clone());
        let id = add(&mut doc, 10.0);
        if pending {
            doc.execute_deferred(Command::EditFeature { id, kind: block(14.0) }).unwrap();
        }
        let before = doc.feature(id).unwrap().kind.clone();
        let body = doc.body();
        let history = doc.undo_depth();
        kernel.take();
        let refused = Command::Batch {
            label: "Refused edit".into(),
            commands: vec![
                Command::EditFeature { id, kind: block(18.0) },
                Command::RenameFeature { id: FeatureId(999), name: "Missing".into() },
            ],
        };
        assert!(doc.execute_deferred(refused).is_err());
        assert_eq!(doc.feature(id).unwrap().kind, before);
        assert_eq!(doc.body(), body);
        assert_eq!(doc.undo_depth(), history);
        assert_eq!(doc.is_dirty(), pending);
        assert_eq!(kernel.take(), [], "a refused deferred batch ran kernel work");
        assert_eq!(doc.export(Format::Step).is_err(), pending);
        doc.rebuild();
        assert_eq!(volume(&doc), if pending { 16800.0 } else { 12000.0 });
    }
}
