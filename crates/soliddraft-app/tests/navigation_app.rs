//! Navigation settings against the real app, through egui_kittest: a new installation
//! starts on Fusion and an existing one on SolidDraft's original controls, even when
//! another setting is written first, and changing navigation leaves the part alone.
//!
//! Skips itself when no GPU adapter is available; `navigation_settings.rs` proves the
//! same behaviour without one. Each scenario writes the settings file it ended with
//! under `target/navigation-test/app/`.

use egui::vec2;
use egui_kittest::Harness;
use soliddraft_app::app::{Automation, SolidDraftApp};
use soliddraft_app::document::Selection;
use soliddraft_app::state::AppAction;
use soliddraft_app::theme::Mode;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// Held for the length of a test: the config directory is global and harnesses take turns
/// on the GPU.
static CONFIG_LOCK: Mutex<()> = Mutex::new(());

fn gpu_available() -> bool {
    let _ = env_logger::try_init();
    let instance = egui_wgpu::wgpu::Instance::default();
    !pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all())).is_empty()
}

fn config_dir(name: &str) -> (MutexGuard<'static, ()>, PathBuf) {
    let guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("soliddraft-nav-app-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    soliddraft_app::file::set_config_dir(Some(dir.clone()));
    (guard, dir)
}

fn harness(doc: fn() -> soliddraft_app::document::Document) -> Harness<'static, SolidDraftApp> {
    Harness::builder()
        .with_size(vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_eframe(move |cc| SolidDraftApp::new(cc, doc(), Automation::default()))
}

fn settings(dir: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap()).unwrap()
}

/// Keeps the settings file a scenario ended with, for review.
fn record(name: &str, dir: &Path) {
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/navigation-test/app");
    std::fs::create_dir_all(&out).unwrap();
    let _ = std::fs::copy(dir.join("settings.json"), out.join(format!("{name}-settings.json")));
}

#[test]
fn install_kind_is_captured_before_any_settings_write() {
    let (_guard, dir) = config_dir("captured");
    if !gpu_available() {
        eprintln!("skipping: no GPU adapter");
        return;
    }
    for (start, preset) in [(None, "fusion"), (Some(r#"{"theme":"dark"}"#), "legacy_soliddraft")] {
        let _ = std::fs::remove_file(dir.join("settings.json"));
        if let Some(text) = start {
            std::fs::write(dir.join("settings.json"), text).unwrap();
        }
        let mut app = harness(soliddraft_app::new_document);
        app.run_steps(2);
        assert_eq!(app.state().state.navigation.active_preset().id, preset, "the app starts on {preset}");
        app.state_mut().state.push(AppAction::SetTheme(Mode::Dark));
        app.run_steps(2);
        let all = settings(&dir);
        assert_eq!(all["appearance"]["theme"], "dark", "the theme was saved");
        assert_eq!(all["navigation"]["preset_id"], preset, "and the preset with it: {all}");
        drop(app);

        let mut app = harness(soliddraft_app::new_document);
        app.run_steps(2);
        assert_eq!(app.state().state.navigation.active_preset().id, preset, "{preset} after a restart");
        record(&format!("captured-{preset}"), &dir);
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn navigation_save_does_not_touch_document_or_undo() {
    let (_guard, dir) = config_dir("document");
    if !gpu_available() {
        eprintln!("skipping: no GPU adapter");
        return;
    }
    let mut app = harness(soliddraft_app::demo_document);
    app.run_steps(2);
    let feature = app.state().state.doc.features[0].id;
    app.state_mut().state.doc.selection = Some(Selection::Feature(feature));
    app.run_steps(1);
    let doc = &app.state().state.doc;
    let before = (doc.revision(), doc.undo_depth(), doc.selection, doc.dirty, doc.model().semantic_hash());
    let editor = app.state().state.editor.is_some();

    let mut changed = app.state().state.navigation.clone();
    changed.select_preset("solidworks").unwrap();
    changed.wheel.invert = true;
    app.state_mut().state.set_navigation(changed.clone()).unwrap();
    app.run_steps(2);

    let state = &app.state().state;
    assert_eq!(state.navigation, changed);
    let doc = &state.doc;
    assert_eq!((doc.revision(), doc.undo_depth(), doc.selection, doc.dirty, doc.model().semantic_hash()), before);
    assert_eq!(state.editor.is_some(), editor, "no editor opened or closed");
    assert_eq!(settings(&dir)["navigation"]["preset_id"], "solidworks");
    record("document-unchanged", &dir);
    let _ = std::fs::remove_dir_all(dir);
}
