//! Previewing, cancelling and applying one candidate table. A preview is session state
//! bound to the document and revision it was made from; it changes no document truth.
//! Apply tries the candidate on a scratch copy first, so a refusal leaves every part of
//! the live document alone, its redo history included.

use super::exchange;
use crate::document::{Command, Document};
use crate::state::AppState;
use serde_json::{Value, json};
use soliddraft_document::{ContentHash, DocumentId};
use soliddraft_model::configs::Grid;
use std::collections::VecDeque;

/// How many finished previews stay known, so a late Apply is told what became of its one.
const RETIRED: usize = 32;
/// How many previews can stay open. Each is bound to the revision it was made from.
const OPEN: usize = 8;

/// The editor's unsaved cells.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    /// The header, then each row.
    pub table: Vec<Vec<String>>,
    /// The Add property field.
    pub new_property: String,
    /// The part's revision when these cells were read.
    pub revision: u64,
    /// Whether anyone has changed them since.
    pub edited: bool,
}

/// A checked candidate waiting for Apply or Cancel.
#[derive(Clone, Debug)]
pub struct Preview {
    pub id: String,
    document: DocumentId,
    revision: ContentHash,
    grid: Grid,
    /// What the panel and the tool show.
    pub report: Value,
}

/// The configuration grid's session state.
#[derive(Clone, Debug, Default)]
pub struct GridState {
    /// Whether the panel is showing.
    pub open: bool,
    pub draft: Option<Draft>,
    /// Open previews, oldest first. Publishing one makes the others stale.
    previews: Vec<Preview>,
    /// The one preview the window shows and applies. Each agent addresses its own by id.
    window: Option<String>,
    /// The panel's last message.
    pub message: Option<String>,
    retired: VecDeque<(String, &'static str)>,
    issued: u64,
}

impl GridState {
    fn retire(&mut self, id: String, how: &'static str) {
        self.retired.push_back((id, how));
        if self.retired.len() > RETIRED {
            self.retired.pop_front();
        }
    }

    /// The window's preview, while it is still open.
    pub fn shown(&self) -> Option<&Preview> {
        let id = self.window.as_ref()?;
        self.previews.iter().find(|p| &p.id == id)
    }

    /// Retires the window's preview, as an edit, Cancel or Close does.
    pub(crate) fn withdraw(&mut self) {
        self.release("cancelled");
    }

    /// Gives the window the preview `report` describes, in place of the one it had.
    pub(crate) fn bind(&mut self, report: &Value) {
        self.release("replaced");
        self.window = report["preview_id"].as_str().map(str::to_owned);
    }

    fn release(&mut self, how: &'static str) {
        if let Some(at) = self.previews.iter().position(|p| Some(&p.id) == self.window.as_ref()) {
            let preview = self.previews.remove(at);
            self.retire(preview.id, how);
        }
        self.window = None;
    }

    /// Takes the open preview `id`, or says what became of it.
    fn take(&mut self, id: &str) -> Result<Preview, String> {
        if let Some(at) = self.previews.iter().position(|p| p.id == id) {
            return Ok(self.previews.remove(at));
        }
        Err(match self.retired.iter().rev().find(|(known, _)| known == id) {
            Some((_, "cancelled")) => format!("{id} was cancelled: expired preview; make a new preview"),
            Some((_, "applied")) => format!("{id} was already applied: consumed preview; make a new preview"),
            Some((_, how)) => format!("{id} was {how}: expired preview; make a new preview"),
            None => format!("no preview {id}; preview_configuration_grid makes one"),
        })
    }
}

/// One cell the candidate changes.
fn diff(live: &Grid, candidate: &Grid) -> Vec<Value> {
    let mut changes = Vec::new();
    let before = exchange::cells(live);
    let after = exchange::cells(candidate);
    // Columns can differ only by a property added or left blank everywhere.
    let lookup = |table: &[Vec<String>], row: &str, column: &str| {
        let at = table[0].iter().position(|h| h == column)?;
        table.iter().skip(1).find(|r| r[0] == row).map(|r| r[at].clone())
    };
    let mut columns = after[0].clone();
    columns.extend(before[0].iter().filter(|h| !after[0].contains(h)).cloned());
    for row in after.iter().skip(1).map(|r| r[0].as_str()) {
        for column in columns.iter().skip(1) {
            let was = lookup(&before, row, column).unwrap_or_default();
            let now = lookup(&after, row, column).unwrap_or_default();
            if was != now {
                changes.push(json!({ "row": row, "column": column, "from": was, "to": now }));
            }
        }
    }
    changes
}

/// Checks `table` as a candidate and holds it as the open preview. A refusal changes nothing.
pub(crate) fn preview(state: &mut AppState, table: &[Vec<String>]) -> Result<Value, String> {
    let model = state.doc.model();
    let grid = exchange::grid_of(table)?;
    let grid = model.check_configuration_grid(&grid).map_err(|e| e.to_string())?;
    let live = model.configuration_grid();
    let changes = diff(&live, &grid);
    let (document, revision) = (model.id(), model.semantic_hash());
    let grid_state = &mut state.grid;
    grid_state.issued += 1;
    let id = format!("grid-{}", grid_state.issued);
    let report = json!({
        "preview_id": id,
        "base_revision": revision.as_str(),
        "columns": exchange::cells(&grid)[0],
        "rows": candidate_rows(&grid),
        "resolved": state.doc.model().resolve_configuration_grid(&grid),
        "active": state.doc.model().configuration_summary()["active"],
        "changes": changes,
    });
    if state.grid.previews.len() == OPEN {
        let old = state.grid.previews.remove(0);
        state.grid.retire(old.id, "replaced");
    }
    state.grid.previews.push(Preview { id, document, revision, grid, report: report.clone() });
    Ok(report)
}

fn candidate_rows(grid: &Grid) -> Vec<Value> {
    let cells = exchange::cells(grid);
    grid.rows
        .iter()
        .zip(cells.iter().skip(1))
        .map(|(row, cells)| json!({ "id": row.id, "name": row.name, "cells": cells[1..] }))
        .collect()
}

/// Discards the preview. The token cannot be used again; the window's cells stay.
pub(crate) fn cancel(state: &mut AppState, id: &str) -> Result<Value, String> {
    let preview = state.grid.take(id)?;
    state.grid.retire(preview.id, "cancelled");
    Ok(json!({ "cancelled": id }))
}

/// Why the part cannot take an edit now, while another one is open.
fn busy(state: &AppState) -> Option<&'static str> {
    if state.drag_preview.is_some() || state.editor.is_some() || state.sketch.is_some() {
        Some("finish the open edit before applying the configuration grid")
    } else {
        None
    }
}

/// Runs the command on a scratch copy of the live part and returns its builds. A feature
/// that built before must still build.
fn dry_run(doc: &Document, command: &Command) -> Result<soliddraft_model::Document, String> {
    let mut scratch = doc.scratch();
    let working: Vec<_> = scratch.features.iter().filter(|f| !crate::command_tools::broken(f)).map(|f| f.id).collect();
    scratch.apply(command.clone())?;
    let failing: Vec<String> = scratch
        .features
        .iter()
        .filter(|f| working.contains(&f.id) && crate::command_tools::broken(f))
        .map(|f| format!("{} ({})", f.name, crate::command_tools::status_text(&scratch, f.id)))
        .collect();
    if !failing.is_empty() {
        let active = doc.model().configuration_summary()["active"].as_str().unwrap_or_default().to_owned();
        return Err(format!("{active} does not rebuild: {}", failing.join("; ")));
    }
    Ok(scratch.into_model())
}

/// Publishes the open preview `id` as one undo step, or refuses and changes nothing.
pub(crate) fn apply(state: &mut AppState, id: &str) -> Result<Value, String> {
    if let Some(reason) = busy(state) {
        return Err(reason.into());
    }
    let from_window = state.grid.window.as_deref() == Some(id);
    let preview = state.grid.take(id)?;
    let model = state.doc.model();
    let stale = if preview.document != model.id() {
        Some(format!("stale preview: {id} belongs to another document"))
    } else if preview.revision != model.semantic_hash() {
        Some(format!("stale preview: the document changed after {id}"))
    } else {
        None
    };
    if let Some(reason) = stale {
        state.grid.retire(preview.id, "stale");
        return Err(reason);
    }
    let command = Command::SetConfigurationGrid { grid: Box::new(preview.grid) };
    let builds = dry_run(&state.doc, &command);
    let outcome = builds.and_then(|builds| {
        state.doc.reuse_builds_from(&builds);
        state.doc.apply(command).map(|_| ())
    });
    let how = if outcome.is_ok() { "applied" } else { "refused" };
    state.grid.retire(preview.id, how);
    outcome?;
    if from_window {
        state.grid.draft = None;
    }
    Ok(json!({
        "applied": id,
        "revision": state.doc.model().semantic_hash().as_str(),
        "changes": preview.report["changes"],
        "undo": state.doc.undo_label(),
    }))
}
