//! Input and observation shared by the ribbon acceptance tests: the real app on the real
//! wgpu pipeline and the real kernel, driven with pointer and keyboard events, and read
//! back from the frame it painted. Layout expectations stay in the tests; nothing here
//! repeats how the app lays itself out.
//!
//! A test file brings this in with `#[path = "support/ribbon.rs"] mod ribbon;`.

#![allow(dead_code)]

use egui::{Key, Modifiers, Pos2, Rect, Shape, Vec2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{By, Queryable};
use soliddraft_app::app::{Automation, SolidDraftApp};
use soliddraft_app::command_line;
use soliddraft_app::document::{Command, Document, ExtrudeParams, FeatureId, ProfileKind, Selection};
use soliddraft_app::state::{EditorParams, Tool};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

pub type App = Harness<'static, SolidDraftApp>;

/// Containment and same-row comparisons, in logical points.
pub const TOLERANCE: f32 = 1.0;

pub const WINDOW: Vec2 = vec2(1440.0, 900.0);

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

/// Opens a top menu and clicks an entry.
pub fn menu(app: &mut App, name: &str, entry: &str) {
    let area = window(app);
    click_button(app, name, area);
    let found: Vec<Rect> =
        app.query_all(By::new().role(egui::accesskit::Role::Button).label_contains(entry)).map(|n| n.rect()).collect();
    assert_eq!(found.len(), 1, "one entry {entry} in {name}: {found:?}");
    click_at(app, found[0].center());
    app.run_steps(4);
}

/// Clicks a document tab.
pub fn document_tab(app: &mut App, title: &str) {
    let rect = title_rect(app, title);
    click_at(app, rect.center());
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
}

impl Fingerprint {
    /// For a start that really ran and was cancelled: the same part and session, with the
    /// recent commands free to have recorded it.
    pub fn without_history(mut self) -> Self {
        self.history.clear();
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
    }
}
