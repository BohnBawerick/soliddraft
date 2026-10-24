//! The command registry is the one list every entry point reads. This enumerates the
//! registry and asks each real entry point which commands it offers: the ribbon, the
//! keyboard shortcuts, the command palette, the assistant, the MCP server and the command
//! line. Each must offer exactly the commands whose registry row says it should, so a
//! command reachable from one entry point is reachable from every entry point its row
//! names, and no entry point keeps a list of its own.
//!
//! A row names its entry points through its metadata: a ribbon place, a shortcut, an
//! action the app runs (the palette's and the command line's), and who may run it for
//! agents. A command no entry point offers, or a shortcut bound twice, fails here.

use serde_json::{Value, json};
use soliddraft_app::command_tools::{CommandTools, ProposalBook, Surface, tool_specs};
use soliddraft_app::commands::{self, Agents, CommandSpec, Context, History, Tab};
use soliddraft_app::palette;
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
    missing("the MCP server", &tools(Surface::Mcp), &expected(|c| c.agents != Agents::None));
    missing("the command line", &command_line(), &expected(|_| true));
}

#[test]
fn every_command_has_an_entry_point_and_the_command_line_lists_it() {
    let reach = |c: &CommandSpec| c.in_app() || c.agents != Agents::None;
    let stranded: Vec<_> = commands::all().iter().filter(|c| !reach(c)).map(|c| c.id).collect();
    assert!(stranded.is_empty(), "no entry point runs {stranded:?}");
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
    }
}

/// Every tool an agent is offered reaches its handler; none falls through to `unknown tool`.
#[test]
fn every_agent_tool_has_a_handler() {
    for (surface, caller) in [
        (Surface::Assistant, Caller::Assistant { prompt: "test".into(), model: "test".into() }),
        (Surface::Mcp, Caller::Mcp { client: "test".into() }),
    ] {
        let mut doc = soliddraft_app::new_document();
        let mut book = ProposalBook::default();
        let mut host = CommandTools { doc: &mut doc, book: &mut book, surface, drawing: None };
        for tool in host.tools() {
            let out = host.call(&tool.name, json!({}), &caller);
            assert!(!out.content.starts_with("unknown tool"), "{surface:?}: {} has no handler", tool.name);
        }
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
