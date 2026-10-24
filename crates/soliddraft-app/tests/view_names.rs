//! Drawing tools name a view by its label or by its id. Labels need not be unique: two
//! sheets can each have a Front view, one sheet can hold two, and custom labels can differ
//! only in case. A label that names more than one view is refused, and nothing changes;
//! the id that `model_summary` lists reaches the one view meant.

use serde_json::{Value, json};
use soliddraft_app::app_tools::AppTools;
use soliddraft_app::command_tools::board_bracket;
use soliddraft_app::state::AppState;
use soliddraft_assistant::Caller;
use soliddraft_assistant::tools::call_checked;
use soliddraft_drawing::project::view_label;
use soliddraft_drawing::{HiddenLines, ViewId};

fn call(state: &mut AppState, name: &str, arguments: Value) -> Result<Value, String> {
    let caller = Caller::Mcp { client: "test".into() };
    let out = call_checked(&mut AppTools { state }, name, arguments, &caller);
    if out.is_error { Err(out.content) } else { Ok(serde_json::from_str(&out.content).unwrap()) }
}

/// Every view's id with whether it draws hidden lines.
fn hidden(state: &AppState) -> Vec<(u32, bool)> {
    let d = &state.drawing.session.as_ref().unwrap().doc.drawing;
    d.sheets.iter().flat_map(|s| &s.views).map(|v| (v.id.0, v.display.hidden == HiddenLines::Visible)).collect()
}

/// Copies the view `from` onto `sheet` under a new id, with `label`, and returns its id.
fn copy_view(state: &mut AppState, from: ViewId, sheet: usize, label: Option<&str>) -> u32 {
    let d = &mut state.drawing.session.as_mut().unwrap().doc.drawing;
    let next = d.sheets.iter().flat_map(|s| &s.views).map(|v| v.id.0).max().unwrap() + 1;
    let mut v = d.view(from).unwrap().clone();
    v.id = ViewId(next);
    v.alignment = None;
    v.label = label.map(str::to_owned);
    v.position.u += 40.0;
    d.sheets[sheet].views.push(v);
    next
}

#[test]
fn a_label_that_names_several_views_is_refused_and_an_id_reaches_one() {
    let mut state = AppState::new(board_bracket());
    call(&mut state, "make_drawing", json!({ "template": "iso-a3" })).unwrap();
    call(&mut state, "drawing_add_sheet", json!({})).unwrap();
    let sheet = &state.drawing.session.as_ref().unwrap().doc.drawing.sheets[0];
    let base = sheet.views.iter().find(|v| v.parent().is_none()).expect("a base view");
    let (front_id, front) = (base.id, view_label(sheet, base));

    // Two sheets with a view of the same label each, such as two Front views.
    let other = copy_view(&mut state, front_id, 1, None);
    let before = hidden(&state);
    let refused = call(&mut state, "drawing_hidden_lines", json!({ "view": front, "shown": false }));
    let why = refused.expect_err("a label two views share is refused");
    assert!(why.contains("ids") && why.contains(&front_id.0.to_string()) && why.contains(&other.to_string()), "{why}");
    assert_eq!(hidden(&state), before, "a refused name changes nothing");

    // The id reaches the one view, on the second sheet.
    call(&mut state, "drawing_hidden_lines", json!({ "view": other, "shown": false })).unwrap();
    let after = hidden(&state);
    for ((id, was), (_, now)) in before.iter().zip(&after) {
        assert_eq!(*now, if *id == other { false } else { *was }, "view {id}");
    }

    // Two views of one sheet, and labels that differ only in case.
    let same_sheet = copy_view(&mut state, front_id, 0, Some("Pump"));
    let other_case = copy_view(&mut state, front_id, 0, Some("PUMP"));
    let before = hidden(&state);
    for name in ["pump", "Pump", "PUMP"] {
        let why = call(&mut state, "drawing_delete", json!({ "view": name, "annotation": null }))
            .expect_err("a label that differs only in case is still ambiguous");
        assert!(why.contains(&same_sheet.to_string()) && why.contains(&other_case.to_string()), "{why}");
    }
    assert_eq!(hidden(&state), before, "nothing was deleted");
    call(&mut state, "drawing_delete", json!({ "view": other_case, "annotation": null })).unwrap();
    assert!(!hidden(&state).iter().any(|(id, _)| *id == other_case));
    // With the twin gone, the label names one view again.
    call(&mut state, "drawing_hidden_lines", json!({ "view": "pump", "shown": false })).unwrap();

    // model_summary lists each view with its id.
    let summary = call(&mut state, "model_summary", json!({})).unwrap();
    let listed: Vec<u64> = summary["drawing"]["sheets"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["views"].as_array().unwrap().iter().map(|v| v["id"].as_u64().expect("a view id")))
        .collect();
    assert!(listed.contains(&u64::from(other)) && listed.contains(&u64::from(same_sheet)), "{listed:?}");

    // An id no view has is refused.
    let why = call(&mut state, "drawing_align", json!({ "view": 999 })).unwrap_err();
    assert!(why.contains("999"), "{why}");
}
