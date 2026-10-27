//! An exported drawing never comes from a preview. The drawing tab shows views drawn
//! from Catallax's unproved preview mesh, which the drawing writers refuse. Exporting
//! draws the views again from the certified mesh, on a copy, and leaves the drawing on
//! screen as it was.

use serde_json::{Value, json};
use soliddraft_app::app_tools::AppTools;
use soliddraft_app::command_tools::board_bracket;
use soliddraft_app::drawing::part::PartContext;
use soliddraft_app::state::AppState;
use soliddraft_assistant::Caller;
use soliddraft_assistant::tools::call_checked;
use soliddraft_drawing::display::layout_sheet;
use soliddraft_drawing::export::{self, Format};
use soliddraft_drawing::{Command, Drawing, DrawingError, apply_with};

fn call(state: &mut AppState, name: &str, arguments: Value) {
    let caller = Caller::Mcp { client: "test".into() };
    let out = call_checked(&mut AppTools { state }, name, arguments, &caller);
    assert!(!out.is_error, "{name}: {}", out.content);
}

fn previews(drawing: &Drawing) -> Vec<bool> {
    drawing.sheets.iter().flat_map(|s| &s.views).map(|v| v.geometry.unproved).collect()
}

#[test]
fn an_exported_drawing_never_comes_from_a_preview() {
    let dir = std::env::temp_dir().join(format!("soliddraft-drawing-proof-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut state = AppState::new(board_bracket());
    call(&mut state, "make_drawing", json!({ "template": "iso-a3" }));

    // The screen shows previews, and no writer takes them as they are.
    let shown = state.drawing.session.as_ref().unwrap().doc.drawing.clone();
    assert!(!previews(&shown).is_empty() && previews(&shown).iter().all(|p| *p), "{:?}", previews(&shown));
    let list = layout_sheet(&shown, 0);
    for format in [Format::Pdf, Format::Dxf, Format::Svg] {
        assert!(matches!(export::bytes(&list, format, "Part1"), Err(DrawingError::Unproved)), "{format:?}");
    }

    // The same views drawn from the certified mesh are proven and can be written.
    let part = PartContext::new(&state.doc).unwrap();
    let mut certified = shown.clone();
    apply_with(&mut certified, &Command::UpdateFromModel, &part.certified_part()).unwrap();
    assert!(previews(&certified).iter().all(|p| !p), "{:?}", previews(&certified));
    assert!(export::bytes(&layout_sheet(&certified, 0), Format::Svg, "Part1").is_ok());

    // So exporting succeeds only by drawing the views again from the certified mesh,
    // and the drawing on screen keeps its previews.
    for (tool, file) in
        [("drawing_export_pdf", "sheet.pdf"), ("drawing_export_dxf", "sheet.dxf"), ("drawing_export_svg", "sheet.svg")]
    {
        call(&mut state, tool, json!({ "path": dir.join(file) }));
        assert!(dir.join(file).is_file(), "{tool} wrote no {file}");
    }
    assert_eq!(state.drawing.session.as_ref().unwrap().doc.drawing, shown);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn drawings_never_pair_requested_definitions_with_a_retained_body() {
    use soliddraft_app::document::{Command as PartCommand, Document, ExtrudeParams, FilletParams, ProfileKind};
    use soliddraft_app::drag::Dimension;
    use soliddraft_app::state::DragStep;
    use soliddraft_kernel_catallax::CatallaxKernel;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    let settle = |state: &mut AppState| {
        let deadline = Instant::now() + Duration::from_secs(120);
        while state.doc.proof_pending() {
            state.doc.poll_proof();
            assert!(Instant::now() < deadline, "the part proof did not settle");
            std::thread::sleep(Duration::from_millis(2));
        }
        soliddraft_app::drawing::follow_part(state);
    };
    let rejected_rebuild = |state: &mut AppState| {
        let caller = Caller::Mcp { client: "test".into() };
        let out = call_checked(&mut AppTools { state }, "drawing_rebuild", json!({}), &caller);
        assert!(out.is_error, "an unproved part must not become the drawing's source");
    };
    let dir = std::env::temp_dir().join(format!("soliddraft-drawing-revision-proof-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut doc = Document::new("Proof part", Arc::new(CatallaxKernel::new()));
    let mut params = ExtrudeParams {
        profile: ProfileKind::Rectangle { width: 40.0, height: 30.0 },
        depth: 10.0,
        ..Default::default()
    };
    let block = doc.apply(PartCommand::AddExtrude { params: params.clone(), via_assistant: false }).unwrap().unwrap();
    let mut state = AppState::new(doc);
    call(&mut state, "make_drawing", json!({ "template": "iso-a3" }));
    soliddraft_app::drawing::follow_part(&mut state);
    let part = PartContext::new(&state.doc).unwrap();
    let mut certified = state.drawing.session.as_ref().unwrap().doc.drawing.clone();
    certified.settings.automatic_update = true;
    apply_with(&mut certified, &Command::UpdateFromModel, &part.certified_part()).unwrap();
    state.drawing.session.as_mut().unwrap().doc.drawing = certified.clone();
    call(&mut state, "drawing_export_svg", json!({ "path": dir.join("proved.svg") }));
    let proved_svg = std::fs::read(dir.join("proved.svg")).unwrap();

    state.drag_dimension(DragStep::Start { feature: block, dimension: Dimension::Depth });
    state.drag_dimension(DragStep::Move(14.0));
    soliddraft_app::drawing::follow_part(&mut state);
    rejected_rebuild(&mut state);
    assert_eq!(state.drawing.session.as_ref().unwrap().doc.drawing, certified);
    state.drag_dimension(DragStep::Cancel);

    params.depth = 14.0;
    state.doc.apply_interactive(PartCommand::EditExtrude { id: block, params }).unwrap();
    assert!(state.doc.proof_pending());
    assert_ne!(state.doc.model().semantic_hash().as_str(), certified.model.pinned.as_str());
    assert!(PartContext::new(&state.doc).is_none());
    soliddraft_app::drawing::follow_part(&mut state);
    rejected_rebuild(&mut state);
    let session = state.drawing.session.as_ref().unwrap();
    assert_eq!(session.doc.drawing, certified);
    assert_eq!(session.doc.seen_digest, certified.model.pinned);
    // Already-certified views remain deliberately pinned; export does not recertify
    // them against the pending definition or silently substitute another body.
    call(&mut state, "drawing_export_svg", json!({ "path": dir.join("pending-pinned.svg") }));
    assert_eq!(std::fs::read(dir.join("pending-pinned.svg")).unwrap(), proved_svg);

    settle(&mut state);
    let part = PartContext::new(&state.doc).unwrap();
    assert_eq!(part.digest.as_str(), state.doc.model().semantic_hash().as_str());
    assert_eq!(part.kernel.mass_properties(part.body).unwrap().volume, 16_800.0);
    assert_eq!(state.drawing.session.as_ref().unwrap().doc.drawing.model.pinned, part.digest);
    let edge = part.topology.edges.iter().find(|edge| (edge.midpoint.z - 14.0).abs() < 1e-9).unwrap().id;
    let reference = state.doc.edge_ref(edge).unwrap();
    let fillet = state
        .doc
        .apply(PartCommand::AddFillet { params: FilletParams { edges: vec![reference.clone()], radius: 1.0 } })
        .unwrap()
        .unwrap();
    soliddraft_app::drawing::follow_part(&mut state);
    let before_failure = state.drawing.session.as_ref().unwrap().doc.drawing.clone();
    state
        .doc
        .apply_interactive(PartCommand::EditFillet {
            id: fillet,
            params: FilletParams { edges: vec![reference], radius: 50.0 },
        })
        .unwrap();
    settle(&mut state);
    assert!(state.doc.proof_state().failed);
    assert!(PartContext::new(&state.doc).is_none());
    rejected_rebuild(&mut state);
    let session = state.drawing.session.as_ref().unwrap();
    assert_eq!(session.doc.drawing, before_failure);
    assert_eq!(session.doc.seen_digest, before_failure.model.pinned);
    std::fs::remove_dir_all(dir).unwrap();
}
