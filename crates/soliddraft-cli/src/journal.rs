//! Development-only replay and privacy-safe crash-report export.

use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "usage: soliddraft journal replay <journal.jsonl> [--out <report.json>]\n       soliddraft journal export <journal.jsonl> <report.jsonl>\n\nReplay runs commands through the application loop without a window. It reports\ndocuments, refusals, an interrupted command and any reproduced panic. Report exports\nreplace personal paths; review the copy before sharing it. Local journals stay private.\n";

pub fn run(args: &[String]) -> ExitCode {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let result = match args.first().map(String::as_str) {
        Some("export") if args.len() == 3 => {
            soliddraft_document::journal::export(Path::new(&args[1]), Path::new(&args[2])).map(|()| true)
        }
        Some("replay") if args.len() == 2 || (args.len() == 4 && args[2] == "--out") => {
            soliddraft_app::journal::Replay::read(Path::new(&args[1]), soliddraft_app::kernel()).and_then(|replay| {
                let report = serde_json::to_string_pretty(&replay.report()).map_err(|e| e.to_string())?;
                if let Some(out) = args.get(3) {
                    // Replay may describe a crash. Never overwrite an existing report.
                    use std::io::Write;
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(out)
                        .map_err(|e| e.to_string())?;
                    writeln!(file, "{report}").map_err(|e| e.to_string())?;
                }
                println!("{report}");
                Ok(replay.failure.is_none())
            })
        }
        _ => {
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("journal: {error}");
            ExitCode::FAILURE
        }
    }
}
