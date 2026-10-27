//! `soliddraft run` drives the registry's commands from a shell or a script, as an MCP
//! client drives them: a part built, saved, exported and drawn without a window, and
//! the refusals a script meets.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn soliddraft(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_soliddraft")).args(args).current_dir(dir).output().expect("run soliddraft")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soliddraft-run-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn results(out: &Output) -> Vec<Value> {
    String::from_utf8_lossy(&out.stdout).lines().map(|l| serde_json::from_str(l).expect("one JSON line")).collect()
}

#[test]
fn a_script_builds_saves_exports_and_draws_a_part() {
    let dir = scratch("script");
    let script = r#"
# A plate with a hole, saved, exported and drawn.
sketch {"plane": "top", "shapes": [{"shape": "rectangle", "center": [40, 30, 0], "width": 80, "height": 60}]}
extrude {"sketch": "Sketch1", "depth": 8, "flip": false}
sketch {"plane": "Boss-Extrude1 · End cap", "shapes": [{"shape": "circle", "center": [40, 30, 8], "diameter": 10}]}
cut {"sketch": "Sketch2", "through_all": true, "depth": 1, "flip": false}
save {"path": "plate.dwprt"}
export_step {"path": "plate.step"}
make_drawing {"template": "iso-a4"}
drawing_save {"path": "plate.dwdrw"}
drawing_export_pdf {"path": "plate.pdf"}
model_summary
"#;
    std::fs::write(dir.join("plate.txt"), script).unwrap();
    let out = soliddraft(&dir, &["run", "--script", "plate.txt"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let lines = results(&out);
    assert_eq!(lines.len(), 10, "{lines:#?}");
    assert_eq!(lines[1]["command"], "extrude");
    for file in ["plate.dwprt", "plate.step", "plate.dwdrw", "plate.pdf"] {
        assert!(dir.join(file).is_file(), "no {file}");
    }
    let summary = &lines[9]["result"];
    assert_eq!(summary["features"].as_array().unwrap().len(), 4, "{summary}");
    assert!(!summary["drawing"]["sheets"][0]["views"].as_array().unwrap().is_empty(), "{summary}");

    // The saved files open again, the drawing with its part.
    let out = soliddraft(&dir, &["run", "--open", "plate.dwdrw", "model_summary"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let summary = &results(&out)[0]["result"];
    assert_eq!(summary["drawing"]["part"], "current", "{summary}");
    assert_eq!(summary["features"].as_array().unwrap().len(), 4, "the drawing opened its part: {summary}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn one_command_runs_from_the_shell() {
    let dir = scratch("one");
    let out = soliddraft(&dir, &["run", "--demo", "feature", r#"{"name": "Boss-Extrude1"}"#]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(results(&out)[0]["result"]["name"], "Boss-Extrude1");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_script_is_refused_what_it_cannot_run() {
    let dir = scratch("refused");
    let refused = |args: &[&str], code: i32, says: &str| {
        let out = soliddraft(&dir, args);
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(code), "{args:?}: {err}");
        assert!(err.contains(says), "{args:?}: {err}");
    };
    refused(&["run", "view_front"], 2, "view_front runs only in the app: It moves the viewport camera");
    refused(&["run", "revolve"], 1, "does not match the `revolve` schema");
    refused(&["run", "no_such_command"], 2, "no command `no_such_command`");
    refused(&["run", "extrude", "{not json"], 2, "the arguments are not JSON");
    refused(&["run", "extrude", r#"{"sketch": "Sketch9", "depth": 1, "flip": false}"#], 1, "no feature named");
    refused(&["run", "extrude", r#"{"depth": 1}"#], 1, "does not match the `extrude` schema");
    refused(&["run"], 2, "name a command");

    // A failing line stops the script, unless it keeps going.
    std::fs::write(dir.join("two.txt"), "undo\nmodel_summary\n").unwrap();
    let out = soliddraft(&dir, &["run", "--script", "two.txt"]);
    assert_eq!((out.status.code(), results(&out).len()), (Some(1), 0));
    assert!(String::from_utf8_lossy(&out.stderr).contains("run: line 1: undo: "));
    let out = soliddraft(&dir, &["run", "--keep-going", "--script", "two.txt"]);
    assert_eq!((out.status.code(), results(&out).len()), (Some(1), 1));
    let _ = std::fs::remove_dir_all(&dir);
}
