//! `soliddraft asm` end to end, as separate processes: make an assembly of a saved part,
//! insert, pattern, move, hide, list the tree, report, then change the part and Rebuild.

use std::path::PathBuf;
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

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let dir = std::env::temp_dir().join(format!("soliddraft-asm-exit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    fn run(&self, args: &[&str]) -> (bool, String, String) {
        let out = Process::new(env!("CARGO_BIN_EXE_soliddraft"))
            .args(args)
            .current_dir(&self.0)
            .env("SOLIDDRAFT_CONFIG_DIR", self.0.join(".config"))
            .output()
            .expect("running soliddraft");
        (out.status.success(), String::from_utf8(out.stdout).unwrap(), String::from_utf8(out.stderr).unwrap())
    }

    fn ok(&self, args: &[&str]) -> String {
        let (ok, stdout, stderr) = self.run(args);
        assert!(ok, "soliddraft {args:?} failed: {stderr}");
        stdout
    }

    fn json(&self, args: &[&str]) -> Value {
        let stdout = self.ok(args);
        serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("{e}: {stdout}"))
    }
}

/// A block `w` x 20 x 10, centred in plan, saved at `path` (keeping its id).
fn save_block(path: &std::path::Path, w: f64) {
    let kernel: Arc<dyn Kernel> = Arc::new(SmlibKernel::new());
    let mut doc = if path.exists() { Document::open(path, kernel).unwrap() } else { Document::new("Block", kernel) };
    for id in doc.features().iter().map(|f| f.id).rev().collect::<Vec<_>>() {
        doc.execute(Command::DeleteFeature { id }).unwrap();
    }
    let mut sketch = Sketch::new();
    sketch.rectangle(P2::new(0.0, 0.0), w, 20.0).unwrap();
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
        end: EndCondition::Blind { depth: 10.0 },
        reverse: false,
    }));
    doc.save(path).unwrap();
}

#[test]
fn assemblies_are_built_edited_and_rebuilt_from_the_command_line() {
    let d = Dir::new("build");
    std::fs::create_dir_all(d.0.join("parts")).unwrap();
    save_block(&d.0.join("parts/block.dwprt"), 40.0);

    d.ok(&["asm", "new", "rig.dwasm", "--name", "Rig"]);
    d.ok(&["asm", "insert", "rig.dwasm", "parts/block.dwprt"]);
    d.ok(&["asm", "insert", "rig.dwasm", "parts/block.dwprt", "--at", "100,0,0", "--rotate", "0,0,1,90"]);
    let tree = d.json(&["asm", "tree", "rig.dwasm", "--json"]);
    let paths: Vec<&str> = tree.as_array().unwrap().iter().map(|r| r["path"].as_str().unwrap()).collect();
    assert_eq!(paths.len(), 2);
    let second = paths[1].to_owned();
    d.ok(&["asm", "pattern", "rig.dwasm", &second, "--linear", "0,1,0", "--spacing", "50", "--count", "4"]);
    let tree = d.json(&["asm", "tree", "rig.dwasm", "--json"]);
    assert_eq!(tree.as_array().unwrap().len(), 5, "two components and three generated members");
    let member = tree.as_array().unwrap()[4]["path"].as_str().unwrap().to_owned();
    assert!(member.contains(':'), "{member}");

    d.ok(&["asm", "hide", "rig.dwasm", &member]);
    d.ok(&["asm", "colour", "rig.dwasm", &second, "--rgb", "#c81414"]);
    let tree = d.json(&["asm", "tree", "rig.dwasm", "--json"]);
    assert_eq!(tree[1]["colour"], "#c81414");
    assert!(tree[0]["colour"].is_null(), "only the coloured component");
    d.ok(&["asm", "move", "rig.dwasm", paths[0], "--to", "0,0,25"]);
    let info = d.json(&["asm", "info", "rig.dwasm", "--json"]);
    assert_eq!(info["occurrences"]["parts"], 5);
    assert_eq!(info["occurrences"]["hidden"], 1);
    assert_eq!(info["states"]["display"], 1, "one definition, shown from the file's copy");
    assert!(info["newer"].as_array().unwrap().is_empty());
    let tree = d.json(&["asm", "tree", "rig.dwasm", "--json"]);
    assert_eq!(tree[0]["translation"], serde_json::json!([0.0, 0.0, 25.0]));

    // The part changes: info says so, Rebuild moves the pin and the file still opens alone.
    save_block(&d.0.join("parts/block.dwprt"), 60.0);
    let info = d.json(&["asm", "info", "rig.dwasm", "--json"]);
    assert_eq!(info["newer"].as_array().unwrap().len(), 1);
    let report = d.json(&["doc", "rebuild", "rig.dwasm", "--json"]);
    assert_eq!(report["sources"][0]["status"], "updated");
    let info = d.json(&["asm", "info", "rig.dwasm", "--json"]);
    assert!(info["newer"].as_array().unwrap().is_empty());
    let refs = d.json(&["doc", "refs", "rig.dwasm", "--json"]);
    assert!(refs.to_string().contains("current"), "{refs}");
}

#[test]
fn the_generated_scale_assembly_opens_and_its_parts_rebuild() {
    let d = Dir::new("generate");
    let out = d.ok(&["asm", "generate", "big", "--unique", "200", "--placements", "2000", "--modules", "4"]);
    assert!(out.contains("200 unique parts"), "{out}");
    let info = d.json(&["asm", "info", "big/root.dwasm", "--json"]);
    assert_eq!(info["occurrences"]["parts"], 2000);
    assert_eq!(info["occurrences"]["assemblies"], 5, "the root and four modules");
    assert_eq!(info["states"]["display"], 200, "every part shown from the root's own copies");
    assert!(info["newer"].as_array().unwrap().is_empty() && info["missing"].as_array().unwrap().is_empty());
    // A module opens on its own too, and a generated part rebuilds on the kernel.
    let module = d.json(&["asm", "info", "big/modules/m002.dwasm", "--json"]);
    assert_eq!(module["occurrences"]["parts"], 500);
    let part = d.json(&["part", "--json", "big/parts/p000123.dwprt"]);
    assert!(part["body"]["valid"].as_bool().unwrap(), "{part}");
}
