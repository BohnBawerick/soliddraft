//! Mate command registry rows.

use crate::assembly::AssemblyAction;
use crate::commands::{Agents, CommandSpec, Size, Tab, Undo};
use crate::icons::Icon;
use crate::state::AppAction;
use serde_json::Value;
use soliddraft_assistant::tools::schema::{
    array, boolean, described, integer, number, object, one_of, optional, string,
};

/// A face of a root component, as a tool takes it.
fn face(what: &str) -> Value {
    described(
        object(&[
            ("component", integer("The component's id: the last number of its path in model_summary")),
            ("face", optional(string("The face's label, such as Boss-Extrude1 · End cap"))),
            ("feature", optional(string("Or the name of the feature that made it"))),
            ("generator", optional(one_of(&["start_cap", "end_cap", "side"], "With feature: which of its faces"))),
            ("normal", optional(array(number("A direction"), "With a side: its outward normal [x, y, z]"))),
        ]),
        what,
    )
}

fn measure_params() -> Vec<(&'static str, Value)> {
    vec![("a", face("The first face")), ("b", face("The second face"))]
}

fn origin_params() -> Vec<(&'static str, Value)> {
    vec![
        ("name", string("A name for the mate")),
        ("component", integer("The component's id")),
        ("align_axes", boolean("true matches its axes to the assembly's too")),
        ("require_fully_constrained", optional(boolean("true refuses a result with any freedom left"))),
    ]
}

const ALIGNMENT: &str = "aligned when the outward normals agree, opposed when they oppose";

fn distance_params() -> Vec<(&'static str, Value)> {
    vec![
        ("name", string("A name for the mate")),
        ("a", face("The face the distance is measured from, along its outward normal")),
        ("b", face("The face the distance is measured to")),
        ("distance", string("A length such as 5 mm or 1.2 cm")),
        ("alignment", one_of(&["aligned", "opposed"], ALIGNMENT)),
        ("require_fully_constrained", optional(boolean("true refuses a result with any freedom left"))),
    ]
}

fn value_params() -> Vec<(&'static str, Value)> {
    vec![("name", string("The value's name")), ("expression", string("An expression such as 5 mm or gap * 2"))]
}

fn drive_params() -> Vec<(&'static str, Value)> {
    vec![
        ("mate", string("The mate's name")),
        ("parameter", one_of(&["distance"], "What to drive")),
        ("expression", string("An expression over assembly values")),
        ("replace", optional(boolean("true replaces the driver the mate has; without it a second driver is refused"))),
    ]
}

fn edit_params() -> Vec<(&'static str, Value)> {
    vec![
        ("mate", string("The mate's name")),
        ("distance", optional(string("A new literal length"))),
        ("alignment", optional(one_of(&["aligned", "opposed"], ALIGNMENT))),
        ("align_axes", optional(boolean("For an Origin mate: whether the axes match too"))),
    ]
}

fn delete_params() -> Vec<(&'static str, Value)> {
    vec![("mate", string("The mate's name"))]
}

fn rename_params() -> Vec<(&'static str, Value)> {
    vec![("name", string("The value's name")), ("to", string("Its new name"))]
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    let row = |id, label, doc| CommandSpec::new(id, label, Icon::Tree, doc).assembly();
    vec![
        row("assembly_mate", "Mate", "Tie two components by a distance between faces")
            .on(Tab::Assembly, "Component", Size::Large)
            .run(|_| AppAction::Assembly(AssemblyAction::Mate(crate::mates::MateAction::Toggle)))
            .app_only("It opens the Mate panel; the assembly_mate tools add mates without it"),
        row("assembly_measure_planes", "Measure planes", "Prove the signed distance between two faces' planes").agents(
            Agents::Mcp,
            "Reads exact plane equations at the accepted revisions and changes nothing.",
            measure_params,
        ),
        row("assembly_mate_origin", "Origin mate", "Put a component's origin on the assembly's")
            .agents(
                Agents::Mcp,
                "An authored mate, not a fixed flag; a partial result is accepted unless strict.",
                origin_params,
            )
            .undo(Undo::Step),
        row("assembly_mate_distance", "Distance mate", "Set the distance between two planar faces")
            .agents(
                Agents::Mcp,
                "Proves, solves and commits together, or refuses and changes nothing.",
                distance_params,
            )
            .undo(Undo::Step),
        row("assembly_set_value", "Set value", "Add or change an assembly value")
            .agents(Agents::Mcp, "Solves everything the value drives in the same step.", value_params)
            .undo(Undo::Step),
        row("assembly_rename_value", "Rename value", "Rename an assembly value")
            .agents(Agents::Mcp, "What reads the value holds its id, so nothing moves.", rename_params)
            .undo(Undo::Step),
        row("assembly_drive_mate", "Drive mate", "Bind a mate's distance to assembly values")
            .agents(Agents::Mcp, "A distance has one driver.", drive_params)
            .undo(Undo::Step),
        row("assembly_edit_mate", "Edit mate", "Change a mate's distance, alignment or axes")
            .agents(Agents::Mcp, "A driven distance is refused: edit the value that drives it.", edit_params)
            .undo(Undo::Step),
        row("assembly_delete_mate", "Delete mate", "Remove a mate and leave the components where they are")
            .agents(Agents::Mcp, "One undo step.", delete_params)
            .undo(Undo::Step),
    ]
}
