//! The windowless Shell commands use the same model feature as the editor.

use serde_json::json;
use soliddraft_app::app_tools::AppTools;
use soliddraft_app::document::{Command, Document, ExtrudeParams, FeatureKind, ProfileKind};
use soliddraft_app::state::AppState;
use soliddraft_assistant::{Caller, tools::call_checked};

fn blank() -> AppState {
    let mut doc = Document::new("tray", soliddraft_app::kernel());
    doc.apply(Command::AddExtrude {
        params: ExtrudeParams {
            profile: ProfileKind::Rectangle { width: 40.0, height: 30.0 },
            depth: 12.0,
            ..Default::default()
        },
        via_assistant: false,
    })
    .unwrap();
    AppState::new(doc)
}

fn volume(state: &AppState) -> f64 {
    state.doc.kernel().mass_properties(state.doc.display_body().unwrap()).unwrap().volume
}

#[test]
fn shell_mcp_creates_edits_and_preserves_the_body_on_bad_requests() {
    let mut state = blank();
    let caller = Caller::Mcp { client: "shell-test".into() };
    let open_faces = json!(["Boss-Extrude1 · End cap"]);
    let out = call_checked(
        &mut AppTools { state: &mut state },
        "shell",
        json!({"open_faces":open_faces,"thickness":2}),
        &caller,
    );
    assert!(!out.is_error, "{}", out.content);
    assert!((volume(&state) - 5040.0).abs() < 1e-6);
    let FeatureKind::Shell(original) = state.doc.features.last().unwrap().kind.clone() else { panic!("shell feature") };
    let out = call_checked(
        &mut AppTools { state: &mut state },
        "edit_shell",
        json!({"feature":"Shell1","thickness":1.5}),
        &caller,
    );
    assert!(!out.is_error, "{}", out.content);
    assert!((volume(&state) - 3910.5).abs() < 1e-6);
    let FeatureKind::Shell(edited) = &state.doc.features.last().unwrap().kind else { panic!("shell feature") };
    assert_eq!(original.open_faces, edited.open_faces);
    let body = state.doc.display_body();
    let undo_depth = state.doc.undo_depth();
    let feature_count = state.doc.features.len();
    for (command, arguments) in [
        ("shell", json!({"open_faces":["missing face"],"thickness":2})),
        ("shell", json!({"open_faces":[],"thickness":0})),
        ("shell", json!({"thickness":2})),
        ("edit_shell", json!({"feature":"Shell1","thickness":-1})),
        ("edit_shell", json!({"feature":"Boss-Extrude1","thickness":2})),
        ("edit_shell", json!({"feature":"Shell1","thickness":"thin"})),
    ] {
        let out = call_checked(&mut AppTools { state: &mut state }, command, arguments, &caller);
        assert!(out.is_error, "{command} must refuse");
        assert_eq!(state.doc.display_body(), body, "{command} changed the body");
        assert_eq!(state.doc.undo_depth(), undo_depth);
        assert_eq!(state.doc.features.len(), feature_count);
    }
    state.doc.undo().unwrap();
    assert!((volume(&state) - 5040.0).abs() < 1e-6);
}

#[test]
fn shell_mcp_accepts_an_explicit_empty_opening_list_for_a_closed_shell() {
    let mut state = blank();
    let out = call_checked(
        &mut AppTools { state: &mut state },
        "shell",
        json!({"open_faces":[],"thickness":2}),
        &Caller::Mcp { client: "shell-test".into() },
    );
    assert!(!out.is_error, "{}", out.content);
    assert!((volume(&state) - (14400.0 - 36.0 * 26.0 * 8.0)).abs() < 1e-6);
}
