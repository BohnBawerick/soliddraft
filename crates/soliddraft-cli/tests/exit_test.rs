//! The phase 0 exit test, run through the `soliddraft` binary as separate processes:
//!
//! Box, cylinder, boolean and fillet recipes repeat across processes, and an expected
//! failure leaves the preceding body unchanged.

use std::path::{Path, PathBuf};
use std::process::Command;

use soliddraft_recipe::{ReplayReport, StepOutcome};

const EXIT_RECIPES: &[&str] = &["box", "cylinder", "union", "subtract", "fillet", "failing-fillet"];
const RUNS: usize = 3;

fn recipes_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../recipes")
}

fn recipe(name: &str) -> PathBuf {
    recipes_dir().join(format!("{name}.json"))
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

#[test]
fn fillet_recipe_rounds_the_top_edges() {
    let report = &replay(&["fillet"])[0];
    // Four 2 mm rounds take a little under (1 - pi/4) * 4 mm2 along the 140 mm loop:
    // the mitred corners take less than the full edge lengths.
    let removed = 12000.0 - report.result.volume;
    let bound = (1.0 - std::f64::consts::FRAC_PI_4) * 4.0 * 140.0;
    assert!(removed > 0.9 * bound && removed < bound, "removed {removed}, bound {bound}");
    assert!(report.result.valid);
}
