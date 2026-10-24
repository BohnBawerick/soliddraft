//! Numeric controls for sketch depth tools. Pointer workflows and previews belong
//! to the interface design lane; these controls exercise the shared operations.

use crate::document::Command;
use crate::sketch::{
    EntityId, P2,
    tools::{End, Operation},
};
use crate::state::{AppAction, SketchSel, SketchSession};
use serde_json::Value;

#[derive(Clone)]
struct Inputs {
    tool: usize,
    center: P2,
    vertex: P2,
    sides: u32,
    distance: f64,
    step: P2,
    count: u32,
    angle: f64,
    start: bool,
}

impl Default for Inputs {
    fn default() -> Self {
        Self {
            tool: 0,
            center: P2::ZERO,
            vertex: P2::new(10.0, 0.0),
            sides: 6,
            distance: -2.0,
            step: P2::new(20.0, 0.0),
            count: 3,
            angle: 90.0,
            start: false,
        }
    }
}

fn point(ui: &mut egui::Ui, label: &str, value: &mut P2) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::DragValue::new(&mut value.u).speed(0.5).prefix("u "));
        ui.add(egui::DragValue::new(&mut value.v).speed(0.5).prefix("v "));
    });
}

/// The controls operate in sketch-plane coordinates, not world coordinates.
pub fn panel(ui: &mut egui::Ui, s: &mut SketchSession, actions: &mut Vec<AppAction>) {
    ui.collapsing("Polygon and modify", |ui| {
        let key = ui.id().with(("sketch_geometry_inputs", s.feature));
        let mut input = ui.data_mut(|d| d.get_temp::<Inputs>(key).unwrap_or_default());
        let names = ["Polygon", "Trim", "Extend", "Offset", "Mirror", "Linear pattern", "Circular pattern"];
        egui::ComboBox::from_id_salt("geometry_tool").selected_text(names[input.tool]).show_ui(ui, |ui| {
            for (i, name) in names.iter().enumerate() { ui.selectable_value(&mut input.tool, i, *name); }
        });
        let mut selected = Vec::<EntityId>::new();
        for pick in &s.selection {
            let id = match pick { SketchSel::Entity(id) => *id, SketchSel::Point(p) => p.entity, _ => continue };
            if !selected.contains(&id) { selected.push(id); }
        }
        ui.small("Coordinates are u/v in the sketch plane, in mm.");
        let operation = match input.tool {
            0 => {
                point(ui, "Centre", &mut input.center);
                point(ui, "First vertex", &mut input.vertex);
                ui.add(egui::DragValue::new(&mut input.sides).range(3..=1000).prefix("Sides "));
                Some(Operation::Polygon { center: input.center, vertex: input.vertex, sides: input.sides })
            }
            1 => {
                ui.small("Select one line or arc. Pick coordinates identify the interval to remove.");
                point(ui, "Pick", &mut input.vertex);
                selected.first().filter(|_| selected.len() == 1).map(|&entity| Operation::Trim { entity, at: input.vertex })
            }
            2 => {
                ui.small("Select the target, then Ctrl-select the boundary.");
                ui.checkbox(&mut input.start, "Extend start instead of end");
                (selected.len() == 2).then(|| Operation::Extend { entity: selected[0], boundary: selected[1], end: if input.start { End::Start } else { End::End } })
            }
            3 => {
                ui.small("Select a connected chain in walking order. Positive is left of its direction. Copies keep the originals.");
                ui.add(egui::DragValue::new(&mut input.distance).speed(0.25).prefix("Distance "));
                (!selected.is_empty()).then(|| Operation::Offset { entities: selected.clone(), distance: input.distance })
            }
            4 => {
                point(ui, "Axis start", &mut input.center);
                point(ui, "Axis end", &mut input.vertex);
                (!selected.is_empty()).then(|| Operation::Mirror { entities: selected.clone(), axis_start: input.center, axis_end: input.vertex })
            }
            5 => {
                point(ui, "Step", &mut input.step);
                ui.add(egui::DragValue::new(&mut input.count).range(2..=1000).prefix("Instances "));
                (!selected.is_empty()).then(|| Operation::LinearPattern { entities: selected.clone(), step: input.step, count: input.count })
            }
            _ => {
                point(ui, "Centre", &mut input.center);
                ui.add(egui::DragValue::new(&mut input.angle).speed(1.0).suffix(" degrees"));
                ui.add(egui::DragValue::new(&mut input.count).range(2..=1000).prefix("Instances "));
                (!selected.is_empty()).then(|| Operation::CircularPattern { entities: selected.clone(), center: input.center, angle_degrees: input.angle, count: input.count })
            }
        };
        if input.tool >= 4 { ui.small("Copies are independent. Instance counts include the seed."); }
        if ui.add_enabled(operation.is_some(), egui::Button::new("Apply sketch tool")).clicked()
            && let Some(operation) = operation
        {
            match s.sketch.apply_operation(&operation) {
                Ok(ids) => {
                    s.selection = ids.into_iter().map(SketchSel::Entity).collect();
                    actions.push(AppAction::Command(Command::EditSketch { id: s.feature, sketch: s.sketch.clone() }));
                    s.say(format!("{} applied", names[input.tool]));
                }
                Err(error) => s.say(error.to_string()),
            }
        }
        ui.data_mut(|d| d.insert_temp(key, input));
    });
}

/// The same operations used by the numeric panel, exposed to command/MCP scenarios.
pub fn operation_schema() -> Value {
    use soliddraft_assistant::tools::schema::{
        any_of, boolean, constant, integer, non_empty_array, number, object, one_of,
    };
    let point = object(&[("u", number("Plane u, mm")), ("v", number("Plane v, mm"))]);
    let id = integer("An existing entity id, greater than 0");
    let ids = non_empty_array(id.clone(), "Distinct entity ids, in selection order");
    let count = integer("Instances including the seed, from 2 to 1000");
    let geometry = any_of(vec![
        object(&[("type", constant("point")), ("p", point.clone())]),
        object(&[("type", constant("line")), ("a", point.clone()), ("b", point.clone())]),
        object(&[
            ("type", constant("arc")),
            ("center", point.clone()),
            ("start", point.clone()),
            ("end", point.clone()),
        ]),
        object(&[("type", constant("circle")), ("center", point.clone()), ("radius", number("Positive radius, mm"))]),
    ]);
    any_of(vec![
        object(&[("tool", constant("add")), ("kind", geometry), ("construction", boolean("Exclude from profiles"))]),
        object(&[
            ("tool", constant("polygon")),
            ("center", point.clone()),
            ("vertex", point.clone()),
            ("sides", integer("Sides from 3 to 1000")),
        ]),
        object(&[("tool", constant("trim")), ("entity", id.clone()), ("at", point.clone())]),
        object(&[
            ("tool", constant("extend")),
            ("entity", id.clone()),
            ("end", one_of(&["start", "end"], "Endpoint to extend")),
            ("boundary", id),
        ]),
        object(&[
            ("tool", constant("offset")),
            ("entities", ids.clone()),
            ("distance", number("Signed distance to the left of the chain, mm")),
        ]),
        object(&[
            ("tool", constant("mirror")),
            ("entities", ids.clone()),
            ("axis_start", point.clone()),
            ("axis_end", point.clone()),
        ]),
        object(&[
            ("tool", constant("linear_pattern")),
            ("entities", ids.clone()),
            ("step", point.clone()),
            ("count", count.clone()),
        ]),
        object(&[
            ("tool", constant("circular_pattern")),
            ("entities", ids),
            ("center", point),
            ("angle_degrees", number("Signed angle per instance, degrees")),
            ("count", count),
        ]),
    ])
}
