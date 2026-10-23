//! The server end to end over real HTTP: kernel calls through the protocol, sessions, the
//! assistant relay and the static files.

use std::sync::Arc;
use std::time::Duration;

use soliddraft_assistant::relay::{LineReader, RelayLine};
use soliddraft_assistant::wire::{ChatRequest, Effort};
use soliddraft_assistant::{MockProvider, MockTurn, Provider, StreamEvent};
use soliddraft_kernel_api::{Kernel, KernelError, contract};
use soliddraft_kernel_remote::{RemoteKernel, Transport};
use soliddraft_kernel_smlib::SmlibKernel;
use soliddraft_server::{Config, SESSION_HEADER, Server};

struct Http {
    base: String,
    session: String,
    client: reqwest::blocking::Client,
}

impl Http {
    fn new(base: &str, session: &str) -> Http {
        let client = reqwest::blocking::Client::builder().no_proxy().build().unwrap();
        Http { base: base.to_owned(), session: session.to_owned(), client }
    }

    fn post(&self, path: &str, body: &str) -> reqwest::blocking::Response {
        self.client
            .post(format!("{}{path}", self.base))
            .header(SESSION_HEADER, &self.session)
            .body(body.to_owned())
            .send()
            .unwrap()
    }
}

impl Transport for Http {
    fn call(&self, body: &str) -> Result<String, String> {
        let resp = self.post("/api/kernel", body);
        if resp.status().as_u16() != 200 {
            return Err(format!("status {}", resp.status()));
        }
        resp.text().map_err(|e| e.to_string())
    }
}

/// A web directory of the test's own. Tests run in parallel, and `fs::write` empties a file
/// before it fills it, so a shared page could be served empty while another test rewrites it.
fn web_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("soliddraft-server-http-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.html"), "<canvas id=soliddraft></canvas>").unwrap();
    dir
}

fn start(name: &str, provider: Result<Arc<dyn Provider>, String>) -> String {
    let config = Config {
        web_dir: web_dir(name),
        kernel: Arc::new(SmlibKernel::new()),
        provider,
        session_idle: Duration::from_secs(600),
        commit: Some("0123456789abcdef0123456789abcdef01234567".into()),
    };
    let addr = Server::bind("127.0.0.1:0", config).unwrap().spawn();
    format!("http://{addr}")
}

fn remote(base: &str, session: &str) -> RemoteKernel {
    RemoteKernel::new(Box::new(Http::new(base, session)))
}

#[test]
fn the_kernel_contract_holds_over_http() {
    let base = start("contract", Err("off".into()));
    let k = remote(&base, "contract-session-1");
    contract::operations_build_what_they_say(&k);
    contract::failed_operations_return_errors(&k);
    contract::tessellation_is_labelled(&k);
    contract::view_curves_are_tagged(&k);
}

#[test]
fn closing_a_session_releases_its_bodies() {
    let base = start("close", Err("off".into()));
    let mine = remote(&base, "session-mine-1");
    let theirs = remote(&base, "session-theirs-1");
    let block = contract::block(&mine);
    assert!(mine.topology(block).is_ok());
    assert_eq!(theirs.topology(block), Err(KernelError::UnknownBody(block)));

    // A beacon names the session in the query, since it cannot set headers.
    let http = Http::new(&base, "unused-session");
    let resp = http.client.post(format!("{base}/api/session/close?session=session-mine-1")).send().unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(mine.topology(block), Err(KernelError::UnknownBody(block)));
}

#[test]
fn a_request_without_a_session_is_refused() {
    let base = start("no-session", Err("off".into()));
    let client = reqwest::blocking::Client::builder().no_proxy().build().unwrap();
    let resp = client.post(format!("{base}/api/kernel")).body(r#"{"op":"backend"}"#).send().unwrap();
    assert_eq!(resp.status().as_u16(), 400);
}

fn chat() -> String {
    serde_json::to_string(&ChatRequest {
        system: "system".into(),
        tools: Vec::new(),
        messages: Vec::new(),
        effort: Effort::Medium,
        max_tokens: 1000,
    })
    .unwrap()
}

#[test]
fn the_assistant_streams_through_the_relay() {
    let provider: Arc<dyn Provider> =
        Arc::new(MockProvider::new(vec![MockTurn::text("A 6 mm hole through the flange.")]));
    let base = start("relay", Ok(provider));
    let info = Http::new(&base, "info-session").client.get(format!("{base}/api/info")).send().unwrap().text().unwrap();
    let info: serde_json::Value = serde_json::from_str(&info).unwrap();
    assert_eq!(info["assistant"]["on"], true);
    assert_eq!(info["kernel"]["name"], "smlib");

    let resp = Http::new(&base, "chat-session-1").post("/api/assistant", &chat());
    assert_eq!(resp.status().as_u16(), 200);
    let mut reader = LineReader::default();
    let mut text = String::new();
    let mut done = false;
    for line in reader.push(&resp.bytes().unwrap()) {
        match line.unwrap() {
            RelayLine::Event(StreamEvent::TextDelta(t)) => text.push_str(&t),
            RelayLine::Event(_) => {}
            RelayLine::Done(_) => done = true,
            RelayLine::Error(e) => panic!("{e}"),
        }
    }
    assert!(done);
    assert_eq!(text, "A 6 mm hole through the flange.");
}

#[test]
fn without_a_model_the_assistant_says_why() {
    let base = start("no-model", Err("no key here".into()));
    let resp = Http::new(&base, "chat-session-2").post("/api/assistant", &chat());
    assert_eq!(resp.status().as_u16(), 503);
    assert_eq!(resp.text().unwrap(), "no key here");
}

#[test]
fn the_page_is_served_and_paths_stay_inside() {
    let base = start("page", Err("off".into()));
    let client = reqwest::blocking::Client::builder().no_proxy().build().unwrap();
    let page = client.get(format!("{base}/")).send().unwrap();
    assert_eq!(page.status().as_u16(), 200);
    assert!(page.text().unwrap().contains("soliddraft"));
    let escape = client.get(format!("{base}/..%2f..%2fetc/passwd")).send().unwrap();
    assert_eq!(escape.status().as_u16(), 404);
}

#[test]
fn version_names_the_commit() {
    let base = start("version", Err("off".into()));
    let client = reqwest::blocking::Client::builder().no_proxy().build().unwrap();
    let resp = client.get(format!("{base}/version")).send().unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let version: serde_json::Value = serde_json::from_str(&resp.text().unwrap()).unwrap();
    assert_eq!(version["commit"], "0123456789abcdef0123456789abcdef01234567");
    assert_eq!(version["version"], env!("CARGO_PKG_VERSION"));
}
