//! Append-only command journals. A complete intent reaches the OS before its command runs.
//! No clock, performance counter, document hash or kernel data is collected here.

use std::cell::RefCell;
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

static NEXT: AtomicU64 = AtomicU64::new(1);
thread_local! {
    static CURRENT: RefCell<Option<Writer>> = const { RefCell::new(None) };
}

/// An in-process document instance, distinct even when the same file is reopened.
#[derive(Debug)]
pub struct Identity(u64);
impl Default for Identity {
    fn default() -> Self {
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}
impl Identity {
    /// The instance's journal key.
    pub fn key(&self) -> u64 {
        self.0
    }
}

enum Sink {
    File(File),
    External(Box<dyn Write>),
}

impl Sink {
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        match self {
            Self::File(file) => file.write_all(bytes),
            Self::External(writer) => writer.write_all(bytes),
        }
    }
}

struct Writer {
    sink: Sink,
    path: PathBuf,
    documents: HashSet<u64>,
    sequence: u64,
    nested: bool,
    failure: Option<String>,
    reported: bool,
}
impl Writer {
    fn append(&mut self, value: &impl Serialize) {
        if self.failure.is_some() {
            return;
        }
        let result = serde_json::to_vec(value).map_err(std::io::Error::other).and_then(|mut bytes| {
            bytes.push(b'\n');
            self.sink.write_all(&bytes)
        });
        if let Err(e) = result {
            self.failure = Some(format!("Command journal could not be written: {e}"));
        }
    }
}

/// The current thread's application session. Drop leaves a crash journal; `close` removes it.
pub struct Session {
    path: PathBuf,
    remove: fn(&Path),
}
impl Session {
    /// Starts an always-on journal in `root`. Nested sessions are rejected.
    pub fn start(root: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(root)?;
        let path = root.join(format!("{}.jsonl", crate::DocumentId::new()));
        let file = private_file(&path)?;
        #[cfg(not(target_arch = "wasm32"))]
        fs2::FileExt::try_lock_exclusive(&file)?;
        Self::install(path, Sink::File(file), |path| {
            let _ = std::fs::remove_file(path);
        })
    }

    /// Starts a session over a synchronous browser store. Its owner must hold a
    /// browser lock until clean close or page termination.
    pub fn start_external(path: PathBuf, writer: impl Write + 'static, remove: fn(&Path)) -> std::io::Result<Self> {
        Self::install(path, Sink::External(Box::new(writer)), remove)
    }

    fn install(path: PathBuf, sink: Sink, remove: fn(&Path)) -> std::io::Result<Self> {
        CURRENT.with(|current| {
            let mut current = current.borrow_mut();
            if current.is_some() {
                return Err(std::io::Error::other("a command journal is already active"));
            }
            let mut writer = Writer {
                sink,
                path: path.clone(),
                documents: HashSet::new(),
                sequence: 0,
                nested: false,
                failure: None,
                reported: false,
            };
            writer.append(&json!({"format":"soliddraft-journal/1"}));
            if let Some(error) = &writer.failure {
                return Err(std::io::Error::other(error.clone()));
            }
            *current = Some(writer);
            Ok(Self { path, remove })
        })
    }
    /// The local replay journal, not suitable for sharing without `export`.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Records deterministic execution policy before the first document or command.
    pub fn configure(&self, settings: Value) -> std::io::Result<()> {
        CURRENT.with(|current| {
            let mut current = current.borrow_mut();
            let writer = current
                .as_mut()
                .filter(|w| w.path == self.path)
                .ok_or_else(|| std::io::Error::other("journal is not active"))?;
            if !writer.documents.is_empty() || writer.sequence != 0 {
                return Err(std::io::Error::other("journal policy must precede documents"));
            }
            writer.append(&Record::Environment { settings });
            match &writer.failure {
                Some(error) => Err(std::io::Error::other(error.clone())),
                None => Ok(()),
            }
        })
    }
    /// Ends a clean session. Journals with write failures remain for diagnosis.
    pub fn close(self) {
        CURRENT.with(|current| {
            if let Some(writer) = current.borrow_mut().take() {
                let failed = writer.failure.is_some();
                drop(writer);
                if !failed {
                    (self.remove)(&self.path);
                }
            }
        });
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        CURRENT.with(|current| {
            if current.borrow().as_ref().is_some_and(|w| w.path == self.path) {
                current.borrow_mut().take();
            }
        });
    }
}

/// One readable journal record.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case")]
pub enum Record {
    /// Deterministic execution policy, never performance observations.
    Environment {
        /// Developer kernel work allowance, when explicitly configured.
        settings: Value,
    },
    /// A document's starting state, stored once, without display caches.
    Document {
        /// Session-local document instance.
        id: u64,
        /// `part` or `drawing`.
        kind: String,
        /// Initial authored state, without display caches.
        state: Value,
    },
    /// Written before entering the command handler.
    Command {
        /// Contiguous command number, not a timestamp.
        sequence: u64,
        /// Session-local document instance.
        document: u64,
        /// The canonical command and its arguments.
        command: Value,
    },
    /// Written only after the command handler returns. Absence identifies the interrupted command.
    Result {
        /// Command number this return belongs to.
        sequence: u64,
        /// The handler's refusal, or `None` on success.
        error: Option<String>,
    },
}

/// Registers a document lazily. Snapshot construction happens only once per instance.
pub fn register(id: &Identity, kind: &str, snapshot: impl FnOnce() -> Value) {
    CURRENT.with(|current| {
        let mut current = current.borrow_mut();
        let Some(w) = current.as_mut().filter(|w| !w.nested) else { return };
        if w.documents.insert(id.0) {
            w.append(&Record::Document { id: id.0, kind: kind.into(), state: snapshot() });
        }
    });
}

/// Starts a command. Nested model calls belong to the outer application command.
pub fn begin(id: &Identity, command: impl FnOnce() -> Value) -> CommandGuard {
    let sequence = CURRENT.with(|current| {
        let mut current = current.borrow_mut();
        let w = current.as_mut().filter(|w| !w.nested)?;
        w.sequence += 1;
        let sequence = w.sequence;
        w.append(&Record::Command { sequence, document: id.0, command: command() });
        w.nested = true;
        Some(sequence)
    });
    CommandGuard(sequence)
}

/// An outer command's completion marker. Unwinding leaves the intent unmatched.
pub struct CommandGuard(Option<u64>);

/// A guard for a speculative document or an inactive session.
pub fn disabled() -> CommandGuard {
    CommandGuard(None)
}
impl CommandGuard {
    /// Records a normal return, preserving the handler's result.
    pub fn finish<T, E: std::fmt::Display>(self, result: Result<T, E>) -> Result<T, E> {
        if let Some(sequence) = self.0 {
            CURRENT.with(|current| {
                if let Some(w) = current.borrow_mut().as_mut() {
                    w.append(&Record::Result { sequence, error: result.as_ref().err().map(ToString::to_string) });
                }
            });
        }
        result
    }
}
impl Drop for CommandGuard {
    fn drop(&mut self) {
        if self.0.is_some() {
            CURRENT.with(|current| {
                if let Some(w) = current.borrow_mut().as_mut() {
                    w.nested = false;
                }
            });
        }
    }
}

/// Returns a persistent storage failure for the app to display.
pub fn failure() -> Option<String> {
    // Keep the writer disabled after failure, so a later record cannot hide a gap.
    CURRENT.with(|current| current.borrow().as_ref().and_then(|w| w.failure.clone()))
}

/// Reports a storage failure once without enabling the failed writer again.
pub fn take_failure() -> Option<String> {
    CURRENT.with(|current| {
        let mut current = current.borrow_mut();
        let w = current.as_mut()?;
        if w.reported {
            return None;
        }
        let error = w.failure.clone()?;
        w.reported = true;
        Some(error)
    })
}

/// Reads complete records. Only a final unterminated write is discarded.
pub fn read(path: &Path) -> Result<Vec<Record>, String> {
    read_from(BufReader::new(File::open(path).map_err(|e| e.to_string())?))
}

fn read_from(mut reader: impl BufRead) -> Result<Vec<Record>, String> {
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line).map_err(|e| e.to_string())?;
    let header: Value = serde_json::from_slice(&line).map_err(|e| e.to_string())?;
    if header["format"] != "soliddraft-journal/1" {
        return Err("unsupported command journal".into());
    }
    let mut records = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line).map_err(|e| e.to_string())? == 0 {
            break;
        }
        if !line.ends_with(b"\n") {
            break;
        }
        records.push(serde_json::from_slice(&line).map_err(|e| format!("journal record {}: {e}", records.len() + 1))?);
    }
    Ok(records)
}

/// Finds journals whose owning process no longer holds the file lock.
pub fn abandoned(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension()?.to_str()? != "jsonl" {
                return None;
            }
            let file = OpenOptions::new().read(true).write(true).open(&path).ok()?;
            #[cfg(not(target_arch = "wasm32"))]
            fs2::FileExt::try_lock_exclusive(&file).ok()?;
            Some(path)
        })
        .collect();
    paths.sort();
    paths
}

/// Writes a report copy, replacing filesystem paths, including relative references
/// and paths in free-text errors. Repeated paths keep the same replacement.
pub fn export(source: &Path, destination: &Path) -> Result<(), String> {
    let bytes = report_bytes(BufReader::new(File::open(source).map_err(|e| e.to_string())?))?;
    let mut file = private_file(destination).map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())
}

/// Makes a path-redacted report from a browser journal's complete byte stream.
pub fn report_bytes(source: impl BufRead) -> Result<Vec<u8>, String> {
    let mut bytes = b"{\"format\":\"soliddraft-journal/1\"}\n".to_vec();
    let mut paths = std::collections::BTreeMap::new();
    for record in read_from(source)? {
        let mut value = serde_json::to_value(record).map_err(|e| e.to_string())?;
        redact(&mut value, &mut paths);
        serde_json::to_writer(&mut bytes, &value).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
    }
    Ok(bytes)
}

fn private_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn replacement(path: &str, paths: &mut std::collections::BTreeMap<String, String>) -> String {
    let next = paths.len() + 1;
    paths.entry(path.into()).or_insert_with(|| format!("<path-{next}>")).clone()
}

fn redact(value: &mut Value, paths: &mut std::collections::BTreeMap<String, String>) {
    match value {
        Value::String(text) => {
            // Canonical MCP commands contain a JSON string, not another object.
            if let Ok(mut nested @ (Value::Object(_) | Value::Array(_))) = serde_json::from_str(text) {
                redact(&mut nested, paths);
                *text = nested.to_string();
                return;
            }
            // Format identifiers are not relative filesystem paths.
            if (text.starts_with("soliddraft-") || text.starts_with("catallax-"))
                && text.rsplit_once('/').is_some_and(|(_, version)| version.chars().all(|c| c.is_ascii_digit()))
            {
                return;
            }
            let chars: Vec<char> = text.chars().collect();
            let mut out = String::new();
            let mut i = 0;
            while i < chars.len() {
                let quote = i.checked_sub(1).map(|n| chars[n]).filter(|c| matches!(c, '\'' | '"' | '`'));
                let boundary = i == 0 || quote.is_some() || matches!(chars[i - 1], ' ' | '\t' | '\n' | '(' | '=');
                let mut end = i;
                if boundary {
                    while end < chars.len()
                        && !matches!(chars[end], '\n' | '\r' | '\'' | '"' | '`')
                        && (i == 0 || quote.is_some() || !chars[end].is_whitespace())
                    {
                        end += 1;
                    }
                }
                let token: String = chars[i..end].iter().collect();
                let separator = token.find(['/', '\\']);
                let path_like = separator.is_some_and(|at| !token[..at].chars().any(char::is_whitespace));
                if path_like {
                    out.push_str(&replacement(&token, paths));
                    i = end;
                } else {
                    out.push(chars[i]);
                    i += 1;
                }
            }
            *text = out;
        }
        Value::Array(values) => values.iter_mut().for_each(|value| redact(value, paths)),
        Value::Object(values) => {
            if values.contains_key("pinned")
                && values.contains_key("kind")
                && let Some(Value::String(path)) = values.get_mut("path")
            {
                *path = replacement(path, paths);
            }
            values.values_mut().for_each(|value| redact(value, paths));
        }
        _ => {}
    }
}
