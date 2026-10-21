//! `soliddraft part` opens saved parts in a fresh process, rebuilds them and reports.

use std::path::PathBuf;
use std::process::Command as Process;
use std::sync::Arc;

use soliddraft_kernel_api::Kernel;
use soliddraft_kernel_smlib::SmlibKernel;
use soliddraft_model::sketch::{P2, Sketch, SolveStatus, solve::solve};
use soliddraft_model::{
    BlendFeature, Command, DatumPlane, Document, EndCondition, ExtrudeFeature, ExtrudeOp, FeatureKind, ProfileSource,
    SketchFeature, SketchPlane,
};

/// A 40 x 30 x 10 block with its top edges rounded, saved to a temporary file.
fn save_block(name: &str) -> (PathBuf, Document) {
    let kernel: Arc<dyn Kernel> = Arc::new(SmlibKernel::new());
    let mut doc = Document::new("Block", kernel);
    let mut sketch = Sketch::new();
    sketch.rectangle(P2::new(0.0, 0.0), 40.0, 30.0).unwrap();
    solve(&mut sketch, None, true);
    assert_eq!(sketch.status, SolveStatus::FullyDefined);
    let plane = SketchPlane::Datum { datum: DatumPlane::Top, offset: 0.0 };
    let add = |doc: &mut Document, kind| {
        doc.execute(Command::AddFeature { kind, name: None, position: None, via_assistant: false })
            .unwrap()
            .created
            .unwrap()
    };
    let s = add(&mut doc, FeatureKind::Sketch(SketchFeature { plane, sketch }));
    let end = EndCondition::Blind { depth: 10.0 };
    add(
        &mut doc,
        FeatureKind::Extrude(ExtrudeFeature {
            profile: ProfileSource::Sketch { feature: s },
            operation: ExtrudeOp::Boss,
            end,
            reverse: false,
        }),
    );
    let t = doc.kernel().topology(doc.body().unwrap()).unwrap();
    let edges = t.edges.iter().filter(|e| e.bbox.min.z > 10.0 - 1e-6).map(|e| doc.edge_ref(e.id).unwrap()).collect();
    add(&mut doc, FeatureKind::Fillet(BlendFeature { edges, size: 2.0 }));
    let path = std::env::temp_dir().join(format!("soliddraft-cli-{}-{name}.dwprt", std::process::id()));
    doc.save(&path).unwrap();
    (path, doc)
}

fn run(args: &[&str]) -> (bool, String, String) {
    let out = Process::new(env!("CARGO_BIN_EXE_soliddraft")).args(args).output().unwrap();
    (out.status.success(), String::from_utf8(out.stdout).unwrap(), String::from_utf8(out.stderr).unwrap())
}

#[test]
fn part_rebuilds_a_saved_file_and_reports_every_feature() {
    let (path, doc) = save_block("ok");
    let want = doc.kernel().mass_properties(doc.body().unwrap()).unwrap().volume;
    let (ok, stdout, stderr) = run(&["part", "--json", path.to_str().unwrap()]);
    assert!(ok, "{stderr}");
    let report: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(report["part"], "Block");
    let names: Vec<&str> = report["features"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Sketch1", "Boss-Extrude1", "Fillet1"]);
    assert!(report["features"].as_array().unwrap().iter().all(|f| f["status"] == "ok"), "{report}");
    let volume = report["body"]["volume"].as_f64().unwrap();
    assert!((volume - want).abs() <= 1e-9 * want, "volume {volume}, saved part {want}");

    let (ok, text, _) = run(&["part", path.to_str().unwrap()]);
    assert!(ok);
    assert!(text.starts_with("Block ("), "{text}");
    assert!(text.contains("Fillet1") && text.contains("body: solid faces 10"), "{text}");
    std::fs::remove_file(path).ok();
}

#[test]
fn part_fails_when_a_feature_does_not_build() {
    let (path, mut doc) = save_block("broken");
    let sketch = doc.features()[0].id;
    doc.execute(Command::DeleteFeature { id: sketch }).unwrap();
    doc.save(&path).unwrap();
    let (ok, stdout, _) = run(&["part", "--json", path.to_str().unwrap()]);
    assert!(!ok, "a part with a failed feature exits with an error");
    let report: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(report["features"][0]["status"], "failed", "{report}");
    assert!(report["body"].is_null());
    let (ok, _, stderr) = run(&["part", "/nonexistent/part.dwpart"]);
    assert!(!ok && !stderr.is_empty());
    std::fs::remove_file(path).ok();
}
