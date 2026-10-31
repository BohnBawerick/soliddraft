//! Input and observation shared by the ribbon acceptance tests: the real app on the real
//! wgpu pipeline and the real kernel, driven with pointer and keyboard events, and read
//! back from the frame it painted. Layout expectations stay in the tests; nothing here
//! repeats how the app lays itself out.
//!
//! A test file brings this in with `#[path = "support/ribbon.rs"] mod ribbon;`.

#![allow(dead_code)]

use egui::{Key, Modifiers, Pos2, Rect, Shape, Vec2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{By, NodeT, Queryable};
use soliddraft_app::app::{Automation, SolidDraftApp};
use soliddraft_app::command_line;
use soliddraft_app::document::{
    Command, Document, ExtrudeParams, FeatureId, PlaneKind, ProfileKind, Selection, SketchPlane,
};
use soliddraft_app::state::{EditorParams, Tool};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub type App = Harness<'static, SolidDraftApp>;

/// Containment and same-row comparisons, in logical points.
pub const TOLERANCE: f32 = 1.0;

pub const WINDOW: Vec2 = vec2(1440.0, 900.0);

const HANG_GUARD: Duration = Duration::from_secs(600);

/// The rendered harnesses share one adapter and the process-wide config; one at a time.
static GPU: Mutex<()> = Mutex::new(());

/// One acceptance case: the GPU to itself, a fresh config directory and a place for frames.
pub struct Case {
    dir: PathBuf,
    _gpu: MutexGuard<'static, ()>,
}

/// Starts a case. A missing renderer fails it: an acceptance run that skips is not one.
pub fn case(name: &str) -> Case {
    let gpu = GPU.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let _ = env_logger::try_init();
    let instance = egui_wgpu::wgpu::Instance::default();
    let adapters = pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all()));
    assert!(!adapters.is_empty(), "this acceptance test requires a renderer");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ribbon-acceptance").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("config")).unwrap();
    soliddraft_app::file::set_config_dir(Some(dir.join("config")));
    Case { dir, _gpu: gpu }
}

impl Case {
    /// Runs the steps, and keeps the frame they failed on.
    pub fn run(&self, app: &mut App, steps: impl FnOnce(&mut App)) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| steps(app)));
        if let Err(panic) = result {
            self.frame(app, "failure");
            std::panic::resume_unwind(panic);
        }
    }

    /// Where a named artefact of this case goes.
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Writes the current frame as `<name>.png`, and returns where.
    pub fn frame(&self, app: &mut App, name: &str) -> PathBuf {
        let path = self.dir.join(format!("{name}.png"));
        app.render().expect("the frame renders").save(&path).expect("the frame is written");
        path
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        soliddraft_app::file::set_config_dir(None);
    }
}

/// The real app on a document, one window of 1440 by 900 at 1.0, rendering every step.
pub fn app(doc: Document) -> App {
    app_at(doc, WINDOW, 1.0)
}

pub fn app_at(doc: Document, size: Vec2, pixels_per_point: f32) -> App {
    let mut app = Harness::builder()
        .with_size(size)
        .with_pixels_per_point(pixels_per_point)
        .with_render_every_step(true)
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_eframe(move |cc| SolidDraftApp::new(cc, doc, Automation::default()));
    app.run_steps(10);
    app
}

// ---------------------------------------------------------------------------
// Fixtures.

/// No features, body, selection, editor or sketch session.
pub fn blank() -> Document {
    Document::new("Ribbon acceptance", soliddraft_app::kernel())
}

/// One extrusion, 60 by 40 by 20: volume 48000, six faces, twelve edges.
pub fn plate() -> Document {
    let mut doc = blank();
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

/// One Top-plane sketch of a closed 60 by 40 rectangle, with nothing extruded from it.
pub fn sketch() -> Document {
    let mut doc = blank();
    let plane = SketchPlane::Reference { plane: PlaneKind::Top, offset: 0.0 };
    let sketch = doc.apply(Command::AddSketch { plane }).unwrap().unwrap();
    let mut drawing = soliddraft_sketch::Sketch::new();
    drawing.rectangle(soliddraft_sketch::P2::new(0.0, 0.0), 60.0, 40.0).unwrap();
    doc.apply(Command::EditSketch { id: sketch, sketch: drawing }).unwrap();
    doc
}

/// Steps until a condition holds, with the ten-minute hang guard.
pub fn until(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let start = Instant::now();
    while !done(app) {
        assert!(start.elapsed() < HANG_GUARD, "still waiting for {what}");
        app.step();
    }
}

// ---------------------------------------------------------------------------
// Input.

/// A press and release at a point, as a pointer makes them. Unlike a toolkit click it
/// reaches a disabled control too.
pub fn click_at(app: &mut App, point: Pos2) {
    app.hover_at(point);
    app.run_steps(3);
    for pressed in [true, false] {
        app.event(egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::default(),
        });
        app.step();
    }
    app.run_steps(3);
}

/// Clicks the one button of that name in a region.
pub fn click_button(app: &mut App, label: &str, within: Rect) {
    let rect = button_rect(app, label, within);
    click_at(app, rect.center());
}

/// The bounds of the one button of that name in a region.
pub fn button_rect(app: &App, label: &str, within: Rect) -> Rect {
    let found: Vec<Rect> = app
        .get_all_by_role_and_label(egui::accesskit::Role::Button, label)
        .map(|n| n.rect())
        .filter(|r| within.intersects(*r))
        .collect();
    assert_eq!(found.len(), 1, "one button {label} in {within:?}: {found:?}");
    found[0]
}

/// The whole window, as a region.
pub fn window(app: &App) -> Rect {
    app.ctx.content_rect()
}

/// A button as assistive technology reads it.
#[derive(Clone, Copy, Debug)]
pub struct Control {
    pub rect: Rect,
    pub enabled: bool,
    pub selected: bool,
}

/// The one button of that name in a region.
pub fn control(app: &App, label: &str, within: Rect) -> Control {
    let found: Vec<Control> = app
        .get_all_by_role_and_label(egui::accesskit::Role::Button, label)
        .filter(|n| within.intersects(n.rect()))
        .map(|n| {
            let node = n.accesskit_node();
            let selected = node.toggled() == Some(egui::accesskit::Toggled::True);
            Control { rect: n.rect(), enabled: !node.is_disabled(), selected }
        })
        .collect();
    assert_eq!(found.len(), 1, "one button {label} in {within:?}: {found:?}");
    found[0]
}

/// A ribbon tab, by the name it shows.
pub fn tab(app: &App, name: &str) -> Control {
    control(app, &format!("{name} tab"), window(app))
}

/// Clicks a ribbon tab.
pub fn click_tab(app: &mut App, name: &str) {
    let rect = tab(app, name).rect;
    click_at(app, rect.center());
    app.run_steps(4);
}

/// Everything above the side panels: the menus, the tabs and the ribbon.
pub fn ribbon_region(app: &App) -> Rect {
    let panels = painted(app).iter().filter(|p| p.text.ends_with(" tree")).map(|p| p.rect.top()).fold(0.0, f32::max);
    let window = window(app);
    Rect::from_min_max(window.min, pos2(window.right(), panels))
}

/// A ribbon button, by the name it shows.
pub fn ribbon_button(app: &App, label: &str) -> Control {
    control(app, label, ribbon_region(app))
}

/// Clicks a ribbon button where it is drawn.
pub fn click_ribbon(app: &mut App, label: &str) {
    let rect = ribbon_button(app, label).rect;
    click_at(app, rect.center());
    app.run_steps(4);
}

/// Rests the pointer on a control until its tooltip shows, and returns what the tooltip
/// added to the frame.
pub fn tooltip(app: &mut App, rect: Rect) -> String {
    let before: Vec<String> = painted(app).into_iter().map(|p| p.text).collect();
    app.hover_at(rect.center());
    app.run_steps(36);
    let said: Vec<String> = painted(app).into_iter().map(|p| p.text).filter(|t| !before.contains(t)).collect();
    let rest = window(app).center();
    app.hover_at(rest);
    app.run_steps(2);
    said.join("\n")
}

/// Where an entry of an open top menu is.
pub fn menu_entry(app: &App, name: &str, entry: &str) -> Rect {
    let found: Vec<Rect> = app.query_all(By::new().label_contains(entry)).map(|n| n.rect()).collect();
    assert_eq!(found.len(), 1, "one entry {entry} in {name}: {found:?}");
    found[0]
}

/// Opens a top menu and clicks an entry.
pub fn menu(app: &mut App, name: &str, entry: &str) {
    let area = window(app);
    click_button(app, name, area);
    click_at(app, menu_entry(app, name, entry).center());
    app.run_steps(4);
}

/// Clicks a document tab.
pub fn document_tab(app: &mut App, title: &str) {
    let rect = title_rect(app, title);
    click_at(app, rect.center());
    app.run_steps(4);
}

/// A press and release that stops on the frame the click lands in: the next `render` is
/// the first paint of whatever it changed.
pub fn click_for_first_paint(app: &mut App, point: Pos2) {
    app.hover_at(point);
    app.run_steps(3);
    for pressed in [true, false] {
        app.event(egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::default(),
        });
        app.step();
    }
}

/// Two quick presses at a point, as a pointer makes them.
pub fn double_click_at(app: &mut App, point: Pos2) {
    app.hover_at(point);
    app.run_steps(3);
    for pressed in [true, false, true, false] {
        app.event(egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::default(),
        });
        app.step();
    }
    app.run_steps(3);
}

/// Clicks a world point in the viewport, as a user picks a face or an edge.
pub fn click_world(app: &mut App, world: glam::Vec3) {
    let point = {
        let state = &app.state().state;
        state.viewport.camera.project(world, state.viewport.rect).expect("the point is on screen")
    };
    click_at(app, point);
}

/// Types a value into a numeric field: click, select all, type, Enter.
pub fn type_field(app: &mut App, label: &str, value: &str) {
    let rect = app.get_by_label(label).rect();
    click_at(app, rect.center());
    app.key_press_modifiers(Modifiers::COMMAND, Key::A);
    app.get_by_label(label).type_text(value);
    app.run_steps(3);
    app.key_press(Key::Enter);
    app.run_steps(4);
}

/// The command line's field.
pub fn input_rect(app: &App) -> Rect {
    app.get_by_label("Command line").rect()
}

/// Clicks the command line, types a line and presses Enter.
pub fn type_command(app: &mut App, line: &str) {
    let rect = input_rect(app);
    click_at(app, rect.center());
    assert!(app.get_by_label("Command line").is_focused(), "the pointer focuses the command line");
    app.get_by_label("Command line").type_text(line);
    app.run_steps(3);
    app.key_press(Key::Enter);
    app.run_steps(6);
}

// ---------------------------------------------------------------------------
// Reading the frame.

/// A piece of text the last frame painted, with the part of it that can be seen.
#[derive(Clone, Debug)]
pub struct Painted {
    pub text: String,
    pub rect: Rect,
}

fn walk(shape: &Shape, clip: Rect, found: &mut Vec<Painted>) {
    match shape {
        Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, clip, found)),
        Shape::Text(text) => {
            let rect = text.visual_bounding_rect().intersect(clip);
            if rect.is_positive() {
                found.push(Painted { text: text.galley.text().to_owned(), rect });
            }
        }
        _ => {}
    }
}

fn walk_images(shape: &Shape, found: &mut Vec<Rect>) {
    match shape {
        Shape::Vec(shapes) => shapes.iter().for_each(|s| walk_images(s, found)),
        Shape::Rect(rect) if rect.fill_texture_id() != egui::TextureId::default() => found.push(rect.rect),
        Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => found.push(mesh.calc_bounds()),
        _ => {}
    }
}

/// The images in a frame's shapes, in paint order, as opposed to its text and lines.
pub fn images(shapes: &[egui::epaint::ClippedShape]) -> Vec<Rect> {
    let mut found = Vec::new();
    for clipped in shapes {
        walk_images(&clipped.shape, &mut found);
    }
    found
}

/// The images the last frame drew in a region.
pub fn images_in(app: &App, region: Rect) -> Vec<Rect> {
    let mut found = images(&app.output().shapes);
    found.retain(|r| region.expand(TOLERANCE).contains_rect(*r));
    found
}

/// The text in the last frame that a user can see, cut to its clip and to the window.
pub fn painted(app: &App) -> Vec<Painted> {
    let window = window(app);
    let mut found = Vec::new();
    for clipped in &app.output().shapes {
        walk(&clipped.shape, clipped.clip_rect.intersect(window), &mut found);
    }
    found
}

/// The painted text inside a region.
pub fn painted_in(app: &App, region: Rect) -> Vec<Painted> {
    painted(app).into_iter().filter(|p| region.expand(TOLERANCE).contains_rect(p.rect)).collect()
}

/// The collapsed bottom bar, as the frame laid it out.
pub fn band(app: &App) -> Rect {
    egui::PanelState::load(&app.ctx, command_line::bar_id()).expect("the bar was laid out").outer_rect
}

/// Everything the status says: what the bar paints, then the details behind it, opened
/// and closed again with the pointer.
pub fn status_lines(app: &mut App) -> Vec<String> {
    let band = band(app);
    let mut lines: Vec<String> = painted_in(app, band).into_iter().map(|p| p.text).collect();
    let details = button_rect(app, "Status details", band);
    click_at(app, details.center());
    app.run_steps(3);
    let opened = painted(app);
    lines.extend(opened.iter().filter(|p| p.rect.bottom() <= band.top() + TOLERANCE).map(|p| p.text.clone()));
    click_at(app, details.center());
    app.run_steps(3);
    lines
}

/// The group captions of the open ribbon tab, left to right: the lowest row of its text.
pub fn captions(app: &App) -> Vec<Painted> {
    let text = painted_in(app, ribbon_region(app));
    let lowest = text.iter().map(|p| p.rect.center().y).fold(0.0, f32::max);
    let mut row: Vec<Painted> = text.into_iter().filter(|p| p.rect.center().y >= lowest - TOLERANCE).collect();
    row.sort_by(|a, b| a.rect.left().total_cmp(&b.rect.left()));
    row
}

/// The caption names of the open ribbon tab.
pub fn caption_names(app: &App) -> Vec<String> {
    captions(app).into_iter().map(|p| p.text).collect()
}

/// The open tab has exactly these groups in this order, each with its commands visible
/// over its caption. The groups run left to right and do not overlap.
pub fn groups_in_order(app: &App, groups: &[(&str, &[&str])]) {
    let names: Vec<&str> = groups.iter().map(|(name, _)| *name).collect();
    assert_eq!(caption_names(app), names, "the group captions");
    let mut right = 0.0_f32;
    for ((name, labels), caption) in groups.iter().zip(captions(app)) {
        let mut bounds = Rect::NOTHING;
        for label in *labels {
            let rect = ribbon_button(app, label).rect;
            let seen = painted_in(app, rect).iter().any(|p| p.text == *label);
            assert!(seen, "{name}: the label {label} is painted on its button {rect:?}");
            bounds = bounds.union(rect);
        }
        let x = caption.rect.center().x;
        assert!(
            x >= bounds.left() - TOLERANCE && x <= bounds.right() + TOLERANCE,
            "{name}: the caption at {x} is over its commands {bounds:?}"
        );
        assert!(bounds.left() >= right - TOLERANCE, "{name} starts after the group before it: {bounds:?} {right}");
        right = bounds.right();
    }
}

/// The panel titles a user reads, by name.
pub fn title_rect(app: &App, title: &str) -> Rect {
    painted(app).into_iter().find(|p| p.text == title).unwrap_or_else(|| panic!("no title {title} in the frame")).rect
}

// ---------------------------------------------------------------------------
// The state a no-op must leave alone.

/// The document and the session around it, for comparing before and after.
#[derive(Clone, Debug, PartialEq)]
pub struct Fingerprint {
    pub features: Vec<String>,
    pub body: Option<String>,
    pub selection: Option<Selection>,
    pub undo_depth: usize,
    pub redo_depth: usize,
    pub dirty: bool,
    pub camera: [f32; 6],
    pub workspace: (bool, bool),
    pub editor: Option<(Option<FeatureId>, EditorParams)>,
    pub sketch_tool: Option<Tool>,
    pub history: Vec<String>,
    pub recent: Vec<String>,
}

impl Fingerprint {
    /// For a start that really ran and was cancelled: the same part and session, with the
    /// recent commands free to have recorded it.
    pub fn without_history(mut self) -> Self {
        self.history.clear();
        self.recent.clear();
        self
    }
}

pub fn fingerprint(app: &App) -> Fingerprint {
    let state = &app.state().state;
    let doc = &state.doc;
    let c = &state.viewport.camera;
    Fingerprint {
        features: doc.features.iter().map(|f| format!("{:?} {} {:?}", f.id, f.name, f.kind)).collect(),
        body: doc.display_body().map(|b| format!("{b:?}")),
        selection: doc.selection,
        undo_depth: doc.undo_depth(),
        redo_depth: doc.redo_depth(),
        dirty: doc.dirty,
        camera: [c.target.x, c.target.y, c.target.z, c.distance, c.yaw, c.pitch],
        workspace: (state.drawing.is_active(), state.assembly.is_active()),
        editor: state.editor.as_ref().map(|e| (e.target, e.params.clone())),
        sketch_tool: state.sketch.as_ref().map(|s| s.tool),
        history: state.command_line.history.clone(),
        recent: state.commands.recent.clone(),
    }
}
