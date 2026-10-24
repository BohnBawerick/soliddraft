//! Every command with a tool runs without a window, as the MCP server and `soliddraft
//! run` run it, and does to the undo stacks what its registry row says (`Undo`). One
//! session on the sample bracket calls each tool in turn, the drawing ones on a drawing
//! the session makes, saves and opens again. A command waiting for Catallax may refuse,
//! but then it changes nothing.

use serde_json::{Value, json};
use soliddraft_app::app_tools::AppTools;
use soliddraft_app::command_tools::{Surface, board_bracket, tool_specs};
use soliddraft_app::commands::{self, Undo};
use soliddraft_app::state::AppState;
use soliddraft_assistant::Caller;
use soliddraft_assistant::tools::call_checked;
use soliddraft_document::DocumentId;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// The undo depths of the part and of the open drawing, and which part it is.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Depths {
    part: usize,
    drawing: Option<usize>,
    part_id: DocumentId,
}

fn depths(st: &AppState) -> Depths {
    Depths {
        part: st.doc.undo_depth(),
        drawing: st.drawing.session.as_ref().map(|s| s.doc.undo_depth()),
        part_id: st.doc.model().id(),
    }
}

/// Whether the change in depths is what `undo` says a command does.
fn undoes_as_said(undo: Undo, before: Depths, after: Depths) -> bool {
    let drawing = |d: Depths| d.drawing.unwrap_or(0);
    let part_moved = after.part as isize - before.part as isize;
    let drawing_moved = drawing(after) as isize - drawing(before) as isize;
    let same_drawing = before.drawing.is_some() == after.drawing.is_some();
    match undo {
        Undo::None => before == after,
        Undo::Step => same_drawing && matches!((part_moved, drawing_moved), (1, 0) | (0, 1)),
        Undo::Steps => same_drawing && part_moved == 0 && drawing_moved >= 1,
        Undo::Moves => same_drawing && (part_moved.abs() + drawing_moved.abs()) == 1,
        // Another part with an empty undo stack, or another drawing, or none.
        Undo::Replaces => {
            let same_part = before.part == after.part && before.part_id == after.part_id;
            (after.part == 0 && before.part_id != after.part_id && before.drawing == after.drawing)
                || (after.drawing == Some(0) && before.drawing != Some(0) && same_part)
                || (after.drawing.is_none() && before.drawing.is_some() && same_part)
        }
    }
}

struct Session {
    state: AppState,
    ran: BTreeSet<String>,
    dir: PathBuf,
}

impl Session {
    /// Calls a tool and checks it ran, and changed the undo stacks as its row says.
    fn call(&mut self, name: &str, arguments: Value) -> Value {
        let row = commands::get(name).unwrap_or_else(|| panic!("{name} is not a registry command"));
        let before = depths(&self.state);
        let caller = Caller::Mcp { client: "test".into() };
        let out = call_checked(&mut AppTools { state: &mut self.state }, name, arguments.clone(), &caller);
        let after = depths(&self.state);
        self.ran.insert(name.to_owned());
        if out.is_error {
            assert!(row.catallax_wait().is_some(), "{name} {arguments}: {}", out.content);
            assert_eq!(before, after, "{name} refused and still changed the undo stacks");
            return Value::Null;
        }
        assert!(
            undoes_as_said(row.undo, before, after),
            "{name}: its row says {:?}, and the undo depths went from {before:?} to {after:?}",
            row.undo
        );
        serde_json::from_str(&out.content).unwrap()
    }

    fn path(&self, name: &str) -> Value {
        json!(self.dir.join(name))
    }

    fn drawing(&mut self) -> Value {
        self.call("model_summary", json!({}))["drawing"].clone()
    }
}

#[test]
fn every_tool_runs_without_a_window_and_undoes_as_its_row_says() {
    let dir = std::env::temp_dir().join(format!("soliddraft-app-tools-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session { state: AppState::new(board_bracket()), ran: BTreeSet::new(), dir: dir.clone() };

    // Reading the part.
    s.call("model_summary", json!({}));
    s.call("feature", json!({ "name": "Boss-Extrude1" }));
    s.call("selection", json!({}));
    s.call("measure", json!({ "a": "Boss-Extrude1 · Start cap", "b": "Boss-Extrude1 · End cap" }));
    let mass = s.call("mass_properties", json!({ "name": "Boss-Extrude1" }));
    assert!((mass["volume_mm3"].as_f64().unwrap() - 120.0 * 90.0 * 10.0).abs() < 1.0, "{mass}");

    // The verbs, each as one undo step.
    let circle = json!({ "shape": "circle", "center": [20, 70, 10], "diameter": 8 });
    s.call("sketch", json!({ "plane": "Boss-Extrude1 · End cap", "shapes": [circle] }));
    s.call("cut", json!({ "sketch": "Sketch5", "through_all": true, "depth": 1, "flip": false }));
    let hexagon =
        json!({ "tool": "polygon", "center": { "u": 100, "v": 70 }, "vertex": { "u": 106, "v": 70 }, "sides": 6 });
    s.call("sketch_geometry", json!({ "plane": "top", "operations": [hexagon] }));
    s.call("extrude", json!({ "sketch": "Sketch6", "depth": 5, "flip": false }));
    s.call("fillet", json!({ "edges": ["Boss-Extrude2 · Edge 1"], "radius": 2 }));
    s.call("chamfer", json!({ "edges": ["Boss-Extrude2 · Edge 2"], "distance": 1 }));
    s.call("edit_fillet", json!({ "feature": "Fillet1", "radius": 3 }));
    s.call("edit_chamfer", json!({ "feature": "Chamfer1", "distance": 2 }));
    s.call("edit_extrude", json!({ "feature": "Boss-Extrude4", "depth": 6, "flip": false }));
    s.call("edit_cut", json!({ "feature": "Cut-Extrude2", "through_all": true, "depth": 1, "flip": false }));
    s.call("suppress", json!({ "feature": "Boss-Extrude4", "suppressed": true }));
    s.call("delete_feature", json!({ "feature": "Boss-Extrude4" }));
    s.call("rename", json!({ "feature": "Cut-Extrude2", "name": "Hole" }));
    s.call("rename_plane", json!({ "plane": "right", "name": "Side" }));
    s.call("edit_dimension", json!({ "sketch": "Sketch1", "dimension": 1, "value": 121 }));
    s.call("rollback", json!({ "before": "Hole" }));
    s.call("roll_to_end", json!({}));
    let lines = json!([
        { "op": "sketch", "plane": "Boss-Extrude1 · End cap", "shapes": [{ "shape": "circle", "center": [20, 20, 10], "diameter": 6 }] },
        { "op": "cut", "sketch": "previous", "through_all": true, "depth": 1, "flip": false },
    ]);
    let proposal = s.call("propose", json!({ "summary": "A Ø6 hole", "commands": lines }));
    s.call("apply", json!({ "proposal_id": proposal["proposal_id"] }));
    s.call("undo", json!({}));
    s.call("redo", json!({}));

    // Files.
    for (tool, file) in [
        ("export_step", "part.step"),
        ("export_stl", "part.stl"),
        ("export_3mf", "part.3mf"),
        ("export_obj", "part.obj"),
        ("save", "part.dwprt"),
        ("save_as", "copy.dwprt"),
    ] {
        let path = s.path(file);
        s.call(tool, json!({ "path": path }));
        assert!(dir.join(file).is_file(), "{tool} wrote no {file}");
    }

    // The drawing.
    s.call("make_drawing", json!({ "template": "iso-a3" }));
    s.call("drawing_show_part", json!({}));
    s.call("show_drawing", json!({}));
    let drawing = s.drawing();
    assert_eq!(drawing["active"], true, "{drawing}");
    let views = drawing["sheets"][0]["views"].as_array().unwrap().clone();
    let name = |v: &Value| v["name"].as_str().unwrap().to_owned();
    let child = views.iter().find(|v| v["aligned"] == true).map(name).expect("an aligned projected view");
    let first = name(&views[0]);
    s.call("drawing_hidden_lines", json!({ "view": first, "shown": false }));
    s.call("drawing_tangent_edges", json!({ "view": first, "tangent": "visible" }));
    s.call("drawing_crop", json!({ "view": first, "crop": [-30, -30, 30, 30] }));
    s.call("drawing_crop", json!({ "view": first, "crop": null }));
    s.call("drawing_break_alignment", json!({ "view": child }));
    s.call("drawing_align", json!({ "view": child }));
    let mark = s.drawing()["sheets"][0]["annotations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "Centre mark")
        .map(|a| a["id"].clone())
        .expect("the new drawing has centre marks");
    s.call("drawing_delete", json!({ "view": null, "annotation": mark }));
    s.call("drawing_auto_centre_marks", json!({ "view": null }));
    s.call("drawing_model_items", json!({}));
    s.call("drawing_add_sheet", json!({}));
    s.call("drawing_rebuild", json!({}));
    s.call("drawing_auto_update", json!({ "on": true }));
    s.call("undo", json!({}));
    s.call("redo", json!({}));
    let drawing_file = s.path("part.dwdrw");
    s.call("drawing_save", json!({ "path": drawing_file }));
    for (tool, file) in
        [("drawing_export_pdf", "sheet.pdf"), ("drawing_export_dxf", "sheet.dxf"), ("drawing_export_svg", "sheet.svg")]
    {
        let path = s.path(file);
        s.call(tool, json!({ "path": path }));
        assert!(dir.join(file).is_file(), "{tool} wrote no {file}");
    }
    let (part, copy) = (s.path("part.dwprt"), s.path("copy.dwprt"));
    s.call("drawing_repair_reference", json!({ "path": part }));
    s.call("drawing_replace_model", json!({ "path": copy }));
    let iso = views.iter().map(name).find(|n| n != &first && n != &child).unwrap();
    s.call("drawing_delete", json!({ "view": iso, "annotation": null }));
    s.call("open", json!({ "path": drawing_file }));
    let reopened = s.drawing();
    assert_eq!(reopened["sheets"].as_array().unwrap().len(), 2, "the saved drawing has the added sheet: {reopened}");
    s.call("drawing_close", json!({}));
    s.call("new_part", json!({}));
    assert!(s.state.doc.features.is_empty());
    s.call("open", json!({ "path": part }));
    assert!(s.state.doc.features.iter().any(|f| f.name == "Hole"), "the saved part opens with its features");

    let tools: BTreeSet<String> = tool_specs(Surface::Mcp).into_iter().map(|t| t.name).collect();
    let missed: Vec<_> = tools.difference(&s.ran).collect();
    assert!(missed.is_empty(), "this test calls no {missed:?}; add each to it");
    let _ = std::fs::remove_dir_all(&dir);
}
