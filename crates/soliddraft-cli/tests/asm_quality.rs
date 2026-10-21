//! `soliddraft asm quality` in separate processes: override one placement's quality,
//! set one for every part, read them back, and check the revision does not move.

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::Arc;

use serde_json::Value;
use soliddraft_kernel_api::Kernel;
use soliddraft_kernel_smlib::SmlibKernel;
use soliddraft_model::sketch::{P2, Sketch, solve::solve};
use soliddraft_model::{
    Command, DatumPlane, Document, EndCondition, ExtrudeFeature, ExtrudeOp, FeatureKind, ProfileSource, SketchFeature,
    SketchPlane,
};

fn dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soliddraft-asm-quality-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(dir: &Path, args: &[&str]) -> (bool, String, String) {
    let out = Process::new(env!("CARGO_BIN_EXE_soliddraft"))
        .args(args)
        .current_dir(dir)
        .env("SOLIDDRAFT_CONFIG_DIR", dir.join(".config"))
        .output()
        .expect("running soliddraft");
    (out.status.success(), String::from_utf8(out.stdout).unwrap(), String::from_utf8(out.stderr).unwrap())
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let (ok, stdout, stderr) = run(dir, args);
    assert!(ok, "soliddraft {args:?} failed: {stderr}");
    stdout
}

fn json(dir: &Path, args: &[&str]) -> Value {
    let out = ok(dir, args);
    serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("{e}: {out}"))
}

/// A pin 60 mm across, saved as `pin.dwprt` in `dir`.
fn save_pin(dir: &Path) {
    let kernel: Arc<dyn Kernel> = Arc::new(SmlibKernel::new());
    let mut doc = Document::new("Pin", kernel);
    let mut sketch = Sketch::new();
    sketch.circle_at(P2::new(0.0, 0.0), 60.0).unwrap();
    solve(&mut sketch, None, true);
    let plane = SketchPlane::Datum { datum: DatumPlane::Top, offset: 0.0 };
    let mut add = |kind| {
        doc.execute(Command::AddFeature { kind, name: None, position: None, via_assistant: false })
            .unwrap()
            .created
            .unwrap()
    };
    let s = add(FeatureKind::Sketch(SketchFeature { plane, sketch }));
    add(FeatureKind::Extrude(ExtrudeFeature {
        profile: ProfileSource::Sketch { feature: s },
        operation: ExtrudeOp::Boss,
        end: EndCondition::Blind { depth: 40.0 },
        reverse: false,
    }));
    doc.save(dir.join("pin.dwprt")).unwrap();
}

fn revision(dir: &Path) -> String {
    json(dir, &["asm", "info", "rig.dwasm", "--json"])["revision"].as_str().unwrap().to_owned()
}

#[test]
fn quality_overrides_are_saved_without_a_new_revision() {
    let d = dir("override");
    save_pin(&d);
    ok(&d, &["asm", "new", "rig.dwasm"]);
    ok(&d, &["asm", "insert", "rig.dwasm", "pin.dwprt"]);
    ok(&d, &["asm", "insert", "rig.dwasm", "pin.dwprt", "--at", "100,0,0"]);
    let before = revision(&d);
    let tree = json(&d, &["asm", "tree", "rig.dwasm", "--json"]);
    let first = tree[0]["path"].as_str().unwrap().to_owned();
    assert!(tree[0]["quality"].is_null());

    ok(&d, &["asm", "quality", "rig.dwasm", &first, "--set", "10"]);
    let q = json(&d, &["asm", "quality", "rig.dwasm", "--json"]);
    assert_eq!(q["overrides"][0]["path"], first.as_str());
    assert_eq!(q["overrides"][0]["quality"], serde_json::json!({"mode": "slider", "value": 10}));
    assert_eq!(q["definitions"].as_array().unwrap().len(), 1);
    assert!(q["definitions"][0]["triangles"].as_u64().unwrap() > 0);
    assert_eq!(revision(&d), before, "a quality is not a revision");
    let tree = json(&d, &["asm", "tree", "rig.dwasm", "--json"]);
    assert_eq!(tree[0]["quality"], "slider 10");
    assert!(tree[1]["quality"].is_null());

    ok(&d, &["asm", "quality", "rig.dwasm", "--every", "2"]);
    let tree = json(&d, &["asm", "tree", "rig.dwasm", "--json"]);
    assert_eq!(tree[0]["quality"], "slider 10", "the override wins");
    assert_eq!(tree[1]["quality"], "slider 2");
    let text = ok(&d, &["asm", "quality", "rig.dwasm"]);
    assert!(text.contains("every part  slider 2") && text.contains(&format!("[{first}]  slider 10")), "{text}");

    ok(&d, &["asm", "quality", "rig.dwasm", &first, "--none"]);
    ok(&d, &["asm", "quality", "rig.dwasm", "--every", "auto"]);
    let q = json(&d, &["asm", "quality", "rig.dwasm", "--json"]);
    assert!(q["overrides"].as_array().is_none_or(|o| o.is_empty()));
    assert_eq!(q["every"]["mode"], "auto");
    assert_eq!(revision(&d), before);

    let (ok_bad, _, err) = run(&d, &["asm", "quality", "rig.dwasm", &first, "--set", "11"]);
    assert!(!ok_bad && !err.is_empty());
    let (ok_bad, _, _) = run(&d, &["asm", "quality", "rig.dwasm", &first]);
    assert!(!ok_bad, "paths need --set or --none");
}
