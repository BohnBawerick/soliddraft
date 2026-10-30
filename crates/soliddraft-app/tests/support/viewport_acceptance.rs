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
use soliddraft_app::document::{
    AxisKind, Command, Document, ExtrudeParams, PlaneKind, ProfileKind, Reference, Selection, SketchPlane,
};
use soliddraft_app::settings::ReferenceDisplay;
use soliddraft_app::sketch::{P2, Sketch};
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

/// S: a closed rectangle on each reference plane, and one on Top offset by 25 mm.
pub fn sketches() -> Document {
    let mut doc = soliddraft_app::new_document();
    for (plane, offset) in
        [(PlaneKind::Front, 0.0), (PlaneKind::Top, 0.0), (PlaneKind::Right, 0.0), (PlaneKind::Top, 25.0)]
    {
        let id = doc.apply(Command::AddSketch { plane: SketchPlane::Reference { plane, offset } }).unwrap().unwrap();
        let mut sketch = Sketch::new();
        sketch.rectangle(P2::new(-20.0, -10.0), 40.0, 20.0).unwrap();
        soliddraft_sketch::solve::solve(&mut sketch, None, false);
        doc.apply(Command::EditSketch { id, sketch }).unwrap();
    }
    doc.selection = None;
    doc
}

// ---------------------------------------------------------------------------
// Driving the real app.

pub fn launch(doc: Document) -> App {
    launch_at(doc, vec2(1440.0, 900.0), 1.0)
}

pub fn launch_at(doc: Document, size: Vec2, pixels_per_point: f32) -> App {
    start(doc, size, pixels_per_point, true)
}

/// For a case that reads frames and never picks: it draws only the frames it asks for.
pub fn launch_for_frames(doc: Document, size: Vec2, pixels_per_point: f32) -> App {
    start(doc, size, pixels_per_point, false)
}

fn start(doc: Document, size: Vec2, pixels_per_point: f32, render_every_step: bool) -> App {
    let mut app = Harness::builder()
        .with_size(size)
        .with_pixels_per_point(pixels_per_point)
        .with_render_every_step(render_every_step)
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
    press(app, point, egui::PointerButton::Primary);
}

pub fn right_click(app: &mut App, point: Pos2) {
    press(app, point, egui::PointerButton::Secondary);
}

fn press(app: &mut App, point: Pos2, button: egui::PointerButton) {
    app.hover_at(point);
    app.run_steps(4);
    for pressed in [true, false] {
        app.event(egui::Event::PointerButton { pos: point, button, pressed, modifiers: Modifiers::default() });
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

/// Opens a menu of the menu bar and clicks an item. Menu labels carry their shortcut
/// text, and the menu bar comes before a ribbon tab of the same name.
pub fn click_menu(app: &mut App, menu: &str, item: &str) {
    app.get_all_by_label(menu).next().unwrap_or_else(|| panic!("missing menu {menu}")).hover();
    app.run_steps(4);
    app.get_all_by_label(menu).next().unwrap_or_else(|| panic!("missing menu {menu}")).click();
    settle(app);
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

/// Two clicks on a row, close enough to be a double click.
pub fn double_click_label(app: &mut App, label: &str) {
    app.get_by_label(label).hover();
    app.run_steps(4);
    let point = app.get_by_label(label).rect().center();
    for pressed in [true, false, true, false] {
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

/// Scrolls the wheel over the view's centre and waits for the smooth scroll to end. A
/// positive amount zooms in.
pub fn wheel(app: &mut App, points: f32) {
    app.hover_at(viewport(app).center());
    app.run_steps(2);
    app.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, points),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::default(),
    });
    let mut resting = 0;
    let mut last = app.state().state.viewport.camera.distance;
    while resting < 4 {
        app.step();
        let distance = app.state().state.viewport.camera.distance;
        resting = if distance == last { resting + 1 } else { 0 };
        last = distance;
    }
}

/// A drag across the view with one button.
pub fn drag(app: &mut App, button: egui::PointerButton, from: Pos2, to: Pos2) {
    app.hover_at(from);
    app.run_steps(2);
    let press = |pos, pressed| egui::Event::PointerButton { pos, button, pressed, modifiers: Modifiers::default() };
    app.event(press(from, true));
    app.step();
    for k in 1..=8 {
        app.hover_at(from + (to - from) * (k as f32 / 8.0));
        app.step();
    }
    app.event(press(to, false));
    settle(app);
}

/// Changes the display choices at the seam the Preferences window will call. That window
/// arrives with the navigation lane's merge D. Until then this holds a display state
/// steady for frame comparison, and no case asserts a preference through it.
pub fn set_display(app: &mut App, change: impl FnOnce(&mut ReferenceDisplay)) {
    let mut references = app.state().state.viewport.references;
    change(&mut references);
    app.state_mut().state.set_reference_display(references).expect("the display choices save");
    app.run_steps(3);
}

/// The same view with the grid on and off, the grid left on.
pub fn grid_frames(case: &mut Case, app: &mut App, name: &str) -> (image::RgbaImage, image::RgbaImage) {
    pointer_to_chrome(app);
    let on = case.frame(app, &format!("{name}-grid-on"));
    set_display(app, |d| d.show_grid = false);
    let off = case.frame(app, &format!("{name}-grid-off"));
    set_display(app, |d| d.show_grid = true);
    (on, off)
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

// ---------------------------------------------------------------------------
// The grid.

/// The spacing the viewport says it is drawing, from its accessible readout: `None` when
/// it has none. Both figures must be there, named.
pub fn readout(app: &App) -> Option<(f32, f32)> {
    use egui_kittest::kittest::NodeT;
    let node = app.query_by_label_contains("Grid spacing:")?;
    let label = node.accesskit_node().value().unwrap_or_default();
    let figure = |name: &str| -> f32 {
        let (_, rest) = label.split_once(name).unwrap_or_else(|| panic!("no {name} in {label:?}"));
        rest.trim().split_whitespace().next().unwrap().trim_end_matches(',').parse().expect("a number")
    };
    let spacing = (figure("minor"), figure("major"));
    assert!(spacing.0 > 0.0 && spacing.0.is_finite() && spacing.1 > spacing.0, "{label}");
    assert!(label.contains("mm"), "{label}");
    Some(spacing)
}

/// D(a, b) of the design: the largest channel difference at a pixel.
fn difference(a: &image::RgbaImage, b: &image::RgbaImage, x: u32, y: u32) -> u8 {
    let (p, q) = (a.get_pixel(x, y).0, b.get_pixel(x, y).0);
    (0..3).map(|c| p[c].abs_diff(q[c])).max().unwrap()
}

/// The largest difference within `reach` physical pixels of a point in logical points.
fn difference_near(a: &image::RgbaImage, b: &image::RgbaImage, point: Pos2, ppp: f32, reach: i32) -> u8 {
    let (x, y) = ((point.x * ppp) as i32, (point.y * ppp) as i32);
    let (width, height) = (a.width() as i32, a.height() as i32);
    (-reach..=reach)
        .flat_map(|dy| (-reach..=reach).map(move |dx| (x + dx, y + dy)))
        .filter(|(x, y)| *x >= 0 && *y >= 0 && *x < width && *y < height)
        .map(|(x, y)| difference(a, b, x as u32, y as u32))
        .max()
        .unwrap_or(0)
}

/// The part of the view clear of the toolbar, the cube, the pills and the view's border.
pub fn grid_crop(app: &App) -> Rect {
    let rect = viewport(app);
    Rect::from_min_max(pos2(rect.left() + 24.0, rect.top() + 110.0), pos2(rect.right() - 120.0, rect.bottom() - 60.0))
}

/// One line found in the difference of two frames: where it runs across the view, in
/// points, and how far it stands from the other frame.
#[derive(Clone, Copy, Debug)]
pub struct FoundLine {
    pub at: f32,
    pub contrast: u8,
    /// The contrast summed across the line. A thin line straddling two pixels has half the
    /// peak of one centred on a pixel and the same sum, so this tells line weights apart
    /// where the peak cannot.
    pub weight: f32,
}

/// The lines running along one direction of a normal view. Each column or row of pixels
/// takes the median difference along its length, which drops the crossing family.
pub fn found_lines(
    on: &image::RgbaImage,
    off: &image::RgbaImage,
    crop: Rect,
    ppp: f32,
    upright: bool,
) -> Vec<FoundLine> {
    let (across, along) = if upright { (crop.x_range(), crop.y_range()) } else { (crop.y_range(), crop.x_range()) };
    let (a0, a1) = ((across.min * ppp) as u32, (across.max * ppp) as u32);
    let (b0, b1) = ((along.min * ppp) as u32, (along.max * ppp) as u32);
    let profile: Vec<u8> = (a0..a1)
        .map(|a| {
            let mut samples: Vec<u8> = (b0..b1)
                .step_by(3)
                .map(|b| if upright { difference(on, off, a, b) } else { difference(on, off, b, a) })
                .collect();
            samples.sort_unstable();
            samples[samples.len() / 2]
        })
        .collect();
    let mut lines = Vec::new();
    let mut k = 0;
    while k < profile.len() {
        if profile[k] < 2 {
            k += 1;
            continue;
        }
        let start = k;
        while k < profile.len() && profile[k] >= 2 {
            k += 1;
        }
        let weight: f32 = profile[start..k].iter().map(|d| f32::from(*d)).sum();
        let centre: f32 = (start..k).map(|i| (i as f32 + 0.5) * f32::from(profile[i])).sum::<f32>() / weight;
        let contrast = *profile[start..k].iter().max().unwrap();
        lines.push(FoundLine { at: (a0 as f32 + centre) / ppp, contrast, weight: weight / ppp });
    }
    lines
}

fn median(mut values: Vec<f32>) -> f32 {
    values.sort_by(f32::total_cmp);
    values[values.len() / 2]
}

/// One family of a normal view's grid, measured with no knowledge of what was drawn.
#[derive(Clone, Debug)]
pub struct Family {
    /// Median distance between neighbouring lines, in points.
    pub minor: f32,
    pub minor_contrast: f32,
    pub major_contrast: f32,
    pub lines: Vec<FoundLine>,
    /// The lines that stand out from the rest.
    pub majors: Vec<FoundLine>,
}

pub fn measure_family(lines: Vec<FoundLine>, when: &str) -> Family {
    assert!(lines.len() >= 7, "{when}: at least six intervals, found {} lines", lines.len());
    let minor = median(lines.windows(2).map(|w| w[1].at - w[0].at).collect());
    let typical = median(lines.iter().map(|l| l.weight).collect());
    let (majors, minors): (Vec<FoundLine>, Vec<FoundLine>) = lines.iter().partition(|l| l.weight >= typical * 1.6);
    let contrast = |lines: &[FoundLine]| median(lines.iter().map(|l| f32::from(l.contrast)).chain([0.0]).collect());
    Family { minor, minor_contrast: contrast(&minors), major_contrast: contrast(&majors), lines, majors }
}

/// The share of a family's expected line samples found, within 2 points across the line.
fn line_presence(
    on: &image::RgbaImage,
    off: &image::RgbaImage,
    ppp: f32,
    at: f32,
    along: (f32, f32),
    upright: bool,
) -> f32 {
    let samples: Vec<f32> = (0..).map(|k| along.0 + k as f32 * 4.0).take_while(|p| *p < along.1).collect();
    let found = samples
        .iter()
        .filter(|p| {
            let point = if upright { pos2(at, **p) } else { pos2(**p, at) };
            difference_near(on, off, point, ppp, (2.0 * ppp) as i32) >= 4
        })
        .count();
    found as f32 / samples.len() as f32
}

/// Checks one family of the Top view against the readout and the camera's projection.
fn assert_family(
    app: &App,
    (on, off): (&image::RgbaImage, &image::RgbaImage),
    (minor, major): (f32, f32),
    upright: bool,
    when: &str,
) -> Family {
    let ppp = app.ctx.pixels_per_point();
    let crop = grid_crop(app);
    let state = &app.state().state;
    let (camera, rect) = (&state.viewport.camera, state.viewport.rect);
    let family = measure_family(found_lines(on, off, crop, ppp, upright), when);

    // The world coordinate of a screen position across the lines, from two projected points.
    let axis = if upright { Vec3::X } else { Vec3::Y };
    let screen = |world: f32| {
        let p = camera.project(axis * world, rect).expect("the line is on screen");
        if upright { p.x } else { p.y }
    };
    let per_mm = (screen(minor) - screen(0.0)) / minor;
    let expected = (minor * per_mm).abs();
    assert!(
        (family.minor - expected).abs() <= (expected * 0.05).max(2.0),
        "{when}: lines {} points apart, the readout's {minor} mm projects to {expected}",
        family.minor,
    );

    // Every line of the lattice is drawn where the camera puts it, and nothing else is.
    let (low, high) = if upright { (crop.left(), crop.right()) } else { (crop.top(), crop.bottom()) };
    let world_at = |s: f32| (s - screen(0.0)) / per_mm;
    let (w0, w1) = (world_at(low).min(world_at(high)), world_at(low).max(world_at(high)));
    let first = (w0 / minor).ceil() as i64;
    let last = (w1 / minor).floor() as i64;
    let mut majors = 0;
    for k in first..=last {
        let at = screen(k as f32 * minor);
        if at < low + 3.0 || at > high - 3.0 {
            continue;
        }
        let line = family.lines.iter().min_by(|a, b| (a.at - at).abs().total_cmp(&(b.at - at).abs()));
        let line = line.unwrap_or_else(|| panic!("{when}: no lines"));
        assert!((line.at - at).abs() <= 2.0, "{when}: no line at {at}, nearest {}", line.at);
        let is_major =
            (k as f32 * minor / major).fract().abs() < 1e-3 || (k as f32 * minor / major).fract().abs() > 0.999;
        if is_major {
            majors += 1;
            let found = family.majors.iter().any(|m| (m.at - at).abs() <= 2.0);
            assert!(found, "{when}: the major line at {at} does not stand out: {:?}", family.majors);
        }
    }
    for line in &family.lines {
        let nearest = ((world_at(line.at) / minor).round() * minor - world_at(line.at)).abs() * per_mm.abs();
        assert!(nearest <= 2.0, "{when}: a line at {} is off the lattice by {nearest} points", line.at);
    }
    assert!(majors >= 1, "{when}: no major line in the view");

    let ratio = major / minor;
    assert!((ratio - ratio.round()).abs() < 1e-3 && (2.0..=10.0).contains(&ratio), "{when}: ratio {ratio}");
    if family.majors.len() >= 2 {
        let interval = median(family.majors.windows(2).map(|w| w[1].at - w[0].at).collect());
        assert!(
            (interval / family.minor - ratio).abs() < 0.3,
            "{when}: majors {interval} apart, minors {}, ratio {ratio}",
            family.minor
        );
    }

    assert!(family.minor_contrast >= 4.0, "{when}: minor contrast {}", family.minor_contrast);
    assert!(
        family.major_contrast >= family.minor_contrast * 1.25,
        "{when}: major contrast {} over minor {}",
        family.major_contrast,
        family.minor_contrast
    );

    // A line 240 points long, away from where the crossing family meets it.
    let along = if upright { (crop.top(), crop.bottom()) } else { (crop.left(), crop.right()) };
    let middle = (along.0 + along.1) / 2.0;
    let reach = (120.0_f32).min((along.1 - along.0) / 2.0);
    let line = family.lines[family.lines.len() / 2].at;
    let presence = line_presence(on, off, ppp, line, (middle - reach, middle + reach), upright);
    assert!(presence >= 0.9, "{when}: {presence} of the line at {line} is there");
    family
}

/// Checks the Top view's grid from a pair of frames and returns the two families.
pub fn assert_top_view_grid(
    app: &App,
    frames: (&image::RgbaImage, &image::RgbaImage),
    when: &str,
) -> (Family, Family, (f32, f32)) {
    let spacing = readout(app).unwrap_or_else(|| panic!("{when}: no grid readout"));
    let x = assert_family(app, frames, spacing, true, &format!("{when}, lines of constant X"));
    let y = assert_family(app, frames, spacing, false, &format!("{when}, lines of constant Y"));
    (x, y, spacing)
}

/// A lattice in world space: `origin + i * step * u + j * step * v`.
#[derive(Clone, Copy, Debug)]
pub struct Lattice {
    pub origin: Vec3,
    pub u: Vec3,
    pub v: Vec3,
    pub step: f32,
}

impl Lattice {
    fn at(&self, i: f32, j: f32) -> Vec3 {
        self.origin + self.u * (i * self.step) + self.v * (j * self.step)
    }
}

/// How a lattice lies on two frames: the share of points on its lines that changed, and
/// the share of points in the middle of its cells that changed.
pub fn lattice_fit(on: &image::RgbaImage, off: &image::RgbaImage, app: &App, lattice: Lattice) -> (f32, f32, usize) {
    let ppp = app.ctx.pixels_per_point();
    let crop = grid_crop(app);
    let state = &app.state().state;
    let (camera, rect) = (&state.viewport.camera, state.viewport.rect);
    let centre = {
        let d = camera.target - lattice.origin;
        ((d.dot(lattice.u) / lattice.step).round() as i32, (d.dot(lattice.v) / lattice.step).round() as i32)
    };
    let (mut on_line, mut on_line_hits, mut in_cell, mut in_cell_hits) = (0, 0, 0, 0);
    let on_view = |p: Vec3| camera.project(p, rect).filter(|p| crop.contains(*p));
    for j in -40..40 {
        for i in -40..40 {
            let (a, b) = ((centre.0 + i) as f32, (centre.1 + j) as f32);
            let (Some(p), Some(east), Some(north)) =
                (on_view(lattice.at(a, b)), on_view(lattice.at(a + 1.0, b)), on_view(lattice.at(a, b + 1.0)))
            else {
                continue;
            };
            // Cells too small to read fade out by design.
            if (east - p).length() < 12.0 || (north - p).length() < 12.0 {
                continue;
            }
            for point in [lattice.at(a + 0.5, b), lattice.at(a, b + 0.5)] {
                if let Some(point) = on_view(point) {
                    on_line += 1;
                    on_line_hits += usize::from(difference_near(on, off, point, ppp, (2.0 * ppp) as i32) >= 4);
                }
            }
            if let Some(point) = on_view(lattice.at(a + 0.5, b + 0.5)) {
                in_cell += 1;
                in_cell_hits += usize::from(difference_near(on, off, point, ppp, ppp as i32) >= 4);
            }
        }
    }
    assert!(on_line > 0 && in_cell > 0, "no lattice cell is readable in the view: {lattice:?}");
    (on_line_hits as f32 / on_line as f32, in_cell_hits as f32 / in_cell as f32, in_cell)
}

/// The grid lies on `lattice` in these frames: its lines are drawn and its cells are clear.
pub fn assert_lattice(on: &image::RgbaImage, off: &image::RgbaImage, app: &App, lattice: Lattice, when: &str) {
    let (lines, cells, count) = lattice_fit(on, off, app, lattice);
    assert!(count >= 6, "{when}: only {count} cells in view");
    assert!(lines >= 0.9, "{when}: {lines} of the line samples found on {lattice:?}");
    assert!(cells <= 0.05, "{when}: {cells} of the cell centres changed on {lattice:?}");
}

/// Whether a line crossing is drawn at a world point: all four arms within 2 points.
pub fn crossing_at(on: &image::RgbaImage, off: &image::RgbaImage, app: &App, lattice: Lattice, i: f32, j: f32) -> bool {
    let ppp = app.ctx.pixels_per_point();
    let state = &app.state().state;
    let (camera, rect) = (&state.viewport.camera, state.viewport.rect);
    let at = |i, j| camera.project(lattice.at(i, j), rect).expect("the crossing is on screen");
    [(0.25, 0.0), (-0.25, 0.0), (0.0, 0.25), (0.0, -0.25)]
        .iter()
        .all(|(di, dj)| difference_near(on, off, at(i + di, j + dj), ppp, (2.0 * ppp) as i32) >= 4)
}
