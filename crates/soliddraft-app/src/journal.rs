//! Developer replay of the same command handlers used by the application and MCP.

use std::collections::BTreeMap;
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

/// Replayed documents, retained even when the final command panics.
#[derive(Default)]
pub struct Replay {
    pub parts: BTreeMap<u64, Document>,
    pub drawings: BTreeMap<u64, DrawingDoc>,
    books: BTreeMap<u64, ProposalBook>,
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
                    if replay.parts.contains_key(&id) || replay.drawings.contains_key(&id) {
                        return Err(format!("duplicate document {id}"));
                    }
                    match kind.as_str() {
                        "part" => {
                            let model = soliddraft_model::Document::from_journal_state(&state, kernel.clone())?;
                            replay.parts.insert(id, Document::from_model(model));
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
                    if replay.failure.is_some() {
                        break;
                    }
                }
                Record::Result { .. } => return Err("journal result without a command".into()),
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
        if output.is_error { Err(output.content) } else { Ok(()) }
    }

    /// State and results for development and crash diagnosis. No capture runs per edit.
    pub fn report(&self) -> Value {
        json!({
            "format":"soliddraft-journal-replay/1", "interrupted":self.interrupted, "failure":self.failure,
            "outcomes":self.outcomes,
            "parts":self.parts.iter().map(|(id, doc)| json!({"document":id, "part":doc.model().truth(),
                "statuses":doc.model().statuses().iter().map(|(id, status)| json!({"id":id,"status":format!("{status:?}")})).collect::<Vec<_>>()
            })).collect::<Vec<_>>(),
            "drawings":self.drawings.iter().map(|(id, doc)| json!({"document":id,"drawing":doc.drawing})).collect::<Vec<_>>()
        })
    }
}
