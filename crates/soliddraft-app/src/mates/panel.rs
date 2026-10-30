//! The Mate panel: two picked faces and a distance, the assembly's values, and what the
//! solver says. Everything goes through the same commands as the MCP tools.

use crate::assembly::AssemblyState;
use crate::mates::tools::rows;
use crate::state::{AppState, NoteKind};
use crate::theme;
use soliddraft_assembly::{Alignment, ComponentId, EndpointSelector, FaceSelector, MateRefusal, OccurrencePath};
use soliddraft_model::FaceRef;

/// A picked face.
#[derive(Clone, Debug, PartialEq)]
pub struct PickedFace {
    pub component: ComponentId,
    pub face: FaceRef,
}

/// The Mate panel's fields.
pub struct Draft {
    pub open: bool,
    pub a: Option<PickedFace>,
    pub b: Option<PickedFace>,
    pub alignment: Alignment,
    pub name: String,
    pub distance: String,
    pub value_name: String,
    pub value_expression: String,
    /// The last refusal, shown until something works.
    pub error: Option<String>,
}

impl Default for Draft {
    fn default() -> Draft {
        Draft {
            open: false,
            a: None,
            b: None,
            alignment: Alignment::Opposed,
            name: String::new(),
            distance: "0 mm".into(),
            value_name: String::new(),
            value_expression: String::new(),
            error: None,
        }
    }
}

/// What the panel asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum MateAction {
    Toggle,
    /// A face was clicked: fills the first free slot.
    Pick(OccurrencePath, String),
    Clear,
    AddDistance,
    AddOrigin(ComponentId),
    SetValue,
    Drive(String, String),
}

fn next_name(state: &AssemblyState, prefix: &str) -> String {
    let taken = |n: &str| state.session.as_ref().is_some_and(|s| s.doc.assembly().known_mates().any(|m| m.name() == n));
    (1..).map(|i| format!("{prefix}{i}")).find(|n| !taken(n)).expect("an unused name")
}

fn report(state: &mut AppState, done: Result<(), MateRefusal>, said: &str) {
    match done {
        Ok(()) => {
            state.assembly.mate.error = None;
            state.note(said.to_owned(), NoteKind::Ok);
        }
        Err(r) => {
            state.assembly.mate.error = Some(r.to_string());
            state.note(r.to_string(), NoteKind::Error);
        }
    }
}

fn pick(state: &mut AppState, path: &OccurrencePath, name: &str) {
    let [step] = path.steps() else {
        state.assembly.mate.error = Some("mates reach the parts placed directly for now".into());
        return;
    };
    let Ok(face) = serde_json::from_str::<FaceRef>(name) else {
        state.assembly.mate.error = Some("this face has no persistent name to mate".into());
        return;
    };
    let picked = PickedFace { component: step.component, face };
    let d = &mut state.assembly.mate;
    d.error = None;
    if d.a.is_none() {
        d.a = Some(picked);
    } else {
        d.b = Some(picked);
    }
}

pub fn apply(state: &mut AppState, action: MateAction) {
    match action {
        MateAction::Toggle => state.assembly.mate.open = !state.assembly.mate.open,
        MateAction::Pick(path, name) => pick(state, &path, &name),
        MateAction::Clear => {
            state.assembly.mate.a = None;
            state.assembly.mate.b = None;
        }
        MateAction::AddDistance => {
            let name = match state.assembly.mate.name.trim() {
                "" => next_name(&state.assembly, "Distance"),
                n => n.to_owned(),
            };
            let d = &state.assembly.mate;
            let (Some(a), Some(b)) = (d.a.clone(), d.b.clone()) else {
                state.assembly.mate.error = Some("Pick two faces first".into());
                return;
            };
            let (distance, alignment) = (d.distance.clone(), d.alignment);
            let end = |p: PickedFace| EndpointSelector { component: p.component, face: FaceSelector::Picked(p.face) };
            let Some(s) = state.assembly.session.as_mut() else { return };
            let done = s.doc.mate_distance(&name, &end(a), &end(b), &distance, alignment, false).map(|_| ());
            report(state, done, "Mate added");
        }
        MateAction::AddOrigin(component) => {
            let name = next_name(&state.assembly, "Origin");
            let Some(s) = state.assembly.session.as_mut() else { return };
            let done = s.doc.mate_origin(&name, component, true, false).map(|_| ());
            report(state, done, "Origin mate added");
        }
        MateAction::SetValue => {
            let (name, expression) =
                (state.assembly.mate.value_name.clone(), state.assembly.mate.value_expression.clone());
            let Some(s) = state.assembly.session.as_mut() else { return };
            let done = s.doc.set_value(name.trim(), &expression);
            report(state, done, "Value set");
        }
        MateAction::Drive(mate, expression) => {
            let Some(s) = state.assembly.session.as_mut() else { return };
            let done = s.doc.drive_mate(&mate, &expression, true);
            report(state, done, "Mate driven");
        }
    }
}

fn labelled(ui: &mut egui::Ui, value: &mut String, hint: &str, name: &'static str) {
    ui.add(egui::TextEdit::singleline(value).hint_text(hint))
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, name));
}

/// The panel. Nothing shows while it is closed.
pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    if !state.assembly.mate.open {
        return;
    }
    let mut actions = Vec::new();
    let selected = state.assembly.session.as_ref().and_then(|s| {
        let path = s.selected.as_ref()?;
        match path.steps() {
            [step] if step.member.is_none() => Some(step.component),
            _ => None,
        }
    });
    let Some(s) = state.assembly.session.as_mut() else { return };
    let (mates, values, solver) = rows(s);
    let warnings: Vec<String> = s
        .components()
        .into_iter()
        .filter_map(|row| crate::assembly::warning(s.doc.store().get(s.doc.table().definition(row))))
        .collect();
    let d = &mut state.assembly.mate;
    ui.add_space(8.0);
    theme::section_label(ui, "Mate");
    let face = |p: &Option<PickedFace>| {
        p.as_ref().map_or("Pick a face".to_owned(), |p| format!("{}: {}", p.component, p.face.label))
    };
    ui.label(format!("First face  {}", face(&d.a)));
    ui.label(format!("Second face  {}", face(&d.b)));
    ui.horizontal(|ui| {
        ui.selectable_value(&mut d.alignment, Alignment::Opposed, "Opposed");
        ui.selectable_value(&mut d.alignment, Alignment::Aligned, "Aligned");
    });
    labelled(ui, &mut d.name, "Name", "Mate name");
    labelled(ui, &mut d.distance, "Distance, such as 5 mm", "Mate distance");
    ui.horizontal(|ui| {
        if ui.button("Add mate").clicked() {
            actions.push(MateAction::AddDistance);
        }
        if ui.button("Clear faces").clicked() {
            actions.push(MateAction::Clear);
        }
        if let Some(c) = selected
            && ui.button("Origin mate").clicked()
        {
            actions.push(MateAction::AddOrigin(c));
        }
    });
    ui.add_space(6.0);
    theme::section_label(ui, "Values");
    for v in values.as_array().into_iter().flatten() {
        ui.label(format!(
            "{} = {} ({} {})",
            v["name"].as_str().unwrap_or(""),
            v["expression"].as_str().unwrap_or(""),
            v["value"],
            v["unit"].as_str().unwrap_or("")
        ));
    }
    labelled(ui, &mut d.value_name, "Name", "Value name");
    labelled(ui, &mut d.value_expression, "Expression, such as 1.2 cm", "Value expression");
    if ui.button("Set value").clicked() {
        actions.push(MateAction::SetValue);
    }
    ui.add_space(6.0);
    theme::section_label(ui, "Mates");
    for m in mates.as_array().into_iter().flatten() {
        let name = m["name"].as_str().unwrap_or("?").to_owned();
        match m["kind"].as_str() {
            Some("distance") => {
                let mm = m["measurement"]["value"].as_f64().or(m["distance"]["value"].as_f64());
                let shown = mm.map_or("unproved".to_owned(), |v| format!("{} mm", crate::document::fmt_mm(v)));
                ui.horizontal(|ui| {
                    ui.label(format!("{name}: {shown}"));
                    let wanted = d.value_name.trim();
                    if !wanted.is_empty() && m["distance"]["expression"].as_str() != Some(wanted) {
                        let drive = ui.button(format!("Drive {name}"));
                        if drive.clicked() {
                            actions.push(MateAction::Drive(name.clone(), wanted.to_owned()));
                        }
                    }
                });
            }
            Some("origin") => {
                ui.label(format!("{name}: on the origin"));
            }
            _ => {
                ui.colored_label(theme::error(), format!("{name}: this version cannot solve it"));
            }
        }
    }
    ui.label(match solver["state"].as_str() {
        Some("solved") => "Fully constrained".to_owned(),
        Some("error") => "The mates cannot be solved".to_owned(),
        _ => format!("{} freedoms remain", solver["remaining_dof"]),
    });
    for w in warnings {
        ui.colored_label(theme::error(), w);
    }
    if let Some(e) = &d.error {
        ui.colored_label(theme::error(), e);
    }
    for a in actions {
        apply(state, a);
    }
}
