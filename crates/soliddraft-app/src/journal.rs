//! Developer replay of the same command handlers used by the application and MCP.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};
use soliddraft_assistant::{Caller, ToolHost};
use soliddraft_document::journal::{self, Record};
use soliddraft_kernel_api::Kernel;

use crate::command_tools::{CommandTools, ProposalBook, Surface};
use crate::document::Document;
use crate::drawing::document::DrawingDoc;
use crate::drawing::part::PartContext;

thread_local! {
    static FILE_INPUTS: RefCell<Option<VecDeque<Value>>> = const { RefCell::new(None) };
}

struct FileInputs;
impl FileInputs {
    fn enter(inputs: VecDeque<Value>) -> Result<Self, String> {
        FILE_INPUTS.with(|current| {
            let mut current = current.borrow_mut();
            if current.is_some() {
                return Err("nested journal replay".into());
            }
            *current = Some(inputs);
            Ok(Self)
        })
    }
    fn consumed(&self) -> bool {
        FILE_INPUTS.with(|current| current.borrow().as_ref().is_none_or(VecDeque::is_empty))
    }
}
impl Drop for FileInputs {
    fn drop(&mut self) {
        FILE_INPUTS.with(|current| {
            current.borrow_mut().take();
        });
    }
}

fn file_input(kind: &str, path: &Path) -> Option<Result<Value, String>> {
    FILE_INPUTS.with(|current| {
        let mut current = current.borrow_mut();
        let inputs = current.as_mut()?;
        Some((|| {
            let input = inputs.pop_front().ok_or("journal is missing a filesystem input")?;
            if input["kind"] != kind || input["path"] != json!(path) {
                return Err("journal filesystem input does not match the command".into());
            }
            serde_json::from_value::<Result<Value, String>>(input["result"].clone()).map_err(|e| e.to_string())?
        })())
    })
}

/// Opens authored state through the same model loader, capturing it before rebuild.
/// Replay supplies the recorded file input and never reads an original user path.
pub fn open_part(
    path: &Path,
    kernel: Arc<dyn Kernel>,
    source: Option<&soliddraft_model::Document>,
) -> Result<Document, String> {
    let model = match file_input("read_part", path) {
        Some(input) => soliddraft_model::Document::from_journal_state(
            &input.map_err(|e| format!("{}: {e}", path.display()))?,
            kernel,
        )?,
        None => {
            let mut captured = false;
            let decoded = std::fs::read(path).map_err(|e| e.to_string()).and_then(|bytes| {
                soliddraft_model::Document::from_bytes_with_input(&bytes, kernel, source, |doc| {
                    captured = true;
                    journal::input(|| {
                        json!({"kind":"read_part","path":path,
                        "result":Ok::<_, String>(doc.journal_state())})
                    });
                })
                .map_err(|e| e.to_string())
            });
            if !captured {
                journal::input(|| {
                    json!({"kind":"read_part","path":path,
                    "result":decoded.as_ref().map(|doc| doc.journal_state())})
                });
            }
            decoded.map_err(|e| format!("{}: {e}", path.display()))?
        }
    };
    let mut doc = Document::from_model(model);
    doc.name = crate::file::part_name(path);
    Ok(doc)
}

/// Serializes normally, but replays the recorded filesystem outcome without a write.
pub fn save_part(doc: &Document, path: &Path) -> Result<(), String> {
    let bytes = doc.model().to_bytes().map_err(|e| format!("cannot save {}: {e}", path.display()))?;
    let result = match file_input("write_part", path) {
        Some(input) => input.map(|_| ()),
        None => {
            let result = soliddraft_document::save_atomic(path, &bytes).map_err(|e| e.to_string());
            journal::input(|| json!({"kind":"write_part","path":path,"result":result.as_ref().map(|_| Value::Null)}));
            result
        }
    };
    result.map_err(|e| format!("cannot save {}: {e}", path.display()))
}

/// Replayed documents, retained even when the final command panics.
#[derive(Default)]
pub struct Replay {
    pub parts: BTreeMap<u64, Document>,
    pub drawings: BTreeMap<u64, DrawingDoc>,
    books: BTreeMap<u64, ProposalBook>,
    closed_parts: BTreeMap<u64, Value>,
    pending_part: Option<Document>,
    pending_book: Option<ProposalBook>,
    pub outcomes: Vec<Value>,
    pub interrupted: Option<u64>,
    pub failure: Option<String>,
}

impl Replay {
    /// Replays complete intents in order. A final intent without a result is executed,
    /// never converted into an artificial crash. A real panic is caught only here,
    /// outside the running application's edit path, to retain the reproduced state.
    pub fn read(path: &Path, mut kernel: Arc<dyn Kernel>) -> Result<Self, String> {
        let records = journal::read(path)?;
        let mut replay = Self::default();
        let mut records = records.into_iter().peekable();
        let mut next_sequence = 1;
        while let Some(record) = records.next() {
            match record {
                Record::Environment { settings } => {
                    if next_sequence != 1 || !replay.parts.is_empty() || !replay.drawings.is_empty() {
                        return Err("journal policy must precede documents".into());
                    }
                    if !settings["max_work"].is_null() {
                        let max_work = settings["max_work"].as_u64().ok_or("invalid journal work allowance")?;
                        kernel = Arc::new(soliddraft_kernel_catallax::CatallaxKernel::with_diagnostics(max_work));
                    }
                }
                Record::Document { id, kind, state } => {
                    if replay.parts.contains_key(&id)
                        || replay.drawings.contains_key(&id)
                        || replay.closed_parts.contains_key(&id)
                    {
                        return Err(format!("duplicate document {id}"));
                    }
                    match kind.as_str() {
                        "part" => {
                            let doc = if let Some(doc) = replay.pending_part.take() {
                                if doc.model().truth() != state["part"] {
                                    return Err("opened part differs from its journal baseline".into());
                                }
                                doc
                            } else {
                                Document::from_model(soliddraft_model::Document::from_journal_state(
                                    &state,
                                    kernel.clone(),
                                )?)
                            };
                            replay.parts.insert(id, doc);
                            if let Some(book) = replay.pending_book.take() {
                                replay.books.insert(id, book);
                            }
                        }
                        "drawing" => {
                            let mut drawing = DrawingDoc::new(
                                serde_json::from_value(state["drawing"].clone()).map_err(|e| e.to_string())?,
                            );
                            drawing.id = serde_json::from_value(state["id"].clone()).map_err(|e| e.to_string())?;
                            replay.drawings.insert(id, drawing);
                        }
                        _ => return Err(format!("unknown journal document kind {kind}")),
                    }
                }
                Record::Command { sequence, document, command } => {
                    if sequence != next_sequence {
                        return Err("journal command sequence is not contiguous".into());
                    }
                    next_sequence += 1;
                    let mut inputs = VecDeque::new();
                    while matches!(records.peek(), Some(Record::Input { .. })) {
                        if let Some(Record::Input { value }) = records.next() {
                            inputs.push_back(value);
                        }
                    }
                    let expected = match records.peek() {
                        Some(Record::Result { sequence: done, .. }) if *done == sequence => {
                            let Some(Record::Result { error, .. }) = records.next() else { unreachable!() };
                            Some(error)
                        }
                        None => {
                            replay.interrupted = Some(sequence);
                            None
                        }
                        _ => return Err(format!("command {sequence} has no result before another record")),
                    };
                    let inputs = FileInputs::enter(inputs)?;
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| replay.apply(document, command)));
                    match result {
                        Ok(result) => {
                            let error = result.err();
                            if let Some(expected) = expected
                                && expected != error
                            {
                                replay.failure =
                                    Some(format!("command {sequence}: expected {expected:?}, got {error:?}"));
                            }
                            replay.outcomes.push(json!({"sequence":sequence, "error":error}));
                        }
                        Err(panic) => {
                            let message = panic
                                .downcast_ref::<&str>()
                                .map(|s| (*s).to_owned())
                                .or_else(|| panic.downcast_ref::<String>().cloned())
                                .unwrap_or_else(|| "non-string panic".into());
                            replay.failure = Some(format!("command {sequence} panicked: {message}"));
                            replay.outcomes.push(json!({"sequence":sequence, "panic":message}));
                        }
                    }
                    if !inputs.consumed() && replay.failure.is_none() {
                        replay.failure = Some(format!("command {sequence} did not consume its filesystem inputs"));
                    }
                    if replay.failure.is_some() {
                        break;
                    }
                }
                Record::Result { .. } => return Err("journal result without a command".into()),
                Record::Input { .. } => return Err("filesystem input without a command".into()),
            }
        }
        Ok(replay)
    }

    fn apply(&mut self, document: u64, command: Value) -> Result<(), String> {
        if let Some(drawing) = self.drawings.get_mut(&document) {
            let part = command["part"].as_u64().and_then(|key| self.parts.get(&key)).and_then(PartContext::new);
            return drawing.run_journal_command(command, part.as_ref());
        }
        let doc = self.parts.get_mut(&document).ok_or("command names an unknown document")?;
        if command["op"] != "tool" {
            return doc.run_journal_command(command);
        }
        let opening = command["name"] == "open";
        let before = opening.then(|| part_report(Some(document), doc));
        let book = self.books.entry(document).or_default();
        book.auto_apply = command["auto_apply"].as_bool().unwrap_or(false);
        let caller = if command["assistant"].as_bool().unwrap_or(false) {
            Caller::Assistant { prompt: String::new(), model: String::new() }
        } else {
            Caller::Mcp { client: "journal replay".into() }
        };
        let drawing = command["drawing"].as_u64().and_then(|key| self.drawings.get_mut(&key));
        let surface = if command["surface"] == "assistant" { Surface::Assistant } else { Surface::Mcp };
        let mut tools = CommandTools { doc, book, surface, drawing };
        let output =
            tools.call(command["name"].as_str().ok_or("missing tool name")?, command["input"].clone(), &caller);
        if opening && !output.is_error {
            self.closed_parts.insert(document, before.expect("an opening command kept its old document"));
            self.pending_part = self.parts.remove(&document);
            self.pending_book = self.books.remove(&document);
        }
        if output.is_error { Err(output.content) } else { Ok(()) }
    }

    /// State and results for development and crash diagnosis. No capture runs per edit.
    pub fn report(&self) -> Value {
        json!({
            "format":"soliddraft-journal-replay/1", "interrupted":self.interrupted, "failure":self.failure,
            "outcomes":self.outcomes,
            "parts":self.closed_parts.values().cloned()
                .chain(self.parts.iter().map(|(id, doc)| part_report(Some(*id), doc)))
                .chain(self.pending_part.as_ref().map(|doc| part_report(None, doc)))
                .collect::<Vec<_>>(),
            "drawings":self.drawings.iter().map(|(id, doc)| json!({"document":id,"drawing":doc.drawing})).collect::<Vec<_>>()
        })
    }
}

fn part_report(id: Option<u64>, doc: &Document) -> Value {
    json!({"document":id, "part":doc.model().truth(),
        "statuses":doc.model().statuses().iter().map(|(id, status)| json!({"id":id,"status":format!("{status:?}")})).collect::<Vec<_>>()})
}
