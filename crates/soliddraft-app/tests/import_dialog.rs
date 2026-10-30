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
    state.import_dialog = Some(Preview::from_bytes(&state.doc, "block-seam-4um.step", BLOCK.to_vec()).unwrap());
    app.run_steps(3);
    app.get_by_role_and_label(egui::accesskit::Role::Button, "Accept").click();
    app.run_steps(3);
    let feature = &app.state().doc.features[0];
    assert_eq!(feature.name, "Import1");
    let soliddraft_app::document::FeatureKind::ImportStep(import) = &feature.kind else { panic!("import feature") };
    assert!(import.decisions.is_empty());
    assert_eq!(feature.status, soliddraft_app::document::FeatureStatus::Ok);
    assert!(app.state().doc.display_body().is_some());
}
