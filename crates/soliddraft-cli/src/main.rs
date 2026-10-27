//! `soliddraft`, the headless command line.
//!
//! `soliddraft replay [--json] [--steps] <recipe.json>...` replays each recipe on the
//! Catallax backend and prints the result body's face and edge counts, volume, area,
//! validity and geometric digest. It exits with status 1 if any recipe fails.
//!
//! `soliddraft part [--json] <part.dwprt>...` opens each part file, rebuilds it on the
//! Catallax backend and prints every feature's state, the final body, and the material
//! and mass. It exits with
//! status 1 if a file cannot be read or a feature did not build.
//!
//! `soliddraft view <recipe.json> --dir front --svg out.svg` computes a hidden-line view
//! of a recipe's result and writes it as SVG; see [`view::USAGE`].
//!
//! `soliddraft draw <part.dwprt | recipe.json> --template iso-a3` makes a drawing of a part
//! or a recipe's result and writes it with its SVG, PDF and DXF; `soliddraft draw --update
//! <drawing>` is the drawing's Rebuild; see [`draw::USAGE`].
//!
//! `soliddraft doc info|refs|rebuild|repair|migrate|where-used` reads documents' ids and
//! references, resolves and repairs references, rebuilds drawings and migrates files from
//! before the document envelope; `soliddraft locations` lists and edits File locations;
//! see [`doc::USAGE`].
//!
//! `soliddraft material list|show|appearances|libraries|check|import` reads and imports
//! material and appearance libraries; `soliddraft material info|faces|set|paint|update`
//! reads and changes a part's material, appearances and mass; see [`material::USAGE`].
//!
//! `soliddraft quality <part.dwprt> [--set auto|1-10|chord=MM,...]` reports and sets a part's
//! viewport quality and export accuracy, with each display level's deviation and
//! triangle count; see [`quality::USAGE`].
//!
//! `soliddraft asm new|info|tree|insert|pattern|move|hide|show|suppress|unsuppress|colour|rename|delete|rebuild`
//! makes, inspects and edits assemblies; `asm generate` and `asm bench` are the scale test; see [`asm::USAGE`].
//!
//! `soliddraft mcp [--demo | --open PART.dwprt] [--http PORT]` serves a headless part's command
//! layer to an MCP client, on stdin and stdout or on a loopback HTTP port.
//!
//! `soliddraft export <part.dwprt | recipe.json> --out bracket.step` writes a part or a
//! recipe's result as STEP, STL, 3MF or OBJ with each file's deviation; see [`export::USAGE`].
//!
//! `soliddraft screenshot <part.dwprt> --png out.png` captures the viewport without a window.
//!
//! `soliddraft commands [--json] [QUERY]` lists or searches the command registry that the
//! app, the assistant and the MCP server share; see [`commands::USAGE`].
//!
//! `soliddraft run <command> [JSON]` and `soliddraft run --script FILE` run the registry's
//! commands without a window, as the MCP server's tools; see [`run::USAGE`].
//!
//! `soliddraft key set|delete openrouter|anthropic` manages a provider's API key in the OS
//! keychain; `soliddraft key status` says which keys exist and which provider the assistant uses.

mod asm;
mod bench;
mod commands;
mod doc;
mod draw;
mod evidence;
mod export;
mod generate;
mod journal;
mod material;
mod quality;
mod run;
mod screenshot;
mod view;

use std::process::ExitCode;
use std::sync::Arc;

use soliddraft_kernel_api::Kernel;
use soliddraft_kernel_api::counting::Counting;
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::{Document, FeatureStatus};
use soliddraft_recipe::{BodySummary, ReplayReport, StepOutcome};

const USAGE: &str = "\
usage: soliddraft replay [--json] [--steps] <recipe.json>...
       soliddraft view <recipe.json> [--dir <view>] [--svg <out.svg>] [--json]
       soliddraft screenshot <part.dwprt> --png <out.png>
       soliddraft part [--json] <part.dwprt>...
       soliddraft mcp [--demo | --open <part.dwprt>] [--http PORT]
       soliddraft commands [--json] [<query>...]
       soliddraft journal replay <journal.jsonl> [--out <report.json>]
       soliddraft journal export <journal.jsonl> <report.jsonl>
       soliddraft run [--demo | --open <file>] <command> [<arguments>] | --script <file>
       soliddraft key set|delete openrouter|anthropic
       soliddraft key status
       soliddraft draw <part.dwprt | recipe.json> [--template iso-a3] [--out <file>]...
       soliddraft draw --update <drawing.dwdrw>
       soliddraft export <part.dwprt | recipe.json> --out <file.step|.stl|.3mf|.obj>... [--json]
       soliddraft doc info|refs|rebuild|repair|migrate|where-used <file>...
       soliddraft asm new|info|tree|insert|pattern|move|hide|show|colour|rename|delete|rebuild|generate|bench ...
       soliddraft locations [list | add <kind> <path> | remove <kind> <path>]
       soliddraft material list|show|appearances|libraries|check|import ...
       soliddraft material info|faces|set|paint|update <part.dwprt> ...
       soliddraft quality <part.dwprt> [--set <quality>] [--export <chord>,<angle>] [--json]

replay  Replays recipes on the Catallax kernel and reports, for each result body, its
        face, edge and vertex counts, volume, area, validity and geometric digest.
          --json    print one JSON report per recipe, one per line
          --steps   also report every step, not only the result

view    `soliddraft view --help` describes the view command.

part    Opens SolidDraft part files, rebuilds them on the Catallax kernel and reports
        each feature's state, the final body, and the material and mass.
          --json    print one JSON report per file, one per line

mcp     Serves a part's command layer to an MCP client such as Claude Code, on
        stdin and stdout (the default) or at http://127.0.0.1:PORT/mcp.
        `soliddraft mcp --help` lists the options and the tools.

commands
        Lists or searches every command of the app and the MCP server, from the
        command registry. `soliddraft commands --help` describes it.

run     Runs commands of the registry without a window, one or a script of them, as
        the MCP server's tools. `soliddraft run --help` describes it.

key     The assistant's API keys in the OS keychain, one per provider: OpenRouter or
        Anthropic. `set` reads the named provider's key from standard input;
        `delete` removes its keychain entry; `status` says where each key comes
        from, without printing it, and which provider and model the assistant
        uses. OPENROUTER_API_KEY and ANTHROPIC_API_KEY, when set, take precedence
        over the keychain.

draw    `soliddraft draw --help` describes the draw command.

export  Writes a part or a recipe's result as STEP AP242, STL, 3MF or OBJ, with how far
        each file may lie from the exact part. `soliddraft export --help`
        describes it.

doc     `soliddraft doc --help` describes the doc and locations commands.

material
        `soliddraft material --help` describes the material commands: the material
        and appearance libraries, and a part's material, appearances and mass.

quality `soliddraft quality --help` describes the quality command: a part's
        viewport quality and export accuracy, and each display level's deviation
        and triangle count.

asm     `soliddraft asm --help` describes the assembly commands.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("journal") => journal::run(&args[1..]),
        Some("replay") => replay(&args[1..]),
        Some("view") if args[1..].iter().any(|a| a == "-h" || a == "--help") => {
            print!("{}", view::USAGE);
            ExitCode::SUCCESS
        }
        Some("view") => view::run(&args[1..]),
        Some("part") => part(&args[1..]),
        Some("screenshot") => screenshot::run(&args[1..]),
        Some("mcp") if args[1..].iter().any(|a| a == "-h" || a == "--help") => {
            print!("{}", commands::mcp_usage());
            ExitCode::SUCCESS
        }
        Some("mcp") => mcp(&args[1..]),
        Some("commands") if args[1..].iter().any(|a| a == "-h" || a == "--help") => {
            print!("{}", commands::USAGE);
            ExitCode::SUCCESS
        }
        Some("commands") => commands::run(&args[1..]),
        Some("run") if args[1..].iter().any(|a| a == "-h" || a == "--help") => {
            print!("{}", run::USAGE);
            ExitCode::SUCCESS
        }
        Some("run") => run::run(&args[1..]),
        Some("key") => key(&args[1..]),
        Some("draw") if args[1..].iter().any(|a| a == "-h" || a == "--help") => {
            print!("{}", draw::USAGE);
            ExitCode::SUCCESS
        }
        Some("draw") => draw::run(&args[1..]),
        Some("export") if args[1..].iter().any(|a| a == "-h" || a == "--help") => {
            print!("{}", export::USAGE);
            ExitCode::SUCCESS
        }
        Some("export") => export::run(&args[1..]),
        Some("doc" | "locations") if args[1..].iter().any(|a| a == "-h" || a == "--help") => {
            print!("{}", doc::USAGE);
            ExitCode::SUCCESS
        }
        Some("doc") => doc::run(&args[1..]),
        Some("asm") if args[1..].iter().any(|a| a == "-h" || a == "--help") => {
            print!("{}", asm::USAGE);
            ExitCode::SUCCESS
        }
        Some("asm") => asm::run(&args[1..]),
        Some("locations") => doc::locations(&args[1..]),
        Some("material") if args[1..].iter().any(|a| a == "-h" || a == "--help") => {
            print!("{}", material::USAGE);
            ExitCode::SUCCESS
        }
        Some("material") => material::run(&args[1..]),
        Some("quality") if args[1..].iter().any(|a| a == "-h" || a == "--help") => {
            print!("{}", quality::USAGE);
            ExitCode::SUCCESS
        }
        Some("quality") => quality::run(&args[1..]),
        Some("--version") => {
            let k = CatallaxKernel::new();
            let b = k.backend();
            println!("soliddraft {} ({} {})", env!("CARGO_PKG_VERSION"), b.name, b.version);
            ExitCode::SUCCESS
        }
        Some("-h" | "--help" | "help") => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn replay(args: &[String]) -> ExitCode {
    let mut json = false;
    let mut steps = false;
    let mut files = Vec::new();
    for a in args {
        match a.as_str() {
            "--json" => json = true,
            "--steps" => steps = true,
            s if s.starts_with('-') => {
                eprintln!("unknown option {s}\n{USAGE}");
                return ExitCode::from(2);
            }
            s => files.push(s.to_string()),
        }
    }
    if files.is_empty() {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    }
    let kernel = CatallaxKernel::new();
    let mut ok = true;
    for file in &files {
        let report = std::fs::read_to_string(file)
            .map_err(|e| e.to_string())
            .and_then(|text| soliddraft_recipe::parse(&text).map_err(|e| e.to_string()))
            .and_then(|recipe| soliddraft_recipe::replay(&kernel, &recipe).map_err(|e| e.to_string()));
        match report {
            Ok(r) if json => println!("{}", serde_json::to_string(&r).expect("reports serialise")),
            Ok(r) => print_report(file, &r, steps),
            Err(e) => {
                ok = false;
                eprintln!("{file}: {e}");
            }
        }
    }
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn line(b: &BodySummary) -> String {
    let kind = match b.kind {
        soliddraft_kernel_api::BodyKind::Solid => "solid",
        soliddraft_kernel_api::BodyKind::Sheet => "sheet",
    };
    format!(
        "{kind} faces {} edges {} vertices {} volume {:.4} area {:.4} valid {} digest {}",
        b.faces,
        b.edges,
        b.vertices,
        b.volume,
        b.area,
        if b.valid { "yes" } else { "no" },
        b.digest
    )
}

fn print_report(file: &str, r: &ReplayReport, steps: bool) {
    println!("{} ({file}) on {} {}", r.recipe, r.backend.name, r.backend.version);
    if steps {
        for s in &r.steps {
            match &s.outcome {
                StepOutcome::Ok { body, faces_changed: f, edges_changed: e } => println!(
                    "  {:<14} {:<9} {}  [faces +{} ~{} -{} /{}, edges +{} ~{} -{} /{}]",
                    s.id,
                    s.op.name(),
                    line(body),
                    f[0],
                    f[1],
                    f[2],
                    f[3],
                    e[0],
                    e[1],
                    e[2],
                    e[3]
                ),
                StepOutcome::Profile { profile } => println!(
                    "  {:<14} profile   deferred, {} closed loops, tags {:?}",
                    s.id, profile.loops, profile.tags
                ),
                StepOutcome::ExpectedError { error } => {
                    println!("  {:<14} {:<9} failed as expected, inputs unchanged: {error}", s.id, s.op.name())
                }
            }
        }
    }
    println!("  result {}: {}", r.result_step, line(&r.result));
}

/// A feature state as one word and, for problems, what went wrong.
fn status_text(status: &FeatureStatus) -> (&'static str, String) {
    match status {
        FeatureStatus::Ok => ("ok", String::new()),
        FeatureStatus::Suppressed => ("suppressed", String::new()),
        FeatureStatus::RolledBack => ("rolled_back", String::new()),
        FeatureStatus::Blocked(input) => ("blocked", format!("waits for {input}")),
        FeatureStatus::RepairNeeded(repairs) => (
            "repair_needed",
            repairs.iter().map(|r| format!("{}: {}", r.slot, r.message)).collect::<Vec<_>>().join("; "),
        ),
        FeatureStatus::Failed(message) => ("failed", message.clone()),
    }
}

fn part(args: &[String]) -> ExitCode {
    let mut json = false;
    let mut files = Vec::new();
    for a in args {
        match a.as_str() {
            "--json" => json = true,
            s if s.starts_with('-') => {
                eprintln!("unknown option {s}\n{USAGE}");
                return ExitCode::from(2);
            }
            s => files.push(s.to_string()),
        }
    }
    if files.is_empty() {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    }
    let kernel: Arc<dyn Kernel> = Arc::new(CatallaxKernel::new());
    let mut ok = true;
    for file in &files {
        let doc = match Document::open(file, kernel.clone()) {
            Ok(d) => d,
            Err(e) => {
                ok = false;
                eprintln!("{file}: {e}");
                continue;
            }
        };
        let body = doc.body().map(|b| soliddraft_recipe::summarize(kernel.as_ref(), b));
        let features: Vec<_> = doc
            .features()
            .iter()
            .map(|f| {
                let status = doc.status(f.id).expect("an opened part is rebuilt");
                let (state, detail) = status_text(status);
                ok &= matches!(status, FeatureStatus::Ok | FeatureStatus::Suppressed | FeatureStatus::RolledBack);
                (f, state, detail)
            })
            .collect();
        if let Some(Err(e)) = &body {
            ok = false;
            eprintln!("{file}: {e}");
        }
        let body = body.and_then(Result::ok);
        if json {
            let report = serde_json::json!({
                "part": doc.name(),
                "file": file,
                "backend": kernel.backend(),
                "features": features.iter().map(|(f, state, detail)| serde_json::json!({
                    "id": f.id,
                    "name": f.name,
                    "kind": f.kind.base_name(),
                    "status": state,
                    "detail": detail,
                })).collect::<Vec<_>>(),
                "body": body,
                "material": doc.material(None).map(|m| &m.id),
                "mass_kg": doc.mass(None).ok().map(|m| m.mass_kg),
            });
            println!("{report}");
        } else {
            let b = kernel.backend();
            println!("{} ({file}) on {} {}", doc.name(), b.name, b.version);
            for (f, state, detail) in &features {
                let detail = if detail.is_empty() { String::new() } else { format!(": {detail}") };
                println!("  {:<4} {:<22} {:<12} {state}{detail}", f.id.to_string(), f.name, f.kind.base_name());
            }
            match &body {
                Some(b) => println!("  body: {}", line(b)),
                None => println!("  body: none"),
            }
            match (doc.material(None), doc.mass(None)) {
                (Some(m), Ok(mass)) => println!("  material: {}, mass {:.6} kg", m.name, mass.mass_kg),
                (Some(m), Err(e)) => println!("  material: {}, mass unknown ({e})", m.name),
                (None, _) => println!("  material: not set"),
            }
        }
    }
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn mcp(args: &[String]) -> ExitCode {
    use soliddraft_app::app_tools::AppTools;
    use soliddraft_app::command_tools::MCP_INSTRUCTIONS;
    use soliddraft_assistant::mcp::{HttpServer, McpSession, serve_stdio};

    let mut bundle = None;
    let mut max_work = 100_000_000;
    let mut demo = false;
    let mut open = None;
    let mut drawing_path = None;
    let mut http = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--drawing" => {
                drawing_path = args.get(i + 1).map(std::path::PathBuf::from);
                if drawing_path.is_none() {
                    eprintln!("--drawing needs a drawing file");
                    return ExitCode::from(2);
                }
                i += 1;
            }
            "--bundle" => {
                bundle = args.get(i + 1).map(std::path::PathBuf::from);
                if bundle.is_none() {
                    eprintln!("--bundle needs a new directory");
                    return ExitCode::from(2);
                }
                i += 1;
            }
            "--max-work" => {
                let Some(value) = args.get(i + 1).and_then(|v| v.parse::<u64>().ok()) else {
                    eprintln!("--max-work needs an unsigned integer");
                    return ExitCode::from(2);
                };
                max_work = value;
                i += 1;
            }
            "--demo" => demo = true,
            "--open" => {
                open = args.get(i + 1).map(std::path::PathBuf::from);
                if open.is_none() {
                    eprintln!("--open needs a part file\n{USAGE}");
                    return ExitCode::from(2);
                }
                i += 1;
            }
            "--http" => {
                http = args.get(i + 1).and_then(|p| p.parse::<u16>().ok());
                if http.is_none() {
                    eprintln!("--http needs a port\n{USAGE}");
                    return ExitCode::from(2);
                }
                i += 1;
            }
            other => {
                eprintln!("unknown option {other}\n{USAGE}");
                return ExitCode::from(2);
            }
        }
        i += 1;
    }
    if bundle.is_none() && args.iter().any(|a| a == "--max-work") {
        eprintln!("--max-work requires --bundle");
        return ExitCode::from(2);
    }
    let Some(root) = soliddraft_app::settings::documents().recovery_folder() else {
        eprintln!("mcp: no command journal folder is configured");
        return ExitCode::FAILURE;
    };
    let journal = match soliddraft_document::journal::Session::start(&root.join("journals")) {
        Ok(journal) => journal,
        Err(e) => {
            eprintln!("mcp: cannot start command journal: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = journal.configure(serde_json::json!({"max_work":bundle.as_ref().map(|_| max_work)})) {
        eprintln!("mcp: cannot record kernel policy: {e}");
        return ExitCode::FAILURE;
    }
    let evidence_kernel =
        bundle.as_ref().map(|_| Arc::new(soliddraft_kernel_catallax::CatallaxKernel::with_diagnostics(max_work)));
    // A bundle counts the work each command asks of the kernel, outside the application.
    let counting = evidence_kernel.clone().map(|k| Arc::new(Counting::new(k)));
    let kernel: Arc<dyn Kernel> = counting.clone().map(|k| k as Arc<dyn Kernel>).unwrap_or_else(soliddraft_app::kernel);
    let doc = match open {
        Some(path) => match soliddraft_app::file::open(&path, kernel.clone(), None) {
            Ok(doc) => doc,
            Err(e) => {
                eprintln!("mcp: cannot open {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        },
        None if demo => {
            let mut doc = soliddraft_app::document::Document::new("Part1", kernel);
            soliddraft_app::populate_demo(&mut doc);
            doc
        }
        None => soliddraft_app::document::Document::new("Part1", kernel),
    };
    let drawing = match drawing_path {
        Some(path) => match soliddraft_app::drawing::document::DrawingDoc::open(&path) {
            Ok(doc) => Some(doc),
            Err(e) => {
                eprintln!("mcp: {e}");
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };
    let mut state = soliddraft_app::state::AppState::new(doc);
    // The drawing opens beside the part: undo and redo stay with the part until show_drawing.
    if let Some(doc) = drawing {
        let mut session = soliddraft_app::drawing::DrawingSession::new(doc);
        session.seen_version = u64::MAX;
        state.drawing.session = Some(session);
    }
    let mut session = McpSession::new("soliddraft", env!("CARGO_PKG_VERSION"), MCP_INSTRUCTIONS);
    let host = AppTools { state: &mut state };
    let mut host = match evidence::Evidence::new(host, evidence_kernel, counting, bundle, max_work) {
        Ok(host) => host,
        Err(e) => {
            eprintln!("mcp: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = match http {
        None => match serve_stdio(&mut session, &mut host) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("mcp: {e}");
                ExitCode::FAILURE
            }
        },
        Some(port) => {
            let (server, calls) = match HttpServer::start(port, None) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("mcp: cannot listen on 127.0.0.1:{port}: {e}");
                    return ExitCode::FAILURE;
                }
            };
            eprintln!("SolidDraft MCP server: {}", server.url());
            for call in calls {
                let reply = session.handle(&mut host, call.message.clone());
                call.respond(reply);
            }
            ExitCode::SUCCESS
        }
    };
    if let Some(error) = soliddraft_document::journal::failure() {
        eprintln!("mcp: {error}");
        return ExitCode::FAILURE;
    }
    if result == ExitCode::SUCCESS {
        journal.close();
    }
    result
}

fn key(args: &[String]) -> ExitCode {
    use soliddraft_assistant::{ProviderKind, choice, key};
    let provider = |verb: &str| match args.get(1).and_then(|a| ProviderKind::parse(a)) {
        Some(kind) => Ok(kind),
        None => {
            eprintln!(
                "key {verb}: name the provider: `soliddraft key {verb} openrouter` or `soliddraft key {verb} anthropic`"
            );
            Err(ExitCode::from(2))
        }
    };
    match args.first().map(String::as_str) {
        Some("set") => {
            let kind = match provider("set") {
                Ok(k) => k,
                Err(code) => return code,
            };
            let mut text = String::new();
            if let Err(e) = std::io::stdin().read_line(&mut text) {
                eprintln!("key: cannot read standard input: {e}");
                return ExitCode::FAILURE;
            }
            let Some(k) = soliddraft_assistant::ApiKey::new(text) else {
                eprintln!("key: standard input held no key");
                return ExitCode::FAILURE;
            };
            match key::store_in_keychain(kind, &k) {
                Ok(()) => {
                    println!(
                        "stored the {kind} key in the OS keychain ({} / {})",
                        key::KEYCHAIN_SERVICE,
                        key::keychain_user(kind)
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("key: the keychain refused: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("status") => {
            for kind in ProviderKind::ALL {
                match key::find(kind) {
                    Some((_, source)) => println!("{kind}: {}", source.describe()),
                    None => {
                        println!("{kind}: no key: set {} or run `soliddraft key set {}`", kind.key_var(), kind.id())
                    }
                }
            }
            match choice::choose(&soliddraft_app::settings::assistant()) {
                Ok(c) => println!("the assistant uses {} with {}", c.kind, c.model),
                Err(why) => println!("{why}"),
            }
            ExitCode::SUCCESS
        }
        Some("delete") => {
            let kind = match provider("delete") {
                Ok(k) => k,
                Err(code) => return code,
            };
            match key::delete_from_keychain(kind) {
                Ok(()) => {
                    println!("removed the {kind} keychain entry");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("key: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
