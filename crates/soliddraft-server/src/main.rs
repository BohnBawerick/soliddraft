//! `soliddraft-server`: serves the web build of SolidDraft. See the library docs.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use soliddraft_assistant::AssistantSettings;
use soliddraft_kernel_smlib::SmlibKernel;
use soliddraft_server::{Config, Server};

const USAGE: &str = "soliddraft-server [--listen ADDR] [--web DIR] [--session-idle MINUTES]

  --listen ADDR           address to listen on (default 127.0.0.1:8766)
  --web DIR               the web build to serve (default ./web)
  --session-idle MINUTES  release a browser tab's bodies after this long without a heartbeat (default 10)

The assistant uses the provider, key and model the environment gives, as the desktop app
does: OPENROUTER_API_KEY or ANTHROPIC_API_KEY, and SOLIDDRAFT_PROVIDER and SOLIDDRAFT_MODEL
to choose. Set SOLIDDRAFT_NO_KEYCHAIN=1 on a server. Without a key the assistant is off and
everything else works. SOLIDDRAFT_COMMIT names the commit the build was made from, which
GET /version reports.";

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let mut listen = "127.0.0.1:8766".to_owned();
    let mut web_dir = PathBuf::from("web");
    let mut idle_minutes = 10u64;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next().unwrap_or_else(|| {
                eprintln!("{name} needs a value\n{USAGE}");
                std::process::exit(2)
            })
        };
        match arg.as_str() {
            "--listen" => listen = value("--listen"),
            "--web" => web_dir = PathBuf::from(value("--web")),
            "--session-idle" => {
                idle_minutes = value("--session-idle").parse().unwrap_or_else(|_| {
                    eprintln!("--session-idle takes a whole number of minutes");
                    std::process::exit(2)
                })
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            other => {
                eprintln!("unknown argument {other}\n{USAGE}");
                std::process::exit(2);
            }
        }
    }
    if !web_dir.join("index.html").is_file() {
        eprintln!("no index.html in {}; build the web app first (see docs/web.md)", web_dir.display());
        std::process::exit(1);
    }
    // The environment picks the provider, key and model, as for the desktop app without a
    // settings file: SOLIDDRAFT_PROVIDER, SOLIDDRAFT_MODEL, and whichever key is set.
    let provider = soliddraft_assistant::choice::choose(&AssistantSettings::default())
        .map_err(|why| why.to_string())
        .and_then(|choice| {
            let how = choice.source.describe();
            let provider = choice.connect().map_err(|e| format!("The assistant could not start: {e}"))?;
            log::info!("assistant on: {} {} ({how})", provider.name(), provider.model());
            Ok(provider)
        });
    // The browser gets a reason that fits it; the desktop's hints (the keychain) do not.
    let provider = provider.map_err(|reason| {
        log::warn!("{reason}");
        "The assistant is off: the SolidDraft server has no model key.".to_owned()
    });
    let config = Config {
        web_dir,
        kernel: Arc::new(SmlibKernel::new()),
        provider,
        session_idle: Duration::from_secs(idle_minutes * 60),
        commit: soliddraft_assistant::environment::var("SOLIDDRAFT_COMMIT").filter(|c| !c.is_empty()),
    };
    let server = Server::bind(&listen, config).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    log::info!("serving the web app at http://{}", server.addr());
    server.run();
}
