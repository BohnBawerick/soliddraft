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

#[path = "support/configuration_tools.rs"]
mod configuration_tools;

/// The undo depths of the open documents, with identities for replacement.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Depths {
    part: usize,
    drawing: Option<usize>,
    assembly: Option<(DocumentId, usize)>,
    part_id: DocumentId,
}

fn depths(st: &AppState) -> Depths {
    Depths {
        part: st.doc.undo_depth(),
        drawing: st.drawing.session.as_ref().map(|s| s.doc.undo_depth()),
        assembly: st.assembly.session.as_ref().map(|s| (s.doc.id(), s.doc.undo_depth())),
        part_id: st.doc.model().id(),
    }
}

/// Whether the change in depths is what `undo` says a command does.
fn undoes_as_said(undo: Undo, before: Depths, after: Depths) -> bool {
    let drawing = |d: Depths| d.drawing.unwrap_or(0);
    let assembly = |d: Depths| d.assembly.map(|(_, depth)| depth).unwrap_or(0);
    let same_assembly = before.assembly.map(|(id, _)| id) == after.assembly.map(|(id, _)| id);
    let part_moved = after.part as isize - before.part as isize;
    let drawing_moved = drawing(after) as isize - drawing(before) as isize;
    let assembly_moved = assembly(after) as isize - if same_assembly { assembly(before) as isize } else { 0 };
    let same_drawing = before.drawing.is_some() == after.drawing.is_some();
    match undo {
        Undo::None => before == after,
        Undo::Step => {
            same_drawing && matches!((part_moved, drawing_moved, assembly_moved), (1, 0, 0) | (0, 1, 0) | (0, 0, 1))
        }
        Undo::Steps => same_assembly && same_drawing && part_moved == 0 && assembly_moved == 0 && drawing_moved >= 1,
        Undo::Moves => {
            same_assembly && same_drawing && (part_moved.abs() + drawing_moved.abs() + assembly_moved.abs()) == 1
        }
        // A raw journal command: its own op says.
        Undo::Carried => true,
        // Opening a document replaces its history; the other documents stay put.
        Undo::Replaces => {
            let same_part = before.part == after.part && before.part_id == after.part_id;
            (before.assembly == after.assembly
                && ((after.part == 0 && before.part_id != after.part_id && before.drawing == after.drawing)
                    || (after.drawing == Some(0) && before.drawing != Some(0) && same_part)
                    || (after.drawing.is_none() && before.drawing.is_some() && same_part)))
                || (same_part
                    && before.drawing == after.drawing
                    && after.assembly.is_some_and(|(_, depth)| depth == 0)
                    && before.assembly != after.assembly)
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
    // Catallax does not yet draft a body with a hole or a bored boss, so those are suppressed
    // around the draft, and the draft comes before the fillet.
    s.call("suppress", json!({ "feature": "Cut-Extrude2", "suppressed": true }));
    s.call("suppress", json!({ "feature": "Cut-Extrude1", "suppressed": true }));
    s.call("suppress", json!({ "feature": "Boss-Extrude3", "suppressed": true }));
    let draft = json!({ "faces": ["Boss-Extrude1 · Side of line 3"], "neutral": "top", "angle": 2, "reverse": false });
    s.call("draft", draft);
    s.call("edit_draft", json!({ "feature": "Draft1", "angle": 3 }));
    s.call("undo", json!({}));
    s.call("undo", json!({}));
    s.call("suppress", json!({ "feature": "Boss-Extrude3", "suppressed": false }));
    s.call("suppress", json!({ "feature": "Cut-Extrude1", "suppressed": false }));
    s.call("suppress", json!({ "feature": "Cut-Extrude2", "suppressed": false }));
    s.call("fillet", json!({ "edges": ["Boss-Extrude2 · Edge 1"], "radius": 2 }));
    // A vertical corner at the plate's back: a flat bevel ending on the plate's top and bottom.
    // A rim would make a cone, which the drawing below is slow to mesh.
    s.call("chamfer", json!({ "edges": ["Boss-Extrude1 · Edge 10"], "distance": 1 }));
    s.call("edit_fillet", json!({ "feature": "Fillet1", "radius": 3 }));
    s.call("edit_chamfer", json!({ "feature": "Chamfer1", "distance": 2 }));
    // The other Catallax features, each undone so the part below stays as it was.
    s.call("hole", json!({ "face": "Boss-Extrude1 · End cap", "points": [[40, 30, 10]], "diameter": 5 }));
    s.call("edit_hole", json!({ "feature": "Hole1", "diameter": 6 }));
    s.call("undo", json!({}));
    s.call("undo", json!({}));
    s.call("linear_pattern", json!({ "direction": [0, 0, 1], "spacing": 100, "count": 2 }));
    s.call("edit_pattern", json!({ "feature": "LPattern1", "count": 2, "step": 110 }));
    s.call("undo", json!({}));
    s.call("undo", json!({}));
    let axis = json!({ "axis_origin": [-10, -10, 0], "axis_direction": [0, 0, 1], "count": 2, "angle": 90 });
    s.call("circular_pattern", axis);
    s.call("undo", json!({}));
    s.call("mirror", json!({ "plane": "Boss-Extrude1 · Side of line 4", "keep_original": true }));
    s.call("edit_mirror", json!({ "feature": "Mirror1", "keep_original": false }));
    s.call("undo", json!({}));
    s.call("undo", json!({}));
    s.call("edit_extrude", json!({ "feature": "Boss-Extrude4", "depth": 6, "flip": false }));
    s.call("edit_cut", json!({ "feature": "Cut-Extrude2", "through_all": true, "depth": 1, "flip": false }));
    s.call("suppress", json!({ "feature": "Boss-Extrude4", "suppressed": true }));
    s.call("delete_feature", json!({ "feature": "Boss-Extrude4" }));
    s.call("rename", json!({ "feature": "Cut-Extrude2", "name": "Hole" }));
    s.call("rename_plane", json!({ "plane": "right", "name": "Side" }));
    // The boss, not the plate: the plate's rectangle is centred, so a new size moves its edges
    // off the wall's, and Catallax refuses the fillet along a wall standing in from the plate's
    // edge (`fillet.unsupported-overflow`).
    s.call("edit_dimension", json!({ "sketch": "Sketch3", "dimension": 1, "value": 26 }));
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
    s.call("document_command", json!({ "command_json": r#"{"op":"app_undo"}"# }));
    s.call("document_command", json!({ "command_json": r#"{"op":"app_redo"}"# }));

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
    for v in &views {
        let (exact, polygon) = (v["accuracy"]["exact"].as_u64().unwrap(), v["accuracy"]["polygon"].as_u64().unwrap());
        assert_eq!(exact + polygon, v["curves"].as_u64().unwrap(), "every curve carries one label: {v}");
    }
    assert!(views.iter().any(|v| v["accuracy"]["exact"].as_u64() > Some(0)), "whole lines stay exact: {drawing}");
    let name = |v: &Value| v["name"].as_str().unwrap().to_owned();
    let child = views.iter().find(|v| v["aligned"] == true).map(name).expect("an aligned projected view");
    let first = name(&views[0]);
    let section = s.call(
        "drawing_section",
        json!({ "view": first, "through": [70, 45, 0], "direction": "vertical", "flip": false }),
    );
    assert_eq!(section["placed"], "Section A-A", "{section}");
    let placed = &section["drawing"]["sheets"][0]["views"][views.len()];
    assert_eq!(placed["kind"], "section", "{placed}");
    assert_eq!(
        (placed["parent"].as_str(), placed["preview"].as_bool(), placed["up_to_date"].as_bool()),
        (Some(first.as_str()), Some(true), Some(true)),
        "{placed}"
    );
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
    s.call("drawing_command", json!({ "command_json": r#"{"op":"undo"}"# }));
    s.call("drawing_command", json!({ "command_json": r#"{"op":"redo"}"# }));
    let drawing_file = s.path("part.dwdrw");
    s.call("drawing_save", json!({ "path": drawing_file }));
    for (tool, file) in
        [("drawing_export_pdf", "sheet.pdf"), ("drawing_export_dxf", "sheet.dxf"), ("drawing_export_svg", "sheet.svg")]
    {
        let path = s.path(file);
        let written = s.call(tool, json!({ "path": path }));
        assert!(dir.join(file).is_file(), "{tool} wrote no {file}");
        let exact = written["accuracy"]["exact"].as_u64().unwrap();
        let polygon = written["accuracy"]["polygon"].as_u64().unwrap();
        // The PDF writes every sheet; the others write the active sheet, which is the
        // added empty one here.
        if written["views"].as_u64().unwrap() > 0 {
            assert!(exact > 0 && polygon > 0, "{tool} reports what it wrote as exact and as polygon: {written}");
        } else {
            assert_eq!((exact, polygon), (0, 0), "{tool} wrote no view: {written}");
        }
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
    s.call("sketch", json!({"plane":"top","shapes":[{"shape":"rectangle","center":[0,0,0],"width":40,"height":30}]}));
    s.call("extrude", json!({"sketch":"Sketch1","depth":12,"flip":false}));
    let area = s.call("measure_entity", json!({"entity":"Boss-Extrude1 · End cap"}));
    assert_eq!(area["quantity"], "face_area");
    assert_eq!(area["value"], 1200.0);
    assert_eq!(area["unit"], "mm2");
    s.call("track", json!({"name":"plate_area","entity":"Boss-Extrude1 · End cap","check":"> 1000"}));
    let values = s.call("model_summary", json!({}))["values"].clone();
    assert_eq!(values[0]["name"], "plate_area");
    assert_eq!(values[0]["tracked"]["check"]["state"], "pass");
    s.call("undo", json!({}));
    assert!(s.call("model_summary", json!({}))["values"].as_array().unwrap().is_empty());
    s.call("set_value", json!({"name":"part_depth","expression":"12"}));
    s.call("drive", json!({"feature":"Boss-Extrude1","parameter":"depth","expression":"part_depth"}));
    s.call("rename_value", json!({"name":"part_depth","to":"plate_depth"}));
    configuration_tools::exercise(&mut s);
    s.call("shell", json!({"open_faces":["Boss-Extrude1 · End cap"],"thickness":2}));
    s.call("edit_shell", json!({"feature":"Shell1","thickness":1.5}));
    s.call("new_part", json!({}));
    assert!(s.state.doc.features.is_empty());
    // Revolve on a fresh part exercises both verbs without changing the bracket.
    s.call(
        "sketch",
        json!({"plane":"top", "shapes":[
            {"shape":"rectangle", "center":[9,6,0], "width":6, "height":12}
        ]}),
    );
    let revolved = s.call(
        "revolve",
        json!({"sketch":"Sketch1", "axis_origin":[0,0,0],
        "axis_direction":[0,1,0], "angle_deg":180}),
    );
    assert!(!revolved.is_null(), "revolve must succeed");
    let edited = s.call("edit_revolve", json!({"feature":"Revolve1", "angle_deg":360}));
    assert!(!edited.is_null(), "edit_revolve must succeed");
    s.call("undo", json!({}));
    s.call("redo", json!({}));
    // The terminal lattice and its edit each add one undo step.
    s.call("new_part", json!({}));
    s.call(
        "sketch",
        json!({"plane":"top", "offset":-2.5, "shapes":[
            {"shape":"rectangle", "center":[0,0,-2.5], "width":25, "height":15}
        ]}),
    );
    s.call("extrude", json!({"sketch":"Sketch1", "depth":10, "flip":false}));
    let lattice =
        s.call("lattice", json!({"cell":"cubic", "cell_size":5, "radius":1, "accuracy":0.5, "min_detail":0.5}));
    assert!(!lattice.is_null(), "lattice must succeed");
    let edited = s.call("edit_lattice", json!({"feature":"Lattice1", "radius":2.25}));
    assert!(!edited.is_null(), "edit_lattice must succeed");
    s.call("undo", json!({}));
    s.call("undo", json!({}));
    // The terminal gyroid and its edit do the same.
    let gyroid = s.call("gyroid", json!({"kind":"network", "period":10, "level":0, "accuracy":0.5, "min_detail":0.5}));
    assert!(!gyroid.is_null(), "gyroid must succeed");
    let edited = s.call("edit_gyroid", json!({"feature":"Gyroid1", "level":-0.5}));
    assert!(!edited.is_null(), "edit_gyroid must succeed");
    s.call("undo", json!({}));
    s.call("undo", json!({}));
    // D0 of the graded-field design: its grade, its moments and its mass stand-in.
    s.call("new_part", json!({}));
    s.call(
        "sketch",
        json!({"plane":"top", "offset":20, "shapes":[
            {"shape":"rectangle", "center":[0,0,20], "width":6, "height":6}
        ]}),
    );
    s.call("extrude", json!({"sketch":"Sketch1", "depth":10, "flip":false}));
    s.call("lattice", json!({"cell":"cubic", "cell_size":100, "radius":1, "accuracy":0.1, "min_detail":0.1}));
    s.call("grade_field", json!({"feature":"Lattice1", "axis":"z", "from":20, "to":30, "start":1, "end":2}));
    let mass = s.call("field_mass", json!({}));
    assert_eq!(mass["field_mass"]["scope"], "original-field-definition", "{mass}");
    let stand_in = s.path("stand-in.step");
    s.call("export_stand_in", json!({ "path": stand_in }));
    assert!(dir.join("stand-in.step").is_file(), "export_stand_in wrote no stand-in.step");
    s.call("open", json!({ "path": part }));
    assert!(s.state.doc.features.iter().any(|f| f.name == "Hole"), "the saved part opens with its features");

    // Assembly tools use a small block and keep their own undo history.
    s.call("new_part", json!({}));
    s.call(
        "sketch",
        json!({ "plane": "top", "shapes": [{
        "shape": "rectangle", "center": [0,0,0], "width": 40, "height": 30
    }] }),
    );
    s.call("extrude", json!({ "sketch": "Sketch1", "depth": 10, "flip": false }));
    let linked_part = s.path("linked.dwprt");
    s.call("save", json!({ "path": linked_part }));
    let made = s.call("make_assembly", json!({}));
    assert_eq!(made["assembly"]["components"].as_array().unwrap().len(), 1);
    s.call("assembly_show_part", json!({}));
    s.call("rename", json!({ "feature": "Boss-Extrude1", "name": "Linked block" }));
    s.call("save", json!({ "path": linked_part }));
    s.call("show_assembly", json!({}));
    let updated = s.call("assembly_update", json!({}));
    assert_eq!(updated["updated"], 1);
    s.call("undo", json!({}));
    s.call("redo", json!({}));
    let assembly_file = s.path("rig.dwasm");
    s.call("assembly_save", json!({ "path": assembly_file }));
    s.call("open", json!({ "path": assembly_file }));
    assert_eq!(s.call("model_summary", json!({}))["assembly"]["active"], true);

    // Mates between two parts, each change one assembly undo step.
    s.call("assembly_show_part", json!({}));
    s.call("new_part", json!({}));
    s.call(
        "sketch",
        json!({ "plane": "top", "shapes": [{
        "shape": "rectangle", "center": [0,0,0], "width": 20, "height": 10
    }] }),
    );
    s.call("extrude", json!({ "sketch": "Sketch1", "depth": 6, "flip": false }));
    let second = s.path("second.dwprt");
    s.call("save", json!({ "path": second }));
    s.call("show_assembly", json!({}));
    let placement = json!({ "translation": [10, 10, 30], "rotation": [0, 0, 0, 1] });
    let inserted = s.call("insert_component", json!({ "paths": [second], "placement": placement }));
    let parts: Vec<u64> = inserted["assembly"]["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["path"].as_str().unwrap().parse().unwrap())
        .collect();
    let top = json!({ "component": parts[0], "feature": "Linked block", "generator": "end_cap" });
    let bottom = json!({ "component": parts[1], "feature": "Boss-Extrude1", "generator": "start_cap" });
    let gap = s.call("assembly_measure_planes", json!({ "a": top, "b": bottom }));
    assert_eq!(gap["measurement"]["rational"], "20");
    s.call("assembly_mate_origin", json!({ "name": "O", "component": parts[0], "align_axes": true }));
    let z = json!({ "name": "Z", "a": top, "b": bottom, "distance": "5 mm", "alignment": "opposed" });
    let mated = s.call("assembly_mate_distance", z);
    assert_eq!(mated["assembly"]["components"][1]["translation"], json!([10.0, 10.0, 15.0]));
    s.call("assembly_set_value", json!({ "name": "gap", "expression": "1.2 cm" }));
    s.call("assembly_rename_value", json!({ "name": "gap", "to": "clearance" }));
    let drive = json!({ "mate": "Z", "parameter": "distance", "expression": "clearance" });
    let driven = s.call("assembly_drive_mate", drive);
    assert_eq!(driven["assembly"]["components"][1]["translation"], json!([10.0, 10.0, 22.0]));
    s.call("assembly_edit_mate", json!({ "mate": "O", "align_axes": false }));
    s.call("assembly_delete_mate", json!({ "mate": "Z" }));
    // The second part moves into a file of its own, placed where it was, in one undo step.
    let at = parts[1].to_string();
    let module = s.path("module.dwasm");
    let formed = s.call("assembly_form_subassembly", json!({ "paths": [at], "path": module, "origin": at }));
    assert_eq!(formed["formed"]["migration"]["occurrences"][0]["from"], json!(at));

    s.call("assembly_show_part", json!({}));
    s.call("new_part", json!({}));
    std::fs::write(s.dir.join("source.step"), include_bytes!("../../../ci/fixtures/import/block-seam-4um.step"))
        .unwrap();
    let source = s.path("source.step");
    let preview = s.call("preview_import", json!({"path": source}));
    let imported = s.call("import_step", json!({"path": source, "preview_token": preview["token"], "decline": []}));
    assert_eq!(imported["features"][0]["type"], "import_step");
    s.call("undo", json!({}));
    s.call("redo", json!({}));

    let tools: BTreeSet<String> = tool_specs(Surface::Mcp).into_iter().map(|t| t.name).collect();
    let missed: Vec<_> = tools.difference(&s.ran).collect();
    assert!(missed.is_empty(), "this test calls no {missed:?}; add each to it");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn editing_and_inspection_measure_only_on_request_and_keep_body_results_separate() {
    use soliddraft_app::document::{Document, Selection};
    use soliddraft_kernel_catallax::CatallaxKernel;
    use std::sync::Arc;

    let kernel = Arc::new(CatallaxKernel::with_operation_log());
    let mut session = Session {
        state: AppState::new(Document::new("lazy mass", kernel.clone())),
        ran: BTreeSet::new(),
        dir: std::env::temp_dir(),
    };
    session.call(
        "sketch",
        json!({
            "plane":"top",
            "shapes":[{"shape":"rectangle","center":[0,0,0],"width":40,"height":30}]
        }),
    );
    session.call("extrude", json!({"sketch":"Sketch1","depth":10,"flip":false}));
    let feature = session.state.doc.features[1].id;
    session.state.doc.describe(Selection::Feature(feature)).unwrap();
    let summary = session.call("model_summary", json!({}));
    assert!(summary["body"]["volume_mm3"].is_null(), "{summary}");
    let evidence = kernel.take_diagnostics().unwrap();
    assert!(
        evidence["calls"].as_array().unwrap().iter().all(|call| call["operation"] != "mass_properties"),
        "{evidence}"
    );

    let measured = session.call("mass_properties", json!({"name":"Boss-Extrude1"}));
    assert_eq!(measured["volume_mm3"], 12000.0);
    let summary = session.call("model_summary", json!({}));
    assert_eq!(summary["body"]["volume_mm3"], 12000.0);
    let description = session.state.doc.describe(Selection::Feature(feature)).unwrap();
    assert!(description.details.iter().any(|(key, _)| key == "Volume"));

    session.call("edit_extrude", json!({"feature":"Boss-Extrude1","depth":20,"flip":false}));
    let summary = session.call("model_summary", json!({}));
    assert!(summary["body"]["volume_mm3"].is_null(), "new body must not inherit old measurements: {summary}");
    session.call("undo", json!({}));
    let summary = session.call("model_summary", json!({}));
    assert_eq!(summary["body"]["volume_mm3"], 12000.0);
    session.call("redo", json!({}));
    let summary = session.call("model_summary", json!({}));
    assert!(summary["body"]["volume_mm3"].is_null(), "{summary}");
    let measured = session.call("mass_properties", json!({"name":"Boss-Extrude1"}));
    assert_eq!(measured["volume_mm3"], 24000.0);
}

fn offset_session() -> Session {
    Session { state: AppState::new(soliddraft_app::new_document()), ran: BTreeSet::new(), dir: std::env::temp_dir() }
}

#[test]
fn datum_offset_survives_dimension_edits_and_save_reopen() {
    let mut s = offset_session();
    s.dir = s.dir.join(format!("soliddraft-datum-offset-{}", std::process::id()));
    std::fs::create_dir_all(&s.dir).unwrap();
    s.call(
        "sketch",
        json!({
            "plane": "front", "offset": -6,
            "shapes": [{"shape": "circle", "center": [30, 6, 18], "diameter": 6}]
        }),
    );
    s.call("extrude", json!({"sketch": "Sketch1", "depth": 7, "flip": false}));
    let mass = s.state.doc.kernel().mass_properties(s.state.doc.display_body().unwrap()).unwrap();
    assert!((mass.volume - 63.0 * std::f64::consts::PI).abs() < 0.005);
    assert!((mass.centroid.y - 2.5).abs() < 1e-6);

    s.call("edit_dimension", json!({"sketch": "Sketch1", "dimension": 1, "value": 8}));
    let path = s.path("offset.dwprt");
    s.call("save", json!({"path": path}));
    s.call("new_part", json!({}));
    s.call("open", json!({"path": path}));
    s.call("edit_dimension", json!({"sketch": "Sketch1", "dimension": 1, "value": 10}));
    let mass = s.state.doc.kernel().mass_properties(s.state.doc.display_body().unwrap()).unwrap();
    assert!((mass.volume - 175.0 * std::f64::consts::PI).abs() < 0.005);
    assert!((mass.centroid.x - 30.0).abs() < 1e-6);
    assert!((mass.centroid.y - 2.5).abs() < 1e-6);
    assert!((mass.centroid.z - 18.0).abs() < 1e-6);
    std::fs::remove_dir_all(&s.dir).unwrap();
}

#[test]
fn geometry_sketch_datum_offset_moves_the_solid_along_the_normal() {
    let mut s = offset_session();
    s.call(
        "sketch_geometry",
        json!({
            "plane": "right", "offset": 3,
            "operations": [{"tool": "polygon", "center": {"u": 0, "v": 0},
                            "vertex": {"u": 2, "v": 0}, "sides": 4}]
        }),
    );
    s.call("extrude", json!({"sketch": "Sketch1", "depth": 2, "flip": false}));
    let mass = s.state.doc.kernel().mass_properties(s.state.doc.display_body().unwrap()).unwrap();
    assert!((mass.volume - 16.0).abs() < 0.005);
    assert!((mass.centroid.x - 4.0).abs() < 1e-6);
    assert!(mass.centroid.y.abs() < 1e-6);
    assert!(mass.centroid.z.abs() < 1e-6);
}

#[test]
fn invalid_datum_offsets_leave_the_part_and_history_unchanged() {
    let mut s = offset_session();
    s.call(
        "sketch",
        json!({"plane": "top", "shapes": [{"shape": "rectangle", "center": [5, 5, 0], "width": 10, "height": 10}]}),
    );
    s.call("extrude", json!({"sketch": "Sketch1", "depth": 5, "flip": false}));
    let before = depths(&s.state);
    let body = s.state.doc.display_body().unwrap();
    for (tool, arguments) in [
        (
            "sketch",
            json!({"plane": "Boss-Extrude1 · End cap", "offset": 1,
                   "shapes": [{"shape": "circle", "center": [5, 5, 5], "diameter": 2}]}),
        ),
        (
            "sketch_geometry",
            json!({"plane": "top", "offset": soliddraft_app::command_tools::MAX_LENGTH_MM + 1.0,
                   "operations": [{"tool": "polygon", "center": {"u": 0, "v": 0},
                                   "vertex": {"u": 2, "v": 0}, "sides": 4}]}),
        ),
    ] {
        let out = call_checked(
            &mut AppTools { state: &mut s.state },
            tool,
            arguments,
            &Caller::Mcp { client: "test".into() },
        );
        assert!(out.is_error, "{}", out.content);
        assert_eq!(depths(&s.state), before);
        assert_eq!(s.state.doc.display_body(), Some(body));
    }
}
