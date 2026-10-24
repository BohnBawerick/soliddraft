//! `soliddraft commands` lists the command registry: every command of the app and the
//! MCP server, with its id, shortcuts, ribbon place and doc. A query searches it the
//! way the app's command search does. `soliddraft mcp --help` lists the MCP tools from
//! the same table, and `soliddraft run` runs them.

use std::process::ExitCode;

use serde_json::{Value, json};
use soliddraft_app::command_tools::{Surface, tool_specs};
use soliddraft_app::commands::{self, Agents, CommandSpec, History, Workspace};

pub const USAGE: &str = "\
usage: soliddraft commands [--json] [<query>...]

Lists every command of the app and the MCP server, from the command registry that
the ribbon, the keyboard shortcuts, the assistant's verbs and the MCP server read.
Each line gives the id, the shortcuts, the ribbon place, who can run it and what it
does. A query keeps the commands whose label, id, ribbon place or description match
every word, best match first, as the app's command search ranks them.
  --json    print one JSON array with every field, parameters included

Who runs it: `tool` for the assistant and MCP clients; `verb` for a recipe verb, which
MCP clients can also call directly; `mcp` for MCP clients only; `app` for a command that
needs the window, and `later` for one nothing runs yet. `soliddraft run` runs every
command an MCP client can. The JSON says why an `app` command needs the window and what
each command does to the undo stacks.
";

/// The MCP server's help, with its tools from the registry.
pub fn mcp_usage() -> String {
    let mut out = String::from(
        "usage: soliddraft mcp [--demo | --open <part.dwprt>] [--drawing <drawing.dwdrw>] [--http PORT] [--bundle DIR] [--max-work N]\n\n\
         Serves a part's command layer to an MCP client such as Claude Code, on stdin and\n\
         stdout (the default) or at http://127.0.0.1:PORT/mcp. The part starts empty unless\n\
         --demo or --open says otherwise.\n  \
           --bundle  save each tool call, part, recipe and kernel log in a new directory\n  \
           --max-work deterministic work allowance per kernel call in a bundle (default 100000000)\n  \
           --demo    start from the sample part\n  \
           --open    start from a saved part file\n  \
           --drawing open a drawing for drawing_command, beside the part\n  \
           --http    serve over HTTP on PORT instead of stdio\n\nTools:\n",
    );
    for tool in tool_specs(Surface::Mcp) {
        let params = commands::get(&tool.name).map(|c| param_names(c).join(", ")).unwrap_or_default();
        out.push_str(&format!("  {}({params})\n", tool.name));
        out.push_str(&format!("      {}\n", commands::get(&tool.name).map_or("", |c| c.doc)));
    }
    out
}

fn param_names(c: &CommandSpec) -> Vec<&'static str> {
    (c.params)().into_iter().map(|(name, _)| name).collect()
}

fn who(c: &CommandSpec) -> &'static str {
    match c.agents {
        Agents::None => "later",
        Agents::App(_) => "app",
        Agents::Tool => "tool",
        Agents::Verb => "verb",
        Agents::Mcp => "mcp",
    }
}

fn keys(c: &CommandSpec) -> String {
    c.keys.iter().map(|k| k.text()).collect::<Vec<_>>().join(" / ")
}

fn to_json(c: &CommandSpec) -> Value {
    json!({
        "id": c.id,
        "label": c.label,
        "doc": c.doc,
        "details": c.details,
        "app": c.in_app(),
        "workspace": match c.workspace {
            Workspace::Part => "part",
            Workspace::Drawing => "drawing",
            Workspace::Both => "both",
        },
        "agents": who(c),
        "app_only": match c.agents {
            Agents::App(why) => Value::from(why),
            _ => Value::Null,
        },
        "undo": c.undo.label(),
        "ribbon": c.ribbon.iter().map(|p| json!({ "tab": p.tab.label(), "panel": p.panel })).collect::<Vec<_>>(),
        "shortcuts": c.keys.iter().map(|k| json!({ "keys": k.text(), "where": k.scope.label() })).collect::<Vec<_>>(),
        "params": (c.params)().into_iter().map(|(name, schema)| json!({
            "name": name,
            "doc": schema.get("description").cloned().unwrap_or(Value::Null),
        })).collect::<Vec<_>>(),
    })
}

pub fn run(args: &[String]) -> ExitCode {
    let json = args.iter().any(|a| a == "--json");
    let words: Vec<&str> = args.iter().map(String::as_str).filter(|a| *a != "--json").collect();
    if let Some(bad) = words.iter().find(|w| w.starts_with("--")) {
        eprint!("unknown option {bad}\n{USAGE}");
        return ExitCode::from(2);
    }
    let found: Vec<&CommandSpec> = if words.is_empty() {
        commands::all().iter().collect()
    } else {
        commands::search_in(commands::all().iter(), &words.join(" "), &History::default())
            .into_iter()
            .map(|m| m.command)
            .collect()
    };
    if json {
        println!("{}", Value::Array(found.iter().map(|c| to_json(c)).collect()));
        return ExitCode::SUCCESS;
    }
    if found.is_empty() {
        eprintln!("no command matches `{}`", words.join(" "));
        return ExitCode::FAILURE;
    }
    for c in found {
        let place = c.path().unwrap_or_default();
        println!("{:<22} {:<22} {:<24} {:<5} {}", c.id, keys(c), place, who(c), c.doc);
    }
    ExitCode::SUCCESS
}
