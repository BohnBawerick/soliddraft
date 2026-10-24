//! `soliddraft quality` in a fresh process: report a part's levels, change its quality,
//! and check the part's revision does not move.

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::Arc;

use serde_json::Value;
use soliddraft_kernel_api::Kernel;
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::sketch::{P2, Sketch, SolveStatus, solve::solve};
use soliddraft_model::{
    Command, DatumPlane, Document, EndCondition, ExtrudeFeature, ExtrudeOp, FeatureKind, ProfileSource, SketchFeature,
    SketchPlane,
};

/// A pin, 20 mm across and 30 mm long, saved in a folder of its own.
fn save_pin(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soliddraft-cli-quality-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let kernel: Arc<dyn Kernel> = Arc::new(CatallaxKernel::new());
    let mut doc = Document::new("Pin", kernel);
    let mut sketch = Sketch::new();
    sketch.circle_at(P2::new(0.0, 0.0), 20.0).unwrap();
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
    add(
        &mut doc,
        FeatureKind::Extrude(ExtrudeFeature {
            profile: ProfileSource::Sketch { feature: s },
            operation: ExtrudeOp::Boss,
            end: EndCondition::Blind { depth: 30.0 },
            reverse: false,
        }),
    );
    let path = dir.join("pin.dwprt");
    doc.save(&path).unwrap();
    path
}

fn run(args: &[&str]) -> (bool, String, String) {
    let out = Process::new(env!("CARGO_BIN_EXE_soliddraft")).args(args).output().unwrap();
    (out.status.success(), String::from_utf8(out.stdout).unwrap(), String::from_utf8(out.stderr).unwrap())
}

fn json(args: &[&str]) -> Value {
    let (ok, out, err) = run(args);
    assert!(ok, "{args:?}: {err}");
    serde_json::from_str(&out).unwrap()
}

fn triangles(v: &Value, level: usize) -> u64 {
    v["report"]["levels"][level]["triangles"].as_u64().unwrap()
}

fn path_str(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn quality_reports_and_changes_without_a_new_revision() {
    let pin = save_pin("change");
    let p = path_str(&pin);
    let before = std::fs::read(&pin).unwrap();

    let auto = json(&["quality", p, "--json"]);
    assert_eq!(auto["settings"]["viewport"]["mode"], "auto");
    assert!(auto["saved"].is_null());
    let levels = auto["report"]["levels"].as_array().unwrap();
    assert_eq!(levels.iter().map(|l| l["level"].as_str().unwrap()).collect::<Vec<_>>(), ["coarse", "standard", "fine"]);
    let diagonal = auto["report"]["diagonal_mm"].as_f64().unwrap();
    let standard = levels[1]["deviation_mm"].as_f64().unwrap();
    assert!((standard - 0.002 * diagonal).abs() < 1e-12, "{standard} vs {diagonal}");
    assert!(triangles(&auto, 2) > triangles(&auto, 0));
    assert_eq!(std::fs::read(&pin).unwrap(), before, "reporting does not touch the file");

    let fine = json(&["quality", p, "--set", "10", "--export", "0.005,5", "--json"]);
    assert_eq!(fine["saved"], p);
    assert_eq!(fine["settings"]["viewport"], serde_json::json!({"mode": "slider", "value": 10}));
    assert_eq!(fine["settings"]["export"]["chord_height"], 0.005);
    assert!(triangles(&fine, 1) > triangles(&auto, 1));
    assert_eq!(fine["revision"], auto["revision"], "the part's revision stays");

    // Reopened, the setting holds, and `doc info` sees the same revision.
    let again = json(&["quality", p, "--json"]);
    assert_eq!(again["settings"], fine["settings"]);
    let (ok, info, err) = run(&["doc", "info", p]);
    assert!(ok, "{err}");
    let revision = auto["revision"].as_str().unwrap();
    let short = &revision.split_once(':').unwrap().1[..8];
    assert!(info.contains(&format!("revision {short}")), "{info}");

    let custom = json(&["quality", p, "--set", "chord=0.05,angle=10", "--display-data", "off", "--json"]);
    assert_eq!(custom["settings"]["viewport"]["mode"], "custom");
    assert_eq!(custom["report"]["levels"][1]["deviation_mm"], 0.05);
    assert_eq!(custom["settings"]["save_display_data"], false);

    // Back to auto and the defaults: the file is the one first saved.
    json(&["quality", p, "--set", "auto", "--export", "0.01,10", "--display-data", "on", "--json"]);
    assert_eq!(std::fs::read(&pin).unwrap(), before);

    let (ok, text, _) = run(&["quality", p]);
    assert!(ok);
    assert!(text.contains("viewport  auto") && text.contains("standard  deviation"), "{text}");
}

#[test]
fn bad_values_are_refused() {
    let pin = save_pin("bad");
    let p = path_str(&pin);
    let before = std::fs::read(&pin).unwrap();
    for args in [["--set", "0"], ["--set", "chord=-1"], ["--export", "0,10"], ["--display-data", "maybe"]] {
        let (ok, _, err) = run(&["quality", p, args[0], args[1]]);
        assert!(!ok, "{args:?} was accepted");
        assert!(!err.is_empty());
    }
    assert_eq!(std::fs::read(&pin).unwrap(), before);
    let (ok, _, _) = run(&["quality"]);
    assert!(!ok);
}
