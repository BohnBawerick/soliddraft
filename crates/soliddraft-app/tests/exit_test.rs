//! Exit tests, driven through egui_kittest against the real wgpu pipeline.
//!
//! Phase 1: open the app, extrude, orbit, click a face and see it named.
//! Phase 2: build the sample L-bracket by hand through the interface, save it,
//! reopen it, change the base thickness and see every downstream feature
//! rebuild; then take a feature away and repair the reference that lost its
//! edge. Frames are written to `target/exit-test/` for review.
//!
//! Skips itself when no GPU adapter is available (plain CI runners).

use egui::{Key, Modifiers, Pos2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use glam::Vec3;
use soliddraft_app::app::{Automation, SolidDraftApp};
use soliddraft_app::chat::ChatState;
use soliddraft_app::document::{FeatureKind, FeatureStatus, Selection};
use soliddraft_app::sketch::{P2, SolveStatus};
use soliddraft_app::sketcher::Frame;
use soliddraft_assistant::{MockProvider, MockTurn};
use std::path::PathBuf;

fn gpu_available() -> bool {
    let _ = env_logger::try_init();
    let instance = egui_wgpu::wgpu::Instance::default();
    !pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all())).is_empty()
}

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/exit-test");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

type App<'a> = Harness<'a, SolidDraftApp>;

fn harness() -> App<'static> {
    Harness::builder()
        .with_size(vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .with_render_every_step(true)
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_eframe(|cc| {
            SolidDraftApp::new(
                cc,
                soliddraft_app::document::Document::new(
                    "Part1",
                    std::sync::Arc::new(soliddraft_kernel_smlib::SmlibKernel::new()),
                ),
                Automation::default(),
            )
        })
}

fn save(harness: &mut App<'_>, name: &str) {
    let image = harness.render().expect("render");
    let path = out_dir().join(format!("{name}.png"));
    image.save(&path).expect("save png");
    eprintln!("wrote {}", path.display());
}

fn viewport_rect(harness: &App<'_>) -> egui::Rect {
    harness.state().state.viewport.rect
}

/// Orbits the camera to look from `yaw` and `pitch` degrees, then fits the part.
fn look_from(harness: &mut App<'_>, yaw: f32, pitch: f32) {
    {
        let cam = &mut harness.state_mut().state.viewport.camera;
        cam.orbit(0.0, 0.0);
        cam.yaw = yaw.to_radians();
        cam.pitch = pitch.to_radians();
    }
    harness.key_press(Key::Z);
    settle(harness);
}

/// Lets camera moves and the one-frame pick lag settle.
fn settle(harness: &mut App<'_>) {
    harness.run_steps(30);
}

fn click_at(harness: &mut App<'_>, pos: Pos2) {
    harness.hover_at(pos);
    harness.run_steps(3);
    harness.event(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Default::default(),
    });
    harness.step();
    harness.event(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Default::default(),
    });
    harness.run_steps(3);
}

/// Screen position of a world point through the current camera.
fn screen_of(harness: &App<'_>, world: Vec3) -> Pos2 {
    let st = &harness.state().state;
    st.viewport.camera.project(world, st.viewport.rect).expect("point in front of the camera")
}

/// Screen position of a point of the open sketch's plane.
fn sketch_screen(harness: &App<'_>, p: P2) -> Pos2 {
    let st = &harness.state().state;
    let s = st.sketch.as_ref().expect("a sketch is open");
    Frame { plane: s.plane, camera: &st.viewport.camera, rect: st.viewport.rect }.screen(p).expect("point on screen")
}

fn sketch_click(harness: &mut App<'_>, p: P2) {
    let pos = sketch_screen(harness, p);
    click_at(harness, pos);
}

fn hover_sketch(harness: &mut App<'_>, p: P2) {
    let pos = sketch_screen(harness, p);
    harness.hover_at(pos);
    harness.run_steps(2);
}

/// Types a value into the dimension popup the smart dimension tool opened.
fn type_dimension(harness: &mut App<'_>, value: &str) {
    harness.run_steps(2);
    let field = harness.get_by_label("Dimension value");
    field.focus();
    harness.run_steps(2);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.get_by_label("Dimension value").type_text(value);
    harness.run_steps(2);
    harness.key_press(Key::Enter);
    harness.run_steps(4);
}

/// Sets a numeric field of the property panel by typing into it.
fn set_field(harness: &mut App<'_>, label: &str, value: &str) {
    let name = format!("{label} value");
    harness.get_by_label(&name).click();
    harness.run_steps(2);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.get_by_label(&name).type_text(value);
    harness.run_steps(2);
    harness.key_press(Key::Enter);
    harness.run_steps(3);
}

/// Dimensions a line picked at `on_line` and places the value at `at`.
fn dimension_line(harness: &mut App<'_>, on_line: P2, at: P2, value: &str) {
    harness.key_press(Key::D);
    harness.run_steps(2);
    sketch_click(harness, on_line);
    sketch_click(harness, at);
    type_dimension(harness, value);
}

fn open_sketch_on_plane(harness: &mut App<'_>, plane: &str) {
    click_button(harness, "Sketch");
    // The plane is offered twice while picking: as a tree row and as a button in the
    // property panel. The tree row comes first.
    harness
        .get_all_by_role_and_label(egui::accesskit::Role::Button, plane)
        .next()
        .expect("a plane row to pick")
        .click();
    settle(harness);
    assert!(harness.state().state.in_sketch(), "a sketch should be open on {plane}");
}

/// Clicks a planar face of the body at a world point and starts a sketch on it.
fn open_sketch_on_face(harness: &mut App<'_>, on_face: Vec3) {
    let pos = screen_of(harness, on_face);
    click_at(harness, pos);
    let sel = harness.state().state.doc.selection;
    assert!(matches!(sel, Some(Selection::Face(_))), "expected a face under {on_face:?}, got {sel:?}");
    click_button(harness, "Sketch");
    settle(harness);
    assert!(harness.state().state.in_sketch(), "a sketch should be open on the face");
}

/// The GPU harnesses share one adapter; one test at a time keeps a panic in one
/// from tearing down a device another is drawing with.
static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu_lock() -> std::sync::MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

/// An empty settings directory of the test's own, so the developer's real panel
/// layout and recent list cannot move the widgets the test clicks.
fn own_config_dir(name: &str) -> PathBuf {
    let dir = out_dir().join(format!("config-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    soliddraft_app::file::set_config_dir(Some(dir.clone()));
    dir
}

/// Clicks a feature row in the tree, scrolling it into view first.
fn click_row(harness: &mut App<'_>, name: &str) {
    harness.get_by_role_and_label(egui::accesskit::Role::Button, name).scroll_to_me();
    harness.run_steps(3);
    harness.get_by_role_and_label(egui::accesskit::Role::Button, name).click();
    harness.run_steps(3);
}

/// Clicks the first button with this label; ribbon items come before panel buttons.
fn click_button(harness: &mut App<'_>, label: &str) {
    harness
        .get_all_by_role_and_label(egui::accesskit::Role::Button, label)
        .next()
        .unwrap_or_else(|| panic!("no button labelled {label}"))
        .click();
    harness.run_steps(3);
}

fn feature_names(harness: &App<'_>) -> Vec<String> {
    harness.state().state.doc.features.iter().map(|f| f.name.clone()).collect()
}

fn feature_status(harness: &App<'_>, name: &str) -> FeatureStatus {
    harness
        .state()
        .state
        .doc
        .features
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no feature {name}"))
        .status
        .clone()
}

fn volume(harness: &App<'_>) -> f64 {
    let doc = &harness.state().state.doc;
    let body = doc.display_body().expect("a body");
    doc.kernel().mass_properties(body).unwrap().volume
}

#[test]
fn open_extrude_orbit_click_face() {
    if !gpu_available() {
        eprintln!("no GPU adapter; skipping the exit test");
        return;
    }
    let _gpu = gpu_lock();
    own_config_dir("open_extrude_orbit_click_face");
    let mut harness = harness();
    harness.run_steps(3);
    save(&mut harness, "01-empty");

    // Extrude from the ribbon with the default rectangle profile, then apply with Enter.
    click_button(&mut harness, "Extrude");
    assert!(harness.state().state.editor.is_some(), "the extrude editor should open");
    save(&mut harness, "02-extrude-editor");
    harness.key_press(Key::Enter);
    harness.run_steps(3);
    let doc = &harness.state().state.doc;
    assert_eq!(doc.features.len(), 1);
    assert_eq!(doc.features[0].name, "Boss-Extrude1");
    assert!(doc.triangle_count() >= 12);
    harness.key_press(Key::Escape);
    harness.key_press(Key::Z);
    harness.run_steps(12);
    save(&mut harness, "03-extruded");

    // Orbit by dragging in the viewport.
    let rect = viewport_rect(&harness);
    let yaw_before = harness.state().state.viewport.camera.yaw;
    let start = rect.center() + vec2(-120.0, 60.0);
    harness.drag_at(start);
    for i in 1..=8 {
        harness.hover_at(start + vec2(20.0 * i as f32, -6.0 * i as f32));
        harness.step();
    }
    harness.drop_at(start + vec2(160.0, -48.0));
    harness.run_steps(3);
    let yaw_after = harness.state().state.viewport.camera.yaw;
    assert!((yaw_after - yaw_before).abs() > 0.1, "orbit should change the camera yaw");
    harness.key_press(Key::I);
    harness.run_steps(12);

    // Hover and click the face under the viewport centre; it must be named.
    let centre = rect.center();
    harness.hover_at(centre);
    harness.run_steps(4);
    let hover = harness.state().state.hover_info.clone().expect("hovering the body should name a face");
    assert!(hover.title.starts_with("Boss-Extrude1 · "), "hover title: {}", hover.title);
    save(&mut harness, "04-hover");
    click_at(&mut harness, centre);
    let sel = harness.state().state.doc.selection;
    assert!(matches!(sel, Some(Selection::Face(_))), "selection: {sel:?}");
    let info = harness.state().state.doc.describe(sel.unwrap()).unwrap();
    assert!(info.title.starts_with("Boss-Extrude1 · "), "selected title: {}", info.title);
    save(&mut harness, "05-face-selected");

    // The tree row selects the whole feature.
    harness.get_by_role_and_label(egui::accesskit::Role::Button, "Boss-Extrude1").click();
    harness.run_steps(3);
    assert_eq!(harness.state().state.doc.selection, Some(Selection::Feature(1)));
    save(&mut harness, "06-feature-selected");

    // Undo removes it, redo brings it back.
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    harness.run_steps(3);
    assert!(harness.state().state.doc.features.is_empty());
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Y);
    harness.run_steps(3);
    assert_eq!(harness.state().state.doc.features.len(), 1);
}

#[test]
fn demo_part_renders() {
    if !gpu_available() {
        return;
    }
    let _gpu = gpu_lock();
    own_config_dir("demo_part_renders");
    let mut harness = Harness::builder()
        .with_size(vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .with_render_every_step(true)
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_eframe(|cc| {
            let mut app = SolidDraftApp::new(
                cc,
                soliddraft_app::demo_document_on(std::sync::Arc::new(soliddraft_kernel_smlib::SmlibKernel::new())),
                Automation::default(),
            );
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
        });
    harness.run_steps(14);
    assert_eq!(harness.state().state.doc.features.len(), 3);
    assert_eq!(harness.state().state.doc.meshes.len(), 1, "the demo part is one merged body");
    save(&mut harness, "07-demo-part");
    // A message typed in the composer comes back as a proposal card; Apply runs it
    // through the command layer and Undo takes it back.
    harness.state_mut().state.chat.draft = "Add a Ø20 boss, 24 mm tall from the Top plane, on the base.".into();
    harness.get_by_label("Send").click();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
    while harness.state().state.chat.busy() && std::time::Instant::now() < deadline {
        harness.step();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    harness.run_steps(3);
    assert!(
        harness.state().state.chat.book.get("P1").is_some(),
        "no proposal: {:#?}",
        harness.state().state.chat.messages
    );
    harness.get_by_label("Preview").click();
    harness.run_steps(3);
    assert!(harness.state().state.preview_body().is_some(), "Preview shows the part after the proposal");
    save(&mut harness, "08-proposal-preview");
    harness.get_by_label("Apply").click();
    harness.run_steps(3);
    assert_eq!(harness.state().state.doc.features.len(), 5, "a sketch and an extrude");
    assert!(harness.state().state.doc.features[4].via_assistant);
    save(&mut harness, "08-proposal-applied");
    harness.get_by_label("Undo P1").click();
    harness.run_steps(3);
    assert_eq!(harness.state().state.doc.features.len(), 3);
    harness.state_mut().state.viewport.display = soliddraft_app::viewport::render::DisplayMode::Wireframe;
    harness.run_steps(3);
    save(&mut harness, "09-wireframe");
    let _ = pos2(0.0, 0.0);
}

/// The phase 2 exit test: the sample bracket by hand, saved, reopened, the base
/// thickness changed, every downstream feature rebuilt; then a repair.
#[test]
fn bracket_by_hand_saved_reopened_rebuilt_and_repaired() {
    if !gpu_available() {
        eprintln!("no GPU adapter; skipping the exit test");
        return;
    }
    let _gpu = gpu_lock();
    let dir = out_dir().join("bracket");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    soliddraft_app::file::set_config_dir(Some(dir.clone()));
    let mut harness = harness();
    harness.run_steps(3);

    // 1. Base: a 120 x 90 rectangle on the Top plane, fully defined, extruded 10 mm.
    open_sketch_on_plane(&mut harness, "Top plane");
    harness.key_press(Key::R);
    harness.run_steps(2);
    sketch_click(&mut harness, P2::ZERO);
    hover_sketch(&mut harness, P2::new(118.0, 88.0));
    save(&mut harness, "10-rectangle-rubber-band");
    sketch_click(&mut harness, P2::new(118.0, 88.0));
    {
        let s = harness.state().state.sketch.as_ref().unwrap();
        assert_eq!(s.sketch.drawn().count(), 4, "four lines");
        assert_eq!(s.sketch.status, SolveStatus::UnderDefined);
        let coincident = s
            .sketch
            .constraints
            .iter()
            .filter(|c| matches!(c.kind, soliddraft_app::sketch::ConstraintKind::Coincident { .. }))
            .count();
        assert_eq!(coincident, 5, "four corners joined and one on the origin: {:?}", s.sketch.constraints);
    }
    dimension_line(&mut harness, P2::new(40.0, 0.0), P2::new(60.0, -14.0), "120");
    dimension_line(&mut harness, P2::new(120.0, 30.0), P2::new(136.0, 45.0), "90");
    {
        let s = harness.state().state.sketch.as_ref().unwrap();
        assert_eq!(s.sketch.status, SolveStatus::FullyDefined, "free: {:?}", s.sketch.free);
        let (lo, hi) = s.sketch.bounds().unwrap();
        assert!((hi.u - lo.u - 120.0).abs() < 1e-6 && (hi.v - lo.v - 90.0).abs() < 1e-6, "{lo:?} {hi:?}");
    }
    save(&mut harness, "11-base-sketch-fully-defined");
    click_button(&mut harness, "Extrude");
    assert!(!harness.state().state.in_sketch());
    assert!(matches!(
        harness.state().state.editor.as_ref().map(|e| &e.params),
        Some(soliddraft_app::state::EditorParams::Extrude(_))
    ));
    set_field(&mut harness, "Depth", "10");
    harness.key_press(Key::Enter);
    settle(&mut harness);
    assert_eq!(feature_names(&harness), ["Sketch1", "Boss-Extrude1"]);
    harness.key_press(Key::I);
    harness.key_press(Key::Z);
    settle(&mut harness);
    save(&mut harness, "12-base");

    // 2. Flange: a rectangle on the base's front face, extruded into the body 10 mm, 60 tall.
    open_sketch_on_face(&mut harness, Vec3::new(60.0, 0.0, 5.0));
    {
        let s = harness.state().state.sketch.as_ref().unwrap();
        assert!((s.plane.normal.y + 1.0).abs() < 1e-6, "front face normal: {:?}", s.plane.normal);
    }
    harness.key_press(Key::R);
    harness.run_steps(2);
    // The face frame has u along +X and v up; the origin is the world origin projected.
    sketch_click(&mut harness, P2::new(0.0, 0.0));
    sketch_click(&mut harness, P2::new(120.0, 60.0));
    dimension_line(&mut harness, P2::new(40.0, 60.0), P2::new(60.0, 72.0), "120");
    dimension_line(&mut harness, P2::new(120.0, 20.0), P2::new(134.0, 30.0), "60");
    click_button(&mut harness, "Extrude");
    set_field(&mut harness, "Depth", "10");
    harness.get_by_label("Flip direction").click();
    harness.run_steps(4);
    save(&mut harness, "13-flange-preview");
    harness.key_press(Key::Enter);
    settle(&mut harness);
    assert_eq!(feature_names(&harness), ["Sketch1", "Boss-Extrude1", "Sketch2", "Boss-Extrude2"]);
    assert_eq!(feature_status(&harness, "Boss-Extrude2"), FeatureStatus::Ok);
    let v_l = volume(&harness);
    assert!((v_l - (120.0 * 90.0 * 10.0 + 120.0 * 50.0 * 10.0)).abs() < 1.0, "L volume {v_l}");

    // 3. Boss: a Ø24 circle on the base's top face, 24 mm tall. The flange hides
    // the top face from the front, so look from behind.
    look_from(&mut harness, 45.0, 35.0);
    open_sketch_on_face(&mut harness, Vec3::new(85.0, 55.0, 10.0));
    harness.key_press(Key::C);
    harness.run_steps(2);
    sketch_click(&mut harness, P2::new(85.0, 55.0));
    sketch_click(&mut harness, P2::new(96.0, 55.0));
    harness.key_press(Key::D);
    harness.run_steps(2);
    let diag = std::f64::consts::FRAC_1_SQRT_2;
    sketch_click(&mut harness, P2::new(85.0 + 11.0 * diag, 55.0 + 11.0 * diag));
    sketch_click(&mut harness, P2::new(104.0, 74.0));
    type_dimension(&mut harness, "24");
    {
        let s = harness.state().state.sketch.as_ref().unwrap();
        let circle = s.sketch.drawn().next().unwrap();
        assert!((circle.radius().unwrap() - 12.0).abs() < 1e-6, "radius {}", circle.radius().unwrap());
    }
    click_button(&mut harness, "Extrude");
    set_field(&mut harness, "Depth", "24");
    harness.key_press(Key::Enter);
    settle(&mut harness);
    assert_eq!(feature_status(&harness, "Boss-Extrude3"), FeatureStatus::Ok);
    save(&mut harness, "14-boss");

    // 4. Bore: Ø12 through the boss, sketched on its top face and cut through all.
    look_from(&mut harness, 45.0, 35.0);
    open_sketch_on_face(&mut harness, Vec3::new(85.0, 55.0, 34.0));
    harness.key_press(Key::C);
    harness.run_steps(2);
    sketch_click(&mut harness, P2::new(85.0, 55.0));
    sketch_click(&mut harness, P2::new(91.0, 55.0));
    harness.key_press(Key::D);
    harness.run_steps(2);
    sketch_click(&mut harness, P2::new(85.0 + 6.0 * diag, 55.0 + 6.0 * diag));
    sketch_click(&mut harness, P2::new(100.0, 70.0));
    type_dimension(&mut harness, "12");
    click_button(&mut harness, "Cut");
    harness.run_steps(2);
    assert!(matches!(
        harness.state().state.editor.as_ref().map(|e| &e.params),
        Some(soliddraft_app::state::EditorParams::Cut(_))
    ));
    save(&mut harness, "15-bore-cut-preview");
    harness.key_press(Key::Enter);
    settle(&mut harness);
    assert_eq!(feature_status(&harness, "Cut-Extrude1"), FeatureStatus::Ok);
    let v_bore = volume(&harness);
    assert!(v_bore < v_l + std::f64::consts::PI * 144.0 * 24.0, "the bore removed material: {v_l} -> {v_bore}");

    // 5. Slot: 40 x 12 through the flange, sketched on its front face.
    harness.key_press(Key::I);
    settle(&mut harness);
    open_sketch_on_face(&mut harness, Vec3::new(30.0, 0.0, 40.0));
    harness.key_press(Key::S);
    harness.run_steps(2);
    sketch_click(&mut harness, P2::new(26.0, 38.0));
    sketch_click(&mut harness, P2::new(54.0, 38.0));
    sketch_click(&mut harness, P2::new(40.0, 44.0));
    {
        let s = harness.state().state.sketch.as_ref().unwrap();
        assert_eq!(s.sketch.drawn().count(), 5, "two lines, two arcs and the centreline");
    }
    dimension_line(&mut harness, P2::new(36.0, 44.0), P2::new(40.0, 52.0), "28");
    save(&mut harness, "16-slot-sketch");
    click_button(&mut harness, "Cut");
    harness.key_press(Key::Enter);
    settle(&mut harness);
    assert_eq!(
        feature_status(&harness, "Cut-Extrude2"),
        FeatureStatus::Ok,
        "{:?}",
        feature_status(&harness, "Cut-Extrude2")
    );

    // 6. Inside fillet R8 on the edge where the flange meets the base, seen from behind.
    look_from(&mut harness, 45.0, 35.0);
    let inside = screen_of(&harness, Vec3::new(70.0, 10.0, 10.0));
    click_at(&mut harness, inside);
    let sel = harness.state().state.doc.selection;
    assert!(matches!(sel, Some(Selection::Edge(_))), "expected the inside edge, got {sel:?}");
    click_button(&mut harness, "Fillet");
    assert_eq!(harness.state().state.editor.as_ref().unwrap().edges().len(), 1);
    set_field(&mut harness, "Radius", "8");
    harness.run_steps(4);
    save(&mut harness, "17-fillet-preview");
    harness.key_press(Key::Enter);
    settle(&mut harness);
    assert_eq!(feature_status(&harness, "Fillet1"), FeatureStatus::Ok, "{:?}", feature_status(&harness, "Fillet1"));

    // 7. Chamfer 2 mm on the flange's top front edge.
    harness.key_press(Key::I);
    settle(&mut harness);
    let top_front = screen_of(&harness, Vec3::new(100.0, 0.0, 60.0));
    click_at(&mut harness, top_front);
    assert!(matches!(harness.state().state.doc.selection, Some(Selection::Edge(_))));
    click_button(&mut harness, "Chamfer");
    set_field(&mut harness, "Distance", "2");
    harness.key_press(Key::Enter);
    settle(&mut harness);
    assert_eq!(feature_status(&harness, "Chamfer1"), FeatureStatus::Ok, "{:?}", feature_status(&harness, "Chamfer1"));
    harness.key_press(Key::Escape);
    harness.key_press(Key::Z);
    settle(&mut harness);
    save(&mut harness, "18-bracket");
    let v_done = volume(&harness);

    // 8. Save, then reopen through the recent-files menu.
    let path = dir.join("bracket.dwprt");
    harness.state_mut().state.doc.path = Some(path.clone());
    harness.key_press_modifiers(Modifiers::COMMAND, Key::S);
    harness.run_steps(3);
    assert!(path.exists(), "the part file should exist");
    assert!(!harness.state().state.doc.dirty);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::N);
    harness.run_steps(3);
    assert!(harness.state().state.doc.features.is_empty());
    harness.get_by_label("File").click();
    harness.run_steps(2);
    // Menu labels carry their shortcut text ("Open recent ⏵").
    harness.get_by_label_contains("Open recent").click();
    harness.run_steps(2);
    harness.get_by_label("bracket.dwprt").click();
    settle(&mut harness);
    assert_eq!(harness.state().state.doc.features.len(), 12, "{:?}", feature_names(&harness));
    assert_eq!(harness.state().state.doc.name, "bracket");
    let v_reopened = volume(&harness);
    assert!((v_reopened - v_done).abs() < 1e-3, "reopened volume {v_reopened} vs {v_done}");
    harness.key_press(Key::Z);
    settle(&mut harness);
    save(&mut harness, "19-reopened");

    // 9. Change the base thickness to 14 mm: everything downstream rebuilds.
    click_row(&mut harness, "Boss-Extrude1");
    click_button(&mut harness, "Edit feature");
    set_field(&mut harness, "Depth", "14");
    harness.key_press(Key::Enter);
    settle(&mut harness);
    for f in &harness.state().state.doc.features {
        assert_eq!(f.status, FeatureStatus::Ok, "{} after the thickness change: {:?}", f.name, f.status);
    }
    let v_thick = volume(&harness);
    assert!(v_thick > v_done + 120.0 * 90.0 * 3.0, "thicker base: {v_done} -> {v_thick}");
    let fillet_edge = harness.state().state.doc.features.iter().find(|f| f.name == "Fillet1").map(|f| match &f.kind {
        FeatureKind::Fillet(p) => p.edges[0].label.clone(),
        _ => unreachable!(),
    });
    eprintln!("fillet follows {fillet_edge:?}");
    save(&mut harness, "20-thickness-14");

    // 10. Suppress the flange: the fillet and chamfer lose their edges and ask for repair.
    click_row(&mut harness, "Boss-Extrude2");
    click_button(&mut harness, "Suppress");
    settle(&mut harness);
    assert!(
        matches!(feature_status(&harness, "Fillet1"), FeatureStatus::NeedsRepair(_)),
        "{:?}",
        feature_status(&harness, "Fillet1")
    );
    assert!(matches!(feature_status(&harness, "Chamfer1"), FeatureStatus::NeedsRepair(_)));
    save(&mut harness, "21-needs-repair");

    // Repair the fillet onto the base's back top edge.
    click_row(&mut harness, "Fillet1");
    click_button(&mut harness, "Repair…");
    assert!(matches!(
        harness.state().state.editor.as_ref().map(|e| &e.params),
        Some(soliddraft_app::state::EditorParams::Repair { .. })
    ));
    look_from(&mut harness, 45.0, 35.0);
    let back_top = screen_of(&harness, Vec3::new(30.0, 90.0, 14.0));
    click_at(&mut harness, back_top);
    save(&mut harness, "22-repair-pick");
    harness.key_press(Key::Enter);
    settle(&mut harness);
    assert_eq!(feature_status(&harness, "Fillet1"), FeatureStatus::Ok, "{:?}", feature_status(&harness, "Fillet1"));
    harness.key_press(Key::I);
    settle(&mut harness);
    save(&mut harness, "23-repaired");
}
