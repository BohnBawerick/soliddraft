//! Subassembly command registry rows.

use crate::commands::{Agents, CommandSpec, Undo};
use crate::icons::Icon;
use serde_json::Value;
use soliddraft_assistant::tools::schema::{non_empty_array, string};

fn form_params() -> Vec<(&'static str, Value)> {
    vec![
        (
            "paths",
            non_empty_array(
                string("A part occurrence's path, such as \"4\""),
                "The root's own parts to form, in order",
            ),
        ),
        ("path", string("The new assembly file, ending in .dwasm; it must not exist")),
        ("origin", string("One of paths: its placement becomes the new file's frame")),
    ]
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(
            "assembly_form_subassembly",
            "Form subassembly",
            Icon::Tree,
            "Move the selected parts into a new assembly file placed where they were",
        )
        .assembly()
        .agents(
            Agents::Mcp,
            "Writes the new file without replacing any, keeps every mate and reference and \
             the free parts' freedom, and changes the root in one undo step; the root's \
             own file is written when it is saved.",
            form_params,
        )
        .undo(Undo::Step),
    ]
}
