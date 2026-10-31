//! D1: typed values drive mates through the real command route, with one grammar and one
//! owner per input. Every refusal leaves the accepted assembly exactly as it was.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use soliddraft_app::app_tools::AppTools;
use soliddraft_app::state::AppState;
use soliddraft_assistant::Caller;
use soliddraft_assistant::tools::call_checked;

struct Rig {
    state: AppState,
    dir: PathBuf,
}

fn side(component: u64, normal: [f64; 3]) -> Value {
    json!({ "component": component, "feature": "Boss-Extrude1", "generator": "side", "normal": normal })
}

fn cap(component: u64, generator: &str) -> Value {
    json!({ "component": component, "feature": "Boss-Extrude1", "generator": generator })
}

impl Rig {
    fn call(&mut self, name: &str, arguments: Value) -> Result<Value, Value> {
        let caller = Caller::Mcp { client: "test".into() };
        let out = call_checked(&mut AppTools { state: &mut self.state }, name, arguments, &caller);
        let text: Value = serde_json::from_str(&out.content).unwrap_or(Value::String(out.content.clone()));
        if out.is_error { Err(text) } else { Ok(text) }
    }

    fn ok(&mut self, name: &str, arguments: Value) -> Value {
        self.call(name, arguments.clone()).unwrap_or_else(|e| panic!("{name} {arguments}: {e}"))
    }

    fn part(&mut self, file: &Path, width: f64, height: f64, depth: f64) {
        let shape = json!({ "shape": "rectangle", "center": [0, 0, 0], "width": width, "height": height });
        self.ok("sketch", json!({ "plane": "top", "shapes": [shape] }));
        self.ok("extrude", json!({ "sketch": "Sketch1", "depth": depth, "flip": false }));
        self.ok("save", json!({ "path": file }));
    }

    fn assembly(&mut self) -> Value {
        self.ok("model_summary", json!({}))["assembly"].clone()
    }

    fn accepted(&mut self) -> Value {
        self.assembly()["accepted_state"].clone()
    }

    fn translation(&mut self) -> Value {
        self.assembly()["components"][1]["translation"].clone()
    }

    /// Refused with `code`, leaving the accepted state alone.
    fn refused(&mut self, name: &str, arguments: Value, code: &str) -> Value {
        let before = self.accepted();
        let refusal = self.call(name, arguments).expect_err(name);
        assert_eq!(refusal["refused"], code, "{refusal}");
        assert_eq!(self.accepted(), before, "{name} changed the accepted state");
        refusal
    }
}

/// The main graph at D = 10, with `gap` driving Z.
fn rig(name: &str) -> Rig {
    let dir = std::env::temp_dir().join(format!("soliddraft-mate-distance-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    soliddraft_app::file::set_config_dir(Some(dir.join("config")));
    let mut r = Rig { state: AppState::new(soliddraft_app::new_document()), dir };
    let (a, b) = (r.dir.join("a.dwprt"), r.dir.join("b.dwprt"));
    r.part(&a, 40.0, 30.0, 10.0);
    r.ok("make_assembly", json!({}));
    r.ok("assembly_save", json!({ "path": r.dir.join("rig.dwasm") }));
    r.ok("new_part", json!({}));
    r.part(&b, 20.0, 10.0, 6.0);
    r.ok("show_assembly", json!({}));
    let placement = json!({ "translation": [10, 10, 30], "rotation": [0, 0, 0, 1] });
    r.ok("insert_component", json!({ "paths": [b], "placement": placement }));
    r.ok("assembly_mate_origin", json!({ "name": "O", "component": 2, "align_axes": true }));
    let (x, y) = ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
    for (name, a, b) in [("X", side(2, x), side(4, x)), ("Y", side(2, y), side(4, y))] {
        r.ok(
            "assembly_mate_distance",
            json!({ "name": name, "a": a, "b": b, "distance": "0 mm", "alignment": "aligned" }),
        );
    }
    let (a, b) = (cap(2, "end_cap"), cap(4, "start_cap"));
    r.ok("assembly_mate_distance", json!({ "name": "Z", "a": a, "b": b, "distance": "5 mm", "alignment": "opposed" }));
    r.ok("assembly_set_value", json!({ "name": "gap", "expression": "5 mm" }));
    r.ok("assembly_drive_mate", json!({ "mate": "Z", "parameter": "distance", "expression": "gap" }));
    r
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn typed_values_have_one_implementation_and_one_owner() {
    let mut r = rig("values");
    assert_eq!(r.translation(), json!([10.0, 10.0, 15.0]));

    // Each kind of bad value is refused whole.
    r.refused("assembly_set_value", json!({ "name": "gap", "expression": "12 deg" }), "unit_mismatch");
    r.refused("assembly_set_value", json!({ "name": "gap", "expression": "gap + 1 mm" }), "value_cycle");
    r.refused("assembly_set_value", json!({ "name": "gap", "expression": "2 parsecs" }), "invalid_value");
    let said = r.refused("assembly_edit_mate", json!({ "mate": "Z", "distance": "9 mm" }), "driven_input");
    assert!(said["message"].as_str().unwrap().contains("edit gap"), "{said}");

    // One owner: a second driver is refused, and replacing is asked for by name.
    r.ok("assembly_set_value", json!({ "name": "other", "expression": "7 mm" }));
    let bind = json!({ "mate": "Z", "parameter": "distance", "expression": "other" });
    r.refused("assembly_drive_mate", bind.clone(), "already_driven");
    let mut swap = bind;
    swap["replace"] = json!(true);
    r.ok("assembly_drive_mate", swap);
    assert_eq!(r.translation(), json!([10.0, 10.0, 17.0]));
    r.ok("assembly_drive_mate", json!({ "mate": "Z", "parameter": "distance", "expression": "gap", "replace": true }));
    assert_eq!(r.translation(), json!([10.0, 10.0, 15.0]));

    // A rename moves the label: the binding is by id, and nothing moves.
    r.ok("assembly_set_value", json!({ "name": "gap", "expression": "1.2 cm" }));
    let before = r.assembly();
    let id = before["values"][0]["id"].clone();
    assert_eq!(before["mates"][3]["distance"]["reads"], json!([id]));
    r.ok("assembly_rename_value", json!({ "name": "gap", "to": "clearance" }));
    let after = r.assembly();
    assert_eq!(after["values"][0]["id"], id);
    assert_eq!(after["values"][0]["name"], "clearance");
    assert_eq!(after["values"][0]["value"], 12.0);
    assert_eq!(after["mates"][3]["distance"]["expression"], "clearance");
    assert_eq!(after["mates"][3]["distance"]["reads"], json!([id]));
    assert_eq!(after["components"], before["components"]);
    let taken = json!({ "name": "clearance", "to": "other" });
    r.refused("assembly_rename_value", taken, "invalid_value");

    // A value nothing reads changes the truth and leaves the pose.
    let revision = r.assembly()["revision"].clone();
    r.ok("assembly_set_value", json!({ "name": "unused", "expression": "3" }));
    let after = r.assembly();
    assert_ne!(after["revision"], revision);
    assert_eq!(after["components"][1]["translation"], json!([10.0, 10.0, 22.0]));

    // The shared evaluator's decimal conversion and arithmetic reach the pose.
    r.ok("assembly_set_value", json!({ "name": "clearance", "expression": "1.5 in" }));
    assert_eq!(r.translation(), json!([10.0, 10.0, 48.1]));
    r.ok("assembly_set_value", json!({ "name": "clearance", "expression": "unused * 2 + 1 mm" }));
    assert_eq!(r.translation(), json!([10.0, 10.0, 17.0]));
}

#[test]
fn a_mate_commits_or_refuses_as_one_step() {
    let mut r = rig("steps");
    let undo = r.assembly()["accepted_state"]["undo"]["depth"].as_u64().unwrap();
    r.ok("assembly_set_value", json!({ "name": "gap", "expression": "12 mm" }));
    let a = r.assembly();
    assert_eq!(a["accepted_state"]["undo"]["depth"], undo + 1);
    assert_eq!(a["components"][1]["translation"], json!([10.0, 10.0, 22.0]));
    assert_eq!(a["solver"]["state"], "solved");
    assert_eq!(a["mates"][3]["measurement"]["rational"], "12");
    r.ok("undo", json!({}));
    assert_eq!(r.translation(), json!([10.0, 10.0, 15.0]));
    // A refusal keeps the redo it found.
    let redo = r.accepted()["redo"].clone();
    r.refused("assembly_set_value", json!({ "name": "gap", "expression": "12 deg" }), "unit_mismatch");
    assert_eq!(r.accepted()["redo"], redo);
    r.ok("redo", json!({}));
    assert_eq!(r.translation(), json!([10.0, 10.0, 22.0]));
}

#[test]
fn faces_are_found_or_refused_by_what_they_are() {
    let mut r = rig("selectors");
    let a = json!({ "component": 2, "face": "Boss-Extrude1 · End cap" });
    let b = json!({ "component": 4, "face": "Boss-Extrude1 · Start cap" });
    let m = r.ok("assembly_measure_planes", json!({ "a": a, "b": b }));
    assert_eq!(m["measurement"]["rational"], "5");
    let read = |c: u64, generator: &str| json!({ "component": c, "feature": "Boss-Extrude1", "generator": generator });
    let sides = json!({ "a": read(2, "side"), "b": read(4, "end_cap") });
    r.refused("assembly_measure_planes", sides, "ambiguous_reference");
    let none = json!({ "a": { "component": 2, "face": "No such face" }, "b": read(4, "end_cap") });
    r.refused("assembly_measure_planes", none, "no_such_face");
    let across = json!({ "a": side(2, [1.0, 0.0, 0.0]), "b": side(4, [0.0, 1.0, 0.0]) });
    r.refused("assembly_measure_planes", across, "not_parallel");
}
