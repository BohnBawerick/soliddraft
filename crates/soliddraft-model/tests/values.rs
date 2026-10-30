//! V1 merge 2 module cases U1-U5 from the independent scenario design.

use std::sync::Arc;

use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::{Command, Document, values::Input};

fn empty() -> Document {
    Document::new("Values", Arc::new(CatallaxKernel::new()))
}

fn set(doc: &mut Document, name: &str, expression: &str) {
    doc.execute(Command::SetValue { name: name.into(), expression: expression.into() }).unwrap();
}

fn refused(doc: &mut Document, command: Command, message: &str) {
    let truth = doc.part_json();
    let revision = doc.semantic_hash();
    let geometry = doc.geometry_hash();
    let undo = (doc.undo_depth(), doc.undo_label().map(str::to_owned));
    let error = doc.execute(command).unwrap_err().to_string();
    assert!(error.contains(message), "{error}");
    assert_eq!(doc.part_json(), truth);
    assert_eq!(doc.semantic_hash(), revision);
    assert_eq!(doc.geometry_hash(), geometry);
    assert_eq!((doc.undo_depth(), doc.undo_label().map(str::to_owned)), undo);
}

#[test]
fn u1_self_reference_leaves_truth_and_history_unchanged() {
    let mut doc = empty();
    set(&mut doc, "plate_w", "140");
    refused(&mut doc, Command::SetValue { name: "plate_w".into(), expression: "plate_w + 1".into() }, "cycle");
}

#[test]
fn u2_unknown_unit_creates_nothing() {
    let mut doc = empty();
    refused(&mut doc, Command::SetValue { name: "plate_w".into(), expression: "6 xx".into() }, "unknown unit xx");
    assert!(doc.values().is_empty());
}

#[test]
fn u3_rename_collision_preserves_values_and_references() {
    let mut doc = empty();
    set(&mut doc, "plate_w", "140");
    set(&mut doc, "flange_h", "plate_w / 2");
    refused(&mut doc, Command::RenameValue { name: "plate_w".into(), to: "flange_h".into() }, "taken");
}

#[test]
fn u4_area_cannot_drive_a_length() {
    let mut doc = old_bracket();
    set(&mut doc, "plate_w", "140");
    let id = doc.features().iter().find(|f| f.name == "Boss-Extrude2").unwrap().id;
    refused(
        &mut doc,
        Command::Drive { input: Input::Depth { feature: id }, expression: "plate_w * plate_w".into() },
        "length",
    );
    assert!(doc.values().expressions(doc.feature(id).unwrap()).is_empty());
}

fn old_bracket() -> Document {
    Document::from_bytes(include_bytes!("fixtures/bracket-140.dwprt"), Arc::new(CatallaxKernel::new())).unwrap()
}

#[test]
fn u5_old_file_keeps_its_recorded_hashes_on_open_and_resave() {
    // Fixture: b730793f hosted run 40987458588, application-loop/bracket/bracket-140.dwprt.
    // main's `soliddraft doc info --json` output:
    // {
    //   "file": "/home/bohn/.treehouse/soliddraft-bf8ba8/2/soliddraft/crates/soliddraft-model/tests/fixtures/bracket-140.dwprt",
    //   "format": "soliddraft-part/2",
    //   "geometry_hash": "blake3:50d671cc7060d1b4f1b3e236a8a9ab122ebea067af6ddb705a3766eda0837078",
    //   "id": "bdc507ba-3860-4ce6-9c19-44c3b0167c1b",
    //   "kernel": "catallax 6142450b05b4f79e86e2f14afdb29f290f3cee5a",
    //   "kind": "part",
    //   "legacy": false,
    //   "references": [],
    //   "revision": "blake3:462c2f93c0c1898d9d30000e2636d58ff243aed677fcd7f200c2aa2266539469",
    //   "writer": "soliddraft 0.1.0"
    // }
    let doc = old_bracket();
    let revision = "blake3:462c2f93c0c1898d9d30000e2636d58ff243aed677fcd7f200c2aa2266539469";
    let geometry = "blake3:50d671cc7060d1b4f1b3e236a8a9ab122ebea067af6ddb705a3766eda0837078";
    assert!(doc.values().is_empty());
    assert_eq!(doc.semantic_hash().to_string(), revision);
    assert_eq!(doc.geometry_hash().to_string(), geometry);
    let manifest = doc.manifest();
    assert_eq!(manifest.semantic_hash.to_string(), revision);
    assert_eq!(manifest.geometry_hash.unwrap().to_string(), geometry);
    let saved = Document::from_bytes(&doc.to_bytes().unwrap(), doc.kernel().clone()).unwrap();
    assert_eq!(saved.semantic_hash().to_string(), revision);
    assert_eq!(saved.geometry_hash().to_string(), geometry);
}

fn check_dependency_depth(reverse_creation: bool) {
    let mut doc = empty();
    if reverse_creation {
        // Predeclare literals so reversed entry order needs no forward references.
        for n in (1..=33).rev() {
            set(&mut doc, &format!("v{n}"), "1");
        }
    } else {
        set(&mut doc, "v1", "1");
    }
    for n in 2..=32 {
        set(&mut doc, &format!("v{n}"), &format!("v{}", n - 1));
    }
    let values = doc.values().summary();
    let boundary = values.as_array().unwrap().iter().find(|v| v["name"] == "v32").unwrap();
    assert_eq!(boundary["value"], serde_json::json!(1.0));
    refused(&mut doc, Command::SetValue { name: "v33".into(), expression: "v32".into() }, "too deep");
}

#[test]
fn dependency_depth_boundary_in_creation_order_is_atomic() {
    check_dependency_depth(false);
}

#[test]
fn dependency_depth_boundary_in_reverse_creation_order_is_atomic() {
    check_dependency_depth(true);
}
