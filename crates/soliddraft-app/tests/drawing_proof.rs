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
