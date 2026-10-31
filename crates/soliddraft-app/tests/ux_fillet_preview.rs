//! Merge F, `ux-fillet-preview`: the Fillet tool collects edges and faces, previews the
//! rounded body as the radius changes, refuses what it cannot round beside the control
//! that asked for it, and commits only on OK.
//!
//! The real app on the real wgpu pipeline and the real Catallax kernel, driven only with
//! pointer presses, typed text and keys. The document, the editor and the meshes are read
//! back, never written. Every expected value comes from
//! `tests/fixtures/ux_tools/fillet_preview.json`, the designer's hand-derived manifest;
//! nothing here asks the kernel or the picker what the answer should be.
//!
//! A missing adapter fails a case: a run that skips is not acceptance. Each case leaves
//! its event trace, measures and frames under `target/ux-tools/ux-fillet-preview/<test>/`.
//! No case asserts how long anything took.

use egui::{Key, Modifiers, Pos2, Rect, pos2};
use egui_kittest::kittest::{NodeT, Queryable};
use glam::Vec3;
use serde_json::Value;
use soliddraft_app::document::{
    Command, DisplayMesh, Document, ExtrudeParams, FeatureKind, FeatureStatus, PlaneKind, ProfileKind, Selection,
    SketchPlane,
};
use soliddraft_app::state::EditorParams;
use soliddraft_kernel_api::CurveKind;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[path = "support/ribbon.rs"]
mod ribbon;

use ribbon::App;

const SCENARIO: &str = "ux-fillet-preview";
const HANG_GUARD: Duration = Duration::from_secs(600);

// ---------------------------------------------------------------------------
// The manifest.

fn manifest() -> &'static Value {
    static MANIFEST: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| {
        serde_json::from_str(include_str!("fixtures/ux_tools/fillet_preview.json")).expect("the manifest is JSON")
    })
}

fn at(path: &str) -> &'static Value {
    manifest().pointer(path).unwrap_or_else(|| panic!("the manifest has {path}"))
}

fn num(path: &str) -> f64 {
    at(path).as_f64().unwrap_or_else(|| panic!("{path} is a number"))
}

fn count(path: &str) -> usize {
    at(path).as_u64().unwrap_or_else(|| panic!("{path} is a count")) as usize
}

fn text(path: &str) -> &'static str {
    at(path).as_str().unwrap_or_else(|| panic!("{path} is text"))
}

fn point(path: &str) -> Vec3 {
    let v: Vec<f32> = at(path)
        .as_array()
        .unwrap_or_else(|| panic!("{path} is a point"))
        .iter()
        .map(|c| c.as_f64().expect("a coordinate") as f32)
        .collect();
    Vec3::new(v[0], v[1], v[2])
}

// ---------------------------------------------------------------------------
// One case: the adapter to itself, a config folder of its own, and its evidence.

static GPU: Mutex<()> = Mutex::new(());

struct Case {
    dir: PathBuf,
    log: Mutex<std::fs::File>,
    started: Instant,
    _gpu: std::sync::MutexGuard<'static, ()>,
}

impl Case {
    fn start(test: &str) -> Case {
        let gpu = GPU.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = env_logger::try_init();
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ux-tools").join(SCENARIO).join(test);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("config")).unwrap();
        let log = std::fs::File::create(dir.join("run.log")).unwrap();
        soliddraft_app::file::set_config_dir(Some(dir.join("config")));
        let case = Case { dir, log: Mutex::new(log), started: Instant::now(), _gpu: gpu };
        case.note(format!("scenario {SCENARIO} test {test}"));
        case.note(format!("parent app {}", text("/pair/parent_app")));
        let candidate = std::env::var("GITHUB_SHA").ok().or_else(|| {
            let out = std::process::Command::new("git")
                .args(["-C", env!("CARGO_MANIFEST_DIR"), "rev-parse", "HEAD"])
                .output()
                .ok()?;
            Some(String::from_utf8_lossy(&out.stdout).trim().to_owned())
        });
        case.note(format!("candidate app {}", candidate.as_deref().unwrap_or("unknown")));
        // A pin move is the test owner's to record: the pair must name the kernel that ran.
        assert_eq!(
            soliddraft_kernel_catallax::CATALLAX_REVISION,
            text("/pair/catallax_pin"),
            "the manifest names the Catallax pin these results were run against"
        );
        case.note(format!("catallax pin {}", soliddraft_kernel_catallax::CATALLAX_REVISION));
        let instance = egui_wgpu::wgpu::Instance::default();
        let adapters = pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all()));
        for adapter in &adapters {
            let info = adapter.get_info();
            case.note(format!("adapter {} on {:?} ({:?})", info.name, info.backend, info.device_type));
        }
        assert!(!adapters.is_empty(), "this acceptance test requires a renderer");
        case
    }

    fn note(&self, line: impl AsRef<str>) {
        let mut log = self.log.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        writeln!(log, "{}", line.as_ref()).unwrap();
        eprintln!("{}", line.as_ref());
    }

    /// Runs the steps on a harness; a failure keeps its frame and its message.
    fn run(&self, app: &mut App, steps: impl FnOnce(&mut App)) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| steps(app)));
        if let Err(panic) = result {
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_default();
            let _ = std::fs::write(self.dir.join("failure.txt"), &message);
            self.note(format!("FAILED: {message}"));
            if let Ok(image) = app.render() {
                let _ = image.save(self.dir.join("failure.png"));
            }
            std::panic::resume_unwind(panic);
        }
    }

    /// Renders and keeps a frame, with the viewport cropped out beside it.
    fn frame(&self, app: &mut App, name: &str) -> image::RgbaImage {
        let image = app.render().expect("the real app renders");
        image.save(self.dir.join(format!("{name}.png"))).unwrap();
        self.crop(app, &image, viewport(app), &format!("{name}-viewport"));
        let size = app.ctx.content_rect().size();
        self.note(format!(
            "frame {name}: {}x{} px for {}x{} points at {} px/pt",
            image.width(),
            image.height(),
            size.x,
            size.y,
            app.ctx.pixels_per_point()
        ));
        image
    }

    fn crop(&self, app: &App, image: &image::RgbaImage, rect: Rect, name: &str) {
        let ppp = app.ctx.pixels_per_point();
        let x = (rect.left() * ppp).max(0.0) as u32;
        let y = (rect.top() * ppp).max(0.0) as u32;
        let w = ((rect.width() * ppp) as u32).min(image.width().saturating_sub(x));
        let h = ((rect.height() * ppp) as u32).min(image.height().saturating_sub(y));
        let _ = image::imageops::crop_imm(image, x, y, w, h).to_image().save(self.dir.join(format!("{name}.png")));
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        self.note(format!("elapsed {:.1} s (recorded only)", self.started.elapsed().as_secs_f32()));
        soliddraft_app::file::set_config_dir(None);
    }
}

// ---------------------------------------------------------------------------
// The fixtures, built before the first observed frame.

/// B20: the centred 60 x 40 rectangle on Top, extruded 20 up.
fn b20() -> Document {
    let f = "/fixtures/B20";
    let mut doc = Document::new("B20", soliddraft_app::kernel());
    doc.apply(Command::AddExtrude {
        params: ExtrudeParams {
            profile: ProfileKind::Rectangle {
                width: num(&format!("{f}/width_mm")),
                height: num(&format!("{f}/height_mm")),
            },
            depth: num(&format!("{f}/depth_mm")),
            ..Default::default()
        },
        via_assistant: false,
    })
    .unwrap();
    doc
}

/// S: the centred 50 x 10 slot on Top, extruded 10 up.
fn slot() -> Document {
    let f = "/fixtures/S";
    let mut doc = Document::new("S", soliddraft_app::kernel());
    let sketch = doc
        .apply(Command::AddSketch { plane: SketchPlane::Reference { plane: PlaneKind::Top, offset: 0.0 } })
        .unwrap()
        .unwrap();
    let mut drawing = soliddraft_sketch::Sketch::new();
    drawing
        .slot(soliddraft_sketch::P2::new(0.0, 0.0), num(&format!("{f}/length_mm")), num(&format!("{f}/width_mm")))
        .unwrap();
    doc.apply(Command::EditSketch { id: sketch, sketch: drawing }).unwrap();
    doc.apply(Command::AddExtrude {
        params: ExtrudeParams { sketch: Some(sketch), depth: num(&format!("{f}/depth_mm")), ..Default::default() },
        via_assistant: false,
    })
    .unwrap();
    doc
}

fn launch(case: &Case, doc: Document) -> App {
    let mut app = ribbon::app(doc);
    settle(&mut app);
    case.note(format!("launch {}x{} at {} px/pt", ribbon::WINDOW.x, ribbon::WINDOW.y, app.ctx.pixels_per_point()));
    app
}

// ---------------------------------------------------------------------------
// Input, as a user makes it.

/// Steps until a condition holds, under the ten-minute hang guard.
fn until(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let start = Instant::now();
    let mut frames = 0;
    while !done(app) {
        assert!(start.elapsed() < HANG_GUARD, "still waiting for {what} after {frames} frames");
        app.step();
        frames += 1;
    }
}

/// Until the rebuild, its proof and the shown meshes stop changing.
fn settle(app: &mut App) {
    until(app, "the rebuild and its proof", |a| !a.state().state.doc.proof_pending());
    let mut last = (u64::MAX, 0);
    let mut quiet = 0;
    let start = Instant::now();
    while quiet < 10 {
        assert!(start.elapsed() < HANG_GUARD, "the display is still changing");
        app.step();
        let doc = &app.state().state.doc;
        let now = (doc.mesh_version, doc.triangle_count());
        quiet = if now == last { quiet + 1 } else { 0 };
        last = now;
    }
}

fn viewport(app: &App) -> Rect {
    app.state().state.viewport.rect
}

fn project(app: &App, world: Vec3) -> Pos2 {
    let state = &app.state().state;
    state.viewport.camera.project(world, state.viewport.rect).expect("the point is in front of the camera")
}

/// A pick in the viewport at a world point.
fn pick(case: &Case, app: &mut App, what: &str, world: Vec3) {
    let at = project(app, world);
    assert!(viewport(app).contains(at), "{what} at {world} is on screen at {at:?}");
    case.note(format!("click {what} at world {world} screen {at:?}"));
    ribbon::click_at(app, at);
    app.run_steps(2);
}

fn click_fillet(case: &Case, app: &mut App) {
    case.note("click ribbon Fillet");
    ribbon::click_tab(app, "Solid");
    ribbon::click_ribbon(app, "Fillet");
}

fn type_radius(case: &Case, app: &mut App, value: &str) {
    case.note(format!("type Radius {value:?}, Enter"));
    ribbon::type_field(app, "Radius value", value);
}

/// The panel's own OK button.
fn click_ok(case: &Case, app: &mut App) {
    case.note("click OK");
    let rect = ribbon::button_rect(app, "OK (Enter)", ribbon::window(app));
    ribbon::click_at(app, rect.center());
    settle(app);
}

fn press(case: &Case, app: &mut App, modifiers: Modifiers, key: Key) {
    case.note(format!("key {modifiers:?} {key:?}"));
    app.key_press_modifiers(modifiers, key);
    app.run_steps(2);
    settle(app);
}

/// The pointer rests in the chrome, so no hover tint is in the frame.
fn pointer_off_model(app: &mut App) {
    let rect = viewport(app);
    app.hover_at(pos2(rect.center().x, 4.0));
    app.run_steps(4);
}

/// Steps until the camera has stopped moving.
fn camera_rests(app: &mut App) {
    let pose = |a: &App| {
        let c = &a.state().state.viewport.camera;
        [c.target.x, c.target.y, c.target.z, c.distance, c.yaw, c.pitch]
    };
    let mut last = pose(app);
    let mut quiet = 0;
    let start = Instant::now();
    while quiet < 5 {
        assert!(start.elapsed() < HANG_GUARD, "the camera is still moving");
        app.step();
        let now = pose(app);
        quiet = if now == last { quiet + 1 } else { 0 };
        last = now;
    }
}

/// A standard view from the View tab, fitted, then back to the Solid tab.
fn view(case: &Case, app: &mut App, name: &str) {
    case.note(format!("View tab: {name}, Fit"));
    ribbon::click_tab(app, "View");
    ribbon::click_ribbon(app, name);
    camera_rests(app);
    ribbon::click_ribbon(app, "Fit");
    camera_rests(app);
    ribbon::click_tab(app, "Solid");
    pointer_off_model(app);
}

// ---------------------------------------------------------------------------
// Reading the result back.

/// The committed part: what a preview, a refusal or a cancel must leave alone.
#[derive(Clone, Debug, PartialEq)]
struct Committed {
    features: Vec<String>,
    geometry: String,
    semantic: String,
    undo: usize,
    redo: usize,
    dirty: bool,
}

fn committed(app: &App) -> Committed {
    let doc = &app.state().state.doc;
    Committed {
        features: doc
            .features
            .iter()
            .map(|f| format!("{:?} {:?} {:?} {} {:?}", f.id, f.kind, f.status, f.suppressed, doc.rollback))
            .collect(),
        geometry: format!("{:?}", doc.model().geometry_hash()),
        semantic: format!("{:?}", doc.model().semantic_hash()),
        undo: doc.undo_depth(),
        redo: doc.redo_depth(),
        dirty: doc.dirty,
    }
}

fn volume(doc: &Document) -> f64 {
    let body = doc.display_body().expect("a body");
    doc.kernel().mass_properties(body).expect("mass properties").volume
}

/// Bounds and topology counts of the shown body. The kernel's own box is a padded
/// enclosure, so the bounds are taken over the vertices and the edges' midpoints, where
/// every fixture here reaches its extremes.
fn shape(doc: &Document) -> ([f64; 3], [f64; 3], usize, usize, usize) {
    let body = doc.display_body().expect("a body");
    let t = doc.kernel().topology(body).expect("topology");
    let points = t.vertices.iter().map(|v| v.position).chain(t.edges.iter().map(|e| e.midpoint));
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for p in points {
        for (i, c) in [p.x, p.y, p.z].into_iter().enumerate() {
            lo[i] = lo[i].min(c);
            hi[i] = hi[i].max(c);
        }
    }
    (lo, hi, t.faces.len(), t.edges.len(), t.vertex_count)
}

fn assert_bounds(doc: &Document, fixture: &str) {
    let (lo, hi, ..) = shape(doc);
    let tol = num("/tolerances/bounds_mm");
    let want_lo = point(&format!("/fixtures/{fixture}/bounds_min"));
    let want_hi = point(&format!("/fixtures/{fixture}/bounds_max"));
    for i in 0..3 {
        assert!((lo[i] - want_lo[i] as f64).abs() <= tol, "{fixture} bounds min {lo:?}, not {want_lo}");
        assert!((hi[i] - want_hi[i] as f64).abs() <= tol, "{fixture} bounds max {hi:?}, not {want_hi}");
    }
}

fn assert_b20(case: &Case, app: &App, when: &str) {
    let doc = &app.state().state.doc;
    let v = volume(doc);
    let (_, _, faces, edges, vertices) = shape(doc);
    case.note(format!("{when}: volume {v} faces {faces} edges {edges} vertices {vertices}"));
    assert!((v - num("/fixtures/B20/volume_mm3")).abs() <= num("/tolerances/box_volume_mm3"), "{when}: volume {v}");
    assert_bounds(doc, "B20");
    assert_eq!(
        (faces, edges, vertices),
        (count("/fixtures/B20/faces"), count("/fixtures/B20/edges"), count("/fixtures/B20/vertices")),
        "{when}: B20 topology"
    );
}

fn assert_s(case: &Case, app: &App, when: &str) {
    let doc = &app.state().state.doc;
    let v = volume(doc);
    case.note(format!("{when}: volume {v}"));
    assert!((v - num("/fixtures/S/volume_mm3")).abs() <= num("/tolerances/slot_volume_mm3"), "{when}: volume {v}");
    assert_bounds(doc, "S");
}

/// The volume a triangle mesh encloses: the sum of its oriented tetrahedra.
fn mesh_volume(mesh: &DisplayMesh) -> f64 {
    mesh.indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| mesh.positions[i as usize].as_dvec3());
            a.dot(b.cross(c)) / 6.0
        })
        .sum()
}

/// The rounded body the open panel previews, as the viewport draws it.
fn preview(app: &App) -> Option<std::sync::Arc<DisplayMesh>> {
    app.state().state.editor.as_ref().and_then(|e| e.preview.result.clone())
}

/// The committed body as the viewport draws it.
fn shown_mesh_volume(app: &App) -> f64 {
    app.state().state.doc.meshes.values().map(|m| mesh_volume(m)).sum()
}

/// A preview mesh within the designer's tolerance of an exact volume: a share of the
/// material the rounding removes, since chords cut the curved surface short.
fn assert_mesh_volume(case: &Case, measured: f64, original: f64, exact: f64, what: &str) {
    let removed = original - exact;
    let tol = num("/tolerances/preview_mesh_volume_fraction_of_removed") * removed;
    case.note(format!("{what}: mesh volume {measured}, exact {exact}, removed {removed}, tolerance {tol}"));
    assert!((measured - exact).abs() <= tol, "{what}: mesh volume {measured}, not {exact} within {tol}");
}

/// An edge the open panel holds, resolved on the shown body: its name, length, curve
/// and midpoint.
#[derive(Clone, Debug)]
struct Held {
    label: String,
    length: f64,
    curve: CurveKind,
    midpoint: Vec3,
}

fn held(app: &App) -> Vec<Held> {
    let state = &app.state().state;
    let editor = state.editor.as_ref().expect("the Fillet panel is open");
    let doc = &state.doc;
    let body = doc.display_body().expect("a body");
    let topology = doc.kernel().topology(body).expect("topology");
    editor
        .edges()
        .iter()
        .map(|r| {
            let id =
                doc.model().resolve_edge(r, body).unwrap_or_else(|e| panic!("{} resolves: {}", r.label, e.message));
            let e = topology.edges.iter().find(|e| e.id == id).expect("the edge is on the body");
            let m = e.midpoint;
            Held {
                label: r.label.clone(),
                length: e.length,
                curve: e.curve,
                midpoint: Vec3::new(m.x as f32, m.y as f32, m.z as f32),
            }
        })
        .collect()
}

fn fillet_radius(app: &App) -> f64 {
    match &app.state().state.editor.as_ref().expect("the Fillet panel is open").params {
        EditorParams::Fillet(f) => f.radius,
        other => panic!("not the Fillet panel: {other:?}"),
    }
}

/// The one live fillet in the tree.
fn the_fillet(app: &App) -> (Vec<String>, f64) {
    let doc = &app.state().state.doc;
    let live: Vec<_> = doc
        .features
        .iter()
        .filter_map(|f| match &f.kind {
            FeatureKind::Fillet(p) if !f.suppressed && f.status != FeatureStatus::Inactive => Some((f, p)),
            _ => None,
        })
        .collect();
    assert_eq!(live.len(), 1, "one fillet: {:?}", doc.features.iter().map(|f| &f.name).collect::<Vec<_>>());
    assert_eq!(live[0].0.status, FeatureStatus::Ok, "the fillet built");
    (live[0].1.edges.iter().map(|e| e.label.clone()).collect(), live[0].1.radius)
}

fn assert_valid_drawn(app: &App, what: &str) {
    let doc = &app.state().state.doc;
    let body = doc.display_body().expect("a body");
    let validity = doc.kernel().validate(body).expect("a validity answer");
    assert!(validity.valid, "{what}: the body is valid: {validity:?}");
    assert_eq!(doc.kernel().topology(body).expect("topology").kind, soliddraft_kernel_api::BodyKind::Solid);
    assert!(doc.triangle_count() > 0, "{what}: the body is drawn");
    assert_eq!(doc.display_failure(), None, "{what}: no display failure");
    assert_eq!(doc.error_count(), 0, "{what}: no errors");
}

/// Text the last frame painted in the side panel, left of the viewport.
fn panel_text(app: &App) -> Vec<ribbon::Painted> {
    let left = viewport(app).left();
    ribbon::painted(app).into_iter().filter(|p| p.rect.right() <= left + ribbon::TOLERANCE).collect()
}

fn panel_rect(app: &App, exact: &str) -> Rect {
    panel_text(app)
        .into_iter()
        .find(|p| p.text == exact)
        .unwrap_or_else(|| {
            panic!("the panel paints {exact:?}: {:?}", panel_text(app).iter().map(|p| &p.text).collect::<Vec<_>>())
        })
        .rect
}

/// A message painted in the panel between two heights: beside the control it is about.
fn assert_painted_between(case: &Case, app: &App, message: &str, top: f32, bottom: f32, what: &str) {
    let found: Vec<_> = panel_text(app).into_iter().filter(|p| p.text.contains(message)).collect();
    case.note(format!(
        "{what}: {message:?} painted at {:?}, band {top}..{bottom}",
        found.iter().map(|p| p.rect).collect::<Vec<_>>()
    ));
    assert!(
        found.iter().any(|p| p.rect.top() >= top - ribbon::TOLERANCE && p.rect.bottom() <= bottom + ribbon::TOLERANCE),
        "{what}: {message:?} is painted beside its control, between {top} and {bottom}: {found:?}"
    );
}

/// The band of the panel that holds the picks: from its caption to the parameters.
fn pick_band(app: &App) -> (f32, f32) {
    (panel_rect(app, "Items to round").bottom(), panel_rect(app, "Fillet parameters").top())
}

/// The band below the Radius field, down to the panel's closing hint.
fn radius_band(app: &App) -> (f32, f32) {
    let field = app.get_by_label("Radius value").rect();
    let hint = panel_text(app)
        .into_iter()
        .find(|p| p.text.starts_with("A face adds"))
        .map_or(viewport(app).bottom(), |p| p.rect.top());
    (field.top(), hint)
}

/// The messages painted in a band of the panel, leaving out a field's own text.
fn band_text(app: &App, (top, bottom): (f32, f32), field: Option<Rect>) -> Vec<String> {
    panel_text(app)
        .into_iter()
        .filter(|p| p.rect.top() >= top - ribbon::TOLERANCE && p.rect.bottom() <= bottom + ribbon::TOLERANCE)
        .filter(|p| field.is_none_or(|f| !f.expand(ribbon::TOLERANCE).contains_rect(p.rect)))
        .map(|p| p.text)
        .collect()
}

/// What a band of the panel paints now that it did not before.
fn new_text(before: &[String], now: Vec<String>) -> Vec<String> {
    now.into_iter().filter(|t| !before.contains(t)).collect()
}

// ---------------------------------------------------------------------------
// The end profile of the rounded top edge, in the frame.

/// How a pixel reads against two references from the same frame.
fn is_body(image: &image::RgbaImage, at: Pos2, ppp: f32, body: [u8; 4], background: [u8; 4]) -> bool {
    let p = image.get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32).0;
    let d = |q: [u8; 4]| (0..3).map(|i| (p[i] as f32 - q[i] as f32).powi(2)).sum::<f32>();
    d(body) < d(background)
}

fn pixel(image: &image::RgbaImage, at: Pos2, ppp: f32) -> [u8; 4] {
    image.get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32).0
}

/// Classifies points of the end profile x = `end_profile_x` in a frame: true where the
/// body is drawn. The references are the end face's middle and a point clear of the box
/// beside the rounded corner.
fn classify(app: &App, image: &image::RgbaImage, yz: &[(f32, f32)]) -> Vec<bool> {
    let x = num("/expected/box_single_edge/end_profile_x") as f32;
    let hi = point("/fixtures/B20/bounds_max");
    let ppp = app.ctx.pixels_per_point();
    let body = pixel(image, project(app, Vec3::new(x, 0.0, hi.z * 0.5)), ppp);
    let background = pixel(image, project(app, Vec3::new(x, hi.y + 4.0, hi.z + 4.0)), ppp);
    yz.iter().map(|&(y, z)| is_body(image, project(app, Vec3::new(x, y, z)), ppp, body, background)).collect()
}

/// Millimetres per physical pixel at the rounded corner of the end profile.
fn mm_per_px(app: &App) -> f32 {
    let x = num("/expected/box_single_edge/end_profile_x") as f32;
    let hi = point("/fixtures/B20/bounds_max");
    let a = project(app, Vec3::new(x, hi.y, hi.z));
    let b = project(app, Vec3::new(x, hi.y - 1.0, hi.z));
    1.0 / (a.distance(b) * app.ctx.pixels_per_point())
}

/// The largest gap between the preview mesh's points on the end profile's arc, as the
/// chord height it leaves: the display's own tessellation error.
fn chord_error(mesh: &DisplayMesh, r: f32) -> f32 {
    let x = num("/expected/box_single_edge/end_profile_x") as f32;
    let hi = point("/fixtures/B20/bounds_max");
    let centre = (hi.y - r, hi.z - r);
    let mut angles: Vec<f32> = mesh
        .positions
        .iter()
        .filter(|p| (p.x - x).abs() < 1e-3)
        .filter(|p| {
            ((p.y - centre.0).hypot(p.z - centre.1) - r).abs() < 1e-3
                && p.y >= centre.0 - 1e-3
                && p.z >= centre.1 - 1e-3
        })
        .map(|p| (p.z - centre.1).atan2(p.y - centre.0))
        .collect();
    angles.sort_by(f32::total_cmp);
    angles.dedup_by(|a, b| (*a - *b).abs() < 1e-5);
    assert!(angles.len() >= 2, "the preview mesh has points on the R{r} end arc");
    let gap = angles.windows(2).map(|w| w[1] - w[0]).fold(0.0, f32::max);
    r * (1.0 - (gap / 2.0).cos())
}

/// The drawn end profile follows the R`r` arc: inside it is body, outside it is clear,
/// from one tangency point to the other, within the chord error and two pixels.
fn assert_arc(
    case: &Case,
    app: &App,
    image: &image::RgbaImage,
    mesh: &DisplayMesh,
    r: f32,
    what: &str,
) -> Vec<(f32, f32)> {
    let hi = point("/fixtures/B20/bounds_max");
    let centre = (hi.y - r, hi.z - r);
    let mm = mm_per_px(app);
    let chord = chord_error(mesh, r);
    let margin = chord + num("/tolerances/outline_physical_px") as f32 * mm;
    case.note(format!("{what}: {mm} mm/px, chord error {chord} mm, margin {margin} mm"));
    assert!(margin < r * 0.5, "{what}: the view resolves the R{r} arc ({margin} mm margin)");
    let mut inside = Vec::new();
    let mut outside = Vec::new();
    for i in 0..=9 {
        let t = (i as f32 * 10.0).to_radians();
        let (c, s) = (t.cos(), t.sin());
        inside.push((centre.0 + (r - margin) * c, centre.1 + (r - margin) * s));
        outside.push((centre.0 + (r + margin) * c, centre.1 + (r + margin) * s));
    }
    let ins = classify(app, image, &inside);
    let outs = classify(app, image, &outside);
    case.note(format!("{what}: inside {ins:?} outside {outs:?}"));
    assert!(ins.iter().all(|b| *b), "{what}: the body is drawn inside the R{r} arc: {ins:?}");
    assert!(outs.iter().all(|b| !*b), "{what}: nothing is drawn outside the R{r} arc: {outs:?}");
    outside
}

/// The designer's distinguishing point: inside the R2 section, outside R4.
fn distinguishing(app: &App, image: &image::RgbaImage) -> bool {
    let yz = at("/expected/box_single_edge/distinguishing_point_yz").as_array().unwrap();
    let p = (yz[0].as_f64().unwrap() as f32, yz[1].as_f64().unwrap() as f32);
    classify(app, image, &[p])[0]
}

/// The corner region of the end profile, cut out of a frame.
fn corner_crop(case: &Case, app: &App, image: &image::RgbaImage, name: &str) {
    let x = num("/expected/box_single_edge/end_profile_x") as f32;
    let hi = point("/fixtures/B20/bounds_max");
    let a = project(app, Vec3::new(x, hi.y - 8.0, hi.z - 8.0));
    let b = project(app, Vec3::new(x, hi.y + 3.0, hi.z + 3.0));
    case.crop(app, image, Rect::from_two_pos(a, b), name);
}

/// The panel's preview of the typed radius. The preview is made the frame after the
/// radius changes, so a few frames on there is either a rounded body or a refusal.
fn previewed(app: &mut App, r: f64) {
    until(app, &format!("the R{r} request"), |a| {
        a.state().state.editor.as_ref().is_some_and(|e| matches!(&e.params, EditorParams::Fillet(f) if f.radius == r))
    });
    app.run_steps(4);
    let editor = app.state().state.editor.as_ref().expect("the Fillet panel is open");
    assert!(editor.preview.result.is_some(), "R{r} previews a rounded body: {:?}", editor.error);
}

// ---------------------------------------------------------------------------
// F1.

#[test]
fn edge_radius_changes_the_visible_preview_before_commit() {
    let case = Case::start("edge_radius_changes_the_visible_preview_before_commit");
    let edge_at = point("/expected/box_single_edge/edge_midpoint");
    let original = num("/fixtures/B20/volume_mm3");
    let r2 = num("/expected/box_single_edge/volume_r2_mm3");
    let r4 = num("/expected/box_single_edge/volume_r4_mm3");

    let mut app = launch(&case, b20());
    case.run(&mut app, |app| {
        assert_eq!(app.state().state.doc.selection, None, "B20 starts with no selection");
        assert_b20(&case, app, "B20 at launch");
        let before = committed(app);
        case.note(format!("committed before: undo {} redo {}", before.undo, before.redo));

        // The plain end profile, seen along X, for the frames to differ from.
        view(&case, app, "Right");
        let right = app.state().state.viewport.camera.clone();
        let plain = case.frame(app, "b20-right");
        corner_crop(&case, app, &plain, "b20-right-corner");
        assert!(distinguishing(app, &plain), "the plain corner covers the distinguishing point");
        view(&case, app, "Isometric");

        // (1) Fillet, then the 60 mm top edge at its midpoint.
        click_fillet(&case, app);
        assert!(app.state().state.editor.is_some(), "Fillet opens its panel");
        pick(&case, app, "the 60 mm top edge", edge_at);
        let edges = held(app);
        case.note(format!("held {edges:?}"));
        assert_eq!(edges.len(), 1, "exactly the picked edge is held: {edges:?}");
        assert!(edges[0].midpoint.distance(edge_at) < 1e-3, "the held edge is the picked one: {edges:?}");
        assert!((edges[0].length - num("/expected/box_single_edge/edge_length_mm")).abs() < 1e-6, "{edges:?}");
        let name = edges[0].label.clone();

        type_radius(&case, app, "2");
        previewed(app, 2.0);
        assert_eq!(committed(app), before, "a preview commits nothing");
        let mesh = preview(app).expect("the R2 preview");
        assert_mesh_volume(&case, mesh_volume(&mesh), original, r2, "R2 preview");

        // (2) The drawn profile is the R2 arc, in place of the plain corner.
        view(&case, app, "Right");
        assert_eq!(app.state().state.viewport.camera.target, right.target, "the same camera as the plain frame");
        assert_eq!(app.state().state.viewport.camera.distance, right.distance, "the same camera as the plain frame");
        let frame2 = case.frame(app, "r2-preview-right");
        corner_crop(&case, app, &frame2, "r2-preview-right-corner");
        let outside = assert_arc(&case, app, &frame2, &mesh, 2.0, "R2 preview");
        // Off the tangency ends the clear band was body in the plain frame: the preview
        // replaced the corner rather than drawing over it.
        let replaced = classify(app, &plain, &outside[3..=6]);
        assert!(replaced.iter().all(|b| *b), "the plain frame drew body where R2 is clear: {replaced:?}");
        assert!(distinguishing(app, &frame2), "R2 still covers the distinguishing point");
        assert_eq!(committed(app), before, "a preview commits nothing");
        assert_b20(&case, app, "B20 under the R2 preview");

        // (3) R4 differs where the designer said it would.
        type_radius(&case, app, "4");
        previewed(app, 4.0);
        pointer_off_model(app);
        let mesh = preview(app).expect("the R4 preview");
        assert_mesh_volume(&case, mesh_volume(&mesh), original, r4, "R4 preview");
        let frame4 = case.frame(app, "r4-preview-right");
        corner_crop(&case, app, &frame4, "r4-preview-right-corner");
        assert_arc(&case, app, &frame4, &mesh, 4.0, "R4 preview");
        assert!(!distinguishing(app, &frame4), "R4 clears the distinguishing point");
        assert_eq!(committed(app), before, "a preview commits nothing");

        // 2 then 4 at once: the settled preview is 4's.
        case.note("type Radius \"2\" then \"4\" without waiting");
        ribbon::type_field(app, "Radius value", "2");
        ribbon::type_field(app, "Radius value", "4");
        previewed(app, 4.0);
        pointer_off_model(app);
        let last = preview(app).expect("the settled preview");
        assert_mesh_volume(&case, mesh_volume(&last), original, r4, "settled preview");
        let settled = case.frame(app, "r2-then-r4-preview-right");
        assert_arc(&case, app, &settled, &last, 4.0, "settled preview");
        assert!(!distinguishing(app, &settled), "the settled preview is R4");
        assert_eq!(committed(app), before, "a preview commits nothing");

        // (4) OK commits exactly that fillet.
        click_ok(&case, app);
        pointer_off_model(app);
        assert!(app.state().state.editor.is_none(), "OK closes the panel");
        let (refs, radius) = the_fillet(app);
        assert_eq!((refs, radius), (vec![name.clone()], 4.0), "the fillet holds the picked edge at R4");
        let v = volume(&app.state().state.doc);
        case.note(format!("committed R4: volume {v}"));
        assert!((v - r4).abs() <= num("/tolerances/box_volume_mm3"), "committed volume {v}, not {r4}");
        assert_bounds(&app.state().state.doc, "B20");
        assert_valid_drawn(app, "the committed fillet");
        let after = committed(app);
        assert_eq!(after.undo, before.undo + 1, "one undo step");
        let shown = shown_mesh_volume(app);
        assert_mesh_volume(&case, shown, original, r4, "committed display");
        assert_mesh_volume(&case, mesh_volume(&last), original, shown, "preview against the committed display");
        let done = case.frame(app, "r4-committed-right");
        corner_crop(&case, app, &done, "r4-committed-right-corner");
        assert_eq!(app.state().state.viewport.camera.target, right.target, "the camera of the preview frames");
        assert_arc(&case, app, &done, &last, 4.0, "committed R4");
        assert!(!distinguishing(app, &done), "the commit is drawn as the preview was");

        // (5) Undo and redo.
        press(&case, app, Modifiers::COMMAND, Key::Z);
        assert_b20(&case, app, "after undo");
        assert_eq!(app.state().state.doc.undo_depth(), before.undo, "undo returns to the launch depth");
        press(&case, app, Modifiers::COMMAND, Key::Y);
        let v = volume(&app.state().state.doc);
        case.note(format!("after redo: volume {v}"));
        assert!((v - r4).abs() <= num("/tolerances/box_volume_mm3"), "redo restores the R4 fillet: {v}");
        assert_eq!(the_fillet(app), (vec![name], 4.0));
    });

    // (5) A fresh harness: R2, then Escape before OK.
    let mut app = launch(&case, b20());
    case.run(&mut app, |app| {
        let before = committed(app);
        let triangles = app.state().state.doc.triangle_count();
        click_fillet(&case, app);
        pick(&case, app, "the 60 mm top edge", edge_at);
        type_radius(&case, app, "2");
        previewed(app, 2.0);
        press(&case, app, Modifiers::NONE, Key::Escape);
        assert!(app.state().state.editor.is_none(), "Escape closes the panel");
        app.run_steps(30);
        assert!(app.state().state.preview_body().is_none(), "no preview arrives after Escape");
        assert_eq!(committed(app), before, "Escape commits nothing");
        assert_b20(&case, app, "after Escape");
        assert_eq!(app.state().state.doc.triangle_count(), triangles, "the original display");
        view(&case, app, "Right");
        let image = case.frame(app, "escape-right");
        corner_crop(&case, app, &image, "escape-right-corner");
        assert!(distinguishing(app, &image), "the plain corner is drawn again");
        // Half a millimetre in from both sides of the plain corner: clear under R2.
        let hi = point("/fixtures/B20/bounds_max");
        assert!(classify(app, &image, &[(hi.y - 0.5, hi.z - 0.5)])[0], "no rounded corner remains in the frame");
    });
}

// ---------------------------------------------------------------------------
// F2.

#[test]
fn faces_collect_complete_boundaries_and_the_slot_stays_drawable() {
    let case = Case::start("faces_collect_complete_boundaries_and_the_slot_stays_drawable");
    let top = point("/expected/box_top_face/interior");
    let edge_at = point("/expected/box_single_edge/edge_midpoint");

    let assert_top_boundary = |app: &App, what: &str| {
        let edges = held(app);
        case.note(format!("{what}: held {edges:?}"));
        assert_eq!(edges.len(), count("/expected/box_top_face/boundary_edges"), "{what}: the top boundary: {edges:?}");
        let mut labels: Vec<_> = edges.iter().map(|e| e.label.clone()).collect();
        labels.sort();
        labels.dedup();
        assert_eq!(labels.len(), edges.len(), "{what}: each edge once: {edges:?}");
        let z = num("/expected/box_top_face/plane_z") as f32;
        assert!(edges.iter().all(|e| (e.midpoint.z - z).abs() < 1e-3), "{what}: only top edges: {edges:?}");
        let mut lengths: Vec<f64> = edges.iter().map(|e| e.length).collect();
        lengths.sort_by(f64::total_cmp);
        let want: Vec<f64> = at("/expected/box_top_face/edge_lengths_mm")
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        assert!(lengths.iter().zip(&want).all(|(a, b)| (a - b).abs() < 1e-6), "{what}: lengths {lengths:?}");
        let painted: Vec<String> = panel_text(app).into_iter().map(|p| p.text).collect();
        for e in &edges {
            assert!(!e.label.trim().is_empty(), "{what}: a usable name");
            assert!(painted.contains(&e.label), "{what}: the panel shows {:?}: {painted:?}", e.label);
        }
    };

    // (1) and (2) Command first: an edge, then the face it bounds.
    let mut app = launch(&case, b20());
    case.run(&mut app, |app| {
        let before = committed(app);
        click_fillet(&case, app);
        pick(&case, app, "a 60 mm top edge", edge_at);
        assert_eq!(held(app).len(), 1, "the edge is held");
        pick(&case, app, "the top face", top);
        assert!(app.state().state.editor.is_some(), "the panel stays open across the face pick");
        assert_top_boundary(app, "edge then face");
        type_radius(&case, app, "2");
        previewed(app, 2.0);
        assert_eq!(app.state().state.editor.as_ref().unwrap().error, None, "no refusal");
        pointer_off_model(app);
        case.frame(app, "box-face-r2-preview");
        assert_eq!(committed(app), before, "a preview commits nothing");
        press(&case, app, Modifiers::NONE, Key::Escape);
        assert!(app.state().state.editor.is_none(), "Escape closes the panel");
        assert_eq!(committed(app), before, "cancel commits nothing");
        assert_b20(&case, app, "after cancel");
    });

    // (2) Face first, then Fillet.
    let mut app = launch(&case, b20());
    case.run(&mut app, |app| {
        let before = committed(app);
        pick(&case, app, "the top face", top);
        assert!(matches!(app.state().state.doc.selection, Some(Selection::Face(_))), "the top face is selected");
        click_fillet(&case, app);
        assert_top_boundary(app, "face then Fillet");
        type_radius(&case, app, "2");
        previewed(app, 2.0);
        assert_eq!(committed(app), before, "a preview commits nothing");
        press(&case, app, Modifiers::NONE, Key::Escape);
        assert_eq!(committed(app), before, "cancel commits nothing");
    });

    // (3) The slot's whole top loop.
    let slot_top = point("/expected/slot_top_face/interior");
    let mut app = launch(&case, slot());
    case.run(&mut app, |app| {
        assert_s(&case, app, "S at launch");
        let before = committed(app);
        click_fillet(&case, app);
        pick(&case, app, "the slot's top face", slot_top);
        type_radius(&case, app, "2");
        previewed(app, 2.0);
        let edges = held(app);
        case.note(format!("slot loop: held {edges:?}"));
        assert_eq!(edges.len(), count("/expected/slot_top_face/boundary_edges"), "{edges:?}");
        let lines: Vec<_> = edges.iter().filter(|e| e.curve == CurveKind::Line).collect();
        let arcs: Vec<_> = edges.iter().filter(|e| e.curve != CurveKind::Line).collect();
        assert_eq!(lines.len(), count("/expected/slot_top_face/line_edges"), "{edges:?}");
        assert_eq!(arcs.len(), count("/expected/slot_top_face/arc_edges"), "{edges:?}");
        assert!(lines.iter().all(|e| (e.length - num("/expected/slot_top_face/line_length_mm")).abs() < 1e-6));
        // The semicircular ends reach the slot's ends at their middles.
        for end in at("/expected/slot_top_face/arc_midpoints").as_array().unwrap() {
            let c: Vec<f32> = end.as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as f32).collect();
            let want = Vec3::new(c[0], c[1], c[2]);
            assert!(arcs.iter().any(|e| e.midpoint.distance(want) < 1e-3), "an end arc through {want}: {edges:?}");
        }
        let editor = app.state().state.editor.as_ref().unwrap();
        assert_eq!(editor.error, None, "no refusal for the slot loop");
        pointer_off_model(app);
        case.frame(app, "slot-loop-r2-preview");
        assert_eq!(committed(app), before, "a preview commits nothing");
        assert_s(&case, app, "S under the preview");
        let last = preview(app).expect("the slot preview");

        click_ok(&case, app);
        let (refs, radius) = the_fillet(app);
        assert_eq!((refs.len(), radius), (4, 2.0));
        let v = volume(&app.state().state.doc);
        let want = num("/expected/slot_top_face/volume_r2_mm3");
        case.note(format!("slot loop R2 committed: volume {v}, expected {want}"));
        assert!((v - want).abs() <= num("/tolerances/slot_volume_mm3"), "slot loop R2 volume {v}, not {want}");
        assert_bounds(&app.state().state.doc, "S");
        assert_valid_drawn(app, "the slot loop fillet");
        assert_eq!(app.state().state.doc.undo_depth(), before.undo + 1, "one undo step");
        let original = num("/fixtures/S/volume_mm3");
        assert_mesh_volume(&case, mesh_volume(&last), original, want, "slot loop preview");
        assert_mesh_volume(&case, shown_mesh_volume(app), original, want, "slot loop display");
        pointer_off_model(app);
        case.frame(app, "slot-loop-r2-committed");
        let status = ribbon::status_lines(app);
        case.note(format!("status {status:?}"));
        assert!(status.iter().any(|l| l.contains("Rebuild OK")), "Rebuild OK is painted: {status:?}");
    });

    // (4) One straight top edge of the slot.
    let mut app = launch(&case, slot());
    case.run(&mut app, |app| {
        let before = committed(app);
        let original = volume(&app.state().state.doc);
        click_fillet(&case, app);
        let straight = point("/expected/slot_straight_edge/point");
        pick(&case, app, "a straight top edge of the slot", straight);
        let edges = held(app);
        assert_eq!(edges.len(), 1, "{edges:?}");
        assert!(edges[0].midpoint.distance(straight) < 1e-3 && edges[0].curve == CurveKind::Line, "{edges:?}");
        let r = num("/expected/slot_straight_edge/radius_mm");
        type_radius(&case, app, &soliddraft_app::document::fmt_mm(r));
        previewed(app, r);
        let last = preview(app).expect("the edge preview");
        let previewed_volume = mesh_volume(&last);
        assert!(previewed_volume > 0.0 && previewed_volume < original, "the preview removes material");
        pointer_off_model(app);
        case.frame(app, "slot-edge-r2-preview");
        assert_eq!(committed(app), before, "a preview commits nothing");

        click_ok(&case, app);
        the_fillet(app);
        let v = volume(&app.state().state.doc);
        case.note(format!("slot edge R2 committed: volume {v}, preview mesh {previewed_volume}"));
        assert!(v > 0.0 && v < original, "the fillet removes material: {v}");
        assert_valid_drawn(app, "the slot edge fillet");
        assert_mesh_volume(&case, previewed_volume, original, v, "slot edge preview against the commit");
        assert_mesh_volume(&case, shown_mesh_volume(app), original, v, "slot edge display");
        pointer_off_model(app);
        case.frame(app, "slot-edge-r2-committed");
    });
}

// ---------------------------------------------------------------------------
// F3.

#[test]
fn unsupported_picks_and_radii_refuse_locally_then_recover() {
    let case = Case::start("unsupported_picks_and_radii_refuse_locally_then_recover");
    let edge_at = point("/expected/box_single_edge/edge_midpoint");

    let mut app = launch(&case, b20());
    case.run(&mut app, |app| {
        let before = committed(app);
        // (1) A vertex: the nearest top corner, clear of the datums.
        let (lo, hi) = (point("/fixtures/B20/bounds_min"), point("/fixtures/B20/bounds_max"));
        let eye = app.state().state.viewport.camera.eye();
        let corner = [lo.x, hi.x]
            .into_iter()
            .flat_map(|x| [lo.y, hi.y].into_iter().map(move |y| Vec3::new(x, y, hi.z)))
            .min_by(|a, b| a.distance(eye).total_cmp(&b.distance(eye)))
            .unwrap();
        click_fillet(&case, app);
        let quiet = band_text(app, pick_band(app), None);
        pick(&case, app, "a top corner vertex", corner);
        {
            let editor = app.state().state.editor.as_ref().expect("the panel stays open");
            assert!(editor.edges().is_empty(), "a vertex adds nothing: {:?}", editor.edges());
            assert!(editor.preview.result.is_none(), "no preview without an edge");
        }
        let said = new_text(&quiet, band_text(app, pick_band(app), None));
        case.note(format!("vertex pick: the pick band now paints {said:?}"));
        let words = at("/expected/refusals/vertex_message_mentions").as_array().unwrap();
        let why = said
            .iter()
            .find(|t| words.iter().all(|w| t.contains(w.as_str().unwrap())))
            .unwrap_or_else(|| panic!("beside the picks, a refusal asks for an edge or a face: {said:?}"))
            .clone();
        let (top, bottom) = pick_band(app);
        assert_painted_between(&case, app, &why, top, bottom, "vertex refusal");
        case.frame(app, "vertex-refused");
        assert_eq!(committed(app), before, "a refused pick changes nothing");

        // The same open tool recovers with an edge.
        pick(&case, app, "the 60 mm top edge", edge_at);
        assert_eq!(held(app).len(), 1, "the edge is held after the refusal");
        {
            let editor = app.state().state.editor.as_ref().unwrap();
            assert!(editor.preview.result.is_some(), "the edge previews: {:?}", editor.error);
        }
        assert!(!panel_text(app).iter().any(|p| p.text == why), "the refusal is no longer painted");

        // (2) Radii that are not sizes.
        let accepted = fillet_radius(app);
        let field = app.get_by_label("Radius value").rect();
        let quiet = band_text(app, radius_band(app), Some(field));
        for typed in at("/expected/refusals/refused_radius_text").as_array().unwrap() {
            let typed = typed.as_str().unwrap();
            type_radius(&case, app, typed);
            app.run_steps(4);
            {
                let editor = app.state().state.editor.as_ref().expect("the panel stays open");
                assert!(editor.preview.result.is_none(), "{typed:?}: no preview stands for a refused radius");
                assert_eq!(fillet_radius(app), accepted, "{typed:?}: nothing is clamped into the parameters");
            }
            let shows = app.get_by_label("Radius value").accesskit_node().value();
            assert_eq!(shows.as_deref(), Some(typed), "the field keeps {typed:?}, not another radius");
            let said = new_text(&quiet, band_text(app, radius_band(app), Some(field)));
            case.note(format!("radius {typed:?}: beside the field {said:?}"));
            let refusal = said.first().unwrap_or_else(|| panic!("{typed:?} is refused beside the field")).clone();
            let (top, bottom) = radius_band(app);
            assert_painted_between(&case, app, &refusal, top, bottom, &format!("radius {typed:?}"));
            case.frame(app, &format!("radius-refused-{}", typed.replace('-', "minus")));

            click_ok(&case, app);
            assert!(app.state().state.editor.is_some(), "{typed:?}: OK leaves the panel open for a correction");
            assert_eq!(committed(app), before, "{typed:?}: OK commits nothing");
            pointer_off_model(app);
            press(&case, app, Modifiers::NONE, Key::Enter);
            assert!(app.state().state.editor.is_some(), "{typed:?}: Enter leaves the panel open");
            assert_eq!(committed(app), before, "{typed:?}: Enter commits nothing");
        }
        type_radius(&case, app, "2");
        previewed(app, 2.0);
        let said = new_text(&quiet, band_text(app, radius_band(app), Some(field)));
        assert!(said.is_empty(), "the correction is accepted, with nothing refused: {said:?}");
        press(&case, app, Modifiers::NONE, Key::Escape);
        assert_eq!(committed(app), before, "nothing committed");
        assert_b20(&case, app, "after the refusals");
    });

    // (3) The slot: R20 overflows the 5 mm half-width.
    let mut app = launch(&case, slot());
    case.run(&mut app, |app| {
        let before = committed(app);
        click_fillet(&case, app);
        pick(&case, app, "a straight top edge of the slot", point("/expected/slot_straight_edge/point"));
        assert_eq!(held(app).len(), 1, "the edge is held");
        type_radius(&case, app, text("/expected/refusals/slot_overflow_radius_text"));
        app.run_steps(4);
        let reason = text("/expected/refusals/slot_overflow_reason");
        let error = app.state().state.editor.as_ref().expect("the panel stays open").error.clone();
        case.note(format!("R20 on the slot: {error:?}"));
        let error = error.expect("R20 is refused");
        assert!(error.contains(reason), "the refusal is {reason}: {error}");
        assert!(preview(app).is_none(), "no preview for the overflow");
        let (top, bottom) = radius_band(app);
        assert_painted_between(&case, app, reason, top, bottom, "overflow refusal");
        case.frame(app, "slot-overflow-refused");
        assert_eq!(committed(app), before, "a refusal commits nothing");
        assert_s(&case, app, "S under the refusal");
        assert!(app.state().state.doc.triangle_count() > 0, "the slot stays drawn");

        type_radius(&case, app, "2");
        previewed(app, 2.0);
        press(&case, app, Modifiers::NONE, Key::Escape);
        assert_eq!(committed(app), before, "cancel commits nothing");
        assert_s(&case, app, "after cancel");
        pick(&case, app, "the slot's top face", point("/expected/slot_top_face/interior"));
        assert!(
            matches!(app.state().state.doc.selection, Some(Selection::Face(_))),
            "the slot is still pickable: {:?}",
            app.state().state.doc.selection
        );
    });
}
