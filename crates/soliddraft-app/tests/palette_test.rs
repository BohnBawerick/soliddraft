//! The command palette against the real app, through egui_kittest: a shortcut opens it,
//! typed text filters the registry's commands, Enter runs the chosen one, Escape closes
//! it, and a disabled command says why instead of running.
//!
//! Skips itself when no GPU adapter is available (plain CI runners).

use egui::{Key, Modifiers, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soliddraft_app::app::{Automation, SolidDraftApp};
use std::path::PathBuf;

/// Held for the length of a test, so harnesses never overlap.
static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu_turn() -> std::sync::MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

fn gpu_available() -> bool {
    let _ = env_logger::try_init();
    let instance = egui_wgpu::wgpu::Instance::default();
    !pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all())).is_empty()
}

type App<'a> = Harness<'a, SolidDraftApp>;

fn harness() -> App<'static> {
    harness_with(soliddraft_app::demo_document)
}

fn harness_with(doc: fn() -> soliddraft_app::document::Document) -> App<'static> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/exit-test/palette");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    soliddraft_app::file::set_config_dir(Some(dir));
    Harness::builder()
        .with_size(vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .with_render_every_step(true)
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_eframe(move |cc| SolidDraftApp::new(cc, doc(), Automation::default()))
}

fn open(harness: &mut App<'_>) {
    harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::P);
    harness.run_steps(3);
}

fn type_text(harness: &mut App<'_>, text: &str) {
    harness.event(egui::Event::Text(text.to_owned()));
    harness.run_steps(3);
}

fn palette(harness: &App<'_>) -> Option<soliddraft_app::palette::Palette> {
    harness.state().state.palette.clone()
}

#[test]
fn the_palette_opens_filters_runs_and_closes() {
    let _turn = gpu_turn();
    if !gpu_available() {
        eprintln!("skipping: no GPU adapter");
        return;
    }
    let mut harness = harness();
    harness.run_steps(4);
    assert!(palette(&harness).is_none());

    open(&mut harness);
    assert!(palette(&harness).is_some(), "Ctrl+Shift+P opens the palette");
    harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::P);
    harness.run_steps(3);
    assert!(palette(&harness).is_none(), "the same chord closes it");

    open(&mut harness);
    type_text(&mut harness, "extru");
    let typed = palette(&harness).expect("still open");
    assert_eq!(typed.query, "extru");
    let first = typed.results(&harness.state().state).first().map(|m| m.command.id);
    assert_eq!(first, Some("extrude"), "typed text filters the registry");
    assert!(harness.query_all_by_label("Extrude").next().is_some(), "the result is listed");
    let image = harness.render().expect("render");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/exit-test/palette");
    image.save(dir.join("palette.png")).expect("save png");

    harness.key_press(Key::Enter);
    harness.run_steps(4);
    let state = &harness.state().state;
    assert!(palette(&harness).is_none(), "Enter closes the palette");
    assert!(state.editor.is_some(), "and runs Extrude, which opens its PropertyManager");
    assert_eq!(state.commands.recent.first().map(String::as_str), Some("extrude"));

    open(&mut harness);
    harness.key_press(Key::Escape);
    harness.run_steps(3);
    assert!(palette(&harness).is_none(), "Escape closes it");
}

#[test]
fn the_arrow_keys_move_the_choice() {
    let _turn = gpu_turn();
    if !gpu_available() {
        eprintln!("skipping: no GPU adapter");
        return;
    }
    let mut harness = harness();
    harness.run_steps(4);
    open(&mut harness);
    type_text(&mut harness, "view");
    let ranked: Vec<&str> =
        palette(&harness).unwrap().results(&harness.state().state).iter().map(|m| m.command.id).collect();
    assert!(ranked.len() > 2, "{ranked:?}");
    harness.key_press(Key::ArrowDown);
    harness.run_steps(2);
    harness.key_press(Key::ArrowDown);
    harness.run_steps(2);
    harness.key_press(Key::ArrowUp);
    harness.run_steps(2);
    assert_eq!(palette(&harness).unwrap().selected, 1);
    harness.key_press(Key::Enter);
    harness.run_steps(3);
    let state = &harness.state().state;
    assert_eq!(state.commands.recent.first().map(String::as_str), Some(ranked[1]), "the second result ran");
}

#[test]
fn shell_without_a_body_says_why_from_the_palette() {
    let _turn = gpu_turn();
    if !gpu_available() {
        eprintln!("skipping: no GPU adapter");
        return;
    }
    let mut harness = harness_with(|| {
        soliddraft_app::document::Document::new(
            "Part1",
            std::sync::Arc::new(soliddraft_kernel_catallax::CatallaxKernel::new()),
        )
    });
    harness.run_steps(3);
    open(&mut harness);
    type_text(&mut harness, "shell");
    harness.key_press(Key::Enter);
    harness.run_steps(3);
    let state = &harness.state().state;
    assert!(state.editor.is_none(), "shell needs an input body");
    assert_eq!(state.note.as_ref().map(|n| n.text.as_str()), Some("Shell needs a body"));
}
