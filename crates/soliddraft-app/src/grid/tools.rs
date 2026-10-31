//! Configuration verbs use the same document commands as other part edits. The table's
//! tools work on the whole app state, because a preview belongs to the session.
use super::exchange::{self, Format};
use super::preview;
use crate::{command_tools::Line, document::Command, state::AppState};
use serde_json::{Value, json};
use soliddraft_assistant::tools::schema::{array, one_of, optional, string};
use std::path::Path;

pub(crate) fn name_params() -> Vec<(&'static str, Value)> {
    vec![("name", json!({"type":"string", "description":"Configuration name"}))]
}

pub(crate) fn value_params() -> Vec<(&'static str, Value)> {
    vec![
        ("configuration", json!({"type":"string", "description":"Non-default configuration name"})),
        ("name", json!({"type":"string", "description":"Existing named value"})),
        ("expression", json!({"type":"string", "description":"Value expression"})),
    ]
}

pub(crate) fn preview_params() -> Vec<(&'static str, Value)> {
    let cells = array(array(string("A cell"), "A row's cells"), "The rows, with columns");
    vec![
        ("path", optional(string("Table file to read, with format"))),
        ("format", optional(one_of(&["csv", "xlsx"], "The file's format"))),
        ("columns", optional(array(string("A header"), "The header cells, instead of a file"))),
        ("rows", optional(cells)),
    ]
}

pub(crate) fn preview_id_params() -> Vec<(&'static str, Value)> {
    vec![("preview_id", string("The id preview_configuration_grid returned"))]
}

pub(crate) fn export_params() -> Vec<(&'static str, Value)> {
    vec![("path", string("The file to write")), ("format", one_of(&["csv", "xlsx"], "The file's format"))]
}

/// Runs the table's tools. `None` hands the call on.
pub(crate) fn run(state: &mut AppState, name: &str, input: &Value) -> Option<Result<Value, String>> {
    let id = || input["preview_id"].as_str().unwrap_or_default();
    Some(match name {
        "configuration_grid" => Ok(read(state)),
        "preview_configuration_grid" => preview_tool(state, input),
        "cancel_configuration_grid" => preview::cancel(state, id()),
        "apply_configuration_grid" => preview::apply(state, id()),
        "export_configuration_grid" => {
            let path = input["path"].as_str().unwrap_or_default();
            Format::parse(input["format"].as_str().unwrap_or_default())
                .and_then(|format| export_file(state, Path::new(path), format))
                .map(|()| json!({ "exported": path }))
        }
        _ => return None,
    })
}

/// The live table and what it takes to compare accepted states.
fn read(state: &AppState) -> Value {
    let doc = &state.doc;
    let model = doc.model();
    let grid = model.configuration_grid();
    let table = exchange::cells(&grid);
    let rows: Vec<Value> = grid
        .rows
        .iter()
        .zip(table.iter().skip(1))
        .map(|(row, cells)| json!({ "id": row.id, "name": row.name, "cells": cells[1..] }))
        .collect();
    json!({
        "columns": table[0],
        "rows": rows,
        "resolved": model.resolve_configuration_grid(&grid),
        "active": model.configuration_summary()["active"],
        "active_id": model.active_configuration(),
        "state": {
            "semantic_revision": model.semantic_hash().as_str(),
            "geometry_hash": model.geometry_hash().as_str(),
            "statuses": doc.features.iter().map(|f| crate::command_tools::status_text(doc, f.id)).collect::<Vec<_>>(),
            "undo_depth": doc.undo_depth(),
            "redo_depth": doc.redo_depth(),
            "undo": doc.undo_label(),
            "redo": doc.redo_label(),
            "dirty": doc.dirty,
            "displayed_revision": doc.displayed_revision(),
            "mesh_version": doc.mesh_version,
        },
    })
}

fn preview_tool(state: &mut AppState, input: &Value) -> Result<Value, String> {
    match (input["path"].as_str(), input["columns"].as_array()) {
        (Some(path), None) => {
            let format = Format::parse(input["format"].as_str().unwrap_or_default())?;
            read_table(state, Path::new(path), format).map(|(_, report)| report)
        }
        (None, Some(columns)) => {
            let text = |v: &Value| v.as_str().map(str::to_owned).ok_or("a cell must be text".to_owned());
            let mut table = vec![columns.iter().map(text).collect::<Result<Vec<_>, _>>()?];
            for row in input["rows"].as_array().ok_or("rows must be an array of rows")? {
                let row = row.as_array().ok_or("each row must be an array of cells")?;
                table.push(row.iter().map(text).collect::<Result<Vec<_>, _>>()?);
            }
            preview::preview(state, &table)
        }
        _ => Err("give either a path and format, or columns and rows".into()),
    }
}

fn read_table(state: &mut AppState, path: &Path, format: Format) -> Result<(Vec<Vec<String>>, Value), String> {
    let file = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    let named = |e: String| format!("{file}: {e}");
    let size = std::fs::metadata(path).map_err(|e| named(e.to_string()))?.len();
    if size > exchange::MAX_BYTES {
        return Err(named(format!("the table file is over {} bytes", exchange::MAX_BYTES)));
    }
    let bytes = std::fs::read(path).map_err(|e| named(e.to_string()))?;
    let table = exchange::decode(format, &bytes).map_err(named)?;
    let report = preview::preview(state, &table).map_err(named)?;
    Ok((table, report))
}

/// Previews a table file for the window, by its extension. Returns its cells for the draft.
pub(crate) fn preview_file(state: &mut AppState, path: &Path) -> Result<Vec<Vec<String>>, String> {
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    read_table(state, path, Format::parse(&extension)?).map(|(table, _)| table)
}

/// Writes the accepted table. No document state changes.
pub(crate) fn export_file(state: &AppState, path: &Path, format: Format) -> Result<(), String> {
    let matches = path.extension().is_some_and(|e| e.eq_ignore_ascii_case(format.extension()));
    if !matches {
        return Err(format!("the path must end in .{}", format.extension()));
    }
    let bytes = exchange::encode(format, &exchange::cells(&state.doc.model().configuration_grid()))?;
    crate::platform::write_file(path, &bytes)
}

pub(crate) fn command(line: &Line) -> Result<Command, String> {
    Ok(match line {
        Line::AddConfiguration { name } => Command::AddConfiguration { name: name.clone() },
        Line::SetConfigurationValue { configuration, name, expression } => Command::SetConfigurationValue {
            configuration: configuration.clone(),
            name: name.clone(),
            expression: expression.clone(),
        },
        Line::ActivateConfiguration { name } => Command::ActivateConfiguration { name: name.clone() },
        _ => return Err("not a configuration command".into()),
    })
}

/// Keep tracked readouts while showing the selected row's named expressions and numbers.
pub(crate) fn values(mut all: Value, active: Value) -> Value {
    if let (Some(rows), Some(active)) = (all.as_array_mut(), active.as_array()) {
        for row in rows {
            if let Some(selected) = active.iter().find(|value| value["name"] == row["name"]) {
                *row = selected.clone();
            }
        }
    }
    all
}
