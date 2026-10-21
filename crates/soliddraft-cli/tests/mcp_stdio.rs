//! An MCP client driving the headless command line over stdio, as Claude Code does
//! with `claude mcp add soliddraft -- soliddraft mcp`.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

struct Client {
    child: std::process::Child,
    out: BufReader<std::process::ChildStdout>,
    next: u64,
}

impl Client {
    fn start(args: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_soliddraft"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start soliddraft mcp");
        let out = BufReader::new(child.stdout.take().unwrap());
        Self { child, out, next: 0 }
    }

    fn send(&mut self, msg: &Value) {
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": self.next, "method": method, "params": params }));
        let mut line = String::new();
        self.out.read_line(&mut line).unwrap();
        // Every line on stdout must be protocol: anything else breaks the client.
        let reply: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON-RPC: {line:?}: {e}"));
        assert_eq!(reply["id"], self.next, "{reply}");
        reply
    }

    fn call(&mut self, name: &str, arguments: Value) -> Value {
        let r = self.request("tools/call", json!({ "name": name, "arguments": arguments }));
        r["result"].clone()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        let _ = self.child.wait();
    }
}

#[test]
fn mcp_over_stdio_builds_measures_and_undoes() {
    let mut c = Client::start(&["mcp"]);
    let init = c.request(
        "initialize",
        json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "stdio-test", "version": "1" } }),
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    c.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));

    let tools = c.request("tools/list", json!({}));
    let names: Vec<&str> =
        tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    let registry: Vec<String> = soliddraft_app::command_tools::tool_specs(soliddraft_app::command_tools::Surface::Mcp)
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, registry, "the MCP tool list is the command registry's");
    assert!(names.contains(&"extrude"));

    let sketch = c.call(
        "sketch",
        json!({ "plane": "top", "shapes": [{ "shape": "rectangle", "center": [0, 0, 0], "width": 40, "height": 30 }] }),
    );
    assert_eq!(sketch["isError"], false, "{sketch}");
    let made = c.call("extrude", json!({ "sketch": "Sketch1", "depth": 10, "flip": false }));
    assert_eq!(made["isError"], false, "{made}");

    let m = c.call("measure", json!({ "a": "Boss-Extrude1 · Start cap", "b": "Boss-Extrude1 · End cap" }));
    assert_eq!(m["structuredContent"]["gap_mm"], 10.0, "{m}");

    // A proposal that names a missing feature is rejected before anything else happens.
    let bad =
        c.call("propose", json!({ "summary": "x", "commands": [{ "op": "delete_feature", "feature": "Fillet1" }] }));
    assert_eq!(bad["isError"], true);

    for _ in 0..2 {
        let undone = c.call("undo", json!({}));
        assert_eq!(undone["isError"], false, "{undone}");
    }
    let summary = c.call("model_summary", json!({}));
    assert_eq!(summary["structuredContent"]["features"], json!([]));
}
