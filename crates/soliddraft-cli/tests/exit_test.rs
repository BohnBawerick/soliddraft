//! The phase 0 exit test, run through the `soliddraft` binary as separate processes:
//!
//! - box, cylinder, union, subtract and a circular fillet replay from recipes with
//!   identical digests across repeated runs;
//! - a deliberately failing operation returns an error and leaves the previous body;
//! - every recipe in the corpus matches the checked-in digests in
//!   `recipes/golden/smlib.json`. Set `SOLIDDRAFT_BLESS=1` to rewrite that file after a
//!   deliberate change.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use soliddraft_recipe::{BodySummary, ReplayReport, StepOutcome};

const EXIT_RECIPES: &[&str] = &["box", "cylinder", "union", "subtract", "fillet", "failing-fillet"];
const RUNS: usize = 3;

fn recipes_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../recipes")
}

fn recipe(name: &str) -> PathBuf {
    recipes_dir().join(format!("{name}.json"))
}

fn corpus() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(recipes_dir())
        .unwrap()
        .filter_map(|e| {
            let p = e.unwrap().path();
            (p.extension()? == "json").then(|| p.file_stem().unwrap().to_string_lossy().into_owned())
        })
        .collect();
    names.sort();
    names
}

/// Runs `soliddraft replay --json --steps` on the files in a fresh process.
fn replay(names: &[&str]) -> Vec<ReplayReport> {
    let out = Command::new(env!("CARGO_BIN_EXE_soliddraft"))
        .arg("replay")
        .arg("--json")
        .args(names.iter().map(|n| recipe(n)))
        .output()
        .expect("running soliddraft");
    assert!(out.status.success(), "soliddraft failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().lines().map(|l| serde_json::from_str(l).expect("report JSON")).collect()
}

#[test]
fn exit_recipes_replay_with_identical_digests_across_runs() {
    let first = replay(EXIT_RECIPES);
    assert_eq!(first.len(), EXIT_RECIPES.len());
    for r in &first {
        assert!(r.result.valid, "{} result is not valid", r.recipe);
        assert_eq!(r.result.kind, soliddraft_kernel_api::BodyKind::Solid, "{}", r.recipe);
    }
    for run in 1..RUNS {
        let again = replay(EXIT_RECIPES);
        for (a, b) in first.iter().zip(&again) {
            // Whole reports, every step's counts, volume, area and digest.
            assert_eq!(a, b, "run {run} of {} differs from run 0", a.recipe);
        }
    }
}

#[test]
fn failing_operation_returns_an_error_and_leaves_the_previous_body() {
    let reports = replay(&["box", "failing-fillet"]);
    let (block, failing) = (&reports[0], &reports[1]);
    let step = failing.steps.iter().find(|s| s.id == "too_big").expect("too_big step");
    assert!(matches!(step.outcome, StepOutcome::ExpectedError { .. }), "{:?}", step.outcome);
    // The recipe's result is the block the failed fillet was applied to; it must equal a
    // block that never saw the failure.
    assert_eq!(failing.result, block.result);
}

#[test]
fn a_step_that_should_succeed_but_fails_fails_the_replay() {
    let dir = std::env::temp_dir().join(format!("soliddraft-exit-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let text = std::fs::read_to_string(recipe("failing-fillet")).unwrap().replace(r#", "expect": "error""#, "");
    let path = dir.join("unexpected.json");
    std::fs::write(&path, text).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_soliddraft")).arg("replay").arg(&path).output().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("step too_big failed"), "{stderr}");
}

#[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
struct Golden {
    backend: String,
    results: BTreeMap<String, BodySummary>,
}

#[test]
fn corpus_matches_golden_digests() {
    let names = corpus();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let reports = replay(&refs);
    let got = Golden {
        backend: format!("{} {}", reports[0].backend.name, reports[0].backend.version),
        results: reports.into_iter().map(|r| (r.recipe, r.result)).collect(),
    };
    let path = recipes_dir().join("golden/smlib.json");
    if std::env::var_os("SOLIDDRAFT_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&got).unwrap() + "\n").unwrap();
        return;
    }
    let text =
        std::fs::read_to_string(&path).expect("recipes/golden/smlib.json; run with SOLIDDRAFT_BLESS=1 to create it");
    let want: Golden = serde_json::from_str(&text).unwrap();
    assert_eq!(got.backend, want.backend, "backend changed; re-bless deliberately");
    assert_eq!(got.results.keys().collect::<Vec<_>>(), want.results.keys().collect::<Vec<_>>(), "corpus changed");
    for (name, w) in &want.results {
        let g = &got.results[name];
        assert_eq!(
            (g.kind, g.faces, g.edges, g.vertices, g.valid),
            (w.kind, w.faces, w.edges, w.vertices, w.valid),
            "{name}"
        );
        assert_eq!(g.digest, w.digest, "{name} digest changed");
    }
}
