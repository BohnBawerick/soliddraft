//! Picked measurement and tracked value rows in the property panel.

use crate::{
    document::{Command, Selection},
    state::AppState,
    theme,
};
use serde_json::Value;

#[derive(Default)]
pub struct State {
    pub open: bool,
    cached: Option<Cached>,
    name: String,
    check: String,
    error: Option<String>,
}

struct Cached {
    selection: Selection,
    geometry: soliddraft_document::ContentHash,
    result: Result<Value, String>,
}

/// Failing checks use the application's error colour.
pub fn row_color(state: &str) -> egui::Color32 {
    if state == "fail" { theme::error() } else { theme::ink() }
}

fn number(value: &Value, unit: &Value) -> String {
    match value.as_f64() {
        Some(n) => format!("{} {}", crate::document::fmt_mm(n), unit.as_str().unwrap_or("")),
        None => "No measurement".into(),
    }
}

pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    if state.measure.open {
        theme::section_label(ui, "Measure");
        let selection = state.doc.selection;
        if let Some(selection @ (Selection::Edge(_) | Selection::Face(_))) = selection {
            let geometry = state.doc.model().geometry_hash();
            if state.measure.cached.as_ref().is_none_or(|c| c.selection != selection || c.geometry != geometry)
                || state.doc.proof_pending()
            {
                state.measure.cached =
                    Some(Cached { selection, geometry, result: super::measure(&state.doc, selection) });
            }
            match state.measure.cached.as_ref().map(|c| &c.result) {
                Some(Ok(value)) => {
                    ui.label(number(&value["value"], &value["unit"]));
                    ui.label(value["certainty"].as_str().unwrap_or(""));
                    ui.add(egui::TextEdit::singleline(&mut state.measure.name).hint_text("Value name")).widget_info(
                        || egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Tracked value name"),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut state.measure.check).hint_text("Optional check, e.g. < 12 mm"),
                    )
                    .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Measurement check"));
                    if ui.button("Track this").clicked() {
                        let name = if state.measure.name.trim().is_empty() {
                            let mut n = 1;
                            let values = super::values(&state.doc);
                            while values
                                .as_array()
                                .is_some_and(|rows| rows.iter().any(|r| r["name"] == format!("measure_{n}")))
                            {
                                n += 1;
                            }
                            format!("measure_{n}")
                        } else {
                            state.measure.name.trim().into()
                        };
                        let result = super::source(&state.doc, selection).and_then(|source| {
                            state.doc.apply(Command::Track {
                                name,
                                source,
                                check: (!state.measure.check.trim().is_empty())
                                    .then(|| state.measure.check.trim().into()),
                            })
                        });
                        state.measure.error = result.err();
                    }
                }
                Some(Err(error)) => {
                    ui.colored_label(theme::error(), error);
                }
                None => {}
            }
        } else {
            ui.label("Pick an edge or a face to measure.");
        }
        if let Some(error) = &state.measure.error {
            ui.colored_label(theme::error(), error);
        }
        ui.separator();
    }
    for row in super::values(&state.doc).as_array().into_iter().flatten().filter(|r| r.get("tracked").is_some()) {
        let tracked = &row["tracked"];
        let check = tracked["check"]["state"].as_str().unwrap_or("");
        ui.colored_label(
            row_color(check),
            format!("{}: {}", row["name"].as_str().unwrap_or(""), number(&row["value"], &row["unit"])),
        );
        ui.label(tracked["source"].as_str().unwrap_or(""));
        if tracked["state"] == "stale" {
            ui.label(format!("Stale: {}", tracked["reason"].as_str().unwrap_or("proof pending")));
        } else {
            ui.label(tracked["certainty"].as_str().unwrap_or(""));
        }
        if !check.is_empty() {
            ui.colored_label(row_color(check), format!("Check: {check}"));
        }
        ui.separator();
    }
}
