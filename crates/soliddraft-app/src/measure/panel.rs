//! Picked measurement and tracked value rows in the property panel.

use super::hover::{Pick, Proven};
use crate::{
    document::{Command, Selection},
    state::AppState,
    theme,
};
use serde_json::Value;
use soliddraft_kernel_api::{EdgeId, PairQuantity};

pub struct State {
    pub open: bool,
    cached: Option<Cached>,
    name: String,
    check: String,
    error: Option<String>,
    /// Up to two picks, in order. A third replaces the second.
    pub picks: Vec<Pick>,
    /// The scalar asked of two picks.
    pub quantity: PairQuantity,
    /// The selection seen last frame, so a selection made elsewhere restarts the picks.
    seen: Option<Selection>,
    /// The hovered edge's proven midpoint the pointer is on this frame.
    pub target: Option<(EdgeId, [f64; 3])>,
    pub(super) proven: Vec<Proven>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            open: false,
            cached: None,
            name: String::new(),
            check: String::new(),
            error: None,
            picks: Vec::new(),
            quantity: PairQuantity::MinimumDistance,
            seen: None,
            target: None,
            proven: Vec::new(),
        }
    }
}

impl State {
    /// Adds a viewport pick. Two midpoints ask for their distance; otherwise the minimum.
    pub fn pick(&mut self, pick: Pick) {
        if self.picks.len() == 2 {
            self.picks[1] = pick;
        } else {
            self.picks.push(pick);
        }
        if let [a, b] = self.picks[..] {
            self.quantity =
                if a.midpoint && b.midpoint { PairQuantity::PointDistance } else { PairQuantity::MinimumDistance };
        }
    }
}

/// What the panel proves. Switching quantity or picks never reuses another request's number.
#[derive(Clone, Copy, PartialEq)]
enum Request {
    Single(Selection),
    Midpoint(Selection),
    Pair(Selection, Selection, PairQuantity),
}

struct Cached {
    request: Request,
    geometry: soliddraft_document::ContentHash,
    result: Result<Value, String>,
    /// A picked midpoint's coordinates, shown beside its edge's length.
    point: Option<Result<Value, String>>,
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

const QUANTITIES: [(PairQuantity, &str); 3] = [
    (PairQuantity::MinimumDistance, "Minimum distance"),
    (PairQuantity::PointDistance, "Midpoint distance"),
    (PairQuantity::DirectionAngle, "Direction angle"),
];

/// The request the current picks make, after following a selection made elsewhere.
fn request(state: &mut AppState) -> Option<Request> {
    let selection = state.doc.selection;
    let m = &mut state.measure;
    if selection != m.seen {
        m.seen = selection;
        match selection {
            Some(s @ (Selection::Edge(_) | Selection::Face(_))) => {
                if !m.picks.iter().any(|p| p.selection == s) {
                    m.picks = vec![Pick { selection: s, midpoint: false }];
                }
            }
            _ => m.picks.clear(),
        }
    }
    match m.picks[..] {
        [a, b] => Some(Request::Pair(a.selection, b.selection, m.quantity)),
        [a] if a.midpoint => Some(Request::Midpoint(a.selection)),
        [a] => Some(Request::Single(a.selection)),
        _ => None,
    }
}

fn prove(state: &AppState, request: Request) -> Result<Value, String> {
    let doc = &state.doc;
    match request {
        Request::Single(s) | Request::Midpoint(s) => super::measure(doc, s),
        Request::Pair(a, b, q) => super::pair(doc, a, b, q).and_then(|source| super::measure_pair(doc, &source)),
    }
}

fn track(state: &mut AppState, request: Request) {
    let name = if state.measure.name.trim().is_empty() {
        let mut n = 1;
        let values = super::values(&state.doc);
        while values.as_array().is_some_and(|rows| rows.iter().any(|r| r["name"] == format!("measure_{n}"))) {
            n += 1;
        }
        format!("measure_{n}")
    } else {
        state.measure.name.trim().into()
    };
    let source = match request {
        Request::Single(s) | Request::Midpoint(s) => super::source(&state.doc, s),
        Request::Pair(a, b, q) => super::pair(&state.doc, a, b, q),
    };
    let check = (!state.measure.check.trim().is_empty()).then(|| state.measure.check.trim().into());
    let result = source.and_then(|source| state.doc.apply(Command::Track { name, source, check }));
    state.measure.error = result.err();
}

pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    if state.measure.open {
        theme::section_label(ui, "Measure");
        if let Some(request) = request(state) {
            let geometry = state.doc.model().geometry_hash();
            if state.measure.cached.as_ref().is_none_or(|c| c.request != request || c.geometry != geometry)
                || state.doc.proof_pending()
            {
                let result = prove(state, request);
                let point = match request {
                    Request::Midpoint(s) => Some(super::midpoint(&state.doc, s)),
                    _ => None,
                };
                state.measure.cached = Some(Cached { request, geometry, result, point });
            }
            if let Request::Pair(..) = request {
                for (i, pick) in state.measure.picks.clone().into_iter().enumerate() {
                    let title = state.doc.describe(pick.selection).map(|d| d.title).unwrap_or_default();
                    let point = if pick.midpoint { " midpoint" } else { "" };
                    ui.label(format!("{}: {title}{point}", if i == 0 { "From" } else { "To" }));
                }
                ui.horizontal_wrapped(|ui| {
                    for (q, label) in QUANTITIES {
                        ui.selectable_value(&mut state.measure.quantity, q, label);
                    }
                });
            }
            match state.measure.cached.as_ref().and_then(|c| c.point.as_ref()) {
                Some(Ok(point)) => {
                    let c = |i: usize| point["coordinates"][i]["value"].as_f64().unwrap_or(f64::NAN);
                    let exact = (0..3).all(|i| point["coordinates"][i]["certainty"] == "exact");
                    let how = if exact { "exact" } else { "bounded" };
                    ui.label(format!("Midpoint {}, {how}", super::hover::format([c(0), c(1), c(2)])));
                }
                Some(Err(error)) => {
                    ui.colored_label(theme::error(), error);
                }
                None => {}
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
                        track(state, request);
                    }
                }
                Some(Err(error)) => {
                    ui.colored_label(theme::error(), error);
                }
                None => {}
            }
        } else {
            ui.label("Pick an edge or a face to measure, or a second one for a distance or angle.");
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
