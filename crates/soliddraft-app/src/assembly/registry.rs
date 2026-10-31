//! Assembly command registry rows.

use super::*;
use crate::assembly::AssemblyAction;
use soliddraft_assistant::tools::schema::string;

const ASSEMBLY: &str = "It works on the assembly open in the window; soliddraft asm makes, places and saves assembly \
                        files without it";

/// Why an assembly command is off in the browser build, whose loader has no threads.
fn desktop_only(_: &AppState) -> Option<String> {
    cfg!(target_arch = "wasm32").then(|| "Assemblies open in the desktop app".to_owned())
}

fn path_params() -> Vec<(&'static str, Value)> {
    vec![("path", string("An assembly file path ending in .dwasm"))]
}

pub(super) fn registry() -> Vec<CommandSpec> {
    use Size::{Large, Small};
    let ctrl = Modifiers::COMMAND;
    let ctrl_shift = Modifiers::COMMAND | Modifiers::SHIFT;
    let cmd = CommandSpec::new;
    vec![
        // Assemblies. `soliddraft asm` does the same to assembly files from the command line.
        cmd("new_assembly", "New assembly", Icon::Tree, "Start a new, empty assembly in a tab beside the part")
            .both()
            .run(|_| AppAction::Assembly(AssemblyAction::New))
            .when(|_| !cfg!(target_arch = "wasm32"))
            .hint(desktop_only)
            .app_only(ASSEMBLY),
        cmd("make_assembly", "Make assembly", Icon::Tree, "Start an assembly that places this saved part")
            .run(|_| AppAction::Assembly(AssemblyAction::FromPart))
            .when(|s| has_body(s) && !cfg!(target_arch = "wasm32"))
            .hint(desktop_only)
            .agents(Agents::Mcp, "Places the saved, unmodified part at the origin, floating.", Vec::new)
            .undo(Undo::Step),
        cmd(
            "insert_component",
            "Insert component",
            Icon::Cad("insert_comp"),
            "Place a part or an assembly file in the assembly",
        )
        .assembly()
        .on(Tab::Assembly, "Component", Large)
        .key(Scope::Assembly, ctrl, Key::I)
        .run(|_| AppAction::Assembly(AssemblyAction::Insert(None)))
        .app_only(ASSEMBLY),
        cmd(
            "assembly_save",
            "Save assembly",
            Icon::Glyph("save"),
            "Save the assembly with copies of the parts it shows",
        )
        .assembly()
        .on(Tab::Assembly, "File", Small)
        .key(Scope::Assembly, ctrl, Key::S)
        .run(|_| AppAction::Assembly(AssemblyAction::Save(None)))
        .agents(Agents::Mcp, "Saves the assembly and its accepted display copies.", path_params),
        cmd("assembly_save_as", "Save assembly as", Icon::Glyph("save"), "Save the assembly under a new name")
            .assembly()
            .on(Tab::Assembly, "File", Small)
            .key(Scope::Assembly, ctrl_shift, Key::S)
            .run(|_| AppAction::Assembly(AssemblyAction::SaveAs))
            .app_only(ASSEMBLY),
        cmd("show_assembly", "Show assembly", Icon::Tree, "Switch to the open assembly")
            .both()
            .run(|_| AppAction::Assembly(AssemblyAction::Show))
            .when(|s| s.assembly.session.is_some())
            .agents(Agents::Mcp, "Makes the assembly active.", Vec::new),
        cmd("assembly_show_part", "Show part", Icon::Extrude, "Switch to the open part")
            .assembly()
            .run(|_| AppAction::Assembly(AssemblyAction::ShowPart))
            .agents(Agents::Mcp, "Makes the part active.", Vec::new),
        cmd("assembly_update", "Update", Icon::Tree, "Accept the current revisions of every source together")
            .assembly()
            .on(Tab::Assembly, "Component", Large)
            .run(|_| AppAction::Assembly(AssemblyAction::Update))
            .agents(Agents::Mcp, "Keeps all accepted revisions if any source is missing or cannot be shown.", Vec::new)
            .undo(Undo::Step),
    ]
}
