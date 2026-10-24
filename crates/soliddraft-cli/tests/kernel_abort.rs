//! Known kernel aborts and slowdowns, each in a process of its own so that an abort is seen and
//! not swallowed.
//!
//! Replaying `recipes/bracket.json` with one of its plane origins moved a hair off a round
//! value (`steps[14].plane.origin[0]` of 25 mm set to 1e-20, or to 5e-324) overflows the
//! stack of the main thread: `soliddraft replay` dies of SIGABRT after about 20 s. Values
//! down to 1e-12 do not abort but take over a minute (1e-3, 1e-6, 1e-9 and 1e-12 took 64 to 68 s),
//! where the recipe takes about a second.
//!
//! The recursion is inside Catallax, not in SolidDraft's input handling, which passes the number
//! through unchanged: `catallax_boolean::region::Sampling::sample` (`region.rs:123`) recurses once
//! per halving of a sampling interval, about 2,800 frames deep at the 4,160 bits of precision the
//! evidence evaluation then runs at, each level calling `catallax_intersect::chart::jets` and
//! `TracePiece::evaluate`. A bigger stack only moves the limit: precision, and so the depth, grow as
//! the offset shrinks.
//!
//! These tests state what must hold once the pinned Catallax revision has the fix the Catallax
//! lane is making (a depth bound in `Sampling::sample` that ends in a typed refusal, and bounded
//! face meshing); they are ignored until the pin is bumped to it, and the `#[ignore]` lines come
//! out then. Run them with `cargo test -p soliddraft-cli --test kernel_abort -- --ignored`; today
//! they fail. `recipe_fuzz` and `command_fuzz` keep nonzero numbers smaller than 0.01 out of their
//! inputs for the same reason, so a green fuzz run does not cover them; those exclusions come out
//! with the `#[ignore]` lines.

use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use serde_json::json;

/// How long a replay of the bracket may take: it takes about a second when it is healthy.
const SLOW: Duration = Duration::from_secs(30);

/// Replays the bracket with `steps[step].plane.origin[0]` set to `value`, in a process of its own.
/// Returns how it ended, or `None` if it was still running after `limit` and was killed.
fn replay_with_origin(step: usize, value: serde_json::Value, limit: Duration) -> Option<ExitStatus> {
    let text =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../recipes/bracket.json")).unwrap();
    let mut recipe: serde_json::Value = serde_json::from_str(&text).unwrap();
    recipe["steps"][step]["plane"]["origin"][0] = value;
    let dir = std::env::temp_dir().join(format!("soliddraft-kernel-abort-{}-{step}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("bracket.json"), recipe.to_string()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_soliddraft"))
        .args(["replay", "bracket.json"])
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("running soliddraft");
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let _ = std::fs::remove_dir_all(&dir);
    status
}

fn assert_ends_normally(status: Option<ExitStatus>) {
    // Success, or a refusal with exit status 1. A signal has no exit code.
    let status = status.expect("the replay was still running after the limit");
    assert!(status.code().is_some_and(|c| c <= 1), "the replay did not end normally: {status}");
}

#[test]
#[ignore = "waits for the Catallax fix: a depth bound in catallax_boolean::region::Sampling::sample (stack overflow on near-zero offsets)"]
fn a_plane_origin_a_hair_off_zero_is_built_or_refused_not_aborted() {
    assert_ends_normally(replay_with_origin(14, json!(1e-20), Duration::from_secs(120)));
}

#[test]
#[ignore = "waits for the Catallax fix: a depth bound in catallax_boolean::region::Sampling::sample (stack overflow on near-zero offsets)"]
fn the_smallest_subnormal_origin_is_built_or_refused_not_aborted() {
    assert_ends_normally(replay_with_origin(14, json!(5e-324), Duration::from_secs(120)));
}

#[test]
#[ignore = "waits for the Catallax fix: a depth bound in catallax_boolean::region::Sampling::sample (about 60 s at offsets of 1e-3 to 1e-12)"]
fn a_small_but_ordinary_origin_replays_in_reasonable_time() {
    for value in [1e-3, 1e-6, 1e-9, 1e-12] {
        assert_ends_normally(replay_with_origin(14, json!(value), SLOW));
    }
}
