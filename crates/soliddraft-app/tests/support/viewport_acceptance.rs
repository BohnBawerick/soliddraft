//! What the viewport acceptance tests share: the real app on a real adapter in a config
//! directory of its own, input as a user makes it, and the measures the cases assert.
//!
//! An adapter is required: a run that skips is not acceptance, and a software Vulkan
//! adapter counts because it runs the real shaders. Each case writes its frames, its log
//! and its settings file under `target/viewport-acceptance/<case>/`, and a failure leaves
//! its message there too. No case asserts how long anything took.

#![allow(dead_code)]

use egui::{Color32, Key, Modifiers, Pos2, Rect, Vec2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use glam::Vec3;
use soliddraft_app::app::{Automation, SolidDraftApp};
use soliddraft_app::document::{AxisKind, Command, Document, ExtrudeParams, ProfileKind, Reference, Selection};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

pub type App = Harness<'static, SolidDraftApp>;

/// The config override is global to the process and the harnesses share one adapter.
static SERIAL: Mutex<()> = Mutex::new(());
/// Where the panic hook leaves a failure's message.
static RUNNING: Mutex<Option<PathBuf>> = Mutex::new(None);

/// One test case: serial, with its own config directory and evidence folder. Dropping it
/// restores the config directory.
pub struct Case {
    _serial: MutexGuard<'static, ()>,
    pub dir: PathBuf,
    log: std::fs::File,
    started: Instant,
}

impl Case {
    /// A case with no renderer: the settings file alone.
    pub fn settings(name: &str) -> Self {
        let serial = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/viewport-acceptance").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("config")).unwrap();
        let log = std::fs::File::create(dir.join("run.log")).unwrap();
        soliddraft_app::file::set_config_dir(Some(dir.join("config")));
        *RUNNING.lock().unwrap() = Some(dir.clone());
        static HOOK: std::sync::Once = std::sync::Once::new();
        HOOK.call_once(|| {
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                if let Ok(running) = RUNNING.try_lock()
                    && let Some(dir) = running.as_ref()
                {
                    let _ = std::fs::write(dir.join("failure.txt"), info.to_string());
                }
                previous(info);
            }));
        });
        Self { _serial: serial, dir, log, started: Instant::now() }
    }

    /// A case on the real renderer. Panics when there is no adapter.
    pub fn gpu(name: &str) -> Self {
        let mut case = Self::settings(name);
        let _ = env_logger::try_init();
        let instance = egui_wgpu::wgpu::Instance::default();
        let adapters = pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all()));
        assert!(!adapters.is_empty(), "acceptance requires a renderer");
        for adapter in &adapters {
            let info = adapter.get_info();
            case.note(format!("adapter {} on {:?} ({:?})", info.name, info.backend, info.device_type));
        }
        case
    }

    pub fn note(&mut self, line: impl AsRef<str>) {
        writeln!(self.log, "{}", line.as_ref()).unwrap();
        eprintln!("{}", line.as_ref());
    }

    pub fn settings_file(&self) -> PathBuf {
        self.dir.join("config/settings.json")
    }

    /// Writes the settings file a case starts from.
    pub fn seed_settings(&self, json: &str) {
        std::fs::write(self.settings_file(), json).unwrap();
    }

    pub fn settings_json(&self) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(self.settings_file()).expect("a settings file")).expect("settings JSON")
    }

    /// Renders a frame and keeps it, with the viewport cropped out beside it.
    pub fn frame(&mut self, app: &mut App, name: &str) -> image::RgbaImage {
        let image = app.render().expect("the real app renders");
        image.save(self.dir.join(format!("{name}.png"))).unwrap();
        let rect = viewport(app);
        let ppp = app.ctx.pixels_per_point();
        let (x, y) = ((rect.left() * ppp) as u32, (rect.top() * ppp) as u32);
        let (w, h) = ((rect.width() * ppp) as u32, (rect.height() * ppp) as u32);
        image::imageops::crop_imm(&image, x, y, w, h)
            .to_image()
            .save(self.dir.join(format!("{name}-viewport.png")))
            .ok();
        image
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        let elapsed = self.started.elapsed().as_secs_f32();
        self.note(format!("elapsed {elapsed:.1} s (recorded only)"));
        if let Ok(settings) = std::fs::read(self.settings_file()) {
            let _ = std::fs::write(self.dir.join("settings-at-end.json"), settings);
        }
        *RUNNING.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        soliddraft_app::file::set_config_dir(None);
    }
}

// ---------------------------------------------------------------------------
// Fixtures.

/// E: a new document with nothing in it.
pub fn empty() -> Document {
    soliddraft_app::new_document()
}

/// B: a 60 x 40 x 20 box on z = 0.
pub fn box_part() -> Document {
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

/// L: a copy of the old part file in the case's folder, with its original bytes.
pub fn old_part(case: &Case) -> (PathBuf, Vec<u8>) {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/near-coincident-flange.dwprt");
    let file = case.dir.join("near-coincident-flange.dwprt");
    std::fs::copy(fixture, &file).unwrap();
    let bytes = std::fs::read(&file).unwrap();
    (file, bytes)
}

// ---------------------------------------------------------------------------
// Driving the real app.

pub fn launch(doc: Document) -> App {
    launch_at(doc, vec2(1440.0, 900.0), 1.0)
}

pub fn launch_at(doc: Document, size: Vec2, pixels_per_point: f32) -> App {
    let mut app = Harness::builder()
        .with_size(size)
        .with_pixels_per_point(pixels_per_point)
        .with_render_every_step(true)
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_eframe(move |cc| SolidDraftApp::new(cc, doc, Automation::default()));
    settle(&mut app);
    app
}

/// Half a second of virtual time: animations end, a later click is not a double click.
pub fn settle(app: &mut App) {
    app.run_steps(30);
}

pub fn viewport(app: &App) -> Rect {
    app.state().state.viewport.rect
}

pub fn click(app: &mut App, point: Pos2) {
    app.hover_at(point);
    app.run_steps(4);
    for pressed in [true, false] {
        app.event(egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::default(),
        });
        app.step();
    }
    settle(app);
}

pub fn click_label(app: &mut App, label: &str) {
    app.get_by_label(label).hover();
    app.run_steps(4);
    app.get_by_label(label).click();
    settle(app);
}

/// Opens a menu of the menu bar and clicks an item. Menu labels carry their shortcut text.
pub fn click_menu(app: &mut App, menu: &str, item: &str) {
    click_label(app, menu);
    app.get_by_label_contains(item).hover();
    app.run_steps(4);
    app.get_by_label_contains(item).click();
    settle(app);
}

pub fn click_button(app: &mut App, label: &str) {
    let role = egui::accesskit::Role::Button;
    app.get_all_by_role_and_label(role, label).next().unwrap_or_else(|| panic!("missing button {label}")).hover();
    app.run_steps(4);
    app.get_all_by_role_and_label(role, label).next().unwrap_or_else(|| panic!("missing button {label}")).click();
    settle(app);
}

pub fn escape(app: &mut App) {
    app.key_press(Key::Escape);
    settle(app);
}

/// Moves the pointer into the chrome, so no hover tint is in the frame.
pub fn pointer_to_chrome(app: &mut App) {
    let rect = viewport(app);
    app.hover_at(pos2(rect.center().x, 4.0));
    app.run_steps(4);
}

/// Opens the tree's reference group, when it is shut.
pub fn open_references(app: &mut App) {
    if app.query_by_label("Origin").is_none() {
        click_label(app, "Origin, planes and axes");
    }
}

/// A viewport point clear of every reference item and of a part at the view's centre:
/// the corner farthest from the six axis rays.
pub fn clear_point(app: &App) -> Pos2 {
    let state = &app.state().state;
    let rect = state.viewport.rect;
    let camera = &state.viewport.camera;
    let inner = rect.shrink2(vec2(40.0, 60.0));
    let origin = camera.project(Vec3::ZERO, rect).expect("the origin is on screen");
    let distance_to_rays = |p: Pos2| {
        [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z]
            .iter()
            .filter_map(|d| camera.project(*d * camera.distance * 0.25, rect))
            .map(|end| {
                let along = (end - origin).normalized();
                let at = (p - origin).dot(along).max(0.0);
                (origin + along * at - p).length()
            })
            .fold(f32::MAX, f32::min)
    };
    [inner.left_bottom(), inner.right_bottom(), inner.left_top() + vec2(0.0, 60.0)]
        .into_iter()
        .max_by(|a, b| distance_to_rays(*a).total_cmp(&distance_to_rays(*b)))
        .unwrap()
}

/// An empty click in the viewport, as a user clears the selection.
pub fn click_empty(app: &mut App) {
    let point = clear_point(app);
    click(app, point);
}

// ---------------------------------------------------------------------------
// Reading what must and must not change.

/// The reference selection the viewport and the commands act on.
pub fn reference(app: &App) -> Option<Reference> {
    match app.state().state.doc.selection {
        Some(Selection::Reference(r)) => Some(r),
        _ => None,
    }
}

/// Whether the tree's accessible row shows selected.
pub fn row_selected(app: &App, label: &str) -> bool {
    use egui_kittest::kittest::NodeT;
    app.get_by_label(label).accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
}

/// Everything a purely visual operation must leave as it was.
#[derive(Debug, PartialEq)]
pub struct Model {
    hash: String,
    revision: u64,
    undo_depth: usize,
    dirty: bool,
    features: usize,
    volume: Option<f64>,
    topology: Option<(usize, usize, usize)>,
}

pub fn model(app: &App) -> Model {
    let doc = &app.state().state.doc;
    let body = doc.display_body();
    Model {
        hash: format!("{:?}", doc.model().semantic_hash()),
        revision: doc.revision(),
        undo_depth: doc.undo_depth(),
        dirty: doc.dirty,
        features: doc.features.len(),
        volume: body.map(|b| doc.kernel().mass_properties(b).expect("mass properties").volume),
        topology: body.map(|b| {
            let t = doc.kernel().topology(b).expect("topology");
            (t.faces.len(), t.edges.len(), t.vertices.len())
        }),
    }
}

/// Where the camera is: a selection or a preference must not move it.
pub fn camera(app: &App) -> (Vec3, Vec3, f32, bool, &'static str) {
    let c = &app.state().state.viewport.camera;
    (c.eye(), c.up(), c.distance, c.orthographic, c.view_name)
}

// ---------------------------------------------------------------------------
// Image oracles.

fn axis_coloured(axis: AxisKind, c: Color32) -> bool {
    match axis {
        AxisKind::X => c.r() > c.g().saturating_add(60) && c.r() > c.b().saturating_add(60),
        AxisKind::Y => c.g() > c.r().saturating_add(40) && c.g() > c.b().saturating_add(20),
        AxisKind::Z => c.b() > c.r().saturating_add(60) && c.b() > c.g().saturating_add(30),
    }
}

/// How many of 30 samples, 4 points apart and starting 8 points out from the projected
/// origin, find the axis's colour in a 3 x 3 pixel neighbourhood on its positive ray.
pub fn ray_hits(image: &image::RgbaImage, app: &App, axis: AxisKind) -> usize {
    let state = &app.state().state;
    let (camera, rect) = (&state.viewport.camera, state.viewport.rect);
    let ppp = app.ctx.pixels_per_point();
    let origin = camera.project(Vec3::ZERO, rect).expect("the origin is on screen");
    let tip = camera.project(axis.direction() * 10.0, rect).expect("the axis is on screen");
    let along = (tip - origin).normalized();
    (2..32)
        .filter(|step| {
            let p = (origin + along * (*step as f32 * 4.0)) * ppp;
            (-1..=1).any(|dx| {
                (-1..=1).any(|dy| {
                    let px = image.get_pixel((p.x as i32 + dx) as u32, (p.y as i32 + dy) as u32).0;
                    axis_coloured(axis, Color32::from_rgb(px[0], px[1], px[2]))
                })
            })
        })
        .count()
}

/// The three rays' sample counts, X, Y, Z.
pub fn rays(image: &image::RgbaImage, app: &App) -> [usize; 3] {
    AxisKind::ALL.map(|axis| ray_hits(image, app, axis))
}

/// How many pixels carry an axis colour where the corner triad used to stand.
pub fn corner_triad_pixels(image: &image::RgbaImage, app: &App) -> usize {
    let rect = viewport(app);
    let ppp = app.ctx.pixels_per_point();
    let corner = Rect::from_center_size(pos2(rect.left() + 34.0, rect.bottom() - 46.0), Vec2::splat(76.0));
    let (lo, hi) = (corner.min * ppp, corner.max * ppp);
    (lo.y as u32..hi.y as u32)
        .flat_map(|y| (lo.x as u32..hi.x as u32).map(move |x| (x, y)))
        .filter(|(x, y)| {
            let px = image.get_pixel(*x, *y).0;
            let c = Color32::from_rgb(px[0], px[1], px[2]);
            AxisKind::ALL.iter().any(|axis| axis_coloured(*axis, c))
        })
        .count()
}

/// The world axes at rest: fewer than 3 samples on every ray and nothing in the corner.
pub fn assert_axes_hidden(image: &image::RgbaImage, app: &App, when: &str) {
    let hits = rays(image, app);
    assert!(hits.iter().all(|n| *n < 3), "{when}: the axes must be hidden, samples {hits:?}");
    let corner = corner_triad_pixels(image, app);
    assert_eq!(corner, 0, "{when}: no triad in the corner");
}

/// The world axes on show: at least 10 of 30 samples on every ray.
pub fn assert_axes_shown(image: &image::RgbaImage, app: &App, when: &str) {
    let hits = rays(image, app);
    assert!(hits.iter().all(|n| *n >= 10), "{when}: the axes must be visible, samples {hits:?}");
}
