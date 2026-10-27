//! Assemblies in the app: File > New assembly, Insert component with two part files,
//! moving one by typed placement with undo, saving, and reopening the file to the same
//! components; Make assembly from the open part; and the same flow in the real window,
//! with the parts dropped on it and one picked in the viewport. The window test writes
//! its frames to `target/assembly-test/` and skips itself when no GPU adapter is
//! available (plain CI runners); the others need no GPU.

use egui::{PointerButton, vec2};
use egui_dock::DockState;
use egui_kittest::Harness;
use glam::{DQuat, DVec3};
use soliddraft_app::app::{Automation, SolidDraftApp, Tab};
use soliddraft_app::assembly::{self, AssemblyAction, AssemblySession};
use soliddraft_app::document::{Command, Document, ExtrudeParams, PlaneKind, ProfileKind};
use soliddraft_app::file;
use soliddraft_app::state::{AppAction, AppState};
use soliddraft_assembly::{Freshness, LoadState, OccurrencePath, Placement};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// One test at a time touches the recent-files list and the GPU.
static TURN: Mutex<()> = Mutex::new(());

fn turn() -> std::sync::MutexGuard<'static, ()> {
    TURN.lock().unwrap_or_else(|e| e.into_inner())
}

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soliddraft-assembly-app-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("parts")).unwrap();
    file::set_config_dir(Some(dir.join("config")));
    dir
}

/// A block `w` by `d` on the top plane, `h` tall, saved at `path`.
fn part(path: &Path, w: f64, d: f64, h: f64) -> PathBuf {
    let mut doc = soliddraft_app::new_document();
    let params = ExtrudeParams {
        sketch: None,
        profile: ProfileKind::Rectangle { width: w, height: d },
        plane: PlaneKind::Top,
        depth: h,
        flip: false,
        end: Default::default(),
    };
    doc.apply(Command::AddExtrude { params, via_assistant: false }).unwrap();
    file::save(&mut doc, path).unwrap();
    path.to_path_buf()
}

fn session(state: &mut AppState) -> &mut AssemblySession {
    state.assembly.session.as_mut().expect("an open assembly")
}

/// The root's components as (label, local placement), in tree order.
fn components(s: &AssemblySession) -> Vec<(String, Placement)> {
    s.components().into_iter().map(|r| (s.doc.label(r), s.doc.table().local(r))).collect()
}

fn path_of(s: &AssemblySession, i: usize) -> OccurrencePath {
    s.doc.table().path(s.components()[i])
}

/// Runs frames until the opened assembly is shown.
fn wait_open(state: &mut AppState, dock: &mut DockState<Tab>, ctx: &egui::Context) {
    let started = Instant::now();
    while state.assembly.opening.is_some() {
        assert!(started.elapsed() < Duration::from_secs(60), "the assembly did not open");
        assembly::frame(state, dock, ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    session(state).doc.finish_loading();
    assembly::frame(state, dock, ctx);
}

fn close(a: Placement, b: Placement) -> bool {
    a.translation.distance(b.translation) < 1e-9 && a.rotation.dot(b.rotation).abs() > 1.0 - 1e-12
}

#[test]
fn an_assembly_is_made_from_two_parts_saved_and_reopened() {
    let _turn = turn();
    let dir = folder("flow");
    let block = part(&dir.join("parts/block.dwprt"), 40.0, 30.0, 10.0);
    let pin = part(&dir.join("parts/pin.dwprt"), 10.0, 10.0, 60.0);
    let mut state = AppState::new(soliddraft_app::new_document());
    let mut dock = DockState::new(vec![Tab::Viewport]);
    let ctx = egui::Context::default();

    // File > New assembly opens an empty one in a tab beside the part.
    assembly::apply(&mut state, &mut dock, AssemblyAction::New);
    assert!(state.assembly.is_active() && dock.find_tab(&Tab::Assembly).is_some());
    assert!(session(&mut state).components().is_empty());

    // Insert component: both parts, the second beside the first.
    assembly::apply(&mut state, &mut dock, AssemblyAction::Insert(Some(vec![block.clone(), pin.clone()])));
    let s = session(&mut state);
    s.doc.finish_loading();
    assembly::frame(&mut state, &mut dock, &ctx);
    let s = session(&mut state);
    let placed = components(s);
    assert_eq!(placed.len(), 2, "{placed:?}");
    assert!(placed[0].0.starts_with("block<") && placed[1].0.starts_with("pin<"), "{placed:?}");
    assert!(
        s.doc
            .store()
            .iter()
            .filter(|(_, d)| d.kind == soliddraft_document::DocKind::Part)
            .all(|(_, d)| d.state == LoadState::DisplayReady)
    );
    assert!(
        s.doc.store().iter().all(|(_, d)| d.freshness != Freshness::Missing),
        "an unsaved assembly has no folder to look in, so it does not call its parts missing"
    );
    let rows = s.components();
    let (a, b) = (s.doc.table().bounds(rows[0]), s.doc.table().bounds(rows[1]));
    assert!(a.max.x < b.min.x, "the pin sits beside the block: {a:?} {b:?}");
    assert_eq!(s.selected.as_ref(), Some(&path_of(s, 1)), "the last inserted is selected");
    assert!(s.doc.is_dirty());

    // Typed placement: up 5 mm and a quarter turn about z, one undo step.
    let first = path_of(s, 0);
    let moved = Placement::new(DQuat::from_rotation_z(90f64.to_radians()), DVec3::new(0.0, 0.0, 5.0));
    assembly::apply(&mut state, &mut dock, AssemblyAction::Place(first.clone(), moved));
    assert!(close(components(session(&mut state))[0].1, moved));
    assembly::apply(&mut state, &mut dock, AssemblyAction::Undo);
    assert!(close(components(session(&mut state))[0].1, Placement::IDENTITY));
    assembly::apply(&mut state, &mut dock, AssemblyAction::Redo);
    assert!(close(components(session(&mut state))[0].1, moved));

    // Save: the references become relative to the assembly's folder.
    let file = dir.join("rig.dwasm");
    assembly::apply(&mut state, &mut dock, AssemblyAction::Save(Some(file.clone())));
    let s = session(&mut state);
    assert!(file.is_file() && !s.doc.is_dirty(), "{:?}", state.note.as_ref().map(|n| n.text.clone()));
    let saved = components(session(&mut state));
    let paths: Vec<String> =
        session(&mut state).doc.assembly().sources.iter().map(|s| s.document.path.clone()).collect();
    assert_eq!(paths, ["parts/block.dwprt", "parts/pin.dwprt"]);
    assert_eq!(
        file::recent().first().and_then(|p| p.file_name().map(|n| n.to_owned())),
        file.file_name().map(Into::into)
    );

    // Reopen, as File > Open does: the same components at the same placements, shown
    // from the file's own copies.
    state.assembly.session = None;
    assembly::apply(&mut state, &mut dock, AssemblyAction::Open(file.clone()));
    wait_open(&mut state, &mut dock, &ctx);
    let s = session(&mut state);
    let reopened = components(s);
    assert_eq!(reopened.len(), 2);
    for ((l0, p0), (l1, p1)) in saved.iter().zip(&reopened) {
        assert_eq!(l0, l1);
        assert!(close(*p0, *p1), "{l0}: {p0:?} {p1:?}");
    }
    assert_eq!(s.doc.path(), Some(file.as_path()));
    assert!(!s.doc.is_dirty());
    assert_eq!(s.doc.table().counts().parts, 2);
    let ready = s.doc.store().iter().filter(|(_, d)| d.state == LoadState::DisplayReady).count();
    assert_eq!(ready, 2, "both parts show from the saved copies");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn make_assembly_places_the_open_part() {
    let _turn = turn();
    let dir = folder("from-part");
    let block = part(&dir.join("parts/block.dwprt"), 40.0, 30.0, 10.0);

    // An unsaved part is refused: the assembly would have no file to reference.
    let mut state = AppState::new(soliddraft_app::demo_document());
    let mut dock = DockState::new(vec![Tab::Viewport]);
    assembly::apply(&mut state, &mut dock, AssemblyAction::FromPart);
    assert!(state.assembly.session.is_none());
    assert!(state.note.as_ref().is_some_and(|n| n.text.contains("Save the part")));

    let doc: Document = file::open(&block, soliddraft_app::kernel(), None).unwrap();
    state.doc = doc;
    assembly::apply(&mut state, &mut dock, AssemblyAction::FromPart);
    let s = session(&mut state);
    s.doc.finish_loading();
    assert_eq!(s.doc.assembly().name, "block assembly");
    let placed = components(s);
    assert_eq!(placed.len(), 1);
    assert!(placed[0].0.starts_with("block<") && close(placed[0].1, Placement::IDENTITY), "{placed:?}");
    let _ = std::fs::remove_dir_all(dir);
}

fn gpu_available() -> bool {
    let _ = env_logger::try_init();
    let instance = egui_wgpu::wgpu::Instance::default();
    !pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all())).is_empty()
}

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/assembly-test");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

type App<'a> = Harness<'a, SolidDraftApp>;

fn save_frame(harness: &mut App<'_>, name: &str) {
    let image = harness.render().expect("render");
    let path = out_dir().join(format!("{name}.png"));
    image.save(&path).expect("save png");
    eprintln!("wrote {}", path.display());
}

/// A file dropped on the window, as the windowing layer hands it over.
#[derive(Debug)]
struct Dropped(PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

/// Steps the window until the open assembly has loaded every part and drawn it.
fn settle(harness: &mut App<'_>) {
    let started = Instant::now();
    loop {
        harness.run_steps(4);
        let st = &harness.state().state;
        let loaded = st.assembly.opening.is_none()
            && st.assembly.session.as_ref().is_some_and(|s| !s.doc.is_loading() && s.doc.store().len() > 1);
        if loaded {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(60), "the assembly did not load");
    }
    harness.run_steps(12);
}

#[test]
fn the_window_builds_an_assembly_from_dropped_parts() {
    let _turn = turn();
    if !gpu_available() {
        eprintln!("skipping: no GPU adapter");
        return;
    }
    let dir = folder("window");
    let block = part(&dir.join("parts/block.dwprt"), 60.0, 40.0, 12.0);
    let pin = part(&dir.join("parts/pin.dwprt"), 12.0, 12.0, 50.0);
    let mut harness: App<'static> = Harness::builder()
        .with_size(vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .with_render_every_step(true)
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_eframe(|cc| SolidDraftApp::new(cc, soliddraft_app::demo_document(), Automation::default()));
    harness.run_steps(3);

    // File > New assembly, then both parts dropped on the window.
    harness.state_mut().state.run_command(soliddraft_app::commands::get("new_assembly").unwrap());
    harness.run_steps(3);
    assert!(harness.state().state.assembly.is_active());
    for f in [&block, &pin] {
        harness.input_mut().dropped_files.push(Arc::new(Dropped(f.clone())));
    }
    settle(&mut harness);
    let st = &harness.state().state;
    let s = st.assembly.session.as_ref().unwrap();
    let names: Vec<String> = components(s).into_iter().map(|(l, _)| l).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names[0].starts_with("block<") && names[1].starts_with("pin<"), "{names:?}");
    save_frame(&mut harness, "1-inserted");

    // A click on the block in the viewport selects it.
    let (point, rect) = {
        let s = harness.state().state.assembly.session.as_ref().unwrap();
        let b = s.doc.table().bounds(s.components()[0]);
        let top = DVec3::new(b.center().x, b.center().y, b.max.z);
        (s.viewport.camera.project(top.as_vec3(), s.viewport.rect).unwrap(), s.viewport.rect)
    };
    assert!(rect.contains(point), "{point:?} in {rect:?}");
    harness.hover_at(point);
    harness.run_steps(6);
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos: point,
            button: PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        harness.run_steps(1);
    }
    harness.run_steps(3);
    let first = {
        let s = harness.state().state.assembly.session.as_ref().unwrap();
        let first = s.doc.table().path(s.components()[0]);
        assert_eq!(s.selected.as_ref(), Some(&first), "the click picked the block");
        assert!(s.draft.as_ref().is_some_and(|d| d.path == first), "the property panel edits its placement");
        first
    };

    // Typed placement: lift the block, then save with Ctrl+S's command.
    let lifted = Placement::translation(DVec3::new(0.0, 0.0, 30.0));
    harness.state_mut().state.push(AppAction::Assembly(AssemblyAction::Place(first, lifted)));
    harness.run_steps(6);
    save_frame(&mut harness, "2-placed");
    let file = dir.join("window.dwasm");
    harness.state_mut().state.push(AppAction::Assembly(AssemblyAction::Save(Some(file.clone()))));
    harness.run_steps(3);
    assert!(file.is_file());

    // Close the tab's session and open the file again through File > Open.
    harness.state_mut().state.assembly.session = None;
    harness.state_mut().state.push(AppAction::Open(Some(file.clone())));
    settle(&mut harness);
    let st = &harness.state().state;
    assert!(st.assembly.is_active());
    let s = st.assembly.session.as_ref().unwrap();
    let back = components(s);
    assert_eq!(back.len(), 2);
    assert!(close(back[0].1, lifted), "{back:?}");
    assert!(back[1].0.starts_with("pin<"));
    save_frame(&mut harness, "3-reopened");
    drop(harness);
    let _ = std::fs::remove_dir_all(dir);
}
