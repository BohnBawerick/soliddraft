//! The configuration grid window. Its cells are a draft, separate from document truth;
//! Preview, Cancel and Apply are the same steps as the agent tools and the file imports.

use super::exchange::{self, Format};
use super::preview::{self, Draft};
use crate::state::{AppAction, AppState};
use crate::theme;
use egui::RichText;

/// What the window's buttons ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridAction {
    /// Shows the window, or hides it.
    Toggle,
    Close,
    /// Adds the typed text property as a column.
    AddProperty,
    Preview,
    Cancel,
    Apply,
    /// Asks for a table file and previews it.
    Import,
    /// Asks where to write the accepted table.
    Export(Format),
}

/// The editable cells, starting from the accepted table.
fn draft_of(state: &AppState) -> Draft {
    Draft {
        table: exchange::cells(&state.doc.model().configuration_grid()),
        revision: state.doc.revision(),
        ..Draft::default()
    }
}

/// One row of the resolved preview, in words.
fn resolved_line(row: &serde_json::Value) -> String {
    let values = row["values"].as_array().map(Vec::as_slice).unwrap_or_default();
    let mut parts: Vec<String> = values
        .iter()
        .map(|v| {
            let number = crate::document::fmt_mm(v["value"].as_f64().unwrap_or_default());
            format!("{} {number} {}", v["name"].as_str().unwrap_or_default(), v["unit"].as_str().unwrap_or_default())
        })
        .collect();
    let inherited = row["inherited_properties"].as_array().map(Vec::as_slice).unwrap_or_default();
    if let Some(properties) = row["properties"].as_object() {
        for (name, text) in properties {
            let from = if inherited.iter().any(|n| n == name) { " (inherited)" } else { "" };
            parts.push(format!("{name} {}{from}", text.as_str().unwrap_or_default()));
        }
    }
    format!("{}: {}", row["name"].as_str().unwrap_or_default(), parts.join(", "))
}

/// Draws one cell and gives it a name assistive tools and tests can find.
fn cell(ui: &mut egui::Ui, text: &mut String, label: &str, hint: &str) -> bool {
    let response = ui.add(egui::TextEdit::singleline(text).hint_text(hint).desired_width(130.0));
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, label));
    response.changed()
}

/// The window, while it is open.
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    if !state.grid.open {
        return;
    }
    let mut draft = match state.grid.draft.take() {
        // Untouched cells follow the part; typed ones wait for Preview or Cancel.
        Some(draft) if draft.edited || draft.revision == state.doc.revision() => draft,
        _ => draft_of(state),
    };
    let mut actions = Vec::new();
    let mut changed = false;
    let mut open = true;
    let previewing = state.grid.shown().is_some();
    egui::Window::new("Configuration grid").open(&mut open).default_width(640.0).show(ctx, |ui| {
        let Draft { table, new_property, .. } = &mut draft;
        let (header, rows) = table.split_first_mut().expect("the table has a header");
        egui::ScrollArea::both().max_height(320.0).show(ui, |ui| {
            egui::Grid::new("configuration_grid").striped(true).show(ui, |ui| {
                for title in header.iter() {
                    ui.label(RichText::new(title.as_str()).family(theme::semibold()));
                }
                ui.end_row();
                for row in rows.iter_mut() {
                    let (name, cells) = row.split_first_mut().expect("the row has a name");
                    ui.label(name.as_str());
                    let hint = if name == "Default" { "" } else { "inherit" };
                    for (text, title) in cells.iter_mut().zip(&header[1..]) {
                        changed |= cell(ui, text, &format!("{name} {title}"), hint);
                    }
                    ui.end_row();
                }
            });
        });
        ui.horizontal(|ui| {
            cell(ui, new_property, "New property", "property name");
            if ui.add_enabled(!new_property.trim().is_empty(), egui::Button::new("Add property")).clicked() {
                actions.push(GridAction::AddProperty);
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Preview").clicked() {
                actions.push(GridAction::Preview);
            }
            if ui.button("Cancel").clicked() {
                actions.push(GridAction::Cancel);
            }
            if ui.add_enabled(previewing && !changed, egui::Button::new("Apply")).clicked() {
                actions.push(GridAction::Apply);
            }
            ui.separator();
            if ui.button("Import").clicked() {
                actions.push(GridAction::Import);
            }
            if ui.button("Export CSV").clicked() {
                actions.push(GridAction::Export(Format::Csv));
            }
            if ui.button("Export XLSX").clicked() {
                actions.push(GridAction::Export(Format::Xlsx));
            }
        });
        if let Some(preview) = state.grid.shown() {
            ui.separator();
            ui.label(RichText::new(format!("Preview {}: not applied", preview.id)).family(theme::semibold()));
            for row in preview.report["resolved"].as_array().map(Vec::as_slice).unwrap_or_default() {
                ui.label(resolved_line(row));
            }
            let changes = preview.report["changes"].as_array().map(Vec::as_slice).unwrap_or_default();
            for change in changes {
                let word = |key: &str| change[key].as_str().unwrap_or_default().to_owned();
                ui.label(format!("{} {}: {} to {}", word("row"), word("column"), word("from"), word("to")));
            }
        }
        if let Some(message) = &state.grid.message {
            ui.label(RichText::new(message.as_str()).color(theme::muted()));
        }
    });
    if changed {
        draft.edited = true;
        state.grid.withdraw();
    }
    state.grid.draft = Some(draft);
    if !open {
        actions.push(GridAction::Close);
    }
    for action in actions {
        state.push(AppAction::Grid(action));
    }
}

/// Runs what a button asked for.
pub fn apply(state: &mut AppState, action: GridAction) {
    let said = match action {
        GridAction::Toggle => {
            state.grid.open = !state.grid.open;
            if !state.grid.open {
                discard(state);
            }
            None
        }
        GridAction::Close => {
            state.grid.open = false;
            discard(state);
            None
        }
        GridAction::AddProperty => add_property(state),
        GridAction::Preview => {
            let draft = state.grid.draft.clone().unwrap_or_else(|| draft_of(state));
            match preview::preview(state, &draft.table) {
                Ok(report) => {
                    state.grid.bind(&report);
                    Some("Previewed. Nothing has changed yet.".to_owned())
                }
                Err(e) => Some(e),
            }
        }
        GridAction::Cancel => {
            discard(state);
            Some("Cancelled. The part is unchanged.".to_owned())
        }
        GridAction::Apply => {
            let id = state.grid.shown().map(|p| p.id.clone()).ok_or("Preview the table first".to_owned());
            Some(match id.and_then(|id| preview::apply(state, &id)) {
                Ok(_) => "Applied as one undo step.".to_owned(),
                Err(e) => e,
            })
        }
        GridAction::Import => import(state),
        GridAction::Export(format) => export(state, format),
    };
    if said.is_some() {
        state.grid.message = said;
    }
}

/// Forgets the draft and the window's preview of it.
fn discard(state: &mut AppState) {
    state.grid.withdraw();
    state.grid.draft = None;
    state.grid.message = None;
}

fn add_property(state: &mut AppState) -> Option<String> {
    let mut draft = state.grid.draft.take().unwrap_or_else(|| draft_of(state));
    let name = draft.new_property.trim().to_owned();
    let header = format!("property:{name} [text]");
    let said = if draft.table[0].contains(&header) {
        Some(format!("{name} is already a column"))
    } else {
        draft.table[0].push(header);
        draft.table.iter_mut().skip(1).for_each(|row| row.push(String::new()));
        draft.new_property.clear();
        draft.edited = true;
        None
    };
    state.grid.draft = Some(draft);
    state.grid.withdraw();
    said
}

fn import(state: &mut AppState) -> Option<String> {
    // The browser's upload route opens documents; a table needs its own.
    if cfg!(target_arch = "wasm32") {
        return Some("Importing a table in the browser is coming soon".to_owned());
    }
    let path = crate::platform::open_path("Import configuration table", "Configuration table", &["csv", "xlsx"])?;
    Some(match super::tools::preview_file(state, &path) {
        Ok(table) => {
            let revision = state.doc.revision();
            state.grid.draft = Some(Draft { table, revision, edited: true, ..Draft::default() });
            "Previewed the file. Nothing has changed yet.".to_owned()
        }
        Err(e) => e,
    })
}

fn export(state: &mut AppState, format: Format) -> Option<String> {
    let extension = format.extension();
    let path = crate::platform::save_path(
        "Export configuration table",
        "Configuration table",
        extension,
        &format!("configurations.{extension}"),
    )?;
    Some(match super::tools::export_file(state, &path, format) {
        Ok(()) => format!("Exported {}", path.display()),
        Err(e) => e,
    })
}
