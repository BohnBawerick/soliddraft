//! SolidDraft: a parametric CAD application in Rust.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(target_arch = "wasm32"))]
use soliddraft_app::app::{Automation, SolidDraftApp};

#[cfg(not(target_arch = "wasm32"))]
const USAGE: &str =
    "soliddraft [FILE.dwprt | FILE.dwasm | FILE.dwdrw] [--demo] [--bench [SECONDS]] [--screenshot PATH [--after FRAMES] [--stay]] [--size WxH] [--no-vsync]
          [--mcp [PORT]] [--no-assistant] [--preset fast|studio|realistic] [--theme light|dark]

  FILE.dwprt        open a saved part (.dwpart still opens)
  FILE.dwasm        open a saved assembly in a tab beside the part
  FILE.dwdrw        open a saved drawing, and its part when it is found
  --demo            start with a sample part (three extrudes)
  --bench [SECONDS] load a 10,000+ triangle body, orbit with vsync off, print frame-time statistics and exit
  --screenshot PATH save the window to a PNG after a few frames and exit (use --stay to keep running)
  --size WxH        window size in logical pixels (default 1440x900)
  --no-vsync        present immediately instead of waiting for the display
  --mcp [PORT]      serve the open part to MCP clients at http://127.0.0.1:PORT/mcp (default 47811)
  --no-assistant    do not connect the assistant, even when an API key is available
  --preset NAME     start in the Fast, Studio or Realistic look instead of the one in the settings
  --theme NAME      start light or dark instead of the appearance in the settings

The assistant runs on OpenRouter or on the Anthropic API. It reads the key from
OPENROUTER_API_KEY or ANTHROPIC_API_KEY, or from the OS keychain (store one with
`soliddraft key set openrouter` or `soliddraft key set anthropic`), and uses the provider
that has a key unless Provider on the Assistant tab or SOLIDDRAFT_PROVIDER picks one.
Without a key the app works and says so.";

// The web build starts from crates/soliddraft-web instead.
#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    env_logger::init();
    soliddraft_app::load_reference_files();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut demo = false;
    let mut automation = Automation {
        screenshot_after_frames: 12,
        exit_after_screenshot: true,
        bench_seconds: 5.0,
        connect_assistant: true,
        ..Default::default()
    };
    let mut size = (1440.0, 900.0);
    let mut vsync = true;
    let mut open: Option<std::path::PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--demo" => demo = true,
            "--bench" => {
                automation.bench = true;
                vsync = false;
                if let Some(s) = args.get(i + 1).and_then(|s| s.parse::<f32>().ok()) {
                    automation.bench_seconds = s;
                    i += 1;
                }
            }
            "--screenshot" => {
                automation.screenshot = args.get(i + 1).map(std::path::PathBuf::from);
                i += 1;
            }
            "--after" => {
                automation.screenshot_after_frames = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(12);
                i += 1;
            }
            "--stay" => automation.exit_after_screenshot = false,
            "--no-vsync" => vsync = false,
            "--no-assistant" => automation.connect_assistant = false,
            "--preset" => {
                let name = args.get(i + 1).map(String::as_str).unwrap_or("");
                match soliddraft_app::viewport::look::Preset::parse(name) {
                    Some(p) => automation.preset = Some(p),
                    None => {
                        eprintln!("unknown preset {name:?}; use fast, studio or realistic\n{USAGE}");
                        std::process::exit(2);
                    }
                }
                i += 1;
            }
            "--theme" => {
                let name = args.get(i + 1).map(String::as_str).unwrap_or("");
                match soliddraft_app::theme::Mode::parse(name) {
                    Some(m) => automation.theme = Some(m),
                    None => {
                        eprintln!("unknown theme {name:?}; use light or dark\n{USAGE}");
                        std::process::exit(2);
                    }
                }
                i += 1;
            }
            "--mcp" => {
                automation.mcp_port = Some(match args.get(i + 1).and_then(|s| s.parse::<u16>().ok()) {
                    Some(p) => {
                        i += 1;
                        p
                    }
                    None => 47811,
                });
            }
            "--size" => {
                if let Some((w, h)) = args.get(i + 1).and_then(|s| s.split_once('x')) {
                    size = (w.parse().unwrap_or(1440.0), h.parse().unwrap_or(900.0));
                }
                i += 1;
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            other if !other.starts_with('-') => open = Some(std::path::PathBuf::from(other)),
            other => {
                eprintln!("unknown argument {other}\n{USAGE}");
                std::process::exit(2);
            }
        }
        i += 1;
    }

    // Benchmarks and screenshots leave autosave off, so they never offer a recovery.
    let documents = soliddraft_app::settings::documents();
    let recovery = documents.recovery_folder().filter(|_| !automation.bench && automation.screenshot.is_none());
    let journal = recovery
        .as_ref()
        .map(|root| soliddraft_document::journal::Session::start(&root.join("journals")))
        .transpose()
        .map_err(|e| eframe::Error::AppCreation(Box::new(e)))?;

    let doc = if automation.bench {
        soliddraft_app::bench_document()
    } else if demo {
        soliddraft_app::demo_document()
    } else {
        soliddraft_app::new_document()
    };

    let mut wgpu_options = egui_wgpu::WgpuConfiguration {
        surface: egui_wgpu::SurfaceConfig {
            present_mode: if vsync {
                egui_wgpu::wgpu::PresentMode::AutoVsync
            } else {
                egui_wgpu::wgpu::PresentMode::AutoNoVsync
            },
            desired_maximum_frame_latency: None,
        },
        ..Default::default()
    };
    soliddraft_app::viewport::assembly::request_features(&mut wgpu_options);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Part1 - SolidDraft")
            .with_app_id("soliddraft")
            .with_inner_size([size.0, size.1])
            .with_min_inner_size([960.0, 600.0]),
        wgpu_options,
        ..Default::default()
    };
    let result = eframe::run_native(
        "soliddraft",
        options,
        Box::new(move |cc| {
            let mut app = SolidDraftApp::new(cc, doc, automation);
            if let Some(root) = recovery {
                app.start_recovery(&root, documents.autosave);
            }
            if let Some(path) = open {
                soliddraft_app::app::open_at_start(&mut app, path);
            }
            Ok(Box::new(app))
        }),
    );
    if result.is_ok()
        && let Some(journal) = journal
    {
        journal.close();
    }
    result
}
