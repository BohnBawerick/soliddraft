//! V6 batch 1 through the production command route: the flat assembly of the design report
//! (base A, pin B, C and F) is built by MCP calls, formed into `modules/pack.dwasm`, saved
//! and reopened. File bytes are compared by hash, which the scenario runner cannot do.

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

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn side(component: u64, normal: [f64; 3]) -> Value {
    json!({ "component": component, "feature": "Boss-Extrude1", "generator": "side", "normal": normal })
}

fn cap(component: u64, generator: &str) -> Value {
    json!({ "component": component, "feature": "Boss-Extrude1", "generator": generator })
}

fn sha(path: &Path) -> String {
    soliddraft_document::ContentHash::of_bytes(&std::fs::read(path).unwrap()).to_string()
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

    fn assembly(&mut self) -> Value {
        self.ok("model_summary", json!({}))["assembly"].clone()
    }

    fn part(&mut self, file: &Path, width: f64, height: f64, depth: f64) {
        let shape = json!({ "shape": "rectangle", "center": [0, 0, 0], "width": width, "height": height });
        self.ok("sketch", json!({ "plane": "top", "shapes": [shape] }));
        self.ok("extrude", json!({ "sketch": "Sketch1", "depth": depth, "flip": false }));
        self.ok("save", json!({ "path": file }));
    }

    fn file(&self, name: &str) -> PathBuf {
        self.dir.join("work").join(name)
    }

    fn form(&self) -> Value {
        json!({ "paths": ["4", "5", "6"], "path": self.file("modules/pack.dwasm"), "origin": "4" })
    }
}

/// Steps 1 to 23: the flat root T0, saved, with F selected as its insertion left it.
fn flat(name: &str) -> Rig {
    let dir = std::env::temp_dir().join(format!("soliddraft-subassembly-app-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("work/parts")).unwrap();
    soliddraft_app::file::set_config_dir(Some(dir.join("config")));
    let mut r = Rig { state: AppState::new(soliddraft_app::new_document()), dir };
    let (base, pin, root) = (r.file("parts/base.dwprt"), r.file("parts/pin.dwprt"), r.file("root.dwasm"));
    r.part(&base, 40.0, 30.0, 10.0);
    r.ok("make_assembly", json!({}));
    r.ok("assembly_save", json!({ "path": root }));
    r.ok("new_part", json!({}));
    r.part(&pin, 20.0, 10.0, 6.0);
    r.ok("show_assembly", json!({}));
    for (t, q) in [([10, 10, 22], [0, 0, 0, 1]), ([10, 10, 31], [0, 0, 0, 1]), ([60, -20, 7], [0, 0, 1, 0])] {
        r.ok("insert_component", json!({ "paths": [pin], "placement": { "translation": t, "rotation": q } }));
    }
    r.ok("assembly_mate_origin", json!({ "name": "O", "component": 2, "align_axes": true }));
    let (x, y) = ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
    let mates = [
        ("Xab", side(2, x), side(4, x), "0 mm", "aligned"),
        ("Yab", side(2, y), side(4, y), "0 mm", "aligned"),
        ("Zab", cap(2, "end_cap"), cap(4, "start_cap"), "12 mm", "opposed"),
        ("Xbc", side(4, x), side(5, x), "0 mm", "aligned"),
        ("Ybc", side(4, y), side(5, y), "0 mm", "aligned"),
        ("Zbc", cap(4, "end_cap"), cap(5, "start_cap"), "3 mm", "opposed"),
    ];
    for (name, a, b, d, alignment) in mates {
        let arguments = json!({ "name": name, "a": a, "b": b, "distance": d, "alignment": alignment });
        r.ok("assembly_mate_distance", arguments);
    }
    r.ok("assembly_set_value", json!({ "name": "gap", "expression": "12 mm" }));
    r.ok("assembly_drive_mate", json!({ "mate": "Zab", "parameter": "distance", "expression": "gap" }));
    r.ok("assembly_save", json!({ "path": root }));
    r
}

/// The world translation of the occurrence at `path`.
fn at(assembly: &Value, path: &str) -> Value {
    assembly["occurrences"][path]["world"]["translation"].clone()
}

/// Steps 24 and 27 to 29: Form changes the open root and writes only the new file; the
/// selection follows F; saving writes the root; both files reopen as formed.
#[test]
fn form_publishes_one_new_file_and_changes_the_root_in_memory() {
    let mut r = flat("form");
    let (root, base, pin, pack) =
        (r.file("root.dwasm"), r.file("parts/base.dwprt"), r.file("parts/pin.dwprt"), r.file("modules/pack.dwasm"));
    let t0 = r.assembly();
    let hashes = (sha(&root), sha(&base), sha(&pin));

    let formed = r.ok("assembly_form_subassembly", r.form());
    assert_eq!(formed["selected"], "16/6", "F stays selected, at its nested path");
    let assembly = formed["assembly"].clone();
    assert_eq!((sha(&root), sha(&base), sha(&pin)), hashes, "Form writes no file but the new one");
    assert!(pack.is_file());
    assert_eq!(formed["formed"]["migration"]["child_revision"], formed["child"]["revision"]);
    assert_eq!(assembly["undo"], "Form subassembly");
    assert_eq!(assembly["dirty"], true);
    assert_eq!(
        assembly["accepted_state"]["undo"]["depth"],
        t0["accepted_state"]["undo"]["depth"].as_u64().unwrap() + 1
    );
    for (before, after) in [("4", "16/4"), ("5", "16/5"), ("6", "16/6")] {
        assert_eq!(at(&assembly, after), at(&t0, before), "{before} stays where it was");
        assert_eq!(assembly["occurrences"][after]["pinned"], t0["occurrences"][before]["pinned"]);
    }
    assert!(assembly["occurrences"].get("4").is_none());
    assert_eq!(assembly["values"], t0["values"]);
    assert_eq!(assembly["solver"]["remaining_dof"], 6);
    assert_eq!(formed["formed"]["status"]["child"]["remaining_dof"], 12);
    let pack_bytes = sha(&pack);

    // A second Form to the same file is refused whole, and keeps what is there.
    r.ok("undo", json!({}));
    let before = r.assembly()["accepted_state"].clone();
    let refused = r.call("assembly_form_subassembly", r.form()).expect_err("the file is taken");
    assert_eq!(
        (refused["refused"].as_str(), refused["cause"].as_str()),
        (Some("destination_exists"), Some("pack.dwasm"))
    );
    assert_eq!(r.assembly()["accepted_state"], before);
    assert_eq!(sha(&pack), pack_bytes);
    r.ok("redo", json!({}));
    assert_eq!(r.assembly()["revision"], assembly["revision"]);
    assert_eq!(sha(&pack), pack_bytes, "redo writes nothing");

    let saved = r.ok("assembly_save", json!({ "path": root }))["assembly"].clone();
    assert_eq!(saved["dirty"], false);
    assert_ne!(sha(&root), hashes.0);
    assert_eq!((sha(&base), sha(&pin), sha(&pack)), (hashes.1.clone(), hashes.2.clone(), pack_bytes.clone()));

    let reopened = r.ok("open", json!({ "path": root }))["assembly"].clone();
    assert_eq!(reopened["accepted_state"]["truth"], saved["accepted_state"]["truth"]);
    assert_eq!(reopened["accepted_state"]["undo"]["depth"], 0);
    for path in ["2", "16/4", "16/5", "16/6"] {
        assert_eq!(at(&reopened, path), at(&saved, path), "{path}");
    }
    assert!(reopened["mates"].as_array().unwrap().iter().all(|m| m["status"] == "ok"), "{}", reopened["mates"]);

    let alone = r.ok("open", json!({ "path": pack }))["assembly"].clone();
    assert_eq!(alone["solver"]["remaining_dof"], 12);
    assert_eq!(at(&alone, "4"), json!([0.0, 0.0, 0.0]));
    assert_eq!(at(&alone, "5"), json!([0.0, 0.0, 9.0]));
    assert_eq!(alone["values"], json!([]));
    assert_eq!((sha(&base), sha(&pin), sha(&pack)), (hashes.1, hashes.2, pack_bytes));
}
