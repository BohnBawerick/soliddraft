//! The three configuration verbs share the proposal and dry-run route; the table's
//! tools hold a preview for the session and commit nothing until Apply.
use super::{panel::GridAction, tools};
use crate::{
    commands::{Agents, CommandSpec, Size, Undo},
    icons::Icon,
    state::{AppAction, RibbonTab},
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
        cmd("configuration_grid", "Configuration grid", Icon::Config, "Edit configurations as one table")
            .on(RibbonTab::View, "Panels", Size::Small)
            .run(|_| AppAction::Grid(GridAction::Toggle))
            .lit(|s| s.grid.open)
            .agents(
                Agents::Mcp,
                "Reads the live table: columns, every row's stable id and authored cells, what each row resolves to, \
                 and the accepted state. A blank cell inherits.",
                Vec::new,
            ),
        cmd(
            "preview_configuration_grid",
            "Preview configuration grid",
            Icon::Config,
            "Check a candidate table without changing the part",
        )
        .agents(
            Agents::Mcp,
            "Takes a CSV or XLSX file, or columns and rows as configuration_grid lists them. Returns a preview_id \
             bound to this document and revision, the full candidate and its changes.",
            tools::preview_params,
        ),
        cmd(
            "cancel_configuration_grid",
            "Cancel configuration grid",
            Icon::Config,
            "Discard a preview so it cannot be applied",
        )
        .agents(Agents::Mcp, "Changes nothing in the part.", tools::preview_id_params),
        cmd("apply_configuration_grid", "Apply configuration grid", Icon::Config, "Publish a previewed table")
            .agents(
                Agents::Mcp,
                "Rebuilds the selected row first; a refusal, an edit since the preview or a used preview changes \
                 nothing.",
                tools::preview_id_params,
            )
            .undo(Undo::Step),
        cmd(
            "export_configuration_grid",
            "Export configuration grid",
            Icon::Config,
            "Write the accepted table as CSV or XLSX",
        )
        .agents(
            Agents::Mcp,
            "Keeps blank cells blank, so inherited rows stay inherited.",
            tools::export_params,
        ),
    ]
}
