//! Merge T, `ux-tree-visibility`: the eye at the end of a feature, plane or axis row
//! shows or hides that item in the rendered viewport, a hidden row and its eye turn grey,
//! a click on the row's name still selects, and the choices survive geometry undo and a
//! native save and reopen.
//!
//! The design is the `sdc-d-ux-tools` report's Merge T as corrected by the
//! `sdc-d-ux-tree-click` ruling (option b, an eye per row). Its expected values live in
//! `tests/fixtures/ux_tools/tree_visibility.json`; nothing here derives them from the
//! application. Every case drives the real app on a real adapter (software Vulkan counts)
//! with pointer and menu input, and only reads state back. A missing adapter fails the
//! case: a run that skips is not acceptance. Frames, the event log and any failure go to
//! `target/ux-tools/ux-tree-visibility/<case>/`. No case asserts how long anything took;
//! every wait has the 600-second hang guard.

use egui::{Modifiers, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use glam::Vec3;
use serde_json::Value;
use soliddraft_app::app::{Automation, SolidDraftApp};
use soliddraft_app::document::{
    AxisKind, Command, Document, ExtrudeParams, FeatureId, PlaneKind, ProfileKind, Reference, Selection, SketchPlane,
    ViewItem,
};
use soliddraft_app::state::EditorParams;
use soliddraft_model::PatternSpec;
use soliddraft_sketch::P2;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

type App = Harness<'static, SolidDraftApp>;

const HANG_GUARD: Duration = Duration::from_secs(600);

fn manifest() -> Value {
    serde_json::from_str(include_str!("fixtures/ux_tools/tree_visibility.json")).expect("the manifest is JSON")
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("a number in the manifest: {v}"))
}

fn v3(v: &Value) -> Vec3 {
    Vec3::new(f(&v[0]) as f32, f(&v[1]) as f32, f(&v[2]) as f32)
}

fn detection(key: &str) -> f64 {
    f(&manifest()["detection"][key])
}

// ---------------------------------------------------------------------------
// A case: one adapter at a time, its own config and evidence folder.

static SERIAL: Mutex<()> = Mutex::new(());

struct Case {
    _serial: MutexGuard<'static, ()>,
    dir: PathBuf,
    log: std::fs::File,
    started: Instant,
}

impl Case {
    fn new(name: &str) -> Self {
        let serial = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ux-tools/ux-tree-visibility").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("config")).unwrap();
        let log = std::fs::File::create(dir.join("run.log")).unwrap();
        soliddraft_app::file::set_config_dir(Some(dir.join("config")));
        let mut case = Self { _serial: serial, dir, log, started: Instant::now() };
        let commit = std::process::Command::new("git")
            .args(["-C", env!("CARGO_MANIFEST_DIR"), "rev-parse", "HEAD"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_default();
        case.note(format!("candidate commit {commit}; pair {}", manifest()["pair"]));
        case
    }

    /// A case on the real renderer. Panics when there is no adapter.
    fn gpu(name: &str) -> Self {
        let mut case = Self::new(name);
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

    fn note(&mut self, line: impl AsRef<str>) {
        writeln!(self.log, "{}", line.as_ref()).unwrap();
        eprintln!("{}", line.as_ref());
    }

    /// Renders a frame through the real pipeline and keeps it.
    fn frame(&mut self, app: &mut App, name: &str) -> image::RgbaImage {
        let image = app.render().expect("the real app renders");
        image.save(self.dir.join(format!("{name}.png"))).unwrap();
        self.note(format!("frame {name}"));
        image
    }

    /// Runs `steps`, and keeps the frame they failed on.
    fn run(&mut self, app: &mut App, steps: impl FnOnce(&mut Self, &mut App)) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| steps(self, app)));
        if let Err(panic) = result {
            if let Ok(image) = app.render() {
                let _ = image.save(self.dir.join("failure.png"));
            }
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            let _ = std::fs::write(self.dir.join("failure.txt"), &message);
            std::panic::resume_unwind(panic);
        }
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        let elapsed = self.started.elapsed().as_secs_f32();
        self.note(format!("elapsed {elapsed:.1} s (recorded only)"));
        soliddraft_app::file::set_config_dir(None);
    }
}

/// Steps the app until `done` holds, failing only at the hang guard.
fn until(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let start = Instant::now();
    while !done(app) {
        assert!(start.elapsed() < HANG_GUARD, "hang guard: {what} never happened");
        app.step();
    }
}

// ---------------------------------------------------------------------------
// Fixtures, from the manifest.

/// B20: the centred 60 x 40 mm rectangle on Top, extruded up 20 mm.
fn b20() -> Document {
    let b = &manifest()["fixtures"]["B20"];
    let mut doc = Document::new("B20", soliddraft_app::kernel());
    doc.apply(Command::AddExtrude {
        params: ExtrudeParams {
            profile: ProfileKind::Rectangle { width: f(&b["width_mm"]), height: f(&b["height_mm"]) },
            depth: f(&b["depth_mm"]),
            ..Default::default()
        },
        via_assistant: false,
    })
    .unwrap();
    doc
}

/// B20 with the independent Top-plane rectangle sketch, given an explicit shown choice.
fn b20_with_sketch() -> Document {
    let s = &manifest()["fixtures"]["sketch"];
    let mut doc = b20();
    let sketch = doc
        .apply(Command::AddSketch { plane: SketchPlane::Reference { plane: PlaneKind::Top, offset: 0.0 } })
        .unwrap()
        .unwrap();
    let mut drawing = soliddraft_sketch::Sketch::new();
    let centre = P2::new(f(&s["centre_mm"][0]), f(&s["centre_mm"][1]));
    drawing.rectangle(centre, f(&s["width_mm"]), f(&s["height_mm"])).unwrap();
    doc.apply(Command::EditSketch { id: sketch, sketch: drawing }).unwrap();
    assert_eq!(s["explicitly_shown"], true);
    doc.set_view_choice(ViewItem::Feature(sketch), Some(true));
    doc
}

fn extrusion(app: &App) -> (FeatureId, String) {
    let f = app.state().state.doc.features.iter().find(|f| !f.is_sketch()).expect("the extrusion");
    (f.id, f.name.clone())
}

fn sketch(app: &App) -> (FeatureId, String) {
    let f = app.state().state.doc.features.iter().find(|f| f.is_sketch()).expect("the sketch");
    (f.id, f.name.clone())
}

// ---------------------------------------------------------------------------
// Driving the real app.

fn launch(case: &mut Case, doc: Document, window: &str) -> App {
    let w = &manifest()["windows"][window];
    if w["theme"] == "dark" {
        std::fs::write(case.dir.join("config/settings.json"), r#"{"appearance":{"theme":"dark"}}"#).unwrap();
    }
    let size = vec2(f(&w["size"][0]) as f32, f(&w["size"][1]) as f32);
    let ppp = f(&w["pixels_per_point"]) as f32;
    case.note(format!("launch {window}: {size:?} at {ppp} px/pt, {}", w["theme"]));
    let mut app = Harness::builder()
        .with_size(size)
        .with_pixels_per_point(ppp)
        .with_render_every_step(true)
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_eframe(move |cc| SolidDraftApp::new(cc, doc, Automation::default()));
    settle(&mut app);
    app
}

/// Half a second of virtual time: animations end, and a later click is not a double click.
fn settle(app: &mut App) {
    app.run_steps(30);
}

fn viewport(app: &App) -> Rect {
    app.state().state.viewport.rect
}

/// A primary press and release at `point`, as a user clicks.
fn click_at(app: &mut App, point: Pos2) {
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

/// Moves the pointer into the menu bar, so no hover tint is in the frame.
fn pointer_away(app: &mut App) {
    let rect = viewport(app);
    app.hover_at(pos2(rect.center().x, 4.0));
    app.run_steps(6);
}

/// The bounds of the node labelled exactly `label`, in logical points.
fn bounds(app: &App, label: &str) -> Rect {
    app.get_by_role_and_label(egui::accesskit::Role::Button, label).rect()
}

/// Scrolls the tree until the row labelled `label` is in its view, as the tree does for a
/// selected row; the tree is shorter than its rows when the origin group is open.
fn reveal(app: &mut App, label: &str) {
    app.get_by_role_and_label(egui::accesskit::Role::Button, label).scroll_to_me();
    app.run_steps(6);
}

/// Clicks the middle of the row name's actual bounds, clear of its chevron and its eye.
fn click_row_name(app: &mut App, label: &str) {
    reveal(app, label);
    let r = bounds(app, label);
    click_at(app, pos2(r.left() + 60.0, r.center().y));
}

/// The eye of the row called `name`: its accessible action names the item.
fn eye_label(app: &App, name: &str) -> String {
    for action in ["Hide", "Show"] {
        let label = format!("{action} {name}");
        if app.query_by_label(&label).is_some() {
            return label;
        }
    }
    panic!("no eye for {name}");
}

fn click_eye(case: &mut Case, app: &mut App, name: &str) {
    reveal(app, name);
    let label = eye_label(app, name);
    let node = app.get_by_label(&label);
    assert!(!node.accesskit_node().is_disabled(), "the eye of {name} is enabled");
    let r = node.rect();
    case.note(format!("click eye '{label}' at {:?}", r.center()));
    click_at(app, r.center());
}

/// The accessible visibility of a row and its eye: the row's state, and the eye's state
/// and action.
fn reported(app: &App, name: &str) -> (bool, bool, String) {
    let row = app.get_by_role_and_label(egui::accesskit::Role::Button, name);
    let state = |n: &egui_kittest::Node<'_>| n.accesskit_node().data().state_description().map(str::to_owned);
    let row_shown = match state(&row).as_deref() {
        Some("Shown") => true,
        Some("Hidden") => false,
        other => panic!("{name}: the row reports {other:?}"),
    };
    let label = eye_label(app, name);
    let eye = app.get_by_label(&label);
    let eye_shown = match state(&eye).as_deref() {
        Some("Shown") => true,
        Some("Hidden") => false,
        other => panic!("{name}: the eye reports {other:?}"),
    };
    (row_shown, eye_shown, label)
}

fn assert_reported(app: &App, name: &str, shown: bool) {
    let (row, eye, label) = reported(app, name);
    assert_eq!((row, eye), (shown, shown), "{name}: row and eye report shown = {shown}");
    let expected = format!("{} {name}", if shown { "Hide" } else { "Show" });
    assert_eq!(label, expected, "{name}: the eye's action");
}

fn row_selected(app: &App, label: &str) -> bool {
    app.get_by_role_and_label(egui::accesskit::Role::Button, label).accesskit_node().toggled()
        == Some(egui::accesskit::Toggled::True)
}

fn open_origin(app: &mut App) {
    if app.query_by_label("Origin").is_none() {
        click_row_name(app, "Origin, planes and axes");
    }
    assert!(app.query_by_label("Origin").is_some(), "the origin group is open");
}

/// Clicks a menu, then each item whose label starts with the next name; menu labels carry
/// their shortcut text and Undo and Redo carry the change's name.
fn menu(case: &mut Case, app: &mut App, path: &[&str]) {
    case.note(format!("menu {}", path.join(" > ")));
    let first = bounds(app, path[0]);
    click_at(app, first.center());
    for (i, item) in path[1..].iter().enumerate() {
        let item = item.to_string();
        // The shortest label that starts with the item: "Save Ctrl+S", not "Save as…".
        let node = app
            .query_all_by(|n| n.label().is_some_and(|l| l.starts_with(item.as_str())))
            .min_by_key(|n| n.accesskit_node().label().map_or(usize::MAX, |l| l.len()))
            .unwrap_or_else(|| panic!("no menu item {item}"));
        assert!(!node.accesskit_node().is_disabled(), "the menu item {item} is enabled");
        let r = node.rect();
        if i + 2 < path.len() {
            app.hover_at(r.center());
            app.run_steps(10);
        } else {
            click_at(app, r.center());
        }
    }
}

// ---------------------------------------------------------------------------
// What must and must not change.

/// The geometry and its history: what a display choice must leave as it was.
#[derive(Debug, PartialEq)]
struct Geometry {
    features: Vec<String>,
    rollback: Option<usize>,
    semantic: String,
    geometry: String,
    undo: usize,
    redo: usize,
    volume: Option<String>,
    bounds: Option<String>,
    topology: Option<(usize, usize, usize)>,
}

fn geometry(app: &App) -> Geometry {
    let doc = &app.state().state.doc;
    let body = doc.model().body();
    Geometry {
        features: doc.features.iter().map(|f| format!("{}:{:?}:{}", f.id, f.kind, f.suppressed)).collect(),
        rollback: doc.rollback,
        semantic: format!("{:?}", doc.model().semantic_hash()),
        geometry: format!("{:?}", doc.model().geometry_hash()),
        undo: doc.undo_depth(),
        redo: doc.redo_depth(),
        volume: body.map(|b| format!("{:?}", doc.kernel().mass_properties(b).unwrap().volume)),
        bounds: body.map(|b| format!("{:?}", doc.kernel().topology(b).unwrap().bbox)),
        topology: body.map(|b| {
            let t = doc.kernel().topology(b).unwrap();
            (t.faces.len(), t.edges.len(), t.vertex_count)
        }),
    }
}

/// The part is B20 by the manifest's numbers.
fn assert_b20(case: &mut Case, app: &App, when: &str) {
    let e = &manifest()["expected"]["B20"];
    let t = &manifest()["tolerances"];
    let doc = &app.state().state.doc;
    let body = doc.model().body().unwrap_or_else(|| panic!("{when}: the part has a body"));
    let volume = doc.kernel().mass_properties(body).unwrap().volume;
    let topology = doc.kernel().topology(body).unwrap();
    // The kernel pads its bounding box; a box's bounds are its corners.
    let corner = |pick: fn(f64, f64) -> f64| {
        let p = topology
            .vertices
            .iter()
            .map(|v| v.position)
            .reduce(|a, b| soliddraft_kernel_api::Vec3::new(pick(a.x, b.x), pick(a.y, b.y), pick(a.z, b.z)));
        p.expect("the body has vertices")
    };
    let (lo, hi) = (corner(f64::min), corner(f64::max));
    case.note(format!(
        "{when}: volume {volume} mm3, bounds {lo:?} {hi:?}, {} faces {} edges {} vertices",
        topology.faces.len(),
        topology.edges.len(),
        topology.vertex_count
    ));
    assert!((volume - f(&e["volume_mm3"])).abs() <= f(&t["volume_mm3"]), "{when}: volume {volume}");
    let (min, max) = (&e["bounds_mm"]["min"], &e["bounds_mm"]["max"]);
    for (axis, (got_lo, got_hi)) in [(lo.x, hi.x), (lo.y, hi.y), (lo.z, hi.z)].into_iter().enumerate() {
        assert!((got_lo - f(&min[axis])).abs() <= f(&t["bounds_mm"]), "{when}: minimum {axis} is {got_lo}");
        assert!((got_hi - f(&max[axis])).abs() <= f(&t["bounds_mm"]), "{when}: maximum {axis} is {got_hi}");
    }
    assert_eq!(topology.faces.len() as u64, e["faces"].as_u64().unwrap(), "{when}: faces");
    assert_eq!(topology.edges.len() as u64, e["edges"].as_u64().unwrap(), "{when}: edges");
    assert_eq!(topology.vertex_count as u64, e["vertices"].as_u64().unwrap(), "{when}: vertices");
    let solids = doc.features.iter().filter(|f| !f.is_sketch()).count() as u64;
    assert_eq!(solids, e["solid_features"].as_u64().unwrap(), "{when}: solid-producing features");
}

// ---------------------------------------------------------------------------
// Reading the painted frame.

fn project(app: &App, world: Vec3) -> Pos2 {
    let state = &app.state().state;
    state.viewport.camera.project(world, state.viewport.rect).expect("the point is in front of the camera")
}

fn px(image: &image::RgbaImage, app: &App, p: Pos2) -> [i32; 3] {
    let ppp = app.ctx.pixels_per_point();
    let (x, y) = ((p.x * ppp) as u32, (p.y * ppp) as u32);
    let c = image.get_pixel(x.min(image.width() - 1), y.min(image.height() - 1)).0;
    [c[0] as i32, c[1] as i32, c[2] as i32]
}

fn diff(a: [i32; 3], b: [i32; 3]) -> i32 {
    (0..3).map(|i| (a[i] - b[i]).abs()).max().unwrap()
}

/// The backdrop's fade depends on height alone: its colour at `p`'s height, read at the
/// viewport's left margin where nothing is drawn.
fn backdrop(image: &image::RgbaImage, app: &App, p: Pos2) -> [i32; 3] {
    px(image, app, pos2(viewport(app).left() + 6.0, p.y))
}

/// Physical pixels around `p`, `r` logical points each way.
fn around(image: &image::RgbaImage, app: &App, p: Pos2, r: f32) -> Vec<(Pos2, [i32; 3])> {
    let ppp = app.ctx.pixels_per_point();
    let n = (r * ppp).ceil() as i32;
    let mut out = Vec::new();
    for dy in -n..=n {
        for dx in -n..=n {
            let q = p + vec2(dx as f32, dy as f32) / ppp;
            out.push((q, px(image, app, q)));
        }
    }
    out
}

/// Nothing but backdrop within `r` points of `p`.
fn bare(image: &image::RgbaImage, app: &App, p: Pos2, r: f32) -> bool {
    let tolerance = detection("backdrop_channel_tolerance") as i32;
    around(image, app, p, r).iter().all(|(q, c)| diff(*c, backdrop(image, app, *q)) <= tolerance)
}

/// Something painted within `r` points of `p` that `other` does not have there.
fn changed(image: &image::RgbaImage, other: &image::RgbaImage, app: &App, p: Pos2, r: f32) -> bool {
    let threshold = detection("changed_channel_difference") as i32;
    around(image, app, p, r).iter().any(|(q, c)| diff(*c, px(other, app, *q)) > threshold)
}

fn b20_corners() -> Vec<Vec3> {
    let b = &manifest()["expected"]["B20"]["bounds_mm"];
    let (lo, hi) = (v3(&b["min"]), v3(&b["max"]));
    (0..8)
        .map(|i| {
            Vec3::new(
                if i & 1 == 0 { lo.x } else { hi.x },
                if i & 2 == 0 { lo.y } else { hi.y },
                if i & 4 == 0 { lo.z } else { hi.z },
            )
        })
        .collect()
}

/// Points inside the body: face interiors of all six faces and its centre.
fn body_samples() -> Vec<Vec3> {
    let b = &manifest()["expected"]["B20"]["bounds_mm"];
    let (lo, hi) = (v3(&b["min"]), v3(&b["max"]));
    let c = (lo + hi) / 2.0;
    let h = (hi - lo) / 2.0;
    vec![
        c,
        Vec3::new(c.x, c.y, hi.z),
        Vec3::new(c.x + h.x * 0.5, c.y + h.y * 0.5, hi.z),
        Vec3::new(c.x - h.x * 0.5, c.y - h.y * 0.5, hi.z),
        Vec3::new(lo.x, c.y, c.z),
        Vec3::new(hi.x, c.y, c.z),
        Vec3::new(c.x, lo.y, c.z),
        Vec3::new(c.x, hi.y, c.z),
        Vec3::new(c.x, c.y, lo.z),
    ]
}

/// Points on the body's twelve edges, at their midpoints.
fn edge_samples() -> Vec<Vec3> {
    let b = &manifest()["expected"]["B20"]["bounds_mm"];
    let (lo, hi) = (v3(&b["min"]), v3(&b["max"]));
    let c = (lo + hi) / 2.0;
    let mut out = Vec::new();
    for a in [lo, hi] {
        for b in [lo, hi] {
            out.push(Vec3::new(c.x, a.y, b.z));
            out.push(Vec3::new(a.x, c.y, b.z));
            out.push(Vec3::new(a.x, b.y, c.z));
        }
    }
    out
}

/// Points along the four sides of the independent sketch.
fn sketch_samples() -> Vec<Vec3> {
    let b = &manifest()["expected"]["sketch_wire_bounds_mm"];
    let (lo, hi) = (v3(&b["min"]), v3(&b["max"]));
    let mut out = Vec::new();
    for i in 1..6 {
        let t = i as f32 / 6.0;
        out.push(Vec3::new(lo.x + (hi.x - lo.x) * t, lo.y, lo.z));
        out.push(Vec3::new(lo.x + (hi.x - lo.x) * t, hi.y, lo.z));
        out.push(Vec3::new(lo.x, lo.y + (hi.y - lo.y) * t, lo.z));
        out.push(Vec3::new(hi.x, lo.y + (hi.y - lo.y) * t, lo.z));
    }
    out
}

/// The convex hull of projected points, counter-clockwise.
fn hull(mut points: Vec<Pos2>) -> Vec<Pos2> {
    points.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    let cross = |o: Pos2, a: Pos2, b: Pos2| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    let mut lower: Vec<Pos2> = Vec::new();
    for p in &points {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], *p) <= 0.0 {
            lower.pop();
        }
        lower.push(*p);
    }
    let mut upper: Vec<Pos2> = Vec::new();
    for p in points.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], *p) <= 0.0 {
            upper.pop();
        }
        upper.push(*p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// Whether `p` lies within `margin` points of the convex polygon `poly`.
fn near_hull(poly: &[Pos2], p: Pos2, margin: f32) -> bool {
    (0..poly.len()).all(|i| {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let edge = b - a;
        let normal = vec2(edge.y, -edge.x).normalized();
        (p - a).dot(normal) <= margin
    })
}

fn body_hull(app: &App) -> Vec<Pos2> {
    hull(b20_corners().into_iter().map(|c| project(app, c)).collect())
}

/// The part of the viewport where no fixture geometry and no overlay is drawn.
fn exterior(app: &App) -> impl Fn(Pos2) -> bool {
    let rect = viewport(app);
    let inner = Rect::from_min_max(rect.min + vec2(20.0, 60.0), rect.max - vec2(150.0, 50.0));
    let body = body_hull(app);
    let sketch = hull(sketch_samples().into_iter().map(|c| project(app, c)).collect());
    move |p| inner.contains(p) && !near_hull(&body, p, 6.0) && !near_hull(&sketch, p, 6.0)
}

fn axis_coloured(axis: AxisKind, c: [i32; 3]) -> bool {
    let [r, g, b] = c;
    match axis {
        AxisKind::X => r > g + 60 && r > b + 60,
        AxisKind::Y => g > r + 40 && g > b + 20,
        AxisKind::Z => b > r + 60 && b > g + 30,
    }
}

/// How many exterior pixels of the viewport carry each axis's colour, X, Y, Z: the axis
/// line beyond the part and its label.
fn axis_pixels(image: &image::RgbaImage, app: &App) -> [usize; 3] {
    let rect = viewport(app);
    let ppp = app.ctx.pixels_per_point();
    let mut counts = [0; 3];
    let exterior = exterior(app);
    let (x0, y0, x1, y1) = (rect.left() * ppp, rect.top() * ppp, rect.right() * ppp, rect.bottom() * ppp);
    for y in (y0 as u32)..(y1 as u32) {
        for x in (x0 as u32)..(x1 as u32) {
            let c = image.get_pixel(x, y).0;
            let c = [c[0] as i32, c[1] as i32, c[2] as i32];
            for (i, axis) in AxisKind::ALL.into_iter().enumerate() {
                if axis_coloured(axis, c) && exterior(pos2(x as f32 / ppp, y as f32 / ppp)) {
                    counts[i] += 1;
                }
            }
        }
    }
    counts
}

fn assert_axes(case: &mut Case, app: &mut App, name: &str, shown: &[AxisKind]) {
    pointer_away(app);
    let image = case.frame(app, name);
    let counts = axis_pixels(&image, app);
    case.note(format!("{name}: axis pixels X Y Z {counts:?}, expected shown {shown:?}"));
    for (i, axis) in AxisKind::ALL.into_iter().enumerate() {
        if shown.contains(&axis) {
            let least = detection("axis_pixels_shown_at_least") as usize;
            assert!(counts[i] >= least, "{name}: {axis:?} line and label are painted, {counts:?}");
        } else {
            let most = detection("axis_pixels_hidden_at_most") as usize;
            assert!(counts[i] <= most, "{name}: {axis:?} is absent, {counts:?}");
        }
    }
    for axis in AxisKind::ALL {
        assert_reported(app, axis.label(), shown.contains(&axis));
    }
}

/// The strongest ink of a painted area against its own background: the mean of the 30
/// largest luminance differences from the colour at its left edge.
fn ink(image: &image::RgbaImage, app: &App, area: Rect, background: Pos2) -> f32 {
    let ppp = app.ctx.pixels_per_point();
    let luma = |c: [i32; 3]| 0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32;
    let bg = luma(px(image, app, background));
    let mut d: Vec<f32> = Vec::new();
    for y in ((area.top() * ppp) as u32)..((area.bottom() * ppp) as u32) {
        for x in ((area.left() * ppp) as u32)..((area.right() * ppp) as u32) {
            let c = image.get_pixel(x, y).0;
            d.push((luma([c[0] as i32, c[1] as i32, c[2] as i32]) - bg).abs());
        }
    }
    d.sort_by(|a, b| b.total_cmp(a));
    let n = d.len().min(30);
    d[..n].iter().sum::<f32>() / n.max(1) as f32
}

/// The painted strength of a row's icon and name, and of its eye.
fn row_ink(case: &mut Case, app: &mut App, name: &str, frame: &str) -> (f32, f32) {
    reveal(app, name);
    pointer_away(app);
    let image = case.frame(app, frame);
    let row = bounds(app, name);
    let eye = app.get_by_label(&eye_label(app, name)).rect();
    let text = Rect::from_min_max(pos2(row.left() + 16.0, row.top() + 3.0), pos2(eye.left() - 2.0, row.bottom() - 3.0));
    let text = Rect::from_min_max(text.min, pos2(text.right().min(text.left() + 160.0), text.bottom()));
    let background = pos2(row.left() + 6.0, row.center().y);
    let (r, e) = (ink(&image, app, text, background), ink(&image, app, eye, background));
    case.note(format!("{frame}: {name} row ink {r:.1}, eye ink {e:.1}"));
    (r, e)
}

/// A hidden row and eye are visibly dimmer than the same row shown.
fn assert_dimmer(name: &str, hidden: (f32, f32), shown: (f32, f32)) {
    let ratio = detection("hidden_row_contrast_ratio_below") as f32;
    assert!(hidden.0 < shown.0 * ratio, "{name}: the hidden row is dimmer, {hidden:?} against {shown:?}");
    assert!(hidden.1 < shown.1 * ratio, "{name}: the hidden eye is dimmer, {hidden:?} against {shown:?}");
}

/// Zooms out with the wheel, as a user does, until the fixture is in view clear of the
/// overlays. The wheel direction depends on the navigation preset, so it is found.
fn frame_fixture(case: &mut Case, app: &mut App) {
    let points: Vec<Vec3> = b20_corners().into_iter().chain(sketch_samples()).collect();
    let fits = |app: &App| {
        let rect = viewport(app);
        let inner = Rect::from_min_max(rect.min + vec2(60.0, 70.0), rect.max - vec2(170.0, 70.0));
        points.iter().all(|p| app.state().state.viewport.camera.project(*p, rect).is_some_and(|q| inner.contains(q)))
    };
    let centre = viewport(app).center();
    app.hover_at(centre);
    app.run_steps(2);
    let mut sign = 1.0;
    let start = Instant::now();
    while !fits(app) {
        assert!(start.elapsed() < HANG_GUARD, "hang guard: the fixture never came into view");
        let before = app.state().state.viewport.camera.distance;
        app.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: vec2(0.0, sign),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        });
        app.run_steps(8);
        if app.state().state.viewport.camera.distance < before {
            sign = -sign;
        }
    }
    settle(app);
    let c = &app.state().state.viewport.camera;
    case.note(format!("view {} at distance {}", c.view_name, c.distance));
}

/// A viewport point clear of the part, the sketch and every axis ray.
fn empty_point(app: &App) -> Pos2 {
    let rect = viewport(app);
    let origin = project(app, Vec3::ZERO);
    let rays: Vec<Pos2> = [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z]
        .iter()
        .map(|d| project(app, *d * 100.0))
        .collect();
    let distance = |p: Pos2| {
        rays.iter()
            .map(|end| {
                let along = (*end - origin).normalized();
                let at = (p - origin).dot(along).max(0.0);
                (origin + along * at - p).length()
            })
            .fold(f32::MAX, f32::min)
    };
    let inner = Rect::from_min_max(rect.min + vec2(40.0, 80.0), rect.max - vec2(170.0, 70.0));
    let exterior = exterior(app);
    let mut candidates = Vec::new();
    for i in 0..=8 {
        for j in 0..=8 {
            let p = pos2(inner.left() + inner.width() * i as f32 / 8.0, inner.top() + inner.height() * j as f32 / 8.0);
            if exterior(p) {
                candidates.push(p);
            }
        }
    }
    candidates.into_iter().max_by(|a, b| distance(*a).total_cmp(&distance(*b))).expect("an empty viewport point")
}

fn click_empty(case: &mut Case, app: &mut App) {
    let p = empty_point(app);
    case.note(format!("click empty viewport at {p:?}"));
    click_at(app, p);
}

/// Rests the pointer on `world` until the current readback names what is under it, or
/// nothing; the readback lags a frame behind every camera, document or display change.
fn rest_on(app: &mut App, world: Vec3, expect_hit: bool) {
    let p = project(app, world);
    app.hover_at(p);
    if expect_hit {
        until(app, "the pick readback names the face", |app| app.state().state.viewport.hover.is_some());
    } else {
        app.run_steps(20);
    }
}

// ---------------------------------------------------------------------------
// T1.

fn one_click(window: &str) {
    let case_name = format!("t1-{window}");
    let mut case = Case::gpu(&case_name);
    let mut app = launch(&mut case, b20_with_sketch(), window);
    case.run(&mut app, |case, app| {
        let (solid, solid_name) = extrusion(app);
        let (wires, sketch_name) = sketch(app);

        // 1. Origin opened by its disclosure; the body selected to show its dimensions.
        frame_fixture(case, app);
        open_origin(app);
        click_row_name(app, "Solid body");
        assert_eq!(app.state().state.doc.selection, Some(Selection::Body), "the body row selects the body");
        until(app, "the body shows its dimensions", |app| !app.state().state.viewport.dimensions.is_empty());
        let before = geometry(app);
        let camera = format!("{:?}", app.state().state.viewport.camera.view());
        assert_b20(case, app, "at launch");
        assert_reported(app, &solid_name, true);
        assert_reported(app, &sketch_name, true);
        let solid_shown_ink = row_ink(case, app, &solid_name, "01-solid-row-shown");
        let sketch_shown_ink = row_ink(case, app, &sketch_name, "01-sketch-row-shown");
        pointer_away(app);
        let shown = case.frame(app, "01-settled");
        let dimensions: Vec<Rect> = app.state().state.viewport.dimensions.iter().map(|d| d.rect).collect();
        case.note(format!("dimension annotations at {dimensions:?}"));

        // 2. The extrusion's eye hides the solid and nothing else.
        click_eye(case, app, &solid_name);
        assert_eq!(app.state().state.doc.selection, Some(Selection::Body), "the eye keeps the selection");
        assert_eq!(geometry(app), before, "hiding changes no geometry and no history");
        assert_eq!(format!("{:?}", app.state().state.viewport.camera.view()), camera, "the camera stays");
        assert_reported(app, &solid_name, false);
        assert!(app.state().state.editor.is_none() && app.state().state.rename.is_none(), "no editor, no rename");
        assert_dimmer(&solid_name, row_ink(case, app, &solid_name, "02-solid-row-hidden"), solid_shown_ink);
        pointer_away(app);
        let hidden = case.frame(app, "02-solid-hidden");
        for p in body_samples().into_iter().chain(edge_samples()) {
            assert!(bare(&hidden, app, project(app, p), 2.0), "no face or edge remains near {p}");
        }
        let covered = body_samples().iter().filter(|p| changed(&shown, &hidden, app, project(app, **p), 1.0)).count();
        let fraction = covered as f64 / body_samples().len() as f64;
        case.note(format!("body samples painted while shown: {covered} of {}", body_samples().len()));
        assert!(fraction >= detection("covered_sample_fraction"), "the shown frame painted the body there");
        assert!(app.state().state.viewport.dimensions.is_empty(), "no dimensions on a hidden body");
        for r in &dimensions {
            assert!(bare(&hidden, app, r.center(), 2.0), "the dimension at {r:?} is gone");
        }
        let top = Vec3::new(0.0, 0.0, f(&manifest()["expected"]["B20"]["bounds_mm"]["max"][2]) as f32);
        rest_on(app, top, false);
        assert_eq!(app.state().state.viewport.hover, None, "no pick of a hidden face, edge or vertex");
        click_at(app, project(app, top));
        assert_eq!(app.state().state.doc.selection, None, "a click there picks nothing");
        assert_eq!(geometry(app), before);

        // 3. The same eye shows the solid again, and a pick works.
        click_eye(case, app, &solid_name);
        assert_eq!(app.state().state.doc.selection, None, "the eye keeps the empty selection");
        assert_reported(app, &solid_name, true);
        pointer_away(app);
        let back = case.frame(app, "03-solid-shown");
        let covered = body_samples().iter().filter(|p| changed(&back, &hidden, app, project(app, **p), 1.0)).count();
        assert!(
            covered as f64 / body_samples().len() as f64 >= detection("covered_sample_fraction"),
            "the body is painted again: {covered}"
        );
        rest_on(app, top, true);
        click_at(app, project(app, top));
        let picked = match app.state().state.doc.selection {
            Some(Selection::Face(face)) => face,
            other => panic!("the top face is picked, not {other:?}"),
        };
        let face = app.state().state.doc.faces.iter().find(|n| n.id == picked).expect("a named face").clone();
        case.note(format!("picked {} at {:?}", face.label, face.centroid));
        assert!((face.centroid.z - top.z).abs() < 1e-3 && face.feature == solid, "the top face of the extrusion");
        let selected = app.state().state.doc.selection;
        assert_eq!(geometry(app), before);

        // The sketch's eye hides and shows its wires; the solid stays B20.
        let hull = body_hull(app);
        for p in sketch_samples() {
            let q = project(app, p);
            assert!(!near_hull(&hull, q, 4.0), "the sketch lies clear of the part on screen at {p}");
        }
        click_eye(case, app, &sketch_name);
        assert_reported(app, &sketch_name, false);
        assert_dimmer(&sketch_name, row_ink(case, app, &sketch_name, "03-sketch-row-hidden"), sketch_shown_ink);
        pointer_away(app);
        let wires_hidden = case.frame(app, "03-sketch-hidden");
        for p in sketch_samples() {
            assert!(bare(&wires_hidden, app, project(app, p), 2.0), "no wire remains near {p}");
        }
        click_eye(case, app, &sketch_name);
        assert_reported(app, &sketch_name, true);
        pointer_away(app);
        let wires_shown = case.frame(app, "03-sketch-shown");
        let drawn = sketch_samples()
            .iter()
            .filter(|p| changed(&wires_shown, &wires_hidden, app, project(app, **p), 1.5))
            .count();
        case.note(format!("wire samples painted: {drawn} of {}", sketch_samples().len()));
        assert!(drawn as f64 / sketch_samples().len() as f64 >= detection("covered_sample_fraction"), "wires show");
        assert_eq!(app.state().state.doc.selection, selected, "the sketch's eye keeps the selection");
        assert!(app.state().state.doc.feature(wires).is_some_and(|f| !f.suppressed), "the sketch is not suppressed");
        assert_eq!(geometry(app), before);
        assert_b20(case, app, "after the sketch toggles");

        // 4. The axis eyes, from an empty selection.
        click_empty(case, app);
        assert_eq!(app.state().state.doc.selection, None);
        assert!(!app.state().state.viewport.references.always_show_origin_axes, "always-on axes are off");
        let others = |app: &App| {
            let doc = &app.state().state.doc;
            (doc.view_choice(ViewItem::Feature(solid)), doc.view_choice(ViewItem::Feature(wires)))
        };
        let other_choices = others(app);
        let references = app.state().state.viewport.references;
        assert_axes(case, app, "04-axes-at-rest", &[]);
        let axis_hidden_ink: Vec<(f32, f32)> = AxisKind::ALL
            .into_iter()
            .map(|a| row_ink(case, app, a.label(), &format!("04-{:?}-row-hidden", a)))
            .collect();
        let sets = &manifest()["expected"]["T1_step_4_shown_axes"];
        let expected = |i: usize| -> Vec<AxisKind> {
            sets[i]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| match a.as_str().unwrap() {
                    "X" => AxisKind::X,
                    "Y" => AxisKind::Y,
                    _ => AxisKind::Z,
                })
                .collect()
        };
        for (i, axis) in AxisKind::ALL.into_iter().chain(AxisKind::ALL).enumerate() {
            click_eye(case, app, axis.label());
            assert_eq!(app.state().state.doc.selection, None, "the eye selects nothing");
            assert_axes(case, app, &format!("04-{i}-after-{:?}-eye", axis), &expected(i));
            if i < 3 {
                let shown_ink = row_ink(case, app, axis.label(), &format!("04-{:?}-row-shown", axis));
                assert_dimmer(axis.label(), axis_hidden_ink[i], shown_ink);
            }
            assert_eq!(others(app), other_choices, "other items keep their choices");
            assert_eq!(app.state().state.viewport.references, references, "the grid and defaults stay");
        }
        click_eye(case, app, "X axis");
        click_empty(case, app);
        assert_eq!(app.state().state.doc.selection, None);
        assert_axes(case, app, "04-x-shown-after-empty-click", &[AxisKind::X]);
        assert_eq!(geometry(app), before);
        assert_eq!(format!("{:?}", app.state().state.viewport.camera.view()), camera, "the camera stays");

        // 5. The plane eyes: their rows and eyes here; their rendering in
        // `one_click_changes_the_rendered_planes`.
        for plane in PlaneKind::ALL {
            let name = plane.label();
            assert_reported(app, name, true);
            click_eye(case, app, name);
            assert_reported(app, name, false);
            assert_eq!(app.state().state.doc.selection, None, "the plane eye selects nothing");
            click_eye(case, app, name);
            assert_reported(app, name, true);
        }
        assert_eq!(geometry(app), before);
    });
    drop(app);

    // 6. Selection precedence and the collector, in a fresh B20 with no choices.
    let mut app = launch(&mut case, b20(), window);
    case.run(&mut app, |case, app| {
        frame_fixture(case, app);
        let before = geometry(app);
        open_origin(app);
        click_row_name(app, "Origin");
        assert_eq!(app.state().state.doc.selection, Some(Selection::Reference(Reference::Origin)));
        assert_axes(case, app, "06-origin-selected", &AxisKind::ALL);

        click_eye(case, app, "X axis");
        assert_eq!(app.state().state.doc.selection, Some(Selection::Reference(Reference::Origin)), "Origin stays");
        assert_axes(case, app, "06-x-hidden-origin-selected", &[AxisKind::Y, AxisKind::Z]);
        let x_hidden = ViewItem::Axis(AxisKind::X);
        click_empty(case, app);
        assert_eq!(app.state().state.doc.selection, None, "the empty click clears the origin");
        assert_axes(case, app, "06-cleared", &[]);
        let choices = |app: &App| AxisKind::ALL.map(|a| app.state().state.doc.view_choice(ViewItem::Axis(a)));
        assert_eq!(choices(app), [Some(false), None, None], "X keeps its hide; Y and Z have no choice");
        assert_eq!(app.state().state.doc.view_choice(x_hidden), Some(false));

        click_row_name(app, "X axis");
        assert_eq!(app.state().state.doc.selection, Some(Selection::Reference(Reference::Axis(AxisKind::X))));
        assert!(row_selected(app, "X axis"), "the X row shows selected");
        assert_axes(case, app, "06-x-selected-still-hidden", &[AxisKind::Y, AxisKind::Z]);
        click_empty(case, app);
        assert_axes(case, app, "06-cleared-again", &[]);
        assert_eq!(choices(app), [Some(false), None, None], "selecting writes no choice");

        click_row_name(app, "Origin, planes and axes");
        assert!(app.query_by_label("Origin").is_none(), "the disclosure closed the group");
        click_row_name(app, "Origin, planes and axes");
        assert!(app.query_by_label("Origin").is_some(), "the disclosure opened the group");
        assert_eq!(app.state().state.doc.selection, None, "the disclosure selects nothing");
        assert_eq!(choices(app), [Some(false), None, None], "the disclosure writes no choice");

        // A pattern editor takes an axis row's name as its direction.
        let linear =
            app.get_all_by_role_and_label(egui::accesskit::Role::Button, "Linear").next().expect("Linear").rect();
        click_at(app, linear.center());
        assert!(
            matches!(app.state().state.editor.as_ref().map(|e| &e.params), Some(EditorParams::Pattern(_))),
            "the ribbon's Linear opened the pattern editor"
        );
        let direction = |app: &App| match &app.state().state.editor.as_ref().expect("the pattern editor").params {
            EditorParams::Pattern(p) => match p.layout {
                PatternSpec::Linear { direction, .. } => direction,
                ref other => panic!("a linear pattern, not {other:?}"),
            },
            other => panic!("the pattern editor, not {other:?}"),
        };
        click_row_name(app, "Y axis");
        assert_eq!(direction(app), soliddraft_kernel_api::Vec3::Y, "the Y row supplies the direction");
        assert_eq!(choices(app), [Some(false), None, None], "the collector pick writes no choice");
        let z_shown = reported(app, "Z axis").0;
        click_eye(case, app, "Z axis");
        assert_eq!(direction(app), soliddraft_kernel_api::Vec3::Y, "an eye is no collector pick");
        assert_eq!(choices(app), [Some(false), None, Some(!z_shown)], "the eye still flips Z");
        click_eye(case, app, "Z axis");
        let cancel = bounds(app, "Cancel (Esc)");
        click_at(app, cancel.center());
        assert!(app.state().state.editor.is_none(), "Cancel closed the editor");
        assert_eq!(geometry(app), before, "the cancelled pattern left the part as it was");
    });
}

/// T1 in the standard window.
#[test]
fn one_click_changes_the_rendered_item_and_its_row() {
    one_click("standard");
}

/// T1 once more in a small Dark window at 2 pixels per point: every row and eye stays
/// reachable.
#[test]
fn one_click_changes_the_rendered_item_and_its_row_small_dark() {
    one_click("small_dark");
}

/// T1 step 5 on the viewport: each plane eye removes and restores that plane's fill,
/// outline, label and hit target. The reference planes are drawn by the viewport lane's
/// plane renderer (grid merge C), which this build does not have yet; without it there
/// is nothing to hide, a dependency failure rather than a pass.
#[test]
#[ignore = "needs the viewport lane's plane renderer (grid merge C)"]
fn one_click_changes_the_rendered_planes() {
    let mut case = Case::gpu("t1-planes");
    let mut app = launch(&mut case, b20(), "standard");
    case.run(&mut app, |case, app| {
        frame_fixture(case, app);
        open_origin(app);
        let b = &manifest()["expected"]["B20"]["bounds_mm"];
        let reach = v3(&b["max"]).max_element() * 1.6;
        // A point inside each plane beyond the part, in its own quadrant.
        let inside = |plane: PlaneKind| match plane {
            PlaneKind::Top => Vec3::new(-reach, reach, 0.0),
            PlaneKind::Front => Vec3::new(-reach, 0.0, reach),
            PlaneKind::Right => Vec3::new(0.0, reach, reach),
        };
        pointer_away(app);
        let all = case.frame(app, "05-planes-shown");
        for hide in PlaneKind::ALL {
            click_eye(case, app, hide.label());
            pointer_away(app);
            let image = case.frame(app, &format!("05-{:?}-hidden", hide));
            for plane in PlaneKind::ALL {
                let p = project(app, inside(plane));
                if plane == hide {
                    assert!(bare(&image, app, p, 2.0), "{plane:?} is gone at {p:?}");
                } else {
                    assert!(!bare(&all, app, p, 2.0), "{plane:?} is painted at {p:?}");
                    assert!(!bare(&image, app, p, 2.0), "{plane:?} remains at {p:?}");
                }
            }
            click_eye(case, app, hide.label());
        }
    });
}

// ---------------------------------------------------------------------------
// T2.

#[test]
fn view_choices_survive_undo_and_native_reopen() {
    let mut case = Case::gpu("t2");
    let file = case.dir.join("b20.dwprt");
    let mut doc = b20();
    soliddraft_app::file::save(&mut doc, &file).expect("the fixture is saved");
    let (legacy, legacy_bytes) = {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join(manifest()["fixtures"]["legacy"].as_str().unwrap());
        let copy = case.dir.join(source.file_name().unwrap());
        std::fs::copy(&source, &copy).unwrap();
        soliddraft_app::file::remember(&copy);
        soliddraft_app::file::remember(&file);
        (copy.clone(), std::fs::read(&copy).unwrap())
    };
    // The saved part keeps the history that built it, so Undo can take the extrusion back.
    let mut app = launch(&mut case, doc, "standard");
    let mut saved: Option<(String, String, String, FeatureId)> = None;
    case.run(&mut app, |case, app| {
        frame_fixture(case, app);
        let (solid, solid_name) = extrusion(app);
        open_origin(app);

        // 1. The choices, each through its eye, then File > Save.
        click_eye(case, app, &solid_name);
        click_row_name(app, "Origin");
        assert_axes(case, app, "t2-origin-selected", &AxisKind::ALL);
        click_eye(case, app, "X axis");
        assert_eq!(app.state().state.doc.selection, Some(Selection::Reference(Reference::Origin)));
        click_empty(case, app);
        assert_eq!(app.state().state.doc.selection, None);
        click_eye(case, app, "Y axis");
        assert_reported(app, "Top plane", true);
        click_eye(case, app, "Top plane");
        let expect = &manifest()["expected"]["T2_choices"];
        let want = |key: &str| Some(expect[key] == "shown");
        let choices = |app: &App| {
            let doc = &app.state().state.doc;
            (
                doc.view_choice(ViewItem::Feature(solid)),
                doc.view_choice(ViewItem::Axis(AxisKind::X)),
                doc.view_choice(ViewItem::Axis(AxisKind::Y)),
                doc.view_choice(ViewItem::Plane(PlaneKind::Top)),
            )
        };
        let chosen = (want("extrusion"), want("X"), want("Y"), want("Top"));
        assert_eq!(choices(app), chosen);
        let before = geometry(app);
        menu(case, app, &["File", "Save"]);
        assert!(!app.state().state.doc.dirty, "File > Save wrote the part");

        // 2. Undo past the extrusion; the datum choices hold, and redo brings it back hidden.
        menu(case, app, &["Edit", "Undo"]);
        until(app, "the undo", |app| app.state().state.doc.features.is_empty());
        settle(app);
        assert!(app.state().state.doc.model().body().is_none(), "no solid after the undo");
        assert!(app.query_by_label(&solid_name).is_none(), "the extrusion left the tree");
        assert_eq!(app.state().state.doc.redo_depth(), before.redo + 1, "one redo entry");
        assert_eq!((choices(app).1, choices(app).2, choices(app).3), (chosen.1, chosen.2, chosen.3));
        // A view choice while the redo waits must not consume it.
        click_eye(case, app, "Z axis");
        click_eye(case, app, "Z axis");
        assert_eq!(app.state().state.doc.redo_depth(), before.redo + 1, "the eyes kept the redo entry");
        menu(case, app, &["Edit", "Redo"]);
        until(app, "the redo", |app| !app.state().state.doc.features.is_empty());
        settle(app);
        assert_eq!(extrusion(app).0, solid, "the redo restores the same feature id");
        assert_reported(app, &solid_name, false);
        assert_eq!(choices(app), chosen, "the redone extrusion is still hidden");
        assert_eq!(geometry(app), before, "the view operations added and consumed no geometry history");
        click_eye(case, app, &solid_name);
        assert_b20(case, app, "shown after the redo");
        click_eye(case, app, &solid_name);
        assert_eq!(choices(app), chosen);

        // 3. Save again; its identity is what the reopened part must have.
        menu(case, app, &["File", "Save"]);
        let model = app.state().state.doc.model();
        saved = Some((
            format!("{:?}", model.id()),
            format!("{:?}", model.semantic_hash()),
            format!("{:?}", model.geometry_hash()),
            solid,
        ));
    });
    drop(app);

    let (id, semantic, geometry_hash, solid) = saved.expect("the first part was saved");
    let mut app = launch(&mut case, soliddraft_app::new_document(), "standard");
    case.run(&mut app, |case, app| {
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        menu(case, app, &["File", "Open recent", &name]);
        until(app, "the saved part opens", |app| !app.state().state.doc.features.is_empty());
        settle(app);
        frame_fixture(case, app);
        let model = app.state().state.doc.model();
        assert_eq!(format!("{:?}", model.id()), id, "the reopened part keeps its id");
        assert_eq!(format!("{:?}", model.semantic_hash()), semantic, "and its revision");
        assert_eq!(format!("{:?}", model.geometry_hash()), geometry_hash, "and its geometry");
        let (reopened, solid_name) = extrusion(app);
        assert_eq!(reopened, solid, "the same feature id");
        open_origin(app);
        assert_reported(app, &solid_name, false);
        assert_reported(app, "Top plane", false);
        assert_reported(app, "Front plane", true);
        assert_reported(app, "Right plane", true);
        assert_axes(case, app, "t2-reopened", &[AxisKind::Y]);
        pointer_away(app);
        let image = case.frame(app, "t2-reopened-hidden-solid");
        // The shown Y axis runs through the origin; samples on its line are not the body's.
        let (o, y) = (project(app, Vec3::NEG_Y * 100.0), project(app, Vec3::Y * 100.0));
        let off_y = |p: Pos2| {
            let along = (y - o).normalized();
            (o + along * (p - o).dot(along) - p).length() > 6.0
        };
        for p in body_samples().into_iter().filter(|p| off_y(project(app, *p))) {
            assert!(bare(&image, app, project(app, p), 2.0), "the reopened solid is hidden near {p}");
        }
        click_eye(case, app, &solid_name);
        assert_b20(case, app, "reopened and shown");
        pointer_away(app);
        let shown = case.frame(app, "t2-reopened-shown");
        let covered = body_samples().iter().filter(|p| changed(&shown, &image, app, project(app, **p), 1.0)).count();
        assert!(covered as f64 / body_samples().len() as f64 >= detection("covered_sample_fraction"), "shown");

        // 4. The old part, by the same menu, keeps the defaults and inherits nothing.
        let old = legacy.file_name().unwrap().to_string_lossy().into_owned();
        menu(case, app, &["File", "Open recent", &old]);
        until(app, "the old part opens", |app| app.state().state.doc.name != "b20");
        settle(app);
        frame_fixture_any(case, app);
        let doc = &app.state().state.doc;
        assert!(doc.model().body().is_some(), "the old part has its solid");
        for f in doc.features.iter() {
            assert_eq!(doc.view_choice(ViewItem::Feature(f.id)), None, "{} has no choice", f.name);
        }
        for a in AxisKind::ALL {
            assert_eq!(doc.view_choice(ViewItem::Axis(a)), None, "{a:?} has no choice");
        }
        for p in PlaneKind::ALL {
            assert_eq!(doc.view_choice(ViewItem::Plane(p)), None, "{p:?} has no choice");
        }
        let defaults = &manifest()["expected"]["defaults"];
        let solids: Vec<String> = doc.features.iter().filter(|f| !f.is_sketch()).map(|f| f.name.clone()).collect();
        open_origin(app);
        for name in &solids {
            assert_reported(app, name, defaults["body_shown"] == true);
        }
        for p in PlaneKind::ALL {
            assert_reported(app, p.label(), defaults["planes_shown"] == true);
        }
        for a in AxisKind::ALL {
            assert_reported(app, a.label(), defaults["axes_shown"] == true);
        }
        pointer_away(app);
        let image = case.frame(app, "t2-legacy");
        assert_eq!(axis_pixels_anywhere(&image, app), [0, 0, 0], "no axis is shown in the old part");
        assert_eq!(std::fs::read(&legacy).unwrap(), legacy_bytes, "reading the old part left its file as it was");
    });
}

/// For a part other than B20: zooms to fit through the heads-up toolbar.
fn frame_fixture_any(case: &mut Case, app: &mut App) {
    let fit = bounds(app, "Zoom to fit (Z)");
    click_at(app, fit.center());
    case.note("zoom to fit");
}

/// Axis-coloured pixels anywhere in the viewport, X, Y, Z.
fn axis_pixels_anywhere(image: &image::RgbaImage, app: &App) -> [usize; 3] {
    let rect = viewport(app).shrink(2.0);
    let ppp = app.ctx.pixels_per_point();
    let mut counts = [0; 3];
    for y in ((rect.top() * ppp) as u32)..((rect.bottom() * ppp) as u32) {
        for x in ((rect.left() * ppp) as u32)..((rect.right() * ppp) as u32) {
            let c = image.get_pixel(x, y).0;
            let c = [c[0] as i32, c[1] as i32, c[2] as i32];
            for (i, axis) in AxisKind::ALL.into_iter().enumerate() {
                if axis_coloured(axis, c) {
                    counts[i] += 1;
                }
            }
        }
    }
    counts
}

// ---------------------------------------------------------------------------
// The serializer boundary.

/// View choices travel in the part file and in a recovery snapshot, outside the part's
/// truth: absent from a file that has none, keyed by id, and never a reason to refuse
/// the geometry.
#[test]
fn view_choices_serialize_outside_the_part_truth() {
    let mut case = Case::new("serializer");
    let kernel = soliddraft_app::kernel();
    let mut doc = soliddraft_model::Document::from_bytes(&b20().model().to_bytes().unwrap(), kernel.clone()).unwrap();
    case.note("B20 written and read back without view choices");
    let plain = doc.to_bytes().unwrap();
    let entries = |bytes: &[u8]| {
        let package = soliddraft_document::manifest::Package::from_bytes(bytes).unwrap();
        package.manifest().unwrap().sections.iter().map(|s| (s.name.clone(), s.required)).collect::<Vec<_>>()
    };
    assert!(!entries(&plain).iter().any(|(n, _)| n == soliddraft_model::view::ENTRY), "no choices, no section");
    let reopened = soliddraft_model::Document::from_bytes(&plain, kernel.clone()).unwrap();
    assert!(reopened.visibility().is_empty(), "a file without the section opens at the defaults");

    let (revision, shape) = (doc.semantic_hash(), doc.geometry_hash());
    let feature = soliddraft_model::view::ViewItem::Feature(doc.features()[0].id);
    let axis = soliddraft_model::view::ViewItem::Axis(soliddraft_model::view::DatumAxis::Y);
    doc.set_visibility(feature, Some(false));
    doc.set_visibility(axis, Some(true));
    assert_eq!((doc.semantic_hash(), doc.geometry_hash()), (revision.clone(), shape.clone()), "outside both hashes");
    let bytes = doc.to_bytes().unwrap();
    assert!(
        entries(&bytes).contains(&(soliddraft_model::view::ENTRY.to_owned(), false)),
        "an optional section: {:?}",
        entries(&bytes)
    );
    let back = soliddraft_model::Document::from_bytes(&bytes, kernel.clone()).unwrap();
    assert_eq!(back.visibility(), doc.visibility(), "the choices round-trip by id");
    assert_eq!(back.visibility().choice(feature), Some(false));
    assert_eq!(back.visibility().choice(axis), Some(true));
    assert_eq!((back.semantic_hash(), back.geometry_hash()), (revision.clone(), shape.clone()));

    let (described, snapshot) = doc.snapshot().encode().unwrap();
    assert_eq!(described.semantic_hash, revision, "the snapshot's revision");
    let recovered = soliddraft_model::Document::from_bytes(&snapshot, kernel.clone()).unwrap();
    assert_eq!(recovered.visibility(), doc.visibility(), "a recovery snapshot keeps the choices");

    // A section this version cannot read leaves the geometry opening, at the defaults.
    let mut package = soliddraft_document::manifest::Package::from_bytes(&bytes).unwrap();
    let m = package.manifest().cloned().unwrap();
    let part = package.read_text("part.json").unwrap().unwrap();
    let view = package.read_text(soliddraft_model::view::ENTRY).unwrap().unwrap();
    let newer = view.replace(soliddraft_model::view::FORMAT, "soliddraft-view/9");
    let rewritten = soliddraft_document::manifest::write_package(
        &m,
        &[
            soliddraft_document::manifest::Entry::new("part.json", part.into_bytes()),
            soliddraft_document::manifest::Entry::new(soliddraft_model::view::ENTRY, newer.into_bytes()),
        ],
    )
    .unwrap();
    let opened = soliddraft_model::Document::from_bytes(&rewritten, kernel).expect("the geometry still opens");
    assert!(opened.visibility().is_empty(), "an unreadable section gives the defaults");
    assert_eq!(opened.semantic_hash(), revision);
    let volume = opened.kernel().mass_properties(opened.body().unwrap()).unwrap().volume;
    case.note(format!("opened with an unreadable view section: volume {volume}"));
    assert!((volume - f(&manifest()["expected"]["B20"]["volume_mm3"])).abs() <= 1e-6);
}
