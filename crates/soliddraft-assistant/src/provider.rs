//! Model providers. The app talks to [`Provider`] so it does not depend on one vendor.
//!
//! [`AnthropicProvider`] calls `POST /v1/messages` with reqwest and reads the
//! server-sent events itself, since Rust has no official Anthropic SDK.
//! [`OpenRouterProvider`] sends the same request to OpenRouter's Anthropic-compatible
//! endpoint; [`ProviderKind`] names the two.
//! [`MockProvider`] replays recorded or scripted streams through the same reader, so
//! tests exercise the real parsing path without a network or a key.

use crate::sse::SseReader;
use crate::wire::{Accumulator, ChatRequest};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::BufRead;
use std::sync::{Arc, Mutex};

// The HTTP providers are for native builds; the web build reaches its model through the
// relay (`crate::relay`).
#[cfg(not(target_arch = "wasm32"))]
use {
    crate::key::ApiKey,
    crate::wire::{FALLBACK_BETA, request_body},
    std::io::{BufReader, Read, Write},
    std::path::PathBuf,
    std::time::Duration,
};

pub use crate::wire::{ProviderResponse, StreamEvent};

/// Why a request failed.
#[derive(Debug, thiserror::Error, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderError {
    /// The API answered with an error status.
    #[error("{kind} ({status}): {message}")]
    Api {
        /// The HTTP status, or 200 for an error event inside a stream.
        status: u16,
        /// The error type, such as `rate_limit_error`.
        kind: String,
        /// The message.
        message: String,
    },
    /// The request did not reach the API, or the connection broke.
    #[error("network error: {0}")]
    Network(String),
    /// The stream did not follow the protocol.
    #[error("unexpected response: {0}")]
    Protocol(String),
    /// The caller stopped the request.
    #[error("stopped")]
    Cancelled,
}

impl ProviderError {
    /// Whether trying again later may succeed.
    pub fn retryable(&self) -> bool {
        match self {
            ProviderError::Api { status, kind, .. } => {
                matches!(status, 408 | 409 | 429 | 500..=599) || kind == "overloaded_error" || kind == "api_error"
            }
            ProviderError::Network(_) => true,
            _ => false,
        }
    }
}

/// Where a request started with [`Provider::start`] reports.
pub struct Delivery {
    /// Sees each event as it arrives and returns `false` to stop the request.
    pub on_event: Box<dyn FnMut(StreamEvent) -> bool + Send>,
    /// Gets the result, once, at the end.
    pub on_done: Box<dyn FnOnce(Result<ProviderResponse, ProviderError>) + Send>,
}

/// A model provider.
pub trait Provider: Send + Sync + 'static {
    /// A label for the interface, such as `Anthropic`.
    fn name(&self) -> String;

    /// The model id requests use.
    fn model(&self) -> String;

    /// Sends one request and streams the reply. `on_event` sees each event as it arrives
    /// and returns `false` to stop the request.
    fn send(
        &self,
        request: &ChatRequest,
        on_event: &mut dyn FnMut(StreamEvent) -> bool,
    ) -> Result<ProviderResponse, ProviderError>;

    /// Starts one request and returns at once; what happens arrives through `delivery`.
    ///
    /// The default runs [`Provider::send`] on a new thread. A WebAssembly build has no
    /// threads, so there the default runs it in place, and a provider that has to wait
    /// on the network overrides this to stream without blocking.
    fn start(self: Arc<Self>, request: ChatRequest, delivery: Delivery) {
        let Delivery { mut on_event, on_done } = delivery;
        let run = move || on_done(self.send(&request, &mut *on_event));
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::Builder::new()
            .name("assistant-request".into())
            .spawn(run)
            .expect("spawn the assistant request thread");
        #[cfg(target_arch = "wasm32")]
        run();
    }
}

/// Reads one Messages API event stream to the end.
pub fn read_stream(
    reader: impl BufRead,
    on_event: &mut dyn FnMut(StreamEvent) -> bool,
) -> Result<ProviderResponse, (ProviderError, bool)> {
    let mut sse = SseReader::new(reader);
    let mut acc = Accumulator::default();
    loop {
        let event = match sse.next_event() {
            Ok(Some(e)) => e,
            Ok(None) => break,
            Err(e) => return Err((ProviderError::Network(e.to_string()), acc.has_content())),
        };
        match acc.feed(&event.event, &event.data) {
            Ok(events) => {
                for e in events {
                    if !on_event(e) {
                        return Err((ProviderError::Cancelled, acc.has_content()));
                    }
                }
            }
            Err(e) => {
                let started = acc.has_content();
                return Err((ProviderError::Api { status: 200, kind: e.kind, message: e.message }, started));
            }
        }
        if acc.done() {
            break;
        }
    }
    if !acc.done() {
        return Err((ProviderError::Network("the stream ended before message_stop".into()), acc.has_content()));
    }
    Ok(acc.finish())
}

/// The services the assistant can talk to. Both speak the Messages API.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    /// The Anthropic API itself.
    Anthropic,
    /// OpenRouter's Anthropic-compatible Messages endpoint.
    OpenRouter,
}

impl ProviderKind {
    /// Every provider, in the order the interface lists them.
    pub const ALL: [ProviderKind; 2] = [ProviderKind::OpenRouter, ProviderKind::Anthropic];

    /// The id used in settings, on the command line and in `SOLIDDRAFT_PROVIDER`.
    pub fn id(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::OpenRouter => "openrouter",
        }
    }

    /// The name for the interface.
    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "Anthropic",
            ProviderKind::OpenRouter => "OpenRouter",
        }
    }

    /// Reads an id, ignoring case.
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.id().eq_ignore_ascii_case(text.trim()))
    }

    /// The environment variable that holds this provider's key.
    pub fn key_var(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
            ProviderKind::OpenRouter => "OPENROUTER_API_KEY",
        }
    }

    /// The model used when no setting names one. Model ids differ between providers:
    /// OpenRouter prefixes the vendor and writes the version with a dot.
    pub fn default_model(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => crate::DEFAULT_MODEL,
            ProviderKind::OpenRouter => crate::OPENROUTER_DEFAULT_MODEL,
        }
    }

    /// A provider of this kind for `model`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn connect(self, key: ApiKey, model: &str) -> Result<std::sync::Arc<dyn Provider>, ProviderError> {
        Ok(match self {
            ProviderKind::Anthropic => std::sync::Arc::new(AnthropicProvider::new(key, model)?),
            ProviderKind::OpenRouter => std::sync::Arc::new(OpenRouterProvider::new(key, model)?),
        })
    }
}

impl std::fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// The endpoint from `var`, or `default` when it is unset or empty.
#[cfg(not(target_arch = "wasm32"))]
fn base_url(var: &str, default: &str) -> String {
    std::env::var(var).ok().filter(|u| !u.is_empty()).unwrap_or_else(|| default.into())
}

/// A streaming Messages API client: the request, the retries and the stream reading that
/// [`AnthropicProvider`] and [`OpenRouterProvider`] share.
#[cfg(not(target_arch = "wasm32"))]
struct MessagesClient {
    kind: ProviderKind,
    key: ApiKey,
    model: String,
    base_url: String,
    fallbacks: bool,
    client: reqwest::blocking::Client,
    record_dir: Option<PathBuf>,
    retries: u32,
}

#[cfg(not(target_arch = "wasm32"))]
impl MessagesClient {
    fn new(kind: ProviderKind, key: ApiKey, model: &str, base_url: String) -> Result<Self, ProviderError> {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            // Long turns stream for minutes; the stream itself is the progress signal.
            .timeout(Duration::from_secs(20 * 60))
            .build()
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        Ok(Self {
            kind,
            key,
            model: model.to_owned(),
            // Server-side fallbacks exist on the Anthropic API itself, not behind proxies.
            fallbacks: base_url == "https://api.anthropic.com",
            base_url,
            client,
            record_dir: crate::environment::var_os("SOLIDDRAFT_RECORD_DIR").map(PathBuf::from),
            retries: 2,
        })
    }

    fn body(&self, request: &ChatRequest) -> Value {
        let mut body = request_body(&self.model, request, self.fallbacks);
        if self.kind == ProviderKind::OpenRouter {
            // OpenRouter asks for summarized thinking unless told otherwise; the Anthropic API's
            // default for these models is omitted. Ask for omitted so both send the same stream.
            body["thinking"] = json!({ "type": "adaptive", "display": "omitted" });
            // Prefer Anthropic's own endpoint, as OpenRouter advises for Claude clients, and let
            // OpenRouter fall back to another host of the same model when it is down.
            body["provider"] = json!({ "order": ["anthropic"] });
        }
        body
    }

    fn send_once(
        &self,
        request: &ChatRequest,
        on_event: &mut dyn FnMut(StreamEvent) -> bool,
    ) -> Result<ProviderResponse, (ProviderError, bool)> {
        let body = self.body(request);
        let secret = match self.kind {
            ProviderKind::Anthropic => self.key.expose().to_owned(),
            ProviderKind::OpenRouter => format!("Bearer {}", self.key.expose()),
        };
        let mut key = reqwest::header::HeaderValue::from_str(&secret)
            .map_err(|_| (ProviderError::Protocol("the API key holds characters a header cannot".into()), false))?;
        key.set_sensitive(true);
        let auth = match self.kind {
            ProviderKind::Anthropic => "x-api-key",
            ProviderKind::OpenRouter => "authorization",
        };
        let mut req = self
            .client
            .post(format!("{}/v1/messages", self.base_url.trim_end_matches('/')))
            .header(auth, key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .header("accept", "text/event-stream");
        if self.fallbacks {
            req = req.header("anthropic-beta", FALLBACK_BETA);
        }
        let resp = req
            .body(body.to_string())
            .send()
            .map_err(|e| (ProviderError::Network(e.without_url().to_string()), false))?;
        let status = resp.status().as_u16();
        if status != 200 {
            let text = resp.text().unwrap_or_default();
            let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            return Err((
                ProviderError::Api {
                    status,
                    kind: v["error"]["type"].as_str().unwrap_or("http_error").to_owned(),
                    message: v["error"]["message"].as_str().map(str::to_owned).unwrap_or(text),
                },
                false,
            ));
        }
        match &self.record_dir {
            Some(dir) => {
                let mut raw = Vec::new();
                let mut resp = resp;
                resp.read_to_end(&mut raw).map_err(|e| (ProviderError::Network(e.to_string()), false))?;
                let _ = std::fs::create_dir_all(dir);
                let name = format!(
                    "{}.sse",
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis()
                );
                if let Ok(mut f) = std::fs::File::create(dir.join(name)) {
                    let _ = f.write_all(&raw);
                }
                read_stream(raw.as_slice(), on_event)
            }
            None => read_stream(BufReader::new(resp), on_event),
        }
    }

    fn send(
        &self,
        request: &ChatRequest,
        on_event: &mut dyn FnMut(StreamEvent) -> bool,
    ) -> Result<ProviderResponse, ProviderError> {
        let mut attempt = 0;
        loop {
            match self.send_once(request, on_event) {
                Ok(r) => return Ok(r),
                // Retry only before any content streamed, so nothing shown is repeated.
                Err((e, false)) if e.retryable() && attempt < self.retries => {
                    attempt += 1;
                    log::warn!("assistant request failed ({e}); retrying");
                    std::thread::sleep(Duration::from_millis(1000 * 2u64.pow(attempt)));
                }
                Err((e, _)) => return Err(e),
            }
        }
    }
}

/// The Anthropic Messages API.
#[cfg(not(target_arch = "wasm32"))]
pub struct AnthropicProvider(MessagesClient);

#[cfg(not(target_arch = "wasm32"))]
impl AnthropicProvider {
    /// A provider for `model` with `key`. `ANTHROPIC_BASE_URL` overrides the endpoint, and
    /// `SOLIDDRAFT_RECORD_DIR` saves every raw stream there, for new test fixtures.
    pub fn new(key: ApiKey, model: &str) -> Result<Self, ProviderError> {
        Self::with_base_url(key, model, &base_url("ANTHROPIC_BASE_URL", "https://api.anthropic.com"))
    }

    /// A provider that sends to `base_url` instead, such as a proxy.
    pub fn with_base_url(key: ApiKey, model: &str, base_url: &str) -> Result<Self, ProviderError> {
        MessagesClient::new(ProviderKind::Anthropic, key, model, base_url.to_owned()).map(Self)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Provider for AnthropicProvider {
    fn name(&self) -> String {
        ProviderKind::Anthropic.label().into()
    }

    fn model(&self) -> String {
        self.0.model.clone()
    }

    fn send(
        &self,
        request: &ChatRequest,
        on_event: &mut dyn FnMut(StreamEvent) -> bool,
    ) -> Result<ProviderResponse, ProviderError> {
        self.0.send(request, on_event)
    }
}

/// OpenRouter, through its Anthropic-compatible Messages endpoint (`POST /api/v1/messages`).
///
/// That endpoint takes the same body and streams the same events as the Anthropic API,
/// thinking blocks and their signatures included, so tool use, proposals and the history
/// rules work unchanged. The key goes in a bearer header, model ids carry the vendor
/// (`anthropic/claude-opus-5.5`), and the Anthropic-only server-side fallbacks are not sent.
#[cfg(not(target_arch = "wasm32"))]
pub struct OpenRouterProvider(MessagesClient);

#[cfg(not(target_arch = "wasm32"))]
impl OpenRouterProvider {
    /// A provider for `model` with `key`. `OPENROUTER_BASE_URL` overrides the endpoint, and
    /// `SOLIDDRAFT_RECORD_DIR` saves every raw stream there.
    pub fn new(key: ApiKey, model: &str) -> Result<Self, ProviderError> {
        Self::with_base_url(key, model, &base_url("OPENROUTER_BASE_URL", "https://openrouter.ai/api"))
    }

    /// A provider that sends to `base_url` instead of `https://openrouter.ai/api`.
    pub fn with_base_url(key: ApiKey, model: &str, base_url: &str) -> Result<Self, ProviderError> {
        MessagesClient::new(ProviderKind::OpenRouter, key, model, base_url.to_owned()).map(Self)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Provider for OpenRouterProvider {
    fn name(&self) -> String {
        ProviderKind::OpenRouter.label().into()
    }

    fn model(&self) -> String {
        self.0.model.clone()
    }

    fn send(
        &self,
        request: &ChatRequest,
        on_event: &mut dyn FnMut(StreamEvent) -> bool,
    ) -> Result<ProviderResponse, ProviderError> {
        self.0.send(request, on_event)
    }
}

/// One scripted reply for [`MockProvider`].
#[derive(Clone, Debug, PartialEq)]
pub enum MockTurn {
    /// A raw recorded event stream, replayed as is.
    Recorded(String),
    /// A reply built from text and tool calls; it is encoded as an event stream, split
    /// into small deltas, and read back like a real one.
    Reply {
        /// Text before the tool calls.
        text: String,
        /// Tool calls: name and input.
        tool_calls: Vec<(String, Value)>,
    },
    /// An error status from the API.
    Error {
        /// HTTP status.
        status: u16,
        /// Error type.
        kind: String,
    },
}

impl MockTurn {
    /// A text-only reply.
    pub fn text(text: &str) -> Self {
        MockTurn::Reply { text: text.into(), tool_calls: vec![] }
    }

    /// A reply with text and tool calls.
    pub fn tools(text: &str, calls: Vec<(&str, Value)>) -> Self {
        MockTurn::Reply { text: text.into(), tool_calls: calls.into_iter().map(|(n, v)| (n.to_owned(), v)).collect() }
    }

    /// Encodes the turn as a Messages API event stream.
    pub fn to_sse(&self, model: &str, seq: usize) -> String {
        let (text, calls) = match self {
            MockTurn::Recorded(s) => return s.clone(),
            MockTurn::Reply { text, tool_calls } => (text, tool_calls),
            MockTurn::Error { .. } => return String::new(),
        };
        let mut out = String::new();
        let mut ev = |name: &str, data: Value| out.push_str(&format!("event: {name}\ndata: {data}\n\n"));
        ev(
            "message_start",
            json!({"type":"message_start","message":{"id":format!("msg_mock_{seq}"),"type":"message","role":"assistant","model":model,"content":[],"stop_reason":null,"usage":{"input_tokens":12,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}),
        );
        let mut index = 0;
        // Opus 5.5 always thinks; with the default display the block arrives empty.
        ev(
            "content_block_start",
            json!({"type":"content_block_start","index":index,"content_block":{"type":"thinking","thinking":""}}),
        );
        ev(
            "content_block_delta",
            json!({"type":"content_block_delta","index":index,"delta":{"type":"thinking_delta","thinking":""}}),
        );
        ev(
            "content_block_delta",
            json!({"type":"content_block_delta","index":index,"delta":{"type":"signature_delta","signature":format!("mock-signature-{seq}")}}),
        );
        ev("content_block_stop", json!({"type":"content_block_stop","index":index}));
        index += 1;
        ev("ping", json!({"type":"ping"}));
        if !text.is_empty() {
            ev(
                "content_block_start",
                json!({"type":"content_block_start","index":index,"content_block":{"type":"text","text":""}}),
            );
            for chunk in chunks(text, 7) {
                ev(
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":chunk}}),
                );
            }
            ev("content_block_stop", json!({"type":"content_block_stop","index":index}));
            index += 1;
        }
        for (i, (name, input)) in calls.iter().enumerate() {
            ev(
                "content_block_start",
                json!({"type":"content_block_start","index":index,"content_block":{"type":"tool_use","id":format!("toolu_mock_{seq}_{i}"),"name":name,"input":{}}}),
            );
            for chunk in chunks(&input.to_string(), 16) {
                ev(
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":chunk}}),
                );
            }
            ev("content_block_stop", json!({"type":"content_block_stop","index":index}));
            index += 1;
        }
        let stop = if calls.is_empty() { "end_turn" } else { "tool_use" };
        ev(
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":stop,"stop_sequence":null},"usage":{"output_tokens":40}}),
        );
        ev("message_stop", json!({"type":"message_stop"}));
        out
    }
}

fn chunks(s: &str, n: usize) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    chars.chunks(n).map(|c| c.iter().collect()).collect()
}

/// A provider that plays back scripted turns in order and records every request.
pub struct MockProvider {
    model: String,
    turns: Mutex<std::collections::VecDeque<MockTurn>>,
    requests: Mutex<Vec<ChatRequest>>,
    seq: Mutex<usize>,
}

impl MockProvider {
    /// A provider that answers with `turns`, one per request.
    pub fn new(turns: Vec<MockTurn>) -> Self {
        Self {
            model: crate::DEFAULT_MODEL.into(),
            turns: Mutex::new(turns.into()),
            requests: Mutex::new(Vec::new()),
            seq: Mutex::new(0),
        }
    }

    /// Adds turns to the end of the script.
    pub fn push(&self, turns: impl IntoIterator<Item = MockTurn>) {
        self.turns.lock().unwrap().extend(turns);
    }

    /// Every request received so far.
    pub fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().unwrap().clone()
    }

    /// How many scripted turns are left.
    pub fn remaining(&self) -> usize {
        self.turns.lock().unwrap().len()
    }
}

impl Provider for MockProvider {
    fn name(&self) -> String {
        "Mock".into()
    }

    fn model(&self) -> String {
        self.model.clone()
    }

    fn send(
        &self,
        request: &ChatRequest,
        on_event: &mut dyn FnMut(StreamEvent) -> bool,
    ) -> Result<ProviderResponse, ProviderError> {
        self.requests.lock().unwrap().push(request.clone());
        let seq = {
            let mut s = self.seq.lock().unwrap();
            *s += 1;
            *s
        };
        let turn = self
            .turns
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| ProviderError::Protocol("the mock provider has no turn left".into()))?;
        if let MockTurn::Error { status, kind } = &turn {
            return Err(ProviderError::Api { status: *status, kind: kind.clone(), message: "mock error".into() });
        }
        let sse = turn.to_sse(&self.model, seq);
        read_stream(sse.as_bytes(), on_event).map_err(|(e, _)| e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{Effort, Message};

    fn request() -> ChatRequest {
        ChatRequest {
            system: "s".into(),
            tools: vec![],
            messages: vec![Message::user_text(&["hi"])],
            effort: Effort::Medium,
            max_tokens: 1000,
        }
    }

    #[test]
    fn scripted_turn_round_trips_through_the_stream_reader() {
        let p = MockProvider::new(vec![MockTurn::tools(
            "Proposing a fillet.",
            vec![("propose", json!({"summary": "R8 fillet", "commands": [{"op": "fillet", "radius": 8}]}))],
        )]);
        let mut text = String::new();
        let r = p
            .send(&request(), &mut |e| {
                if let StreamEvent::TextDelta(t) = e {
                    text.push_str(&t);
                }
                true
            })
            .unwrap();
        assert_eq!(text, "Proposing a fillet.");
        assert_eq!(r.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(r.content.len(), 3);
        assert_eq!(r.content[2]["input"]["commands"][0]["radius"], 8);
        assert_eq!(p.requests().len(), 1);
        assert_eq!(p.remaining(), 0);
    }

    #[test]
    fn stopping_mid_stream_cancels() {
        let p = MockProvider::new(vec![MockTurn::text("a long reply that will be stopped")]);
        let err = p.send(&request(), &mut |_| false).unwrap_err();
        assert!(matches!(err, ProviderError::Cancelled));
    }

    /// Serves one request on a loopback port with `reply` as the event stream, and hands
    /// back the request as received: the head (lower-cased) and the body.
    fn serve_once(reply: String) -> (String, std::thread::JoinHandle<(String, Value)>) {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim().is_empty() {
                    break;
                }
                head.push_str(&line.to_ascii_lowercase());
            }
            let len: usize = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .map(|v| v.trim().parse().unwrap())
                .unwrap_or(0);
            let mut body = vec![0; len];
            reader.read_exact(&mut body).unwrap();
            let mut stream = stream;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                reply.len()
            )
            .unwrap();
            (head, serde_json::from_slice(&body).unwrap())
        });
        (url, handle)
    }

    #[test]
    fn openrouter_sends_the_messages_request_with_a_bearer_key() {
        let model = "anthropic/claude-opus-5.5";
        let reply = MockTurn::tools("Proposing.", vec![("propose", json!({"summary": "R8 fillet"}))]).to_sse(model, 1);
        let (url, server) = serve_once(reply);
        let p = OpenRouterProvider::with_base_url(ApiKey::new("sk-or-test").unwrap(), model, &format!("{url}/api"))
            .unwrap();
        let r = p.send(&request(), &mut |_| true).unwrap();
        assert_eq!(r.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(r.content.last().unwrap()["input"]["summary"], "R8 fillet");
        let (head, body) = server.join().unwrap();
        assert!(head.starts_with("post /api/v1/messages "), "{head}");
        assert!(head.contains("authorization: bearer sk-or-test"), "{head}");
        assert!(!head.contains("x-api-key"), "{head}");
        assert!(!head.contains("anthropic-beta"), "{head}");
        assert_eq!(body["model"], model);
        assert_eq!(body["thinking"], json!({"type": "adaptive", "display": "omitted"}));
        assert_eq!(body["provider"]["order"], json!(["anthropic"]));
        assert!(body.get("fallbacks").is_none());
        assert_eq!(body["output_config"]["effort"], "medium");
        assert_eq!(p.name(), "OpenRouter");
    }

    #[test]
    fn anthropic_sends_the_key_in_its_own_header() {
        let reply = MockTurn::text("hi").to_sse("claude-opus-5-5", 1);
        let (url, server) = serve_once(reply);
        let p = AnthropicProvider::with_base_url(ApiKey::new("sk-ant-test").unwrap(), "claude-opus-5-5", &url).unwrap();
        p.send(&request(), &mut |_| true).unwrap();
        let (head, body) = server.join().unwrap();
        assert!(head.starts_with("post /v1/messages "), "{head}");
        assert!(head.contains("x-api-key: sk-ant-test"), "{head}");
        assert!(!head.contains("authorization"), "{head}");
        assert!(body.get("thinking").is_none() && body.get("provider").is_none());
    }

    #[test]
    fn provider_kinds_parse_and_name_their_keys() {
        assert_eq!(ProviderKind::parse(" OpenRouter "), Some(ProviderKind::OpenRouter));
        assert_eq!(ProviderKind::parse("anthropic"), Some(ProviderKind::Anthropic));
        assert_eq!(ProviderKind::parse("bedrock"), None);
        assert_eq!(ProviderKind::OpenRouter.key_var(), "OPENROUTER_API_KEY");
        assert_eq!(serde_json::to_value(ProviderKind::OpenRouter).unwrap(), "openrouter");
    }

    #[test]
    fn truncated_stream_is_an_error() {
        let sse = MockTurn::text("hello").to_sse("m", 1);
        let cut = &sse[..sse.find("event: message_delta").unwrap()];
        let err = read_stream(cut.as_bytes(), &mut |_| true).unwrap_err();
        assert!(matches!(err.0, ProviderError::Network(_)));
        assert!(err.1, "content had started");
    }
}
