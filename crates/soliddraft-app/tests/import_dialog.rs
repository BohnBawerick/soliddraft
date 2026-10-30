//! The reviewed import's visible buttons run the shared command on real Catallax.

use egui_kittest::{Harness, kittest::Queryable};
use soliddraft_app::{document::Document, import::Preview, state::AppState};

const BLOCK: &[u8] = include_bytes!("../../../ci/fixtures/import/block-seam-4um.step");

#[test]
fn preview_cancel_decline_and_accept() {
    let doc = Document::new("Import test", soliddraft_app::kernel());
    let original_revision = doc.revision();
    let mut state = AppState::new(doc);
    state.import_dialog = Some(Preview::from_bytes(&state.doc, "block-seam-4um.step", BLOCK.to_vec()).unwrap());
    let mut app = Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .build_ui_state(|ui, state| soliddraft_app::import::window(ui.ctx(), state), state);
    app.run_steps(3);
    app.get_by_label("g0: seam_close");
    app.get_by_label("0.004 to 0.004 mm seam; tolerance 0.01 mm");
    app.get_by_label("automatic");
    app.get_by_role_and_label(egui::accesskit::Role::Button, "Cancel").click();
    app.run_steps(3);
    assert!(app.state().import_dialog.is_none());
    assert_eq!(app.state().doc.revision(), original_revision);
    assert!(app.state().doc.features.is_empty());

    let state = app.state_mut();
    state.import_dialog = Some(Preview::from_bytes(&state.doc, "block-seam-4um.step", BLOCK.to_vec()).unwrap());
    app.run_steps(3);
    app.get_by_role_and_label(egui::accesskit::Role::Button, "Decline").click();
    app.run_steps(3);
    let feature = &app.state().doc.features[0];
    assert_eq!(feature.name, "Import1");
    let soliddraft_app::document::FeatureKind::ImportStep(import) = &feature.kind else { panic!("import feature") };
    assert_eq!(import.decisions["repair:1a602addaa4b:g0"], "decline");
    assert!(
        matches!(&feature.status, soliddraft_app::document::FeatureStatus::Error(message) if message.contains("not imported until the gap is resolved"))
    );
    assert!(app.state().doc.display_body().is_none());

    let state = app.state_mut();
    state.doc.undo().unwrap();
    let path = std::env::temp_dir().join(format!("soliddraft-import-ui-binding-{}.step", std::process::id()));
    std::fs::write(&path, BLOCK).unwrap();
    state.import_dialog = Some(Preview::read(&state.doc, &path).unwrap());
    let mut replacement = BLOCK.to_vec();
    replacement.push(b'\n');
    std::fs::write(&path, replacement).unwrap();
    app.run_steps(3);
    app.get_by_role_and_label(egui::accesskit::Role::Button, "Accept").click();
    app.run_steps(3);
    let feature = &app.state().doc.features[0];
    assert_eq!(feature.name, "Import1");
    let soliddraft_app::document::FeatureKind::ImportStep(import) = &feature.kind else { panic!("import feature") };
    assert!(import.decisions.is_empty());
    assert_eq!(import.source, BLOCK, "the UI commits the bytes it previewed, even after path replacement");
    assert_eq!(feature.status, soliddraft_app::document::FeatureStatus::Ok);
    assert!(app.state().doc.display_body().is_some());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn mcp_refuses_source_substitution_after_preview() {
    use serde_json::{Value, json};
    use soliddraft_app::command_tools::{CommandTools, ProposalBook, Surface};
    use soliddraft_assistant::{Caller, ToolHost};

    fn call(doc: &mut Document, book: &mut ProposalBook, name: &str, input: Value) -> soliddraft_assistant::ToolOutput {
        CommandTools { doc, book, surface: Surface::Mcp, drawing: None }.call(
            name,
            input,
            &Caller::Mcp { client: "review binding test".into() },
        )
    }

    let temporary = std::env::temp_dir().join(format!("soliddraft-import-binding-{}", std::process::id()));
    std::fs::create_dir_all(&temporary).unwrap();
    let path = temporary.join("source.step");
    let mut replacement = BLOCK.to_vec();
    replacement.push(b'\n');
    let mut observed = Vec::new();
    for decline in [json!(["g0"]), json!([])] {
        std::fs::write(&path, BLOCK).unwrap();
        let kernel = std::sync::Arc::new(soliddraft_kernel_api::counting::Counting::new(soliddraft_app::kernel()));
        let mut doc = Document::new("Import binding", kernel);
        let mut book = ProposalBook::default();
        let before = (doc.revision(), doc.undo_depth());
        let preview = call(&mut doc, &mut book, "preview_import", json!({"path":path}));
        assert!(!preview.is_error, "{}", preview.content);
        let preview: Value = serde_json::from_str(&preview.content).unwrap();
        let token = preview.get("token").cloned().unwrap_or(json!("unbound legacy preview"));
        std::fs::write(&path, &replacement).unwrap();
        let answer =
            call(&mut doc, &mut book, "import_step", json!({"path":path,"preview_token":token,"decline":decline}));
        observed.push((answer.is_error, doc.features.len(), doc.revision(), doc.undo_depth()));
        if answer.is_error {
            assert!(answer.content.contains("review"), "{}", answer.content);
            assert_eq!((doc.revision(), doc.undo_depth()), before);
            let missing = call(&mut doc, &mut book, "import_step", json!({"path":path,"decline":[]}));
            assert!(missing.is_error);
            assert!(doc.features.is_empty());
            let fresh = call(&mut doc, &mut book, "preview_import", json!({"path":path}));
            assert!(!fresh.is_error, "{}", fresh.content);
            let fresh: Value = serde_json::from_str(&fresh.content).unwrap();
            assert_eq!(fresh["proposals"][0]["id"], "g0", "replacement is a supported part with the same gap id");
            let accepted = call(
                &mut doc,
                &mut book,
                "import_step",
                json!({"path":path,"preview_token":fresh["token"],"decline":[]}),
            );
            assert!(!accepted.is_error, "{}", accepted.content);
            let soliddraft_app::document::FeatureKind::ImportStep(import) = &doc.features[0].kind else {
                panic!("import")
            };
            assert_eq!(import.source, replacement);
        }
    }
    std::fs::remove_dir_all(temporary).unwrap();
    assert_eq!(
        observed,
        [(true, 0, 0, 0), (true, 0, 0, 0)],
        "neither decline nor acceptance can commit a substituted source"
    );
}
