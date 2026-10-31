//! The three configuration verbs share the proposal and dry-run route.
use super::tools;
use crate::{
    commands::{Agents, CommandSpec, Undo},
    icons::Icon,
};

pub(crate) fn commands() -> Vec<CommandSpec> {
    let cmd = CommandSpec::new;
    vec![
        cmd("add_configuration", "Add configuration", Icon::Config, "Add an inactive configuration row")
            .agents(Agents::Verb, "Adds a row with no overrides and keeps the selected row.", tools::name_params)
            .undo(Undo::Step),
        cmd("set_configuration_value", "Set configuration value", Icon::Config, "Set one named-value override")
            .agents(
                Agents::Verb,
                "Checks the row's units and cycles before applying the override.",
                tools::value_params,
            )
            .undo(Undo::Step),
        cmd("activate_configuration", "Activate configuration", Icon::Config, "Select and rebuild a configuration")
            .agents(
                Agents::Verb,
                "Switches the whole part in one undo step; a failed rebuild leaves it unchanged.",
                tools::name_params,
            )
            .undo(Undo::Step),
    ]
}
