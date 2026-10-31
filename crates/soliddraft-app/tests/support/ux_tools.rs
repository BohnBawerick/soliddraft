//! What the UX tool acceptance tests share beyond the ribbon's input helpers: the real app
//! on a real adapter in a config directory of its own, the facts a geometry change must or
//! must not move, and the evidence each case leaves under `target/ux-tools/<case>/`.
//!
//! An adapter is required: a run that skips is not acceptance, and a software Vulkan
//! adapter counts. Expected values stay in the manifests. No case asserts how long
//! anything took.
//!
//! A test file brings this in after `support/ribbon.rs`, with
//! `#[path = "support/ux_tools.rs"] mod ux_tools;`.

#![allow(dead_code)]

use super::ribbon::{self, App};
use egui::{Modifiers, Pos2, Rect, Vec2, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{By, NodeT, Queryable};
use soliddraft_app::app::{Automation, SolidDraftApp};
use soliddraft_app::document::{Document, Selection};
use soliddraft_app::theme;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

/// The config override is global to the process and the harnesses share one adapter.
static SERIAL: Mutex<()> = Mutex::new(());

/// A window for a case.
#[derive(Clone, Copy, Debug)]
pub struct Look {
    pub size: Vec2,
    pub pixels_per_point: f32,
    pub theme: Option<theme::Mode>,
}

pub const STANDARD: Look = Look { size: vec2(1440.0, 900.0), pixels_per_point: 1.0, theme: None };
pub const SMALL_DARK: Look = Look { size: vec2(960.0, 600.0), pixels_per_point: 2.0, theme: Some(theme::Mode::Dark) };

/// One case: serial, with its own config directory and evidence folder.
pub struct Case {
    _serial: MutexGuard<'static, ()>,
    pub dir: PathBuf,
    log: std::fs::File,
    started: Instant,
}

impl Case {
    /// Starts a case. A missing renderer fails it.
    pub fn new(name: &str) -> Self {
        let serial = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ux-tools").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("config")).unwrap();
        soliddraft_app::file::set_config_dir(Some(dir.join("config")));
        let log = std::fs::File::create(dir.join("run.log")).unwrap();
        let mut case = Self { _serial: serial, dir, log, started: Instant::now() };
        let _ = env_logger::try_init();
        let instance = egui_wgpu::wgpu::Instance::default();
        let adapters = pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all()));
        assert!(!adapters.is_empty(), "this acceptance test requires a renderer");
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

    /// Runs the steps, and keeps the frame and the reason they failed on.
    pub fn run(&mut self, app: &mut App, steps: impl FnOnce(&mut Self, &mut App)) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| steps(self, app)));
        if let Err(panic) = result {
            let reason = panic.downcast_ref::<String>().map(String::as_str).or(panic.downcast_ref::<&str>().copied());
            let _ = std::fs::write(self.dir.join("failure.txt"), reason.unwrap_or("a panic with no text"));
            self.frame(app, "failure");
            std::panic::resume_unwind(panic);
        }
    }

    /// Renders a frame and keeps it, with the viewport cropped out beside it.
    pub fn frame(&mut self, app: &mut App, name: &str) -> image::RgbaImage {
        let image = app.render().expect("the real app renders");
        image.save(self.dir.join(format!("{name}.png"))).unwrap();
        let rect = viewport(app);
        let ppp = app.ctx.pixels_per_point();
        let (x, y) = ((rect.left() * ppp) as u32, (rect.top() * ppp) as u32);
        let (w, h) = ((rect.width() * ppp) as u32, (rect.height() * ppp) as u32);
        let crop = image::imageops::crop_imm(&image, x, y, w, h).to_image();
        crop.save(self.dir.join(format!("{name}-viewport.png"))).ok();
        image
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        let elapsed = self.started.elapsed().as_secs_f32();
        self.note(format!("elapsed {elapsed:.1} s (recorded only)"));
        soliddraft_app::file::set_config_dir(None);
    }
}

/// The real app on a document, rendering every step, settled.
pub fn launch(doc: Document, look: Look) -> App {
    let automation = Automation { theme: look.theme, ..Automation::default() };
    let mut app = Harness::builder()
        .with_size(look.size)
        .with_pixels_per_point(look.pixels_per_point)
        .with_render_every_step(true)
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_eframe(move |cc| SolidDraftApp::new(cc, doc, automation));
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

/// Moves the pointer into the chrome, so no hover tint is in the frame.
pub fn pointer_to_chrome(app: &mut App) {
    let rect = viewport(app);
    app.hover_at(pos2(rect.center().x, 4.0));
    app.run_steps(4);
}

/// A world point on screen.
pub fn screen(app: &App, world: glam::Vec3) -> Pos2 {
    let state = &app.state().state;
    state.viewport.camera.project(world, state.viewport.rect).expect("the point is on screen")
}

/// Double-clicks a point of the part once the id buffer under the pointer is current.
pub fn double_click_part(app: &mut App, world: glam::Vec3) {
    settle(app);
    let point = screen(app, world);
    app.hover_at(point);
    ribbon::until(app, "the part under the pointer", |a| a.state().state.viewport.hover.is_some());
    settle(app);
    ribbon::double_click_at(app, point);
}

/// Waits until the display answers the latest edit, or the edit has failed.
pub fn rebuilt(app: &mut App) {
    ribbon::until(app, "the rebuild", |a| {
        let proof = a.state().state.doc.proof_state();
        proof.failed || (!proof.pending && !proof.transient && proof.displayed_revision == proof.requested_revision)
    });
}

/// Opens a menu of the bar and clicks the one entry whose label passes `wanted`.
pub fn menu_entry(app: &mut App, menu: &str, wanted: impl Fn(&str) -> bool) {
    let window = ribbon::window(app);
    ribbon::click_button(app, menu, window);
    let entries: Vec<Rect> = app
        .query_all(By::new().role(egui::accesskit::Role::Button))
        .filter(|n| n.accesskit_node().label().is_some_and(|l| wanted(&l)))
        .map(|n| n.rect())
        .collect();
    assert_eq!(entries.len(), 1, "one entry in {menu}: {entries:?}");
    ribbon::click_at(app, entries[0].center());
    settle(app);
}

/// Clicks File > Open recent > the file, on the app's own recent list.
pub fn open_recent(app: &mut App, file: &str) {
    let window = ribbon::window(app);
    ribbon::click_button(app, "File", window);
    let submenu = app.get_by_label_contains("Open recent").rect();
    ribbon::click_at(app, submenu.center());
    app.run_steps(6);
    let entry = app.get_by_label(file).rect();
    ribbon::click_at(app, entry.center());
    settle(app);
}

/// The window at Ctrl+Z.
pub fn undo(app: &mut App) {
    app.key_press_modifiers(Modifiers::COMMAND, egui::Key::Z);
    settle(app);
    rebuilt(app);
}

/// The window at Ctrl+Shift+Z.
pub fn redo(app: &mut App) {
    app.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, egui::Key::Z);
    settle(app);
    rebuilt(app);
}

/// Everything a geometry change moves, for comparing before and after. Selection, hover,
/// mesh caches and tessellation are not in it.
#[derive(Clone, Debug, PartialEq)]
pub struct Facts {
    pub features: Vec<String>,
    pub rollback: Option<usize>,
    pub hash: String,
    pub volume: f64,
    pub min: [f64; 3],
    pub max: [f64; 3],
    pub topology: (usize, usize, usize),
    pub undo: usize,
    pub redo: usize,
}

pub fn facts(app: &App) -> Facts {
    let doc = &app.state().state.doc;
    let body = doc.display_body().expect("a body");
    let topology = doc.kernel().topology(body).expect("the body's topology");
    // The vertices, not `bbox`: the kernel pads its box by its tolerance.
    let corners = |pick: fn(f64, f64) -> f64, start: f64| {
        topology.vertices.iter().fold([start; 3], |at, v| {
            let p = v.position;
            [pick(at[0], p.x), pick(at[1], p.y), pick(at[2], p.z)]
        })
    };
    Facts {
        features: doc.features.iter().map(|f| format!("{:?} {} {} {:?}", f.id, f.name, f.suppressed, f.kind)).collect(),
        rollback: doc.rollback,
        hash: format!("{:?}", doc.model().geometry_hash()),
        volume: doc.kernel().mass_properties(body).expect("mass properties").volume,
        min: corners(f64::min, f64::INFINITY),
        max: corners(f64::max, f64::NEG_INFINITY),
        topology: (topology.faces.len(), topology.edges.len(), topology.vertices.len()),
        undo: doc.undo_depth(),
        redo: doc.redo_depth(),
    }
}

/// How many of the samples along a screen segment changed between two frames of one view.
/// Samples under `skip` do not count, because a painted value covers them.
pub fn changed_along(
    a: &image::RgbaImage,
    b: &image::RgbaImage,
    from: Pos2,
    to: Pos2,
    skip: Rect,
    pixels_per_point: f32,
) -> (usize, usize) {
    let different = |x: i32, y: i32| {
        let (x, y) = (x as u32, y as u32);
        let (p, q) = (a.get_pixel(x, y).0, b.get_pixel(x, y).0);
        p.iter().zip(q).any(|(p, q)| p.abs_diff(q) > 24)
    };
    let samples: Vec<Pos2> =
        (1..24).map(|i| from + (to - from) * (i as f32 / 24.0)).filter(|p| !skip.contains(*p)).collect();
    let changed = samples.iter().filter(|p| {
        let c = **p * pixels_per_point;
        (-2..=2).any(|dx| (-2..=2).any(|dy| different(c.x as i32 + dx, c.y as i32 + dy)))
    });
    (changed.count(), samples.len())
}

/// The selection the viewport's dimensions hang on.
pub fn body_selected(app: &App) -> bool {
    app.state().state.doc.selection == Some(Selection::Body)
}
