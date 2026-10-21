//! `soliddraft commands` and `soliddraft mcp --help` print the command registry the app
//! and the MCP server read, so their names and docs match the app's.

use serde_json::Value;
use soliddraft_app::command_tools::{Surface, tool_specs};
use soliddraft_app::commands;
use std::process::Command;

fn run(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_soliddraft")).args(args).output().expect("run soliddraft");
    (out.status.success(), String::from_utf8(out.stdout).unwrap())
}

#[test]
fn commands_json_lists_the_whole_registry() {
    let (ok, text) = run(&["commands", "--json"]);
    assert!(ok);
    let listed: Value = serde_json::from_str(&text).unwrap();
    let ids: Vec<&str> = listed.as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    let registry: Vec<&str> = commands::all().iter().map(|c| c.id).collect();
    assert_eq!(ids, registry);
    let extrude = listed.as_array().unwrap().iter().find(|c| c["id"] == "extrude").unwrap();
    assert_eq!(extrude["label"], "Extrude");
    assert_eq!(extrude["shortcuts"][0]["keys"], "E");
    assert_eq!(extrude["agents"], "verb");
    assert_eq!(extrude["params"][1]["name"], "depth");
}

#[test]
fn a_query_searches_as_the_app_does() {
    let (ok, text) = run(&["commands", "fil"]);
    assert!(ok);
    assert!(text.lines().next().unwrap().starts_with("fillet "), "{text}");
    let (ok, _) = run(&["commands", "no-such-command-anywhere"]);
    assert!(!ok);
}

#[test]
fn mcp_help_lists_the_registry_tools() {
    let (ok, text) = run(&["mcp", "--help"]);
    assert!(ok);
    for tool in tool_specs(Surface::Mcp) {
        let c = commands::get(&tool.name).expect("every MCP tool is a registry command");
        assert!(text.contains(&format!("  {}(", tool.name)), "{} missing from:\n{text}", tool.name);
        assert!(text.contains(c.doc), "{}'s doc missing", tool.name);
    }
}
