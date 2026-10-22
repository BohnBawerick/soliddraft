//! Keyboard shortcuts and ribbon buttons against the real app, through egui_kittest:
//! every binding the command registry lists reaches its command, chords with more
//! modifiers win over their plain form, and the context decides between commands that
//! share a key.
//!
//! Skips itself when no GPU adapter is available (plain CI runners). The tests take
//! turns on the GPU: short harnesses made and dropped at the same moment crashed the
//! driver.

use egui::{Key, Modifiers, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soliddraft_app::app::{Automation, SolidDraftApp};
use soliddraft_app::commands;
use soliddraft_app::drawing::{DrawingAction, DrawingTab, DrawingTool};
use soliddraft_app::state::{AppAction, RibbonTab};
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
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/exit-test/commands");
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

fn features(harness: &App<'_>) -> usize {
    harness.state().state.doc.features.len()
}

#[test]
fn ctrl_shift_z_redoes() {
    let _turn = gpu_turn();
    if !gpu_available() {
        eprintln!("skipping: no GPU adapter");
        return;
    }
    let mut harness = harness();
    harness.run_steps(4);
    let n = features(&harness);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    harness.run_steps(2);
    assert_eq!(features(&harness), n - 1, "Ctrl+Z undoes the last feature");
    harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    harness.run_steps(2);
    assert_eq!(features(&harness), n, "Ctrl+Shift+Z redoes it, not a second undo");
}

#[test]
fn every_ribbon_button_is_a_registry_command() {
    let _turn = gpu_turn();
    if !gpu_available() {
        eprintln!("skipping: no GPU adapter");
        return;
    }
    let mut harness = harness();
    for tab in RibbonTab::ALL {
        harness.state_mut().state.ribbon_tab = tab;
        harness.run_steps(2);
        let panels = commands::ribbon(tab);
        assert!(!panels.is_empty(), "{} has no panels", tab.label());
        for (panel, items) in panels {
            for (command, _) in items {
                assert!(
                    harness.query_all_by_label(command.label).next().is_some(),
                    "{} › {panel}: no button labelled {}",
                    tab.label(),
                    command.label
                );
            }
        }
    }
    // A click runs the command and remembers it as recent.
    harness.state_mut().state.ribbon_tab = RibbonTab::Features;
    harness.run_steps(2);
    harness.get_by_label("Extrude").click();
    harness.run_steps(3);
    assert!(harness.state().state.editor.is_some(), "Extrude opens its PropertyManager");
    assert_eq!(harness.state().state.commands.recent.first().map(String::as_str), Some("extrude"));
}

#[test]
fn a_disabled_command_says_why_instead_of_running() {
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
    harness.key_press(Key::C);
    harness.run_steps(2);
    let state = &harness.state().state;
    assert!(state.editor.is_none(), "no chamfer without a body");
    assert_eq!(state.note.as_ref().map(|n| n.text.as_str()), Some("Arrives with Catallax stage 3"));
}

#[test]
fn the_shortcuts_window_lists_the_registry() {
    let _turn = gpu_turn();
    if !gpu_available() {
        eprintln!("skipping: no GPU adapter");
        return;
    }
    let mut harness = harness();
    harness.run_steps(3);
    harness.key_press(Key::Questionmark);
    harness.run_steps(3);
    assert!(harness.state().state.show_shortcuts, "? opens the shortcuts window");
    for c in commands::all().iter().filter(|c| c.in_app() && !c.keys.is_empty()) {
        assert!(harness.query_all_by_label(c.doc).next().is_some(), "the window lists {}", c.id);
    }
    assert!(harness.query_by_label("Ctrl+Y / Ctrl+Shift+Z").is_some());
    let image = harness.render().expect("render");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/exit-test/commands");
    image.save(dir.join("shortcuts-window.png")).expect("save png");
}

#[test]
fn the_drawing_ribbon_and_keys_come_from_the_registry() {
    let _turn = gpu_turn();
    if !gpu_available() {
        eprintln!("skipping: no GPU adapter");
        return;
    }
    let mut harness = harness();
    harness.run_steps(3);
    harness.state_mut().state.push(AppAction::Drawing(DrawingAction::MakeDrawing));
    harness.run_steps(3);
    harness.state_mut().state.push(AppAction::Drawing(DrawingAction::CreateDrawing));
    harness.run_steps(6);
    assert!(harness.state().state.drawing.is_active(), "the drawing tab is open and active");
    for tab in DrawingTab::ALL {
        harness.state_mut().state.drawing.session.as_mut().unwrap().tab = tab;
        harness.run_steps(2);
        for (panel, items) in commands::ribbon(tab) {
            for (command, _) in items.into_iter().filter(|(c, _)| (c.shown)(&harness.state().state)) {
                let label = command.label_in(&harness.state().state);
                assert!(
                    harness.query_all_by_label(&label).next().is_some(),
                    "drawing {} › {panel}: no button labelled {label}",
                    tab.label()
                );
            }
        }
    }
    let tool = |h: &App<'_>| h.state().state.drawing.session.as_ref().unwrap().tool.clone();
    harness.key_press(Key::N);
    harness.run_steps(2);
    assert_eq!(tool(&harness), DrawingTool::Note, "N picks the note tool in a drawing");
    harness.key_press(Key::Escape);
    harness.run_steps(2);
    assert_eq!(tool(&harness), DrawingTool::Select, "Esc puts it down");
    let grid = |h: &App<'_>| h.state().state.drawing.session.as_ref().unwrap().show_grid;
    let before = grid(&harness);
    harness.key_press(Key::G);
    harness.run_steps(2);
    assert_ne!(grid(&harness), before, "G toggles the grid");
    // Ctrl+Z is the drawing's undo here, not the part's.
    let n = features(&harness);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    harness.run_steps(2);
    assert_eq!(features(&harness), n, "the part is untouched");
}
