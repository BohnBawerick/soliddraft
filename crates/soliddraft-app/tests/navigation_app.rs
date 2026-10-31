//! Navigation against the real app, through egui_kittest: a new installation starts on
//! Fusion and an existing one on SolidDraft's original controls, changing navigation
//! leaves the part alone, and the presets' gestures move the camera from the first drag,
//! with no click first, while selection, sketches and feature panels carry on.
//!
//! Skips itself when no GPU adapter is available; `navigation_settings.rs` and
//! `navigation_routing.rs` prove the same behaviour without one. Each scenario writes the
//! settings file it ended with, a camera trace and a frame under
//! `target/navigation-test/app/`. Nothing here is timed.

use egui::{Key, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use glam::Vec3;
use soliddraft_app::app::{Automation, SolidDraftApp};
use soliddraft_app::document::{Command, Document, ExtrudeParams, PlaneKind, ProfileKind, Selection, SketchPlane};
use soliddraft_app::navigation::NAV_DRAG_THRESHOLD;
use soliddraft_app::state::{AppAction, Tool};
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

type App = Harness<'static, SolidDraftApp>;

/// The app on `doc`, drawing every step so picks read back, with the camera fitted.
fn gesture_app(doc: Document, preset: &str) -> App {
    let mut app = Harness::builder()
        .with_size(vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .with_render_every_step(true)
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_eframe(move |cc| SolidDraftApp::new(cc, doc, Automation::default()));
    app.run_steps(6);
    use_preset(&mut app, preset);
    app.state_mut().state.push(AppAction::ZoomFit);
    app.run_steps(30);
    app
}

/// Switches the preset through the app's settings path, as the Preferences will.
fn use_preset(app: &mut App, preset: &str) {
    let mut nav = app.state().state.navigation.clone();
    nav.select_preset(preset).unwrap();
    app.state_mut().state.set_navigation(nav).unwrap();
    app.run_steps(1);
}

/// A 60 x 40 x 20 box on the top plane.
fn box_document() -> Document {
    let mut doc = Document::new("Box", soliddraft_app::kernel());
    doc.apply(Command::AddExtrude {
        params: ExtrudeParams {
            profile: ProfileKind::Rectangle { width: 60.0, height: 40.0 },
            depth: 20.0,
            ..Default::default()
        },
        via_assistant: false,
    })
    .unwrap();
    doc
}

/// Yaw, pitch, target and distance.
type Cam = (f32, f32, Vec3, f32);

fn cam(app: &App) -> Cam {
    let c = &app.state().state.viewport.camera;
    (c.yaw, c.pitch, c.target, c.distance)
}

fn viewport(app: &App) -> egui::Rect {
    app.state().state.viewport.rect
}

fn modifiers(app: &App, m: Modifiers) {
    app.event(egui::Event::ModifiersChanged(m));
}

fn press(app: &mut App, button: PointerButton, pos: Pos2, m: Modifiers) {
    app.hover_at(pos);
    app.step();
    modifiers(app, m);
    app.event(egui::Event::PointerButton { pos, button, pressed: true, modifiers: m });
    app.step();
}

fn move_to(app: &mut App, pos: Pos2) {
    app.event(egui::Event::PointerMoved(pos));
    app.step();
}

fn release(app: &mut App, button: PointerButton, pos: Pos2, m: Modifiers) {
    app.event(egui::Event::PointerButton { pos, button, pressed: false, modifiers: m });
    app.step();
    modifiers(app, Modifiers::NONE);
    app.run_steps(2);
}

/// Drags from `from` to `to` in a few moves and lets go.
fn drag(app: &mut App, button: PointerButton, from: Pos2, to: Pos2, m: Modifiers) {
    press(app, button, from, m);
    for i in 1..=4 {
        move_to(app, from + (to - from) * (i as f32 / 4.0));
    }
    release(app, button, to, m);
}

/// Keeps a gesture's camera before and after, and a frame after, for review.
fn trace(app: &mut App, name: &str, before: Cam, after: Cam) {
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/navigation-test/app");
    std::fs::create_dir_all(&out).unwrap();
    let row = |c: Cam| serde_json::json!({"yaw": c.0, "pitch": c.1, "target": [c.2.x, c.2.y, c.2.z], "distance": c.3});
    let json = serde_json::json!({"gesture": name, "before": row(before), "after": row(after)});
    std::fs::write(out.join(format!("{name}.json")), serde_json::to_string_pretty(&json).unwrap()).unwrap();
    if let Ok(image) = app.render() {
        let _ = image.save(out.join(format!("{name}.png")));
    }
}

/// Runs a gesture and says what it changed: (turned, panned, zoomed).
fn effect(app: &mut App, name: &str, gesture: impl FnOnce(&mut App)) -> (bool, bool, bool) {
    let before = cam(app);
    gesture(app);
    let after = cam(app);
    trace(app, name, before, after);
    let turned = (after.0 - before.0).abs() > 1e-4 || (after.1 - before.1).abs() > 1e-4;
    let panned = (after.2 - before.2).length() > 1e-3;
    let zoomed = (after.3 - before.3).abs() > 1e-3;
    (turned, panned, zoomed)
}

const ORBIT: (bool, bool, bool) = (true, false, false);
const PAN: (bool, bool, bool) = (false, true, false);
const NOTHING: (bool, bool, bool) = (false, false, false);

fn modelling_unchanged(app: &App) -> impl Fn(&App, &str) + use<> {
    let s = &app.state().state;
    let before = (s.doc.selection, s.doc.undo_depth(), s.doc.revision(), s.editor.is_some());
    move |app: &App, what: &str| {
        let s = &app.state().state;
        assert_eq!((s.doc.selection, s.doc.undo_depth(), s.doc.revision(), s.editor.is_some()), before, "{what}");
    }
}

fn skip_without_gpu() -> bool {
    if gpu_available() {
        return false;
    }
    eprintln!("skipping: no GPU adapter");
    true
}

#[test]
fn held_ordinary_keys_are_modifiers_and_taps_still_run_their_command() {
    let (_guard, dir) = config_dir("held-keys");
    if skip_without_gpu() {
        return;
    }
    let mut app = gesture_app(soliddraft_app::demo_document(), "inventor");
    let feature = app.state().state.doc.features[0].id;
    app.state_mut().state.doc.selection = Some(Selection::Feature(feature));
    app.run_steps(2);
    let c = viewport(&app).center();
    let effect_of = effect(&mut app, "inventor-f2-left-drag", |app| {
        app.key_down(Key::F2);
        app.step();
        drag(app, PointerButton::Primary, c, c + vec2(80.0, 30.0), Modifiers::NONE);
        app.key_up(Key::F2);
        app.run_steps(3);
    });
    assert_eq!(effect_of, PAN, "held F2 and a left drag pans");
    let s = &app.state().state;
    assert!(s.rename.is_none(), "the held F2 did not rename");
    assert_eq!(s.doc.selection, Some(Selection::Feature(feature)), "the drag selected nothing");
    app.key_down(Key::F2);
    app.step();
    app.key_up(Key::F2);
    app.run_steps(3);
    assert!(app.state().state.rename.is_some(), "a tap of F2 still renames");
    record("held-keys", &dir);
}

#[test]
fn drag_capture_requires_viewport_origin_and_threshold() {
    let (_guard, _dir) = config_dir("capture");
    if skip_without_gpu() {
        return;
    }
    let mut app = gesture_app(soliddraft_app::demo_document(), "legacy_soliddraft");
    let rect = viewport(&app);
    // Empty space near a corner, away from the part, the cube and the toolbar.
    let start = rect.left_bottom() + vec2(120.0, -120.0);
    let short = effect(&mut app, "legacy-under-threshold", |app| {
        drag(app, PointerButton::Primary, start, start + vec2(0.0, NAV_DRAG_THRESHOLD - 1.0), Modifiers::NONE)
    });
    assert_eq!(short, NOTHING, "under the threshold the camera stays");
    let long = effect(&mut app, "legacy-past-threshold", |app| {
        drag(app, PointerButton::Primary, start, start + vec2(0.0, NAV_DRAG_THRESHOLD + 1.0), Modifiers::NONE)
    });
    assert_eq!(long, ORBIT, "past it the first drag orbits, with no click first");
    let tree = pos2(rect.left() - 120.0, rect.center().y);
    let from_tree = effect(&mut app, "legacy-press-in-tree", |app| {
        drag(app, PointerButton::Primary, tree, rect.center(), Modifiers::NONE)
    });
    assert_eq!(from_tree, NOTHING, "a press in the tree does not navigate");
    let out = effect(&mut app, "legacy-out-of-window", |app| {
        press(app, PointerButton::Primary, start, Modifiers::NONE);
        move_to(app, start + vec2(60.0, -40.0));
        assert!(app.state().state.viewport.router.dragging(), "captured");
        move_to(app, pos2(-50.0, -30.0));
        assert!(app.state().state.viewport.router.dragging(), "still captured outside the window");
        release(app, PointerButton::Primary, pos2(-50.0, -30.0), Modifiers::NONE);
    });
    assert!(out.0, "the camera turned while inside");
    assert!(!app.state().state.viewport.router.captured(), "capture ends at the release");
    // Over the ribbon and the view cube the drag goes on, and neither is clicked.
    let tab = app.state().state.ribbon_tab;
    let over = effect(&mut app, "legacy-release-over-overlays", |app| {
        press(app, PointerButton::Primary, start, Modifiers::NONE);
        move_to(app, start + vec2(40.0, 0.0));
        move_to(app, rect.right_top() + vec2(-70.0, 70.0));
        move_to(app, pos2(rect.center().x, 60.0));
        release(app, PointerButton::Primary, pos2(rect.center().x, 60.0), Modifiers::NONE);
    });
    assert!(over.0);
    assert_eq!(app.state().state.ribbon_tab, tab, "the ribbon was not clicked");
    assert_eq!(app.state().state.viewport.camera.view_name, "Custom", "the cube was not clicked");
}

#[test]
fn navigation_drag_does_not_select_or_open_a_secondary_click_path() {
    let (_guard, _dir) = config_dir("click-paths");
    if skip_without_gpu() {
        return;
    }
    let mut app = gesture_app(box_document(), "solidworks");
    let top = {
        let s = &app.state().state;
        s.viewport.camera.project(Vec3::new(0.0, 0.0, 20.0), s.viewport.rect).unwrap()
    };
    let unchanged = modelling_unchanged(&app);
    let orbit = effect(&mut app, "solidworks-middle-drag-on-face", |app| {
        drag(app, PointerButton::Middle, top, top + vec2(70.0, 20.0), Modifiers::NONE)
    });
    assert_eq!(orbit, ORBIT);
    unchanged(&app, "a middle drag on a face selects nothing and opens nothing");

    use_preset(&mut app, "onshape");
    let unchanged = modelling_unchanged(&app);
    let r_drag = effect(&mut app, "onshape-right-drag", |app| {
        drag(app, PointerButton::Secondary, top, top + vec2(-60.0, 25.0), Modifiers::NONE)
    });
    assert_eq!(r_drag, ORBIT);
    unchanged(&app, "the right drag's release is not a click");
    let r_click = effect(&mut app, "onshape-right-click", |app| {
        press(app, PointerButton::Secondary, top, Modifiers::NONE);
        release(app, PointerButton::Secondary, top, Modifiers::NONE);
    });
    assert_eq!(r_click, NOTHING, "a right click does not move the camera");

    // A click without a drag still selects what is under the pointer.
    let top = {
        let s = &app.state().state;
        s.viewport.camera.project(Vec3::new(0.0, 0.0, 20.0), s.viewport.rect).unwrap()
    };
    app.hover_at(top);
    app.run_steps(4);
    press(&mut app, PointerButton::Primary, top, Modifiers::NONE);
    release(&mut app, PointerButton::Primary, top, Modifiers::NONE);
    assert!(matches!(app.state().state.doc.selection, Some(Selection::Face(_))), "the click picked the top face");
}

fn wheel_event(unit: egui::MouseWheelUnit, dy: f32, m: Modifiers) -> egui::Event {
    egui::Event::MouseWheel { unit, delta: vec2(0.0, dy), phase: egui::TouchPhase::Move, modifiers: m }
}

#[test]
fn one_physical_zoom_gesture_produces_one_zoom() {
    let (_guard, _dir) = config_dir("zoom");
    if skip_without_gpu() {
        return;
    }
    let mut app = gesture_app(soliddraft_app::demo_document(), "fusion");
    let c = viewport(&app).center();
    app.hover_at(c);
    app.run_steps(2);
    let d0 = cam(&app).3;
    app.event(wheel_event(egui::MouseWheelUnit::Line, 1.0, Modifiers::NONE));
    app.run_steps(3);
    let d1 = cam(&app).3;
    assert!((d0 / d1 - 0.1f32.exp()).abs() < 1e-3, "one line, one zoom: {d0} to {d1}");
    // Both in one frame, as the browser delivers them.
    app.input_mut().events.push(egui::Event::Zoom(1.25));
    app.input_mut().events.push(wheel_event(egui::MouseWheelUnit::Point, 30.0, Modifiers::CTRL));
    app.run_steps(3);
    let d2 = cam(&app).3;
    assert!((d1 / d2 - 1.25).abs() < 1e-3, "a pinch and its Ctrl-wheel copy zoom once: {d1} to {d2}");
    trace(&mut app, "fusion-wheel-and-pinch", (0.0, 0.0, Vec3::ZERO, d0), (0.0, 0.0, Vec3::ZERO, d2));
}

#[test]
fn navigation_works_with_active_sketch_and_collector() {
    let (_guard, _dir) = config_dir("sketch-collector");
    if skip_without_gpu() {
        return;
    }
    // A sketch with the line tool, under Fusion.
    let mut app = gesture_app(soliddraft_app::new_document(), "fusion");
    app.state_mut().state.push(AppAction::StartSketchOn(SketchPlane::Reference { plane: PlaneKind::Top, offset: 0.0 }));
    app.run_steps(60);
    app.state_mut().state.push(AppAction::SketchTool(Tool::Line));
    app.run_steps(3);
    let c = viewport(&app).center();
    let pan = effect(&mut app, "fusion-sketch-middle-drag", |app| {
        drag(app, PointerButton::Middle, c, c + vec2(60.0, 0.0), Modifiers::NONE)
    });
    assert_eq!(pan, PAN);
    let orbit = effect(&mut app, "fusion-sketch-shift-middle-drag", |app| {
        drag(app, PointerButton::Middle, c, c + vec2(40.0, 30.0), Modifiers::SHIFT)
    });
    assert_eq!(orbit, ORBIT);
    let s = app.state().state.sketch.as_ref().expect("the sketch stays open");
    assert_eq!(s.tool, Tool::Line, "the line tool stays");
    let left = effect(&mut app, "fusion-sketch-left-drag", |app| {
        drag(app, PointerButton::Primary, c, c + vec2(50.0, 40.0), Modifiers::NONE)
    });
    assert_eq!(left, NOTHING, "a left drag in the sketch does not navigate");
    let lines = |app: &App| app.state().state.sketch.as_ref().map_or(0, |s| s.sketch.entities.len());
    let before = lines(&app);
    for p in [c + vec2(-80.0, -40.0), c + vec2(90.0, 50.0)] {
        app.hover_at(p);
        app.run_steps(2);
        press(&mut app, PointerButton::Primary, p, Modifiers::NONE);
        release(&mut app, PointerButton::Primary, p, Modifiers::NONE);
    }
    assert!(lines(&app) > before, "left clicks still draw a line");
    assert_eq!(app.state().state.sketch.as_ref().unwrap().tool, Tool::Line);

    // A fillet panel collecting edges, under SOLIDWORKS.
    let mut app = gesture_app(box_document(), "fusion");
    let edge = {
        let doc = &app.state().state.doc;
        doc.edges.iter().find(|e| (e.midpoint.z - 20.0).abs() < 1e-9).unwrap().id
    };
    app.state_mut().state.doc.selection = Some(Selection::Edge(edge));
    app.state_mut().state.push(AppAction::StartFillet);
    app.run_steps(20);
    let snapshot = |app: &App| {
        let s = &app.state().state;
        let e = s.editor.as_ref().expect("the fillet panel is open");
        (e.edges().to_vec(), e.preview.result.clone(), s.doc.undo_depth())
    };
    let (edges, preview, depth) = snapshot(&app);
    assert_eq!(edges.len(), 1);
    let c = viewport(&app).left_bottom() + vec2(150.0, -150.0);
    let orbit = effect(&mut app, "fusion-fillet-shift-middle-drag", |app| {
        drag(app, PointerButton::Middle, c, c + vec2(50.0, 20.0), Modifiers::SHIFT)
    });
    assert_eq!(orbit, ORBIT);
    let pan = effect(&mut app, "fusion-fillet-middle-drag", |app| {
        drag(app, PointerButton::Middle, c, c + vec2(-40.0, 10.0), Modifiers::NONE)
    });
    assert_eq!(pan, PAN);
    let (edges_after, preview_after, depth_after) = snapshot(&app);
    assert_eq!(edges_after, edges, "the collector keeps its edges");
    assert_eq!(depth_after, depth, "no undo step");
    match (&preview, &preview_after) {
        (Some(a), Some(b)) => assert!(std::sync::Arc::ptr_eq(a, b), "the preview mesh is the same"),
        (a, b) => assert_eq!(a.is_some(), b.is_some(), "the preview stays as it was"),
    }
    // A left click on another top edge still adds it.
    let other = {
        let doc = &app.state().state.doc;
        doc.edges.iter().filter(|e| (e.midpoint.z - 20.0).abs() < 1e-9).find(|e| e.id != edge).unwrap().midpoint
    };
    let p = {
        let s = &app.state().state;
        let m = Vec3::new(other.x as f32, other.y as f32, other.z as f32);
        s.viewport.camera.project(m, s.viewport.rect).unwrap()
    };
    app.hover_at(p);
    app.run_steps(4);
    press(&mut app, PointerButton::Primary, p, Modifiers::NONE);
    release(&mut app, PointerButton::Primary, p, Modifiers::NONE);
    app.run_steps(10);
    assert_eq!(snapshot(&app).0.len(), 2, "the click added the edge to the collector");

    // A focused number field keeps the keys: Z does not fit, and a drag in the viewport
    // ends the edit as a click outside does, without spoiling what was typed.
    let radius = |app: &App| match &app.state().state.editor.as_ref().expect("the panel stays open").params {
        soliddraft_app::state::EditorParams::Fillet(f) => f.radius,
        other => panic!("{other:?}"),
    };
    let before = radius(&app);
    app.get_by_label("Radius value").click();
    app.run_steps(2);
    app.key_press_modifiers(Modifiers::COMMAND, Key::A);
    app.get_by_label("Radius value").type_text("3");
    app.run_steps(2);
    let typed = effect(&mut app, "fusion-fillet-z-while-typing", |app| {
        app.key_press(Key::Z);
        app.run_steps(30);
    });
    assert_eq!(typed, NOTHING, "Z typed into the field does not fit the view");
    drag(&mut app, PointerButton::Middle, c, c + vec2(30.0, 0.0), Modifiers::NONE);
    app.run_steps(10);
    let after = radius(&app);
    assert!(after == 3.0 || after == before, "the radius is the typed 3 or the untouched {before}, not {after}");
}

#[test]
fn legacy_preset_reproduces_current_navigation() {
    let (_guard, _dir) = config_dir("legacy");
    if skip_without_gpu() {
        return;
    }
    let mut app = gesture_app(box_document(), "legacy_soliddraft");
    let c = viewport(&app).left_bottom() + vec2(150.0, -150.0);
    let none = Modifiers::NONE;
    for (name, button, m, want) in [
        ("legacy-left-drag", PointerButton::Primary, none, ORBIT),
        ("legacy-middle-drag", PointerButton::Middle, none, PAN),
        ("legacy-right-drag", PointerButton::Secondary, none, PAN),
        ("legacy-shift-left-drag", PointerButton::Primary, Modifiers::SHIFT, PAN),
    ] {
        let got = effect(&mut app, name, |app| drag(app, button, c, c + vec2(50.0, 25.0), m));
        assert_eq!(got, want, "{name}");
    }
    let unchanged = modelling_unchanged(&app);
    let ctrl = effect(&mut app, "legacy-ctrl-left-drag", |app| {
        drag(app, PointerButton::Primary, c, c + vec2(50.0, 25.0), Modifiers::CTRL)
    });
    assert_eq!(ctrl, NOTHING, "no other gesture moves the camera");
    unchanged(&app, "and it selects nothing");
    app.hover_at(viewport(&app).center());
    app.run_steps(2);
    let wheel = effect(&mut app, "legacy-wheel", |app| {
        app.event(wheel_event(egui::MouseWheelUnit::Line, -2.0, Modifiers::NONE));
        app.run_steps(3);
    });
    assert!(wheel.2, "the wheel zooms");
    let fit = effect(&mut app, "legacy-z-fit", |app| {
        app.key_press(Key::Z);
        app.run_steps(40);
    });
    assert!(fit.2 || fit.1, "Z fits");
    let face = {
        let doc = &app.state().state.doc;
        doc.faces.iter().find(|f| (f.centroid.z - 20.0).abs() < 1e-9).unwrap().id
    };
    app.state_mut().state.doc.selection = Some(Selection::Face(face));
    app.run_steps(2);
    let normal = effect(&mut app, "legacy-n-normal", |app| {
        app.key_press(Key::N);
        app.run_steps(60);
    });
    assert!(normal.0, "N turns to the face");
    let dir = app.state().state.viewport.camera.direction();
    assert!(dir.dot(Vec3::Z) > 0.999, "looking straight down the top face's normal: {dir:?}");
    // A left drag in a sketch belongs to the sketch.
    app.state_mut()
        .state
        .push(AppAction::StartSketchOn(SketchPlane::Reference { plane: PlaneKind::Front, offset: 0.0 }));
    app.run_steps(60);
    let sketch = effect(&mut app, "legacy-sketch-left-drag", |app| {
        drag(app, PointerButton::Primary, c, c + vec2(50.0, 25.0), Modifiers::NONE)
    });
    assert!(!sketch.0, "no orbit in a sketch");
    // A click selects.
    app.state_mut().state.push(AppAction::ExitSketch(None));
    app.run_steps(5);
    app.state_mut().state.doc.selection = None;
    let top = {
        let s = &app.state().state;
        s.viewport.camera.project(Vec3::new(5.0, 5.0, 20.0), s.viewport.rect).unwrap()
    };
    app.hover_at(top);
    app.run_steps(4);
    press(&mut app, PointerButton::Primary, top, Modifiers::NONE);
    release(&mut app, PointerButton::Primary, top, Modifiers::NONE);
    assert!(app.state().state.doc.selection.is_some(), "a click without a drag selects");
}

#[test]
fn fusion_solidworks_onshape_end_to_end() {
    let (_guard, _dir) = config_dir("three-presets");
    if skip_without_gpu() {
        return;
    }
    let mut app = gesture_app(box_document(), "fusion");
    let c = viewport(&app).left_bottom() + vec2(150.0, -150.0);
    let none = Modifiers::NONE;
    let zoom = (false, false, true);
    for (preset, button, m, want) in [
        ("fusion", PointerButton::Middle, Modifiers::SHIFT, ORBIT),
        ("fusion", PointerButton::Middle, none, PAN),
        ("solidworks", PointerButton::Middle, none, ORBIT),
        ("solidworks", PointerButton::Middle, Modifiers::CTRL, PAN),
        ("solidworks", PointerButton::Middle, Modifiers::SHIFT, zoom),
        ("onshape", PointerButton::Secondary, none, ORBIT),
        ("onshape", PointerButton::Middle, none, PAN),
        ("onshape", PointerButton::Secondary, Modifiers::CTRL, PAN),
    ] {
        use_preset(&mut app, preset);
        let unchanged = modelling_unchanged(&app);
        let name = format!("{preset}-{button:?}-{m:?}").replace(['{', '}', ' ', ':', ','], "");
        let got = effect(&mut app, &name, |app| drag(app, button, c, c + vec2(40.0, -30.0), m));
        let got = if want == zoom { (false, false, got.2) } else { got };
        assert_eq!(got, want, "{preset}: {button:?} with {m:?}");
        unchanged(&app, &name);
    }
    // Onshape: a middle double click fits, and N turns normal to a face through normal_to.
    app.event(wheel_event(egui::MouseWheelUnit::Line, -5.0, Modifiers::NONE));
    app.run_steps(3);
    let fit = effect(&mut app, "onshape-middle-double-click", |app| {
        for _ in 0..2 {
            press(app, PointerButton::Middle, c, Modifiers::NONE);
            app.event(egui::Event::PointerButton {
                pos: c,
                button: PointerButton::Middle,
                pressed: false,
                modifiers: Modifiers::NONE,
            });
            app.step();
        }
        app.run_steps(40);
    });
    assert!(fit.2 || fit.1, "the double click fitted the view");
    let face = {
        let doc = &app.state().state.doc;
        doc.faces.iter().find(|f| (f.centroid.z - 20.0).abs() < 1e-9).unwrap().id
    };
    app.state_mut().state.doc.selection = Some(Selection::Face(face));
    app.key_press(Key::N);
    app.run_steps(60);
    assert!(app.state().state.viewport.camera.direction().dot(Vec3::Z) > 0.999, "N looks along the face");
}

#[test]
fn wheel_direction_is_global_and_independent() {
    let (_guard, _dir) = config_dir("wheel");
    if skip_without_gpu() {
        return;
    }
    let mut app = gesture_app(soliddraft_app::demo_document(), "fusion");
    let mut nav = app.state().state.navigation.clone();
    nav.wheel.invert = true;
    app.state_mut().state.set_navigation(nav).unwrap();
    for preset in ["fusion", "solidworks", "onshape", "fusion"] {
        use_preset(&mut app, preset);
        assert!(app.state().state.navigation.wheel.invert, "{preset}: still inverted");
        app.hover_at(viewport(&app).center());
        app.run_steps(2);
        let d0 = cam(&app).3;
        app.event(wheel_event(egui::MouseWheelUnit::Line, 1.0, Modifiers::NONE));
        app.run_steps(3);
        assert!(cam(&app).3 > d0, "{preset}: inverted, a forward turn zooms out");
    }
}

/// Uses a view-key profile through the app's settings path.
fn use_view_keys(app: &mut App, profile: &str) {
    let mut nav = app.state().state.navigation.clone();
    soliddraft_app::navigation::view_keys::apply_to(&mut nav, profile).unwrap();
    app.state_mut().state.set_navigation(nav).unwrap();
    app.run_steps(1);
}

fn top_face(app: &App) -> Selection {
    let doc = &app.state().state.doc;
    Selection::Face(doc.faces.iter().find(|f| (f.centroid.z - 20.0).abs() < 1e-9).unwrap().id)
}

fn view_name(app: &App) -> &'static str {
    app.state().state.viewport.camera.view_name
}

fn direction(app: &App) -> Vec3 {
    app.state().state.viewport.camera.direction()
}

fn ribbon_button(app: &mut App, label: &str) {
    app.state_mut().state.ribbon_tab = soliddraft_app::state::RibbonTab::View;
    app.run_steps(2);
    app.get_all_by_role_and_label(egui::accesskit::Role::Button, label)
        .next()
        .unwrap_or_else(|| panic!("the View ribbon has {label}"))
        .click();
    app.run_steps(40);
}

#[test]
fn inventor_held_keys_and_wheel_direction() {
    let (_guard, _dir) = config_dir("inventor");
    if skip_without_gpu() {
        return;
    }
    let mut app = gesture_app(soliddraft_app::demo_document(), "inventor");
    let c = viewport(&app).left_bottom() + vec2(150.0, -150.0);
    let none = Modifiers::NONE;
    let shift_m = effect(&mut app, "inventor-shift-middle", |app| {
        drag(app, PointerButton::Middle, c, c + vec2(40.0, 20.0), Modifiers::SHIFT)
    });
    assert_eq!(shift_m, ORBIT);
    assert_eq!(
        effect(&mut app, "inventor-middle", |app| drag(app, PointerButton::Middle, c, c + vec2(40.0, 20.0), none)),
        PAN
    );
    for (key, name, want) in [
        (Key::F4, "inventor-f4-left", ORBIT),
        (Key::F2, "inventor-f2-left", PAN),
        (Key::F3, "inventor-f3-left", (false, false, true)),
    ] {
        let got = effect(&mut app, name, |app| {
            app.key_down(key);
            app.step();
            drag(app, PointerButton::Primary, c, c + vec2(30.0, -40.0), none);
            app.key_up(key);
            app.run_steps(2);
        });
        let got = if want.2 { (false, false, got.2) } else { got };
        assert_eq!(got, want, "{name}");
    }
    app.hover_at(viewport(&app).center());
    app.run_steps(2);
    let d0 = cam(&app).3;
    app.event(wheel_event(egui::MouseWheelUnit::Line, 1.0, none));
    app.run_steps(3);
    let d1 = cam(&app).3;
    assert!(d1 > d0, "the wheel forward zooms out: {d0} to {d1}");
    app.event(wheel_event(egui::MouseWheelUnit::Line, -1.0, none));
    app.run_steps(3);
    assert!(cam(&app).3 < d1, "backward zooms in");
    let feature = app.state().state.doc.features[0].id;
    app.state_mut().state.doc.selection = Some(Selection::Feature(feature));
    app.key_down(Key::F2);
    app.step();
    app.key_up(Key::F2);
    app.run_steps(3);
    assert!(app.state().state.rename.is_some(), "an F2 tap renames");
}

#[test]
fn f_conflict_does_not_steal_front_or_fillet() {
    let (_guard, _dir) = config_dir("f-conflict");
    if skip_without_gpu() {
        return;
    }
    let mut app = gesture_app(box_document(), "solidworks");
    use_view_keys(&mut app, "solidworks");
    app.state_mut().state.push(AppAction::SetView(soliddraft_app::viewport::camera::StandardView::Isometric));
    app.run_steps(40);
    app.hover_at(viewport(&app).center());
    app.run_steps(2);
    app.event(wheel_event(egui::MouseWheelUnit::Line, -6.0, Modifiers::NONE));
    app.run_steps(3);
    let far = cam(&app).3;
    app.key_press(Key::F);
    app.run_steps(40);
    assert_eq!(view_name(&app), "Front", "F with nothing selected is Front");
    assert!((cam(&app).3 - far).abs() < 1e-3, "and only Front: the view was not fitted");
    assert!(app.state().state.editor.is_none());

    let edge = {
        let doc = &app.state().state.doc;
        doc.edges.iter().find(|e| (e.midpoint.z - 20.0).abs() < 1e-9).unwrap().id
    };
    app.state_mut().state.doc.selection = Some(Selection::Edge(edge));
    app.state_mut().state.push(AppAction::SetView(soliddraft_app::viewport::camera::StandardView::Isometric));
    app.run_steps(40);
    app.key_press(Key::F);
    app.run_steps(10);
    assert!(app.state().state.editor.is_some(), "F with an edge selected is Fillet");
    assert_eq!(view_name(&app), "Isometric", "and only Fillet");
    app.key_press(Key::Escape);
    app.run_steps(5);
    app.state_mut().state.doc.selection = None;
    app.run_steps(2);

    // Every command in the conflict, and the other views, keep their buttons.
    ribbon_button(&mut app, "Fit");
    assert!(cam(&app).3 < far, "the View ribbon's Fit fits");
    for (label, name) in [("Front", "Front"), ("Top", "Top"), ("Right", "Right"), ("Isometric", "Isometric")] {
        ribbon_button(&mut app, label);
        assert_eq!(view_name(&app), name, "{label} still works from its button");
    }
    app.state_mut().state.doc.selection = Some(top_face(&app));
    ribbon_button(&mut app, "Normal to");
    assert!(direction(&app).dot(Vec3::Z) > 0.999, "Normal to still works from its button");
    assert_eq!(app.state().state.navigation.keyboard_profile, "solidworks", "the profile stayed applied throughout");
}

#[test]
fn view_keys_fit_normal_reverse_and_principal_views() {
    let (_guard, _dir) = config_dir("view-keys");
    if skip_without_gpu() {
        return;
    }
    use soliddraft_app::viewport::camera::StandardView;
    let mut app = gesture_app(box_document(), "solidworks");
    use_view_keys(&mut app, "solidworks");
    // Ctrl+8: with a face, normal to it; with none, a note and no move.
    app.state_mut().state.doc.selection = Some(top_face(&app));
    app.key_press_modifiers(Modifiers::CTRL, Key::Num8);
    app.run_steps(60);
    assert!(direction(&app).dot(Vec3::Z) > 0.999, "Ctrl+8 looks along the face");
    app.state_mut().state.doc.selection = None;
    app.run_steps(2);
    let before = cam(&app);
    app.key_press_modifiers(Modifiers::CTRL, Key::Num8);
    app.run_steps(30);
    assert_eq!(cam(&app), before, "no face, no move");
    assert!(app.state().state.note.as_ref().is_some_and(|n| n.text.contains("planar face")));
    // Ctrl+1 to Ctrl+7.
    let views = [
        (Key::Num1, StandardView::Front),
        (Key::Num2, StandardView::Back),
        (Key::Num3, StandardView::Left),
        (Key::Num4, StandardView::Right),
        (Key::Num5, StandardView::Top),
        (Key::Num6, StandardView::Bottom),
        (Key::Num7, StandardView::Isometric),
    ];
    let axis = |v: StandardView| {
        let (yaw, pitch) = v.angles();
        let (y, p) = (yaw.to_radians(), pitch.to_radians());
        Vec3::new(p.cos() * y.cos(), p.cos() * y.sin(), p.sin())
    };
    for (key, view) in views {
        app.key_press_modifiers(Modifiers::CTRL, key);
        app.run_steps(60);
        assert_eq!(view_name(&app), view.label(), "Ctrl+{key:?}");
        assert!(direction(&app).dot(axis(view)) > 0.999, "Ctrl+{key:?} looks from {}", view.label());
    }
    // Onshape Shift+1 to Shift+7, from the physical digit even when the key is "!".
    use_view_keys(&mut app, "onshape");
    for (key, view) in views {
        app.event(egui::Event::Key {
            key: if key == Key::Num1 { Key::Exclamationmark } else { key },
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::SHIFT,
        });
        app.run_steps(60);
        assert_eq!(view_name(&app), view.label(), "Shift+{key:?}");
    }
    // Onshape N, then N again: normal, then reversed, one action per press.
    app.state_mut().state.doc.selection = Some(top_face(&app));
    app.key_press(Key::N);
    app.run_steps(60);
    let first = direction(&app);
    app.key_press(Key::N);
    app.run_steps(60);
    let second = direction(&app);
    assert!(first.dot(Vec3::Z) > 0.999 && first.dot(second) < -0.99, "{first:?} then {second:?}");
    app.state_mut().state.doc.selection = None;

    let zoomed_out = |app: &mut App| {
        app.hover_at(viewport(app).center());
        app.run_steps(2);
        app.event(wheel_event(egui::MouseWheelUnit::Line, -8.0, Modifiers::NONE));
        app.run_steps(3);
        cam(app).3
    };
    for (profile, key, m) in [
        ("fusion", Key::F6, Modifiers::NONE),
        ("inventor", Key::Home, Modifiers::NONE),
        ("blender", Key::Home, Modifiers::NONE),
    ] {
        use_view_keys(&mut app, profile);
        let far = zoomed_out(&mut app);
        app.key_press_modifiers(m, key);
        app.run_steps(60);
        assert!(cam(&app).3 < far * 0.8, "{profile} {key:?} fits");
    }
    // Solid Edge: a middle double click fits.
    use_preset(&mut app, "solid_edge");
    let far = zoomed_out(&mut app);
    let c = viewport(&app).left_bottom() + vec2(150.0, -150.0);
    for _ in 0..2 {
        press(&mut app, PointerButton::Middle, c, Modifiers::NONE);
        app.event(egui::Event::PointerButton {
            pos: c,
            button: PointerButton::Middle,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
        app.step();
    }
    app.run_steps(60);
    assert!(cam(&app).3 < far * 0.8, "Solid Edge's middle double click fits");
    // Creo Ctrl+D and NX End: the default and isometric views.
    use_view_keys(&mut app, "creo");
    app.key_press(Key::F);
    app.run_steps(40);
    app.key_press_modifiers(Modifiers::CTRL, Key::D);
    app.run_steps(60);
    assert_eq!(view_name(&app), "Isometric", "Creo's Ctrl+D");
    use_view_keys(&mut app, "nx12");
    app.key_press(Key::F);
    app.run_steps(40);
    app.key_press(Key::End);
    app.run_steps(60);
    assert_eq!(view_name(&app), "Isometric", "NX End");
    // NX F8: normal to a face, or with none the nearest standard view.
    app.state_mut().state.doc.selection = Some(top_face(&app));
    app.key_press(Key::F8);
    app.run_steps(60);
    assert!(direction(&app).dot(Vec3::Z) > 0.999, "F8 with a face");
    app.state_mut().state.doc.selection = None;
    app.state_mut().state.viewport.camera.look_along(Vec3::new(0.95, 0.2, 0.15));
    app.run_steps(60);
    app.key_press(Key::F8);
    app.run_steps(60);
    assert_eq!(view_name(&app), "Right", "F8 with nothing selected snaps to the nearest view");
}

/// Opens Preferences from the Tools menu, as a user does.
fn open_preferences(app: &mut App) {
    app.get_by_label("Tools").click();
    app.run_steps(3);
    app.get_by_label("Preferences…").click();
    app.run_steps(4);
    assert!(app.state().preferences.is_some(), "the menu opened Preferences");
}

/// Keeps a frame of the Preferences window for review.
fn preferences_frame(app: &mut App, name: &str) {
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/navigation-test/preferences");
    std::fs::create_dir_all(&out).unwrap();
    if let Ok(image) = app.render() {
        let _ = image.save(out.join(format!("{name}.png")));
    }
}

fn settings_bytes(dir: &Path) -> Vec<u8> {
    std::fs::read(dir.join("settings.json")).unwrap_or_default()
}

#[test]
fn preferences_window_opens_with_search_and_categories() {
    let (_guard, dir) = config_dir("prefs-open");
    if skip_without_gpu() {
        return;
    }
    std::fs::write(dir.join("settings.json"), r#"{"theme":"dark"}"#).unwrap();
    let mut app = gesture_app(soliddraft_app::demo_document(), "legacy_soliddraft");
    let before = settings_bytes(&dir);
    open_preferences(&mut app);
    assert!(app.query_by_label("Search settings").is_some());
    assert!(app.query_by_label("Navigation > Mouse").is_some(), "it opens on Navigation > Mouse");
    assert!(app.query_by_label("Orbit and zoom").is_some() && app.query_by_label("View keys").is_some());
    preferences_frame(&mut app, "mouse");
    app.state_mut().preferences.as_mut().unwrap().search = "wheel".into();
    app.run_steps(3);
    preferences_frame(&mut app, "search-wheel");
    app.state_mut().preferences.as_mut().unwrap().search.clear();
    app.run_steps(2);
    app.get_by_label("Close window").click();
    app.run_steps(4);
    assert!(app.state().preferences.is_none(), "the window closed");
    assert!(!app.ctx.egui_wants_keyboard_input(), "the keyboard is the app's again");
    assert_eq!(settings_bytes(&dir), before, "opening and closing changed no setting");
}

#[test]
fn mouse_page_preset_switch_updates_list_practice_and_settings() {
    let (_guard, dir) = config_dir("prefs-mouse");
    if skip_without_gpu() {
        return;
    }
    let mut app = gesture_app(box_document(), "legacy_soliddraft");
    open_preferences(&mut app);
    app.get_by_role_and_label(egui::accesskit::Role::RadioButton, "Onshape").click();
    app.run_steps(4);
    assert_eq!(app.state().state.navigation.preset_id, "onshape", "the app uses it at once");
    assert!(app.query_by_label("Orbit: Right drag").is_some());
    let saved: serde_json::Value = serde_json::from_slice(&settings_bytes(&dir)).unwrap();
    assert_eq!(saved["navigation"]["preset_id"], "onshape");
    // The viewport follows: a right drag orbits.
    app.state_mut().preferences = None;
    app.run_steps(2);
    let c = viewport(&app).left_bottom() + vec2(150.0, -150.0);
    let r_drag = effect(&mut app, "prefs-onshape-right-drag", |app| {
        drag(app, PointerButton::Secondary, c, c + vec2(50.0, 20.0), Modifiers::NONE)
    });
    assert_eq!(r_drag, ORBIT);
    drop(app);
    let app = Harness::builder()
        .with_size(vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_eframe(|cc| SolidDraftApp::new(cc, box_document(), Automation::default()));
    assert_eq!(app.state().state.navigation.preset_id, "onshape", "still Onshape after a restart");
}

#[test]
fn practice_viewport_reports_the_gesture_without_moving_the_camera() {
    let (_guard, _dir) = config_dir("prefs-practice");
    if skip_without_gpu() {
        return;
    }
    let mut app = gesture_app(box_document(), "solidworks");
    open_preferences(&mut app);
    let area = app.get_by_label("Practice area").rect();
    let before = cam(&app);
    drag(&mut app, PointerButton::Middle, area.center(), area.center() + vec2(40.0, 20.0), Modifiers::NONE);
    assert_eq!(app.state().preferences.as_ref().unwrap().practice_label, "Orbit");
    drag(&mut app, PointerButton::Middle, area.center(), area.center() + vec2(40.0, 20.0), Modifiers::CTRL);
    assert_eq!(app.state().preferences.as_ref().unwrap().practice_label, "Pan");
    app.hover_at(area.center());
    app.step();
    app.event(wheel_event(egui::MouseWheelUnit::Line, 2.0, Modifiers::NONE));
    app.run_steps(3);
    assert_eq!(app.state().preferences.as_ref().unwrap().practice_label, "Zoom");
    assert_eq!(cam(&app), before, "the part's camera did not move");
}

#[test]
fn orbit_zoom_page_enabled_controls_change_camera() {
    let (_guard, _dir) = config_dir("prefs-orbit-zoom");
    if skip_without_gpu() {
        return;
    }
    let mut app = gesture_app(box_document(), "solidworks");
    let c = viewport(&app).left_bottom() + vec2(150.0, -150.0);
    let turn = |app: &mut App| {
        let before = cam(app);
        drag(app, PointerButton::Middle, c, c + vec2(40.0, 0.0), Modifiers::NONE);
        cam(app).0 - before.0
    };
    let once = turn(&mut app);
    open_preferences(&mut app);
    app.get_by_label("Orbit and zoom").click();
    app.run_steps(3);
    preferences_frame(&mut app, "orbit-zoom");
    app.state_mut().preferences.as_mut().unwrap().edit(|s| s.camera.orbit_sensitivity = 2.0);
    app.run_steps(3);
    assert_eq!(app.state().state.navigation.camera.orbit_sensitivity, 2.0, "saved and used");
    app.state_mut().preferences = None;
    app.run_steps(2);
    let twice = turn(&mut app);
    assert!((twice - 2.0 * once).abs() < 1e-4, "the same drag turns twice as far: {once} then {twice}");
}

#[test]
fn view_keys_page_shows_profiles_and_conflicts() {
    let (_guard, _dir) = config_dir("prefs-view-keys");
    if skip_without_gpu() {
        return;
    }
    let mut app = gesture_app(box_document(), "solidworks");
    open_preferences(&mut app);
    app.get_by_label("View keys").click();
    app.run_steps(3);
    app.get_by_role_and_label(egui::accesskit::Role::RadioButton, "SOLIDWORKS").click();
    app.run_steps(4);
    assert_eq!(app.state().state.navigation.keyboard_profile, "solidworks");
    assert!(app.query_all_by_label_contains("Fit: F — blocked").next().is_some());
    preferences_frame(&mut app, "view-keys");
    app.state_mut().preferences = None;
    app.run_steps(2);
    // Every command in the conflict still runs from its button.
    ribbon_button(&mut app, "Fit");
    ribbon_button(&mut app, "Front");
    assert_eq!(view_name(&app), "Front");
}

#[test]
fn preferences_does_not_disturb_existing_settings() {
    let (_guard, dir) = config_dir("prefs-undisturbed");
    if skip_without_gpu() {
        return;
    }
    let mut app = gesture_app(soliddraft_app::demo_document(), "fusion");
    let root = dir.join("recovery-root");
    app.state_mut().start_recovery(&root, Default::default());
    open_preferences(&mut app);
    app.state_mut().preferences = None;
    app.run_steps(2);
    // The existing windows still open, from the commands their menu entries run.
    let command = |id: &str| soliddraft_app::commands::all().iter().find(|c| c.id == id).unwrap();
    app.state_mut().state.run_command(command("file_locations"));
    app.run_steps(4);
    assert!(app.state().state.locations_dialog.is_some(), "File locations opens");
    app.state_mut().state.locations_dialog = None;
    app.state_mut().state.push(AppAction::ShowRecovery);
    app.run_steps(4);
    assert!(app.query_by_label("Recover documents").is_some(), "the recovery window opens");
    app.state_mut().state.run_command(command("provider_settings"));
    app.run_steps(4);
    assert!(app.state().state.provider_dialog.is_some(), "the Assistant settings open");
    assert!(app.query_by_label("Assistant settings").is_some());
}
