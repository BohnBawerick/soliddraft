//! The assistant relay and static files end to end over real HTTP.

use std::sync::Arc;

use soliddraft_assistant::relay::{LineReader, RelayLine};
use soliddraft_assistant::wire::{ChatRequest, Effort};
use soliddraft_assistant::{MockProvider, MockTurn, Provider, StreamEvent};
use soliddraft_server::{Config, Server};

struct Http {
    base: String,
    client: reqwest::blocking::Client,
}

impl Http {
    fn new(base: &str) -> Http {
        let client = reqwest::blocking::Client::builder().no_proxy().build().unwrap();
        Http { base: base.to_owned(), client }
    }

    fn post(&self, path: &str, body: &str) -> reqwest::blocking::Response {
        self.client.post(format!("{}{path}", self.base)).body(body.to_owned()).send().unwrap()
    }
}

fn web_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("soliddraft-server-http-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.html"), "<canvas id=soliddraft></canvas>").unwrap();
    dir
}

fn start(name: &str, provider: Result<Arc<dyn Provider>, String>) -> String {
    let config =
        Config { web_dir: web_dir(name), provider, commit: Some("0123456789abcdef0123456789abcdef01234567".into()) };
    let addr = Server::bind("127.0.0.1:0", config).unwrap().spawn();
    format!("http://{addr}")
}

#[test]
fn removed_kernel_and_session_endpoints_are_not_served() {
    let base = start("removed-endpoints", Err("off".into()));
    let http = Http::new(&base);
    for path in ["/api/kernel", "/api/session/ping", "/api/session/close"] {
        assert_eq!(http.post(path, "{}").status().as_u16(), 404);
    }
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
    let info = Http::new(&base).client.get(format!("{base}/api/info")).send().unwrap().text().unwrap();
    let info: serde_json::Value = serde_json::from_str(&info).unwrap();
    assert_eq!(info["assistant"]["on"], true);

    let resp = Http::new(&base).post("/api/assistant", &chat());
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
    let resp = Http::new(&base).post("/api/assistant", &chat());
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
