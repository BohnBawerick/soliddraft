//! The command registry is the one list every entry point reads. This enumerates the
//! registry and asks each real entry point which commands it offers: the ribbon, the
//! keyboard shortcuts, the command palette, the assistant, the MCP server, the command
//! line's listing and `soliddraft run`. Each must offer exactly the commands whose
//! registry row says it should, so a command reachable from one entry point is reachable
//! from every entry point its row names, and no entry point keeps a list of its own.
//!
//! A row names its entry points through its metadata: a ribbon place, a shortcut, an
//! action the app runs (the palette's), and who may run it for agents. Every command the
//! app runs is also an MCP tool and runs with `soliddraft run`, unless its row says why it
//! needs the window. A command no entry point offers, a command only the app runs without
//! a reason, or a shortcut bound twice, fails here.

use serde_json::{Value, json};
use soliddraft_app::app_tools::AppTools;
use soliddraft_app::command_tools::{CommandTools, ProposalBook, Surface, tool_specs};
use soliddraft_app::commands::{self, Agents, CommandSpec, Context, History, Tab};
use soliddraft_app::palette;
use soliddraft_app::state::AppState;
use soliddraft_assistant::{Caller, ToolHost};
use std::collections::BTreeSet;
use std::process::Command;

type Ids = BTreeSet<&'static str>;

/// The registry's id for a name an entry point gave back.
fn known(id: &str) -> &'static str {
    commands::get(id).unwrap_or_else(|| panic!("an entry point offers `{id}`, which is not in the registry")).id
}

/// The commands on a ribbon tab, any tab.
fn ribbon() -> Ids {
    Tab::all()
        .into_iter()
        .flat_map(commands::ribbon)
        .flat_map(|(_, items)| items.into_iter().map(|(c, _)| c.id))
        .collect()
}

/// The commands a key does something for, in any context.
fn shortcuts() -> Ids {
    Context::all().into_iter().flat_map(commands::bindings).map(|(c, _)| c.id).collect()
}

/// The commands the palette lists when their label is typed, in a context that offers them.
fn palette() -> Ids {
    let history = History::default();
    let mut found = Ids::new();
    for context in Context::all() {
        for c in commands::all().iter().filter(|c| c.available(context)) {
            if palette::find(c.label, &history, context).iter().any(|m| m.command.id == c.id) {
                found.insert(c.id);
            }
        }
    }
    found
}

/// The recipe verbs the assistant's `propose` tool takes.
fn proposable() -> Ids {
    let propose = tool_specs(Surface::Assistant).into_iter().find(|t| t.name == "propose").expect("a propose tool");
    let lines = &propose.input_schema["properties"]["commands"]["items"]["anyOf"];
    lines
        .as_array()
        .expect("the recipe line union")
        .iter()
        .map(|l| known(l["properties"]["op"]["const"].as_str().unwrap()))
        .collect()
}

fn tools(surface: Surface) -> Ids {
    tool_specs(surface).iter().map(|t| known(&t.name)).collect()
}

/// The commands `soliddraft commands --json` lists.
fn command_line() -> Ids {
    let out =
        Command::new(env!("CARGO_BIN_EXE_soliddraft")).args(["commands", "--json"]).output().expect("run soliddraft");
    assert!(out.status.success());
    let listed: Value = serde_json::from_slice(&out.stdout).unwrap();
    listed.as_array().unwrap().iter().map(|c| known(c["id"].as_str().unwrap())).collect()
}

/// The commands `soliddraft run` runs: a script calls every command of the registry
/// with no arguments, and each that is not refused as unknown, as needing the window or
/// as waiting reached its tool (most then refuse the empty arguments). A command that
/// needs the window is refused with its row's reason.
fn runnable() -> Ids {
    let dir = std::env::temp_dir().join(format!("soliddraft-entry-points-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("every-command.txt");
    let ids: Vec<&str> = commands::all().iter().map(|c| c.id).collect();
    std::fs::write(&script, ids.join("\n")).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_soliddraft"))
        .args(["run", "--keep-going", "--script"])
        .arg(&script)
        .current_dir(&dir)
        .output()
        .expect("run soliddraft");
    let _ = std::fs::remove_dir_all(&dir);
    let errors = String::from_utf8(out.stderr).unwrap();
    let mut refused = BTreeSet::new();
    for line in errors.lines() {
        let Some((n, why)) = line.strip_prefix("run: line ").and_then(|l| l.split_once(": ")) else { continue };
        let c = &commands::all()[n.parse::<usize>().unwrap() - 1];
        if let Agents::App(reason) = c.agents {
            assert_eq!(
                why,
                format!("{} runs only in the app: {reason}", c.id),
                "soliddraft run gives the row's reason"
            );
        }
        let not_run =
            [" runs only in the app: ", " cannot run yet: "].iter().any(|w| why.starts_with(&format!("{}{w}", c.id)));
        if not_run || why.starts_with("no command") {
            refused.insert(c.id);
        }
    }
    ids.into_iter().filter(|id| !refused.contains(id)).collect()
}

fn expected(wanted: fn(&CommandSpec) -> bool) -> Ids {
    commands::all().iter().filter(|c| wanted(c)).map(|c| c.id).collect()
}

fn missing(entry_point: &str, found: &Ids, wanted: &Ids) {
    let lacks: Vec<_> = wanted.difference(found).collect();
    let extra: Vec<_> = found.difference(wanted).collect();
    assert!(
        lacks.is_empty() && extra.is_empty(),
        "{entry_point} lacks {lacks:?} and offers {extra:?} beyond its registry rows"
    );
}

#[test]
fn each_entry_point_offers_what_the_registry_says() {
    missing("the ribbon", &ribbon(), &expected(|c| !c.ribbon.is_empty()));
    missing("the shortcuts", &shortcuts(), &expected(|c| !c.keys.is_empty()));
    missing("the palette", &palette(), &expected(CommandSpec::in_app));
    missing(
        "the assistant",
        &(&tools(Surface::Assistant) | &proposable()),
        &expected(|c| matches!(c.agents, Agents::Tool | Agents::Verb)),
    );
    missing("the MCP server", &tools(Surface::Mcp), &expected(|c| c.agents.has_tool()));
    missing("the command line", &command_line(), &expected(|_| true));
    missing("soliddraft run", &runnable(), &expected(|c| c.agents.has_tool()));
}

#[test]
fn every_command_has_an_entry_point_and_the_command_line_lists_it() {
    let reach = |c: &CommandSpec| c.in_app() || c.agents.has_tool();
    let stranded: Vec<_> = commands::all().iter().filter(|c| !reach(c)).map(|c| c.id).collect();
    assert!(stranded.is_empty(), "no entry point runs {stranded:?}");
}

/// No command is the app's alone without saying why: a command the app runs has a tool
/// for MCP clients and scripts, or its row says why it needs the window. A command
/// nothing runs yet says what it waits for.
#[test]
fn a_command_without_a_tool_says_why() {
    let state = AppState::new(soliddraft_app::new_document());
    let mut unexplained = Vec::new();
    for c in commands::all() {
        match c.agents {
            Agents::Tool | Agents::Verb | Agents::Mcp => {}
            Agents::App(why) => assert!(c.action.is_some() && !why.trim().is_empty(), "{}", c.id),
            Agents::None if c.action.is_some() => unexplained.push(c.id),
            Agents::None => assert!(c.hint_now(&state).is_some(), "{} runs nowhere and does not say why", c.id),
        }
    }
    assert!(
        unexplained.is_empty(),
        "the app runs {unexplained:?}, which have no tool for MCP clients and scripts and do not say why: give \
         each a tool, or `.app_only(why)` when it needs the window"
    );
}

#[test]
fn the_command_line_describes_each_command_as_the_registry_does() {
    let out = Command::new(env!("CARGO_BIN_EXE_soliddraft")).args(["commands", "--json"]).output().unwrap();
    let listed: Vec<Value> = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(listed.len(), commands::all().len());
    for (row, c) in listed.iter().zip(commands::all()) {
        assert_eq!(row["id"], c.id);
        assert_eq!(row["label"], c.label, "{}", c.id);
        assert_eq!(row["doc"], c.doc, "{}", c.id);
        assert_eq!(row["shortcuts"].as_array().unwrap().len(), c.keys.len(), "{}", c.id);
        assert_eq!(row["ribbon"].as_array().unwrap().len(), c.ribbon.len(), "{}", c.id);
        assert_eq!(row["undo"], c.undo.label(), "{}", c.id);
        match c.agents {
            Agents::App(why) => assert_eq!(row["app_only"], why, "{}", c.id),
            _ => assert!(row["app_only"].is_null(), "{}", c.id),
        }
    }
}

/// Every tool an agent is offered reaches its handler; none falls through to `unknown tool`.
#[test]
fn every_agent_tool_has_a_handler() {
    let caller = Caller::Assistant { prompt: "test".into(), model: "test".into() };
    let mut doc = soliddraft_app::new_document();
    let mut book = ProposalBook::default();
    let mut host = CommandTools { doc: &mut doc, book: &mut book, surface: Surface::Assistant, drawing: None };
    for tool in host.tools() {
        let out = host.call(&tool.name, json!({}), &caller);
        assert!(!out.content.starts_with("unknown tool"), "the assistant's {} has no handler", tool.name);
    }
    let caller = Caller::Mcp { client: "test".into() };
    let mut state = AppState::new(soliddraft_app::new_document());
    let mut host = AppTools { state: &mut state };
    for tool in host.tools() {
        let out = host.call(&tool.name, json!({}), &caller);
        assert!(!out.content.starts_with("unknown tool"), "the MCP server's {} has no handler", tool.name);
    }
}

#[test]
fn no_shortcut_is_bound_twice() {
    // The same key and modifiers twice in one scope, even on one command.
    let mut seen = Vec::new();
    for c in commands::all() {
        for k in &c.keys {
            let twin = seen.iter().find(|(_, o): &&(&str, commands::Shortcut)| k.scope == o.scope && k.same_chord(o));
            assert!(
                twin.is_none(),
                "{} binds {} in `{}`, which {} already binds",
                c.id,
                k.text(),
                k.scope.label(),
                twin.map_or("", |t| t.0)
            );
            seen.push((c.id, *k));
        }
    }
    // In every context, a key reaches one command: of the commands bound to it that apply,
    // exactly one is not overridden by a narrower scope.
    for context in Context::all() {
        let applying = commands::bindings(context);
        for (c, k) in &applying {
            let winners: Vec<&str> = applying
                .iter()
                .filter(|(_, o)| {
                    k.same_chord(o) && !applying.iter().any(|(_, n)| k.same_chord(n) && n.scope.narrower_than(o.scope))
                })
                .map(|(o, _)| o.id)
                .collect();
            assert_eq!(winners.len(), 1, "{} in {context:?}: {} reaches {winners:?}", k.text(), c.id);
        }
    }
}
