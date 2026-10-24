//! `soliddraft run` runs registry commands without a window: one command, or a script
//! of them, through the tools the MCP server serves, so each has the same id, the same
//! arguments, the same checks and the same undo steps as there and in the app.

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::{Value, json};
use soliddraft_app::app_tools::AppTools;
use soliddraft_app::commands::{self, Agents};
use soliddraft_app::document::Document;
use soliddraft_app::state::AppState;
use soliddraft_assistant::Caller;
use soliddraft_assistant::tools::call_checked;

pub const USAGE: &str = "\
usage: soliddraft run [--demo | --open <file>] [--keep-going] <command> [<arguments>]
       soliddraft run [--demo | --open <file>] [--keep-going] --script <file | ->

Runs commands of the registry without a window, on a part that starts empty, the
sample part with --demo, or the part or drawing --open names. Each command is a tool
of the MCP server: the same id, the same JSON arguments checked against the same
schema, and the same undo steps. `soliddraft commands --json` lists every command
with its parameters, and says why a command that needs the window does.

A script holds one command per line: the id, then its arguments as one JSON object,
left out when the command takes none. Blank lines and lines starting with # are
skipped. Each result prints as one line of JSON. Nothing is saved unless a command
says so, such as save or drawing_save.
  --demo        start from the sample part
  --open        start from a .dwprt part, or a .dwdrw drawing with its part
  --script      read the commands from a file, or from standard input with -
  --keep-going  run the rest of a script after a command fails

Exit status: 0 when every command ran, 1 when one failed, and 2 for a usage error, a
command that does not exist, or one that needs the window or cannot run yet.
";

/// Why a command did not run.
enum Failure {
    /// The tool ran and refused, or failed.
    Failed(String),
    /// Not a command `run` can run: unknown, app only, waiting, or a bad line.
    Refused(String),
}

impl Failure {
    fn code(&self) -> u8 {
        match self {
            Failure::Failed(_) => 1,
            Failure::Refused(_) => 2,
        }
    }

    fn text(&self) -> &str {
        match self {
            Failure::Failed(t) | Failure::Refused(t) => t,
        }
    }
}

struct Options {
    demo: bool,
    open: Option<PathBuf>,
    script: Option<String>,
    keep_going: bool,
    words: Vec<String>,
}

fn options(args: &[String]) -> Result<Options, String> {
    let mut o = Options { demo: false, open: None, script: None, keep_going: false, words: Vec::new() };
    let mut i = 0;
    while i < args.len() {
        let value = |name: &str| args.get(i + 1).cloned().ok_or(format!("{name} needs a value"));
        match args[i].as_str() {
            "--demo" => o.demo = true,
            "--keep-going" => o.keep_going = true,
            "--open" => {
                o.open = Some(PathBuf::from(value("--open")?));
                i += 1;
            }
            "--script" => {
                o.script = Some(value("--script")?);
                i += 1;
            }
            flag if flag.starts_with("--") && o.words.is_empty() => return Err(format!("unknown option {flag}")),
            word => o.words.push(word.to_owned()),
        }
        i += 1;
    }
    if o.demo && o.open.is_some() {
        return Err("--demo and --open start from different parts; give one".into());
    }
    match (&o.script, o.words.len()) {
        (Some(_), 0) | (None, 1 | 2) => Ok(o),
        (Some(_), _) => Err("--script reads the commands from the file; give no command beside it".into()),
        (None, 0) => Err("name a command, or a script with --script".into()),
        (None, _) => Err("a command takes its arguments as one JSON object".into()),
    }
}

/// The lines to run, with their line numbers.
fn script_lines(source: &str) -> Result<Vec<(usize, String)>, String> {
    let text = if source == "-" {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text).map_err(|e| format!("standard input: {e}"))?;
        text
    } else {
        std::fs::read_to_string(source).map_err(|e| format!("{source}: {e}"))?
    };
    Ok(text
        .lines()
        .enumerate()
        .map(|(n, l)| (n + 1, l.trim().to_owned()))
        .filter(|(_, l)| !l.is_empty() && !l.starts_with('#'))
        .collect())
}

/// Runs one command line: an id and its JSON arguments.
fn run_line(host: &mut AppTools<'_>, line: &str, caller: &Caller) -> Result<Value, Failure> {
    let (id, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    let arguments: Value = if rest.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(rest).map_err(|e| Failure::Refused(format!("{id}: the arguments are not JSON: {e}")))?
    };
    let command = commands::get(id)
        .ok_or_else(|| Failure::Refused(format!("no command `{id}`; `soliddraft commands` lists them")))?;
    match command.agents {
        Agents::App(why) => return Err(Failure::Refused(format!("{id} runs only in the app: {why}"))),
        Agents::None => {
            let why = command.hint_now(host.state).unwrap_or_default();
            return Err(Failure::Refused(format!("{id} cannot run yet: {why}")));
        }
        Agents::Tool | Agents::Verb | Agents::Mcp => {}
    }
    let out = call_checked(host, id, arguments, caller);
    if out.is_error {
        return Err(Failure::Failed(format!("{id}: {}", out.content)));
    }
    let result = serde_json::from_str(&out.content).unwrap_or(Value::String(out.content));
    Ok(json!({ "command": id, "result": result }))
}

pub fn run(args: &[String]) -> ExitCode {
    let o = match options(args) {
        Ok(o) => o,
        Err(e) => {
            eprint!("run: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let lines = match &o.script {
        Some(source) => match script_lines(source) {
            Ok(lines) => lines,
            Err(e) => {
                eprintln!("run: {e}");
                return ExitCode::from(2);
            }
        },
        None => vec![(0, o.words.join(" "))],
    };
    let mut doc = Document::new("Part1", soliddraft_app::kernel());
    if o.demo {
        soliddraft_app::populate_demo(&mut doc);
    }
    let mut state = AppState::new(doc);
    let mut host = AppTools { state: &mut state };
    let caller = Caller::Mcp { client: "soliddraft run".into() };
    if let Some(path) = &o.open {
        let out = call_checked(&mut host, "open", json!({ "path": path }), &caller);
        if out.is_error {
            eprintln!("run: cannot open {}: {}", path.display(), out.content);
            return ExitCode::FAILURE;
        }
    }
    let mut code = 0;
    for (n, line) in lines {
        match run_line(&mut host, &line, &caller) {
            Ok(v) => println!("{v}"),
            Err(f) => {
                let at = if n == 0 { String::new() } else { format!("line {n}: ") };
                eprintln!("run: {at}{}", f.text());
                code = code.max(f.code());
                if !o.keep_going {
                    break;
                }
            }
        }
    }
    ExitCode::from(code)
}
