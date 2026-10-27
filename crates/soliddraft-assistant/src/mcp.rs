//! The Model Context Protocol server: the same [`ToolHost`] the assistant uses, served
//! to outside agents such as Claude Code.
//!
//! Only the tools part of the protocol is implemented: `initialize`, `ping`,
//! `tools/list` and `tools/call`. Two transports: newline-delimited JSON-RPC on
//! stdin and stdout ([`serve_stdio`]) for the headless command line, and Streamable
//! HTTP on a loopback port ([`HttpServer`]) for the running app. The HTTP server hands
//! each message to the caller through a channel, so the app runs the tools on its own
//! thread, next to its document.

use crate::tools::{Caller, ToolHost, call_checked, clip_error};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

/// Protocol versions this server speaks, newest first.
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// One client's view of the server: what it said in `initialize`.
pub struct McpSession {
    name: String,
    version: String,
    instructions: String,
    client: String,
}

impl McpSession {
    /// A session for a server called `name` at `version`, with `instructions` for the client.
    pub fn new(name: &str, version: &str, instructions: &str) -> Self {
        Self { name: name.into(), version: version.into(), instructions: instructions.into(), client: "unknown".into() }
    }

    /// The client's name from `initialize`.
    pub fn client(&self) -> &str {
        &self.client
    }

    /// Handles one JSON-RPC message or batch. Returns the response, or `None` for
    /// notifications and responses.
    pub fn handle(&mut self, host: &mut dyn ToolHost, message: Value) -> Option<Value> {
        match message {
            // An empty batch is an invalid request, and the client waits for the answer.
            Value::Array(batch) if batch.is_empty() => {
                Some(error_reply(Value::Null, -32600, "invalid request: empty batch"))
            }
            Value::Array(batch) => {
                let replies: Vec<Value> = batch.into_iter().filter_map(|m| self.handle_one(host, m)).collect();
                (!replies.is_empty()).then_some(Value::Array(replies))
            }
            m => self.handle_one(host, m),
        }
    }

    /// Handles one line of text: parses it, or answers with a parse error.
    pub fn handle_text(&mut self, host: &mut dyn ToolHost, text: &str) -> Option<Value> {
        match serde_json::from_str::<Value>(text) {
            Ok(v) => self.handle(host, v),
            Err(e) => Some(error_reply(Value::Null, -32700, &format!("parse error: {e}"))),
        }
    }

    fn handle_one(&mut self, host: &mut dyn ToolHost, msg: Value) -> Option<Value> {
        let id = msg.get("id").cloned();
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            // A response from the client, or junk: nothing to answer unless it had an id.
            return id
                .filter(|_| msg.get("result").is_none() && msg.get("error").is_none())
                .map(|id| error_reply(id, -32600, "invalid request"));
        };
        let Some(id) = id else {
            // Notifications, such as notifications/initialized, need no answer.
            return None;
        };
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => {
                if let Some(name) = params["clientInfo"]["name"].as_str() {
                    self.client = name.to_owned();
                }
                let asked = params["protocolVersion"].as_str().unwrap_or_default();
                let version = PROTOCOL_VERSIONS.iter().find(|v| **v == asked).copied().unwrap_or(PROTOCOL_VERSIONS[0]);
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": self.name, "version": self.version },
                    "instructions": self.instructions,
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({
                "tools": host.tools().into_iter().map(|t| json!({
                    "name": t.name,
                    "description": t.description,
                    "inputSchema": t.input_schema,
                })).collect::<Vec<_>>()
            })),
            "tools/call" => match params["name"].as_str() {
                None => Err((-32602, "tools/call needs a tool name".to_owned())),
                Some(name) if !host.tools().iter().any(|t| t.name == name) => {
                    Err((-32602, format!("unknown tool `{name}`")))
                }
                Some(name) => {
                    let args = match params.get("arguments") {
                        Some(Value::Null) | None => json!({}),
                        Some(a) => a.clone(),
                    };
                    let caller = Caller::Mcp { client: self.client.clone() };
                    let out = call_checked(host, name, args, &caller);
                    let mut result = json!({
                        "content": [{ "type": "text", "text": out.content }],
                        "isError": out.is_error,
                    });
                    if let Ok(v @ Value::Object(_)) = serde_json::from_str::<Value>(&out.content) {
                        result["structuredContent"] = v;
                    }
                    Ok(result)
                }
            },
            other => Err((-32601, format!("method `{other}` is not supported"))),
        };
        Some(match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err((code, msg)) => error_reply(id, code, &msg),
        })
    }
}

fn error_reply(id: Value, code: i64, message: &str) -> Value {
    let message = clip_error(message.to_owned());
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Serves MCP on stdin and stdout until stdin closes. Logs must go to stderr.
pub fn serve_stdio(session: &mut McpSession, host: &mut dyn ToolHost) -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    serve_lines(session, host, stdin.lock(), &mut stdout)
}

/// The longest message either transport takes: a line on stdin or an HTTP body.
const MAX_MESSAGE: usize = 16 * 1024 * 1024;

/// Serves newline-delimited JSON-RPC from `input` to `output`. A line that is not UTF-8,
/// not JSON or longer than [`MAX_MESSAGE`] is answered with an error and the loop goes
/// on: only the end of `input` or a failed read or write ends it.
pub fn serve_lines(
    session: &mut McpSession,
    host: &mut dyn ToolHost,
    mut input: impl BufRead,
    output: &mut impl Write,
) -> std::io::Result<()> {
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = (&mut input).take(MAX_MESSAGE as u64 + 1).read_until(b'\n', &mut line)?;
        if read == 0 {
            return Ok(());
        }
        let reply = if line.len() > MAX_MESSAGE && line.last() != Some(&b'\n') {
            // No end of line within the limit: throw the rest of this line away.
            loop {
                let buffered = input.fill_buf()?;
                let Some(end) = buffered.iter().position(|b| *b == b'\n') else {
                    let n = buffered.len();
                    if n == 0 {
                        break;
                    }
                    input.consume(n);
                    continue;
                };
                input.consume(end + 1);
                break;
            }
            Some(error_reply(Value::Null, -32600, "invalid request: the message is too long"))
        } else {
            match std::str::from_utf8(&line) {
                Ok(text) if text.trim().is_empty() => continue,
                Ok(text) => session.handle_text(host, text),
                Err(e) => {
                    Some(error_reply(Value::Null, -32700, &format!("parse error: the message is not UTF-8: {e}")))
                }
            }
        };
        if let Some(reply) = reply {
            writeln!(output, "{reply}")?;
            output.flush()?;
        }
    }
}

/// One message from an HTTP client, waiting for the app to answer it.
pub struct HttpCall {
    /// The JSON-RPC message or batch.
    pub message: Value,
    reply: Sender<Option<Value>>,
}

impl HttpCall {
    /// Sends the answer back to the client; `None` for notifications.
    pub fn respond(self, reply: Option<Value>) {
        let _ = self.reply.send(reply);
    }
}

/// A Streamable HTTP endpoint at `http://127.0.0.1:<port>/mcp`.
pub struct HttpServer {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
}

impl HttpServer {
    /// Listens on `127.0.0.1:port` (0 picks a free port). Calls arrive on the returned
    /// channel; `waker` runs after each one so an idle interface wakes to answer it.
    pub fn start(port: u16, waker: Option<Arc<dyn Fn() + Send + Sync>>) -> std::io::Result<(Self, Receiver<HttpCall>)> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let addr = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let stopping = stop.clone();
        std::thread::Builder::new().name("mcp-http".into()).spawn(move || {
            for stream in listener.incoming() {
                if stopping.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                let tx = tx.clone();
                let waker = waker.clone();
                let _ = std::thread::Builder::new().name("mcp-conn".into()).spawn(move || {
                    if let Err(e) = connection(stream, &tx, waker.as_deref()) {
                        log::debug!("mcp connection closed: {e}");
                    }
                });
            }
        })?;
        Ok((Self { addr, stop }, rx))
    }

    /// The bound address.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The endpoint URL to give a client.
    pub fn url(&self) -> String {
        format!("http://{}/mcp", self.addr)
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop so it sees the flag.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(200));
    }
}

/// The longest request line or header line, and the most header lines, a request may have.
const MAX_LINE: usize = 64 * 1024;
const MAX_HEADERS: usize = 100;

/// How long, and how many bytes, a refused client may go on sending before the server
/// closes on it.
const DRAIN_TIME: Duration = Duration::from_secs(2);
const DRAIN_BYTES: usize = MAX_MESSAGE;

/// One line of a request's head.
enum Head {
    Line(String),
    /// The client closed the connection.
    Closed,
    /// No end of line within [`MAX_LINE`] bytes.
    TooLong,
    /// The line is not UTF-8.
    NotText,
}

/// Reads a line of the head without letting a client make the server buffer more than
/// [`MAX_LINE`] bytes of it.
fn head_line(reader: &mut impl BufRead) -> std::io::Result<Head> {
    let mut bytes = Vec::new();
    if reader.by_ref().take(MAX_LINE as u64 + 1).read_until(b'\n', &mut bytes)? == 0 {
        return Ok(Head::Closed);
    }
    if bytes.len() > MAX_LINE && bytes.last() != Some(&b'\n') {
        return Ok(Head::TooLong);
    }
    Ok(String::from_utf8(bytes).map_or(Head::NotText, Head::Line))
}

/// Serves HTTP/1.1 requests on one connection until the client closes it.
fn connection(
    stream: TcpStream,
    tx: &Sender<HttpCall>,
    waker: Option<&(dyn Fn() + Send + Sync)>,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(300)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    loop {
        let request_line = match head_line(&mut reader)? {
            Head::Line(line) => line,
            Head::Closed => return Ok(()),
            Head::TooLong => return refuse(&mut writer, &mut reader, 431),
            Head::NotText => return refuse(&mut writer, &mut reader, 400),
        };
        let mut parts = request_line.split_whitespace();
        let (method, path) = (parts.next().unwrap_or_default().to_owned(), parts.next().unwrap_or_default().to_owned());
        let mut length = 0usize;
        let mut origin = None;
        let mut close = false;
        for headers in 0.. {
            let line = match head_line(&mut reader)? {
                Head::Line(line) => line,
                Head::Closed => return Ok(()),
                Head::TooLong => return refuse(&mut writer, &mut reader, 431),
                Head::NotText => return refuse(&mut writer, &mut reader, 400),
            };
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if headers >= MAX_HEADERS {
                return refuse(&mut writer, &mut reader, 431);
            }
            if let Some((k, v)) = line.split_once(':') {
                let v = v.trim();
                match k.to_ascii_lowercase().as_str() {
                    "content-length" => length = v.parse().unwrap_or(0),
                    "origin" => origin = Some(v.to_owned()),
                    "connection" => close = v.eq_ignore_ascii_case("close"),
                    _ => {}
                }
            }
        }
        if length > MAX_MESSAGE {
            return refuse(&mut writer, &mut reader, 413);
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body)?;
        // Browsers send Origin; only pages served from this machine may talk to the app.
        if origin.as_deref().is_some_and(|o| !local_origin(o)) {
            respond(&mut writer, 403, None, close)?;
        } else if path.split('?').next() != Some("/mcp") {
            respond(&mut writer, 404, None, close)?;
        } else if method != "POST" {
            // No server-initiated stream (GET) and no session to delete (DELETE).
            respond(&mut writer, 405, None, close)?;
        } else {
            let reply = match serde_json::from_slice::<Value>(&body) {
                Err(e) => Some(error_reply(Value::Null, -32700, &format!("parse error: {e}"))),
                Ok(message) => {
                    let (rtx, rrx) = channel();
                    if tx.send(HttpCall { message, reply: rtx }).is_err() {
                        respond(&mut writer, 503, None, true)?;
                        return Ok(());
                    }
                    if let Some(w) = waker {
                        w();
                    }
                    match rrx.recv_timeout(Duration::from_secs(120)) {
                        Ok(r) => r,
                        Err(_) => {
                            respond(&mut writer, 503, None, true)?;
                            return Ok(());
                        }
                    }
                }
            };
            match reply {
                Some(v) => respond(&mut writer, 200, Some(&v), close)?,
                None => respond(&mut writer, 202, None, close)?,
            }
        }
        if close {
            return Ok(());
        }
    }
}

fn local_origin(origin: &str) -> bool {
    let host = origin.split("://").nth(1).unwrap_or(origin);
    let host = host
        .rsplit_once(':')
        .map(|(h, p)| if p.chars().all(|c| c.is_ascii_digit()) { h } else { host })
        .unwrap_or(host);
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

/// Answers a request the server will not read to its end, then closes the connection.
/// Closing on unread request bytes resets the connection, and Windows then drops the
/// answer before the client reads it. So the server stops writing, reads and discards
/// what the client still sends, for at most [`DRAIN_TIME`] and [`DRAIN_BYTES`], and only
/// then closes.
fn refuse(writer: &mut TcpStream, reader: &mut BufReader<TcpStream>, status: u16) -> std::io::Result<()> {
    respond(writer, status, None, true)?;
    writer.shutdown(Shutdown::Write)?;
    let deadline = Instant::now() + DRAIN_TIME;
    let mut sink = [0; 8192];
    let mut drained = 0;
    while drained < DRAIN_BYTES {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        reader.get_ref().set_read_timeout(Some(left))?;
        match reader.read(&mut sink) {
            Ok(0) => break,
            Ok(n) => drained += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    Ok(())
}

fn respond(w: &mut TcpStream, status: u16, body: Option<&Value>, close: bool) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        _ => "Service Unavailable",
    };
    let text = body.map(Value::to_string).unwrap_or_default();
    let mut head = format!("HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\n", text.len());
    if body.is_some() {
        head.push_str("Content-Type: application/json\r\n");
    }
    if status == 405 {
        head.push_str("Allow: POST\r\n");
    }
    if close {
        head.push_str("Connection: close\r\n");
    }
    head.push_str("\r\n");
    w.write_all(head.as_bytes())?;
    w.write_all(text.as_bytes())?;
    w.flush()
}

/// A minimal MCP client over Streamable HTTP, for tests and scripts.
#[cfg(not(target_arch = "wasm32"))]
pub struct HttpClient {
    url: String,
    http: reqwest::blocking::Client,
    next_id: u64,
}

#[cfg(not(target_arch = "wasm32"))]
impl HttpClient {
    /// Connects and runs the `initialize` handshake as `client_name`.
    pub fn connect(url: &str, client_name: &str) -> Result<(Self, Value), String> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(120))
            .no_proxy()
            .build()
            .map_err(|e| e.to_string())?;
        let mut c = Self { url: url.into(), http, next_id: 0 };
        let init = c.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSIONS[0],
                "capabilities": {},
                "clientInfo": { "name": client_name, "version": "1" }
            }),
        )?;
        c.post(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))?;
        Ok((c, init))
    }

    /// Sends a request and returns its result, or the JSON-RPC error as text.
    pub fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let reply = self
            .post(&json!({ "jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params }))?
            .ok_or("no reply")?;
        match reply.get("error") {
            Some(e) => Err(e["message"].as_str().unwrap_or("error").to_owned()),
            None => Ok(reply["result"].clone()),
        }
    }

    /// Calls a tool and returns its result: `content`, `isError` and `structuredContent`.
    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, String> {
        self.request("tools/call", json!({ "name": name, "arguments": arguments }))
    }

    fn post(&self, body: &Value) -> Result<Option<Value>, String> {
        let resp = self
            .http
            .post(&self.url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", PROTOCOL_VERSIONS[0])
            .body(body.to_string())
            .send()
            .map_err(|e| e.to_string())?;
        match resp.status().as_u16() {
            202 => Ok(None),
            200 => {
                let text = resp.text().map_err(|e| e.to_string())?;
                serde_json::from_str(&text).map(Some).map_err(|e| e.to_string())
            }
            s => Err(format!("HTTP {s}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::schema::*;
    use crate::tools::{ToolOutput, ToolSpec};

    struct Echo {
        callers: Vec<Caller>,
    }

    impl ToolHost for Echo {
        fn tools(&self) -> Vec<ToolSpec> {
            vec![ToolSpec {
                name: "echo".into(),
                description: "Echoes".into(),
                input_schema: object(&[("text", string("t"))]),
            }]
        }

        fn call(&mut self, _: &str, input: Value, caller: &Caller) -> ToolOutput {
            self.callers.push(caller.clone());
            ToolOutput::json(&json!({ "echo": input["text"] }))
        }
    }

    #[test]
    fn stdio_handshake_list_and_call() {
        let mut host = Echo { callers: vec![] };
        let mut session = McpSession::new("soliddraft", "0.1.0", "CAD");
        let input = [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"claude-code","version":"2"}}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"echo","arguments":{"text":"hi"}}}),
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"echo","arguments":{"text":5}}}),
            json!({"jsonrpc":"2.0","id":5,"method":"resources/list"}),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\nnot json\n";
        let mut out = Vec::new();
        serve_lines(&mut session, &mut host, input.as_bytes(), &mut out).unwrap();
        let replies: Vec<Value> =
            String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(replies.len(), 6, "the notification gets no reply");
        assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(replies[1]["result"]["tools"][0]["name"], "echo");
        assert_eq!(replies[2]["result"]["structuredContent"]["echo"], "hi");
        assert_eq!(replies[2]["result"]["isError"], false);
        assert_eq!(replies[3]["result"]["isError"], true, "schema check failed");
        assert_eq!(replies[4]["error"]["code"], -32601);
        assert_eq!(replies[5]["error"]["code"], -32700);
        assert_eq!(host.callers, vec![Caller::Mcp { client: "claude-code".into() }]);
    }

    #[test]
    fn http_round_trip_through_the_channel() {
        let (server, rx) = HttpServer::start(0, None).unwrap();
        let url = server.url();
        let worker = std::thread::spawn(move || {
            let mut host = Echo { callers: vec![] };
            let mut session = McpSession::new("soliddraft", "0.1.0", "CAD");
            while let Ok(call) = rx.recv_timeout(Duration::from_secs(10)) {
                let msg = call.message.clone();
                let reply = session.handle(&mut host, msg);
                call.respond(reply);
                if host.callers.len() == 1 {
                    break;
                }
            }
            host.callers
        });
        let (mut client, init) = HttpClient::connect(&url, "test-agent").unwrap();
        assert_eq!(init["serverInfo"]["name"], "soliddraft");
        let out = client.call_tool("echo", json!({"text": "over http"})).unwrap();
        assert_eq!(out["structuredContent"]["echo"], "over http");
        assert_eq!(worker.join().unwrap(), vec![Caller::Mcp { client: "test-agent".into() }]);

        // A page from another site may not drive the app.
        let resp = reqwest::blocking::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(&url)
            .header("origin", "https://evil.example")
            .body("{}")
            .send()
            .unwrap();
        assert_eq!(resp.status().as_u16(), 403);
        assert!(
            local_origin("http://localhost:3000")
                && local_origin("http://127.0.0.1")
                && !local_origin("http://localhost.evil.example")
        );
    }
}
