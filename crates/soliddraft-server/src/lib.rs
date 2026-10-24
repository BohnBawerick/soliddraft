//! The server behind the web build of SolidDraft.
//!
//! The application and Catallax run in one browser WebAssembly module. The server
//! serves its static files and relays assistant requests to the configured provider.
//! API keys stay on the server. `GET /api/info` reports the assistant's state and
//! `GET /version` identifies the running build.

use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use serde_json::json;
use soliddraft_assistant::Provider;
use soliddraft_assistant::relay;
use soliddraft_assistant::wire::ChatRequest;
use tiny_http::{Header, Method, Request, Response, StatusCode};

/// Largest request body accepted, in bytes.
const MAX_BODY: u64 = 64 << 20;

/// Replies at least this long are compressed when the client accepts gzip.
const COMPRESS_FROM: usize = 16 << 10;

/// What the server runs on.
pub struct Config {
    /// The directory holding the web build (`index.html` and the WebAssembly).
    pub web_dir: PathBuf,
    /// The assistant's provider, or why there is none.
    pub provider: Result<Arc<dyn Provider>, String>,
    /// The commit the build was made from, if the build said (`GET /version`).
    pub commit: Option<String>,
}

struct Shared {
    config: Config,
}

/// A running server.
pub struct Server {
    http: Arc<tiny_http::Server>,
    shared: Arc<Shared>,
}

impl Server {
    /// Binds `addr`. Port 0 picks a free port; [`Server::addr`] says which.
    pub fn bind(addr: &str, config: Config) -> Result<Server, String> {
        let http = tiny_http::Server::http(addr).map_err(|e| format!("cannot listen on {addr}: {e}"))?;
        Ok(Server { http: Arc::new(http), shared: Arc::new(Shared { config }) })
    }

    /// The address the server listens on.
    pub fn addr(&self) -> SocketAddr {
        self.http.server_addr().to_ip().expect("an IP listener")
    }

    /// Serves until the process ends. Each request gets its own thread, since an assistant
    /// reply streams for as long as the model writes.
    pub fn run(self) {
        for request in self.http.incoming_requests() {
            let shared = self.shared.clone();
            let spawned = std::thread::Builder::new().name("request".into()).spawn(move || handle(&shared, request));
            if let Err(e) = spawned {
                log::error!("cannot start a request thread: {e}");
            }
        }
    }

    /// Serves on a background thread, for tests.
    pub fn spawn(self) -> SocketAddr {
        let addr = self.addr();
        std::thread::spawn(move || self.run());
        addr
    }
}

fn handle(shared: &Shared, request: Request) {
    let path = request.url().split('?').next().unwrap_or("/").to_owned();
    let method = request.method().clone();
    let result = match (&method, path.as_str()) {
        (Method::Get, "/api/info") => Ok(info(shared)),
        (Method::Get, "/version") => Ok(version(shared)),
        (Method::Post, "/api/assistant") => return assistant(shared, request),
        (Method::Get | Method::Head, p) if !p.starts_with("/api/") => static_file(&shared.config.web_dir, p),
        _ => Err((StatusCode(404), "not found".to_owned())),
    };
    let response = match result {
        Ok(reply) => reply.into_response(&request),
        Err((status, message)) => Reply::text(status, message).into_response(&request),
    };
    if let Err(e) = request.respond(response) {
        log::debug!("{method} {path}: the client went away: {e}");
    }
}

type HandlerResult = Result<Reply, (StatusCode, String)>;

/// A complete reply.
struct Reply {
    status: StatusCode,
    content_type: &'static str,
    body: Vec<u8>,
    cache: &'static str,
    /// A validator for a static file, so a browser can ask whether its copy is current.
    etag: Option<String>,
}

impl Reply {
    fn json(body: String) -> Reply {
        Reply {
            status: StatusCode(200),
            content_type: "application/json",
            body: body.into_bytes(),
            cache: "no-store",
            etag: None,
        }
    }

    fn text(status: StatusCode, text: String) -> Reply {
        Reply {
            status,
            content_type: "text/plain; charset=utf-8",
            body: text.into_bytes(),
            cache: "no-store",
            etag: None,
        }
    }

    fn into_response(self, request: &Request) -> Response<std::io::Cursor<Vec<u8>>> {
        if let Some(etag) = &self.etag
            && request.headers().iter().any(|h| h.field.equiv("If-None-Match") && h.value.as_str() == etag)
        {
            return Response::from_data(Vec::new())
                .with_status_code(StatusCode(304))
                .with_header(header("ETag", etag))
                .with_header(header("Cache-Control", self.cache));
        }
        let gzip = self.body.len() >= COMPRESS_FROM && accepts_gzip(request);
        let body = if gzip { compress(&self.body) } else { self.body };
        let mut response = Response::from_data(body)
            .with_status_code(self.status)
            .with_header(header("Content-Type", self.content_type))
            .with_header(header("Cache-Control", self.cache));
        if gzip {
            response.add_header(header("Content-Encoding", "gzip"));
        }
        if let Some(etag) = &self.etag {
            response.add_header(header("ETag", etag));
        }
        response
    }
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("a valid header")
}

fn accepts_gzip(request: &Request) -> bool {
    request
        .headers()
        .iter()
        .any(|h| h.field.equiv("Accept-Encoding") && h.value.as_str().split(',').any(|e| e.trim() == "gzip"))
}

fn compress(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(bytes).expect("writing to memory");
    encoder.finish().expect("writing to memory")
}

fn info(shared: &Shared) -> Reply {
    let assistant = match &shared.config.provider {
        Ok(p) => json!({ "on": true, "provider": p.name(), "model": p.model() }),
        Err(reason) => json!({ "on": false, "reason": reason }),
    };
    Reply::json(json!({ "assistant": assistant }).to_string())
}

fn version(shared: &Shared) -> Reply {
    Reply::json(json!({ "commit": shared.config.commit, "version": env!("CARGO_PKG_VERSION") }).to_string())
}

fn read_body(request: &mut Request) -> Result<String, (StatusCode, String)> {
    let mut body = String::new();
    request
        .as_reader()
        .take(MAX_BODY + 1)
        .read_to_string(&mut body)
        .map_err(|e| (StatusCode(400), format!("cannot read the request: {e}")))?;
    if body.len() as u64 > MAX_BODY {
        return Err((StatusCode(413), "request too large".into()));
    }
    Ok(body)
}

/// Streams one assistant request's reply as JSON lines.
fn assistant(shared: &Shared, mut request: Request) {
    let provider = match &shared.config.provider {
        Ok(p) => p.clone(),
        Err(reason) => {
            let response = Reply::text(StatusCode(503), reason.clone()).into_response(&request);
            let _ = request.respond(response);
            return;
        }
    };
    let chat: ChatRequest = match read_body(&mut request)
        .and_then(|b| serde_json::from_str(&b).map_err(|e| (StatusCode(400), format!("bad assistant request: {e}"))))
    {
        Ok(c) => c,
        Err((status, message)) => {
            let response = Reply::text(status, message).into_response(&request);
            let _ = request.respond(response);
            return;
        }
    };
    // The provider writes lines into a channel on its own thread; the response reads them
    // out as the body, chunk by chunk, so each event reaches the browser as it happens.
    let (tx, rx) = sync_channel::<Vec<u8>>(256);
    std::thread::Builder::new()
        .name("assistant-relay".into())
        .spawn(move || {
            relay::serve(provider.as_ref(), &chat, &mut |line| send_line(&tx, line));
        })
        .expect("spawn the assistant relay");
    let response = Response::new(
        StatusCode(200),
        vec![header("Content-Type", "application/x-ndjson"), header("Cache-Control", "no-store")],
        LineStream { rx, current: Vec::new(), offset: 0 },
        None,
        None,
    );
    if let Err(e) = request.respond(response) {
        log::debug!("assistant reply: the client went away: {e}");
    }
}

fn send_line(tx: &SyncSender<Vec<u8>>, line: &str) -> bool {
    let mut bytes = line.as_bytes().to_vec();
    bytes.push(b'\n');
    tx.send(bytes).is_ok()
}

/// The lines of a relayed reply as a byte stream. Ends when the relay thread does.
struct LineStream {
    rx: Receiver<Vec<u8>>,
    current: Vec<u8>,
    offset: usize,
}

impl Read for LineStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while self.offset == self.current.len() {
            match self.rx.recv() {
                Ok(next) => {
                    self.current = next;
                    self.offset = 0;
                }
                Err(_) => return Ok(0),
            }
        }
        let n = buf.len().min(self.current.len() - self.offset);
        buf[..n].copy_from_slice(&self.current[self.offset..self.offset + n]);
        self.offset += n;
        Ok(n)
    }
}

/// A file of the web build. Paths that leave the directory are not found.
fn static_file(root: &Path, url_path: &str) -> HandlerResult {
    let not_found = || (StatusCode(404), "not found".to_owned());
    let relative = url_path.trim_start_matches('/');
    let relative = if relative.is_empty() { "index.html" } else { relative };
    let relative = Path::new(relative);
    if !relative.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(not_found());
    }
    let path = root.join(relative);
    let body = std::fs::read(&path).map_err(|_| not_found())?;
    let content_type = match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript",
        "wasm" => "application/wasm",
        "css" => "text/css",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "ttf" => "font/ttf",
        _ => "application/octet-stream",
    };
    // trunk names the app's files by content hash, so those can be cached for good. The
    // page keeps its name from build to build: the browser checks
    // it each time and downloads it again only when the ETag changed.
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let hashed = stem
        .trim_end_matches("_bg")
        .rsplit_once('-')
        .is_some_and(|(_, h)| h.len() == 16 && h.bytes().all(|b| b.is_ascii_hexdigit()));
    let cache = if hashed { "public, max-age=31536000, immutable" } else { "no-cache" };
    let modified = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    let etag = Some(format!("\"{modified:x}-{:x}\"", body.len()));
    Ok(Reply { status: StatusCode(200), content_type, body, cache, etag })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_paths_stay_inside_the_web_directory() {
        let dir = std::env::temp_dir().join(format!("soliddraft-server-static-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "<p>hi</p>").unwrap();
        assert_eq!(static_file(&dir, "/").unwrap().body, b"<p>hi</p>");
        assert_eq!(static_file(&dir, "/index.html").unwrap().content_type, "text/html; charset=utf-8");
        for bad in ["/../etc/passwd", "/a/../../x", "//etc/passwd"] {
            assert_eq!(static_file(&dir, bad).err().map(|e| e.0), Some(StatusCode(404)), "{bad}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
