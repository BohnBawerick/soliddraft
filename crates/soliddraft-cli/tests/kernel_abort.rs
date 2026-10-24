//! Known kernel aborts, each in a process of its own so that the abort is seen and not swallowed.
//!
//! Replaying `recipes/bracket.json` with one of its plane origins moved a hair off a round
//! value (`steps[14].plane.origin[0]` of 25 mm set to 1e-20, or to 5e-324) overflows the
//! stack of the main thread: `soliddraft replay` dies of SIGABRT after about 20 s. Values
//! down to 1e-12 do not abort but take over a minute, where the recipe takes about a second.
//!
//! The recursion is inside Catallax, not in SolidDraft's input handling, which passes the number
//! through unchanged: `catallax_boolean::region::Sampling::sample` (`region.rs:123`) recurses once
//! per halving of a sampling interval, about 2,800 frames deep at the 4,160 bits of precision the
//! evidence evaluation then runs at, each level calling `catallax_intersect::chart::jets` and
//! `TracePiece::evaluate`. A bigger stack only moves the limit: precision, and so the depth, grow as
//! the offset shrinks. The fix is a depth bound in Catallax that ends in a typed refusal.
//!
//! These tests state what must hold once the pinned Catallax revision has that fix; they are
//! ignored until then. Run them with `cargo test -p soliddraft-cli --test kernel_abort -- --ignored`;
//! today they fail with the signal. `recipe_fuzz` and `command_fuzz` keep nonzero numbers smaller than
//! 0.01 out of their inputs for the same reason.

use std::path::Path;
use std::process::Command;

use serde_json::json;

/// Replays the bracket with `steps[step].plane.origin[0]` set to `value`, in a process of its own;
/// returns how it ended.
fn replay_with_origin(step: usize, value: serde_json::Value) -> std::process::ExitStatus {
    let text =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../recipes/bracket.json")).unwrap();
    let mut recipe: serde_json::Value = serde_json::from_str(&text).unwrap();
    recipe["steps"][step]["plane"]["origin"][0] = value;
    let dir = std::env::temp_dir().join(format!("soliddraft-kernel-abort-{}-{step}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("bracket.json"), recipe.to_string()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_soliddraft"))
        .args(["replay", "bracket.json"])
        .current_dir(&dir)
        .output()
        .expect("running soliddraft");
    let _ = std::fs::remove_dir_all(&dir);
    out.status
}

fn assert_ends_normally(status: std::process::ExitStatus) {
    // Success, or a refusal with exit status 1. A signal has no exit code.
    assert!(status.code().is_some_and(|c| c <= 1), "the replay did not end normally: {status}");
}

#[test]
#[ignore = "Catallax recurses without a stack-safe bound on near-zero offsets; see the module comment"]
fn a_plane_origin_a_hair_off_zero_is_built_or_refused_not_aborted() {
    assert_ends_normally(replay_with_origin(14, json!(1e-20)));
}

#[test]
#[ignore = "Catallax recurses without a stack-safe bound on near-zero offsets; see the module comment"]
fn the_smallest_subnormal_origin_is_built_or_refused_not_aborted() {
    assert_ends_normally(replay_with_origin(14, json!(5e-324)));
}
