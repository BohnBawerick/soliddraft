//! The phase D0 exit test, through the `soliddraft view` binary as separate processes:
//!
//! - the bracket's front, top, right and isometric views match the golden visible and
//!   hidden counts and digests in `recipes/golden/smlib-views.json`, identically across
//!   three runs, each view under 500 ms (the best of the three runs, so a busy machine
//!   does not fail it). Set `SOLIDDRAFT_BLESS=1` to rewrite the golden file after a
//!   deliberate change;
//! - `--svg` writes the view with visible and hidden line work;
//! - the mesh method agrees with the kernel on every edge's visible length to within 2
//!   percent of the edge's length, on the box, cylinder and bracket recipes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use soliddraft_kernel_api::Kernel;
use soliddraft_kernel_api::contract;
use soliddraft_kernel_smlib::SmlibKernel;

const VIEWS: &str = "front,top,right,iso";
const RUNS: usize = 3;
const MAX_MILLIS: f64 = 500.0;

fn recipes_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../recipes")
}

#[derive(serde::Deserialize)]
struct Summary {
    backend: String,
    view: String,
    visible: usize,
    hidden: usize,
    digest: String,
    millis: f64,
}

#[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug, Clone)]
struct Golden {
    visible: usize,
    hidden: usize,
    digest: String,
}

#[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
struct GoldenFile {
    backend: String,
    views: BTreeMap<String, BTreeMap<String, Golden>>,
}

fn soliddraft(args: &[&str]) -> std::process::Output {
    let out = Command::new(env!("CARGO_BIN_EXE_soliddraft")).args(args).output().expect("running soliddraft");
    assert!(out.status.success(), "soliddraft {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    out
}

fn views(recipe: &str) -> Vec<Summary> {
    let path = recipes_dir().join(format!("{recipe}.json"));
    let out = soliddraft(&["view", path.to_str().unwrap(), "--dir", VIEWS, "--json"]);
    String::from_utf8(out.stdout).unwrap().lines().map(|l| serde_json::from_str(l).expect("summary JSON")).collect()
}

#[test]
fn bracket_views_match_golden_across_runs_and_are_fast() {
    let runs: Vec<Vec<Summary>> = (0..RUNS).map(|_| views("bracket")).collect();
    let got: BTreeMap<String, Golden> = runs[0]
        .iter()
        .map(|s| (s.view.clone(), Golden { visible: s.visible, hidden: s.hidden, digest: s.digest.clone() }))
        .collect();
    assert_eq!(got.len(), 4);
    for (i, run) in runs.iter().enumerate().skip(1) {
        for s in run {
            assert_eq!(
                got[&s.view],
                Golden { visible: s.visible, hidden: s.hidden, digest: s.digest.clone() },
                "run {i}"
            );
        }
    }
    for view in got.keys() {
        let best = runs.iter().flatten().filter(|s| &s.view == view).map(|s| s.millis).fold(f64::INFINITY, f64::min);
        assert!(best < MAX_MILLIS, "the {view} view took {best:.0} ms at best");
    }

    let backend = runs[0][0].backend.clone();
    let path = recipes_dir().join("golden/smlib-views.json");
    if std::env::var_os("SOLIDDRAFT_BLESS").is_some() {
        let file = GoldenFile { backend, views: BTreeMap::from([("bracket".to_string(), got)]) };
        std::fs::write(&path, serde_json::to_string_pretty(&file).unwrap() + "\n").unwrap();
        return;
    }
    let text = std::fs::read_to_string(&path)
        .expect("recipes/golden/smlib-views.json; run with SOLIDDRAFT_BLESS=1 to create it");
    let want: GoldenFile = serde_json::from_str(&text).unwrap();
    assert_eq!(backend, want.backend, "backend changed; re-bless deliberately");
    assert_eq!(got, want.views["bracket"], "bracket views changed");
}

#[test]
fn view_writes_svg() {
    let dir = std::env::temp_dir().join(format!("soliddraft-view-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let svg = dir.join("bracket-{view}.svg");
    let recipe = recipes_dir().join("bracket.json");
    soliddraft(&["view", recipe.to_str().unwrap(), "--dir", "front,iso", "--svg", svg.to_str().unwrap()]);
    for view in ["front", "iso"] {
        let text = std::fs::read_to_string(dir.join(format!("bracket-{view}.svg"))).unwrap();
        assert!(text.starts_with("<svg") && text.trim_end().ends_with("</svg>"), "{view}");
        assert!(text.contains("stroke-dasharray") && text.matches("<path").count() > 20, "{view}: {text}");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn mesh_method_agrees_with_the_kernel_on_visible_lengths() {
    let k = SmlibKernel::new();
    for name in ["box", "cylinder", "bracket"] {
        let text = std::fs::read_to_string(recipes_dir().join(format!("{name}.json"))).unwrap();
        let recipe = soliddraft_recipe::parse(&text).unwrap();
        let (_, body) = soliddraft_recipe::build(&k, &recipe).unwrap();
        let diffs = contract::visible_length_differences(&k, body, &contract::comparison_views());
        let (worst, what) = &diffs[0];
        assert!(*worst <= 0.02, "{name}: {what} ({:.2} percent)", worst * 100.0);
        k.release(body).unwrap();
    }
}
