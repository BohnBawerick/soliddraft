//! The command line and the assistant session, driven through egui_kittest against the
//! real wgpu pipeline: typed commands run with Tab completion and history, an unknown
//! name gets the closest ones, `ai` opens the session where free text reaches the
//! assistant and a command's name still runs the command, Escape collapses it, and the
//! chip row reorders and takes ribbon buttons by drag and drop and keeps them for the next
//! run. Frames are written to `target/command-line-test/` for review.
//!
//! Skips itself when no GPU adapter is available (plain CI runners).

use egui::{Key, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soliddraft_app::app::{Automation, SolidDraftApp};
use soliddraft_app::chat::{ChatState, Role};
use soliddraft_app::command_line::{AI_COMMAND, DEFAULT_TOOLS};
use soliddraft_assistant::{MockProvider, MockTurn};
use std::path::PathBuf;

/// The hang guard every test shares: a wait this long means stuck, not slow.
const HANG: std::time::Duration = std::time::Duration::from_secs(600);

fn gpu_available() -> bool {
    let _ = env_logger::try_init();
    let instance = egui_wgpu::wgpu::Instance::default();
    !pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all())).is_empty()
}

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/command-line-test");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

type App<'a> = Harness<'a, SolidDraftApp>;

/// The demo part, with a scripted assistant that proposes a boss and then explains it.
fn harness() -> App<'static> {
    Harness::builder()
        .with_size(vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .with_render_every_step(true)
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_eframe(|cc| {
            let mut app = SolidDraftApp::new(cc, soliddraft_app::demo_document(), Automation::default());
            let provider = std::sync::Arc::new(MockProvider::new(vec![
                MockTurn::tools(
                    "",
                    vec![(
                        "propose",
                        serde_json::json!({
                            "summary": "Ø20 boss, 24 mm",
                            "commands": [
                                { "op": "sketch", "plane": "top", "shapes": [
                                    { "shape": "circle", "center": [-40, -25, 0], "diameter": 20 }
                                ]},
                                { "op": "extrude", "sketch": "previous", "depth": 24, "flip": false }
                            ]
                        }),
                    )],
                ),
                MockTurn::text("A Ø20 boss on the base, 24 mm tall from the Top plane, clear of the flange."),
            ]));
            app.state.chat = ChatState::connected(provider, "mock provider");
            app
        })
}

fn save(harness: &mut App<'_>, name: &str) {
    let image = harness.render().expect("render");
    let path = out_dir().join(format!("{name}.png"));
    image.save(&path).expect("save png");
    eprintln!("wrote {}", path.display());
}

fn settle(harness: &mut App<'_>) {
    harness.run_steps(20);
}

/// Types a line at the prompt, which has the keyboard, and presses a key after it.
fn type_line(harness: &mut App<'_>, line: &str, then: Key) {
    harness.get_by_label("Command line").type_text(line);
    harness.run_steps(2);
    harness.key_press(then);
    harness.run_steps(3);
}

fn input(harness: &App<'_>) -> String {
    harness.state().state.command_line.input.clone()
}

fn recent(harness: &App<'_>) -> Option<String> {
    harness.state().state.commands.recent.first().cloned()
}

/// Drags from one point to another in small steps, as a hand would.
fn drag(harness: &mut App<'_>, from: Pos2, to: Pos2) {
    harness.hover_at(from);
    harness.run_steps(2);
    harness.drag_at(from);
    harness.run_steps(2);
    for step in 1..=6 {
        harness.hover_at(from + (to - from) * (step as f32 / 6.0));
        harness.run_steps(2);
    }
    harness.drop_at(to);
    harness.run_steps(4);
}

fn chip_rect(harness: &App<'_>, label: &str) -> egui::Rect {
    harness.get_by_label(&format!("{label} on the command bar")).rect()
}

fn tools(harness: &App<'_>) -> Vec<String> {
    harness.state().state.command_line.settings.tools.clone()
}

/// The GPU harnesses share one adapter; one test at a time.
static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu_lock() -> std::sync::MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

fn config_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soliddraft-command-line-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    soliddraft_app::file::set_config_dir(Some(dir.clone()));
    dir
}

#[test]
fn typed_commands_run_and_the_session_talks_to_the_assistant() {
    if !gpu_available() {
        eprintln!("skipped: no GPU adapter");
        return;
    }
    let _gpu = gpu_lock();
    let dir = config_dir("session");
    let mut harness = harness();
    settle(&mut harness);
    assert!(!harness.state().state.command_line.session_open, "the session starts closed");
    assert!(harness.state().state.viewport.rect.right() >= 1439.0, "no assistant right of the viewport");
    save(&mut harness, "c01-command-line");

    // A typed id or label runs the registry command.
    harness.get_by_label("Command line").click();
    harness.run_steps(2);
    type_line(&mut harness, "view_top", Key::Enter);
    assert_eq!(recent(&harness).as_deref(), Some("view_top"));
    assert_eq!(input(&harness), "", "the prompt clears");
    assert!(harness.get_by_label("Command line").is_focused(), "and keeps the keyboard");
    type_line(&mut harness, "Isometric", Key::Enter);
    assert_eq!(recent(&harness).as_deref(), Some("view_isometric"));

    // Tab completes; Enter on an empty line repeats the last one.
    type_line(&mut harness, "zoom", Key::Tab);
    assert_eq!(input(&harness), "zoom_fit");
    harness.key_press(Key::Enter);
    harness.run_steps(3);
    assert_eq!(recent(&harness).as_deref(), Some("zoom_fit"));
    harness.state_mut().state.commands.recent.clear();
    harness.key_press(Key::Enter);
    harness.run_steps(3);
    assert_eq!(recent(&harness).as_deref(), Some("zoom_fit"), "Enter repeats");

    // An unknown name says what is closest; Up brings the line back, Escape clears it.
    type_line(&mut harness, "filet", Key::Enter);
    let note = harness.state().state.note.as_ref().map(|n| n.text.clone()).unwrap_or_default();
    assert!(note.contains("Closest: fillet"), "{note}");
    save(&mut harness, "c02-unknown-command");
    harness.key_press(Key::ArrowUp);
    harness.run_steps(2);
    assert_eq!(input(&harness), "filet");
    harness.key_press(Key::ArrowUp);
    harness.run_steps(2);
    assert_eq!(input(&harness), "zoom_fit");
    harness.key_press(Key::Escape);
    harness.run_steps(2);
    assert_eq!(input(&harness), "");

    // The AI command opens the session at the same prompt.
    type_line(&mut harness, AI_COMMAND, Key::Enter);
    settle(&mut harness);
    assert!(harness.state().state.command_line.session_open);
    let viewport = harness.state().state.viewport.rect;
    assert!(viewport.bottom() < 900.0 - 250.0, "the session grows upward: {viewport:?}");
    assert!(viewport.right() >= 1439.0, "and leaves the viewport its width");
    save(&mut harness, "c03-session-welcome");

    // Free text goes to the assistant, which proposes; Apply builds it.
    type_line(&mut harness, "Add a Ø20 boss, 24 mm tall, on the base", Key::Enter);
    let deadline = std::time::Instant::now() + HANG;
    while harness.state().state.chat.busy() && std::time::Instant::now() < deadline {
        harness.step();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    settle(&mut harness);
    assert!(harness.state().state.chat.book.get("P1").is_some(), "{:#?}", harness.state().state.chat.messages);
    save(&mut harness, "c04-session-proposal");
    harness.get_by_label("Apply").click();
    settle(&mut harness);
    assert_eq!(harness.state().state.doc.features.len(), 5, "a sketch and an extrude");
    save(&mut harness, "c05-session-applied");

    // A command's name in the session runs the command and says so there. Apply took
    // the keyboard, so the prompt is clicked first.
    harness.get_by_label("Command line").click();
    harness.run_steps(2);
    let sent = harness.state().state.chat.messages.iter().filter(|m| m.role == Role::User).count();
    type_line(&mut harness, "zoom_fit", Key::Enter);
    let messages = &harness.state().state.chat.messages;
    assert!(messages.iter().any(|m| m.role == Role::Command && m.text == "zoom_fit"));
    assert_eq!(messages.last().map(|m| m.text.as_str()), Some("Fit"));
    assert_eq!(messages.iter().filter(|m| m.role == Role::User).count(), sent, "nothing went to the assistant");
    assert_eq!(recent(&harness).as_deref(), Some("zoom_fit"));

    // Escape collapses it back to the bar; exit does too.
    harness.key_press(Key::Escape);
    settle(&mut harness);
    assert!(!harness.state().state.command_line.session_open);
    type_line(&mut harness, AI_COMMAND, Key::Enter);
    settle(&mut harness);
    assert!(harness.state().state.command_line.session_open);
    type_line(&mut harness, "exit", Key::Enter);
    settle(&mut harness);
    assert!(!harness.state().state.command_line.session_open);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_chip_row_reorders_takes_ribbon_buttons_and_comes_back_next_run() {
    if !gpu_available() {
        eprintln!("skipped: no GPU adapter");
        return;
    }
    let _gpu = gpu_lock();
    let dir = config_dir("chips");
    let mut harness = harness();
    settle(&mut harness);
    assert_eq!(tools(&harness), DEFAULT_TOOLS);

    // A chip runs its command.
    harness.get_by_label("Normal to on the command bar").hover();
    harness.run_steps(2);
    harness.get_by_label("Fit on the command bar").click();
    harness.run_steps(3);
    assert_eq!(recent(&harness).as_deref(), Some("zoom_fit"));

    // Fit dragged to the front of the row moves there.
    let fit = chip_rect(&harness, "Fit");
    let select = chip_rect(&harness, "Select");
    drag(&mut harness, fit.center(), select.left_center() + vec2(3.0, 0.0));
    assert_eq!(tools(&harness).first().map(String::as_str), Some("zoom_fit"), "{:?}", tools(&harness));
    assert_eq!(tools(&harness).len(), DEFAULT_TOOLS.len());

    // The ribbon's Fillet dropped on the row becomes a chip.
    let fillet = harness.get_all_by_label("Fillet").next().expect("the ribbon's Fillet").rect();
    let last = chip_rect(&harness, "Normal to");
    drag(&mut harness, fillet.center(), last.right_center() - vec2(3.0, 0.0));
    assert_eq!(tools(&harness).last().map(String::as_str), Some("fillet"), "{:?}", tools(&harness));
    save(&mut harness, "c06-chips-arranged");

    // Right-click removes one.
    harness.get_by_label("Cancel on the command bar").click_secondary();
    harness.run_steps(3);
    harness.get_by_label("Remove Cancel from the bar").click();
    harness.run_steps(4);
    assert!(!tools(&harness).iter().any(|t| t == "cancel"));

    // The row is in the settings file, and the next run starts from it.
    let chosen = tools(&harness);
    assert_eq!(soliddraft_app::settings::command_line().tools, chosen);
    drop(harness);
    let mut again = self::harness();
    settle(&mut again);
    assert_eq!(tools(&again), chosen);
    save(&mut again, "c07-chips-next-run");
    let _ = std::fs::remove_dir_all(dir);
}
