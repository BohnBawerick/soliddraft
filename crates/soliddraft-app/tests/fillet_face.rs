//! Fillet and Chamfer accept a picked face, and a kernel refusal leaves the part usable.
//! Driven through egui_kittest against the real wgpu pipeline and the real Catallax
//! kernel, with the gestures a user makes: pick in the viewport, press the ribbon
//! command, press OK.
//!
//! The box is a body the kernel can round today, so it carries the positive cases and
//! the refuse-then-retry recovery. The extruded slot is the captain's body; Catallax
//! rounds one straight top edge of it but still refuses its tangent-terminated top
//! boundary, so it carries the refusal cases: the panel must show the kernel's typed
//! reason, the part must stay usable, and no red failed feature may block the next pick.
//!
//! Asserts a renderer exists: an acceptance run that skips is not acceptance.

use egui::{Key, Modifiers, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soliddraft_app::app::{Automation, SolidDraftApp};
use soliddraft_app::document::{
    Command, Document, EdgeRef, ExtrudeParams, FeatureKind, FeatureStatus, PlaneKind, ProfileKind, Selection,
    SketchPlane,
};
use soliddraft_app::state::{EditorParams, NoteKind};
use soliddraft_kernel_api::{EdgeId, FaceId, OpTag};
use soliddraft_sketch::P2;

type App = Harness<'static, SolidDraftApp>;

/// The rendered harnesses share one adapter; one test at a time.
static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu() -> std::sync::MutexGuard<'static, ()> {
    let guard = GPU.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let _ = env_logger::try_init();
    let instance = egui_wgpu::wgpu::Instance::default();
    let adapters = pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all()));
    assert!(!adapters.is_empty(), "this acceptance test requires a renderer");
    let config = std::env::temp_dir().join(format!("soliddraft-fillet-face-{}", std::process::id()));
    std::fs::create_dir_all(&config).unwrap();
    soliddraft_app::file::set_config_dir(Some(config));
    guard
}

// ---------------------------------------------------------------------------
// The two bodies.

/// A 60 x 40 x 20 plate standing on z = 0: six planar faces, twelve straight edges,
/// and a top boundary the kernel can round (proved by the edge-pick regression test).
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

/// A 50 x 10 slot, 10 deep: line and arc top edges meeting tangentially. Catallax
/// rounds one straight top edge, but refuses a fillet or chamfer of the whole boundary
/// until its stage-4 work lands, so this is the body whose refusal must not poison the
/// part.
fn slot_document() -> Document {
    let mut doc = Document::new("Slot", soliddraft_app::kernel());
    let sketch = doc
        .apply(Command::AddSketch { plane: SketchPlane::Reference { plane: PlaneKind::Top, offset: 0.0 } })
        .unwrap()
        .unwrap();
    let mut drawing = soliddraft_sketch::Sketch::new();
    drawing.slot(P2::new(0.0, 0.0), 50.0, 10.0).unwrap();
    doc.apply(Command::EditSketch { id: sketch, sketch: drawing }).unwrap();
    doc.apply(Command::AddExtrude {
        params: ExtrudeParams { sketch: Some(sketch), depth: 10.0, ..Default::default() },
        via_assistant: false,
    })
    .unwrap();
    doc
}

/// The midpoint of a straight top edge of the slot.
fn straight_top_edge(doc: &Document) -> glam::Vec3 {
    doc.edges
        .iter()
        .find(|e| (e.midpoint.z - 10.0).abs() < 1e-9 && (e.midpoint.y.abs() - 5.0).abs() < 1e-9)
        .map(|e| e.midpoint)
        .expect("a straight top edge of the slot")
}

// ---------------------------------------------------------------------------
// Driving the real app.

fn harness(doc: Document) -> App {
    let mut app = Harness::builder()
        .with_size(vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .with_render_every_step(true)
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_eframe(move |cc| SolidDraftApp::new(cc, doc, Automation::default()));
    app.run_steps(10);
    app.key_press(Key::Z);
    app.run_steps(30);
    app
}

fn button(app: &mut App, label: &str) {
    app.get_all_by_role_and_label(egui::accesskit::Role::Button, label)
        .next()
        .unwrap_or_else(|| panic!("missing button {label}"))
        .click();
    app.run_steps(4);
}

fn click(app: &mut App, point: Pos2) {
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
    app.run_steps(4);
}

/// Clicks a world point in the viewport, as a user picks a face or an edge.
fn click_world(app: &mut App, world: glam::Vec3) {
    let point = {
        let state = &app.state().state;
        state.viewport.camera.project(world, state.viewport.rect).expect("the point is on screen")
    };
    click(app, point);
}

/// Types a value into a panel's millimetre field; Enter ends the text edit, as a user
/// does before pressing OK.
fn type_field(app: &mut App, label: &str, value: &str) {
    app.get_by_label(label).click();
    app.run_steps(2);
    app.key_press_modifiers(Modifiers::COMMAND, Key::A);
    app.get_by_label(label).type_text(value);
    app.run_steps(3);
    app.key_press(Key::Enter);
    app.run_steps(4);
}

/// The panel's own OK button, labelled with its shortcut ("OK (Enter)").
fn ok(app: &mut App) {
    app.get_all_by_role_and_label(egui::accesskit::Role::Button, "OK (Enter)")
        .next()
        .expect("the open panel has an OK button")
        .click();
    app.run_steps(30);
}

fn escape(app: &mut App) {
    app.key_press(Key::Escape);
    app.run_steps(4);
}

// ---------------------------------------------------------------------------
// Reading the contracts the tests assert against.

fn volume(doc: &Document) -> f64 {
    let body = doc.display_body().expect("a body to measure");
    doc.kernel().mass_properties(body).expect("mass properties of the body").volume
}

/// Volume, face count and edge count of the shown body: the shape a refusal must not
/// change.
fn shape(doc: &Document) -> (f64, usize, usize) {
    let body = doc.display_body().expect("a body to measure");
    let topology = doc.kernel().topology(body).expect("topology of the body");
    let volume = doc.kernel().mass_properties(body).expect("mass properties of the body").volume;
    (volume, topology.faces.len(), topology.edges.len())
}

/// The edge ids on a face's boundary, from the kernel's own topology: the set a face
/// pick must expand into.
fn face_boundary(doc: &Document, face: FaceId) -> Vec<EdgeId> {
    let body = doc.display_body().expect("a body");
    let topology = doc.kernel().topology(body).expect("topology of the body");
    let mut edges =
        topology.faces.iter().find(|f| f.id == face).expect("the picked face is in the body's topology").edges.clone();
    edges.sort();
    edges
}

/// The panel's edge references resolved on the shown body, in panel order: the same
/// resolution the preview and the commit make.
fn panel_edges(doc: &Document, refs: &[EdgeRef]) -> Vec<EdgeId> {
    let body = doc.display_body().expect("a body");
    refs.iter()
        .map(|r| doc.model().resolve_edge(r, body).unwrap_or_else(|repair| panic!("{}: {}", r.label, repair.message)))
        .collect()
}

/// The kernel's typed reason, such as `fillet.unsupported-contact`, from a refusal
/// text: the app must show the kernel's own classification, not a generic sentence.
fn typed_reason(text: &str) -> String {
    let start = ["fillet.", "chamfer."]
        .iter()
        .filter_map(|namespace| text.find(namespace))
        .min()
        .unwrap_or_else(|| panic!("no typed `fillet.` / `chamfer.` reason in {text:?}"));
    let end = text[start..].find(':').map_or(text.len(), |i| start + i);
    text[start..end].to_owned()
}

/// What the kernel itself says about this edge set and size. Nothing about the message
/// is pinned by hand: the app has to show the reason the kernel gave for the very same
/// edges, radius (or distance) and body.
fn kernel_refusal(doc: &Document, edges: &[EdgeId], size: f64, chamfer: bool) -> String {
    let body = doc.display_body().expect("a body");
    let tag = OpTag::new("contract");
    let refused = if chamfer {
        doc.kernel().chamfer(&tag, body, edges, size).expect_err("the kernel refuses this chamfer")
    } else {
        doc.kernel().fillet(&tag, body, edges, size).expect_err("the kernel refuses this fillet")
    };
    refused.to_string()
}

/// The one built fillet (or chamfer) the positive case expects. A refused earlier
/// attempt may still sit in the tree suppressed, so this names the active one.
fn the_rounding(doc: &Document, chamfer: bool) -> &soliddraft_app::document::Feature {
    let live: Vec<_> = doc
        .features
        .iter()
        .filter(|f| match &f.kind {
            FeatureKind::Chamfer(_) => chamfer,
            FeatureKind::Fillet(_) => !chamfer,
            _ => false,
        })
        .filter(|f| !f.suppressed && f.status != FeatureStatus::Inactive)
        .collect();
    assert_eq!(
        live.len(),
        1,
        "one built {}: {:?}",
        if chamfer { "chamfer" } else { "fillet" },
        doc.features.iter().map(|f| format!("{} {:?}", f.name, f.status)).collect::<Vec<_>>()
    );
    assert_eq!(live[0].status, FeatureStatus::Ok, "{}", live[0].name);
    live[0]
}

/// The refusal left no blocking failure: either the command was rolled back (no feature
/// at all) or the refused feature is suppressed and marked inactive. Both keep
/// `edge_ref`/`face_ref` live, which is what the next pick needs. A red `Error` or
/// `NeedsRepair` feature does neither, and must never be the outcome.
fn assert_no_blocking_failure(doc: &Document, what: &str) {
    let red: Vec<_> = doc
        .features
        .iter()
        .filter(|f| matches!(f.status, FeatureStatus::Error(_) | FeatureStatus::NeedsRepair(_)))
        .map(|f| format!("{} {:?}", f.name, f.status))
        .collect();
    assert!(red.is_empty(), "{what} left a failed feature that blocks every later reference: {red:?}");
    let live: Vec<_> = doc
        .features
        .iter()
        .filter(|f| matches!(f.kind, FeatureKind::Fillet(_) | FeatureKind::Chamfer(_)))
        .filter(|f| !f.suppressed && f.status != FeatureStatus::Inactive)
        .map(|f| format!("{} {:?}", f.name, f.status))
        .collect();
    assert!(live.is_empty(), "{what} left a refused feature active and unmarked: {live:?}");
    assert_eq!(doc.error_count(), 0, "{what} left the part with errors");
}

/// A refused pick must not take the part's names away: the next face pick resolves, has
/// a persistent name, and its own panel fills with the face's boundary edges.
fn assert_picks_still_work(app: &mut App, pick: glam::Vec3, expected_edges: usize, command: &str) {
    {
        let state = &app.state().state;
        assert!(state.editor.is_none(), "the refusal left its panel open");
    }
    click_world(app, pick);
    {
        let state = &app.state().state;
        assert!(
            matches!(state.doc.selection, Some(Selection::Face(_))),
            "the face after a refusal is still picked, not {:?}",
            state.doc.selection
        );
        let Some(Selection::Face(face)) = state.doc.selection else { unreachable!() };
        assert!(state.doc.face_ref(face).is_some(), "a face after a refusal still has a persistent name");
    }
    button(app, command);
    let state = &app.state().state;
    let editor = state.editor.as_ref().expect("the next pick opens a panel");
    assert_eq!(
        editor.edges().len(),
        expected_edges,
        "the next panel shows the picked face's boundary edges, not an empty list: {:?}",
        editor.error
    );
    assert_ne!(editor.error.as_deref(), Some("select at least one edge"), "the part's names went dead");
}

// ---------------------------------------------------------------------------
// The geometry the positive cases assert. Derived from the shape, not from a recording.

/// A w x h x d box whose four top edges are rounded at r: the straight runs lose
/// `r^2 (1 - pi/4)` each, and each of the four corners loses a `r^3 / 3` patch.
fn rounded_box_volume(w: f64, h: f64, d: f64, r: f64) -> f64 {
    let straight = 2.0 * (w - 2.0 * r) + 2.0 * (h - 2.0 * r);
    w * h * d - straight * r * r * (1.0 - std::f64::consts::FRAC_PI_4) - 4.0 * r * r * r / 3.0
}

/// The same box with its four top edges bevelled at distance s: the top s of the body is
/// a stack of rectangles (w - 2z) x (h - 2z), so the cut-off volume is
/// `(w + h) s^2 - 4 s^3 / 3`.
fn bevelled_box_volume(w: f64, h: f64, d: f64, s: f64) -> f64 {
    w * h * d - ((w + h) * s * s - 4.0 * s * s * s / 3.0)
}

/// The size the open panel will apply: the radius of a fillet, the distance of a
/// chamfer. Read from the panel so the tests do not pin the app's defaults.
fn rounding_size(editor: &soliddraft_app::state::FeatureEditor) -> f64 {
    match &editor.params {
        EditorParams::Fillet(p) => p.radius,
        EditorParams::Chamfer(c) => c.distance,
        _ => panic!("not a fillet or chamfer panel"),
    }
}

fn close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() <= 1e-6 * expected.abs()
}

/// The shown body is the same shape: same volume (to the kernel's precision), same
/// face and edge counts. Body ids may differ when an implementation rebuilds, so only
/// the geometry is compared.
fn same_shape(before: (f64, usize, usize), after: (f64, usize, usize)) -> bool {
    close(after.0, before.0) && after.1 == before.1 && after.2 == before.2
}

// ---------------------------------------------------------------------------
// (1) A face pick is a fillet or chamfer input.

#[test]
fn fillet_a_picked_face_rounds_all_its_edges() {
    let _gpu = gpu();
    let mut app = harness(box_document());
    click_world(&mut app, glam::Vec3::new(0.0, 0.0, 20.0));
    let state = &app.state().state;
    let Some(Selection::Face(face)) = state.doc.selection else {
        panic!("the top face is picked, not {:?}", state.doc.selection);
    };
    let expected = face_boundary(&state.doc, face);
    assert_eq!(expected.len(), 4, "a box top face has four boundary edges");
    let before = volume(&state.doc);

    button(&mut app, "Fillet");
    let editor = app.state().state.editor.as_ref().expect("Fillet opens on a face pick");
    assert!(matches!(&editor.params, EditorParams::Fillet(_)), "the Fillet panel opened");
    let radius = rounding_size(editor);
    let mut shown = panel_edges(&app.state().state.doc, editor.edges());
    shown.sort();
    assert_eq!(shown, expected, "the face pick expands into exactly the face's boundary edges");
    assert!(editor.error.is_none(), "the panel previews: {:?}", editor.error);
    assert!(editor.preview.result.is_some(), "the preview shows the rounded body");

    ok(&mut app);
    let state = &app.state().state;
    let note = state.note.as_ref().expect("committing leaves a note");
    assert_eq!(note.kind, NoteKind::Ok, "{}", note.text);
    let fillet = the_rounding(&state.doc, false);
    assert_eq!(state.doc.display_body(), fillet.body, "the rounded body is shown");
    let expected = rounded_box_volume(60.0, 40.0, 20.0, radius);
    assert!(close(volume(&state.doc), expected), "{} not {expected}", volume(&state.doc));
    assert!(volume(&state.doc) < before, "a fillet removes material");
}

#[test]
fn chamfer_a_picked_face_bevels_all_its_edges() {
    let _gpu = gpu();
    let mut app = harness(box_document());
    click_world(&mut app, glam::Vec3::new(0.0, 0.0, 20.0));
    let state = &app.state().state;
    let Some(Selection::Face(face)) = state.doc.selection else {
        panic!("the top face is picked, not {:?}", state.doc.selection);
    };
    let expected = face_boundary(&state.doc, face);
    let before = volume(&state.doc);

    button(&mut app, "Chamfer");
    let editor = app.state().state.editor.as_ref().expect("Chamfer opens on a face pick");
    assert!(matches!(&editor.params, EditorParams::Chamfer(_)), "the Chamfer panel opened");
    let distance = rounding_size(editor);
    let mut shown = panel_edges(&app.state().state.doc, editor.edges());
    shown.sort();
    assert_eq!(shown, expected, "the face pick expands into exactly the face's boundary edges");
    assert!(editor.error.is_none(), "the panel previews: {:?}", editor.error);
    assert!(editor.preview.result.is_some(), "the preview shows the bevelled body");

    ok(&mut app);
    let state = &app.state().state;
    let note = state.note.as_ref().expect("committing leaves a note");
    assert_eq!(note.kind, NoteKind::Ok, "{}", note.text);
    let chamfer = the_rounding(&state.doc, true);
    assert_eq!(state.doc.display_body(), chamfer.body, "the bevelled body is shown");
    let expected = bevelled_box_volume(60.0, 40.0, 20.0, distance);
    assert!(close(volume(&state.doc), expected), "{} not {expected}", volume(&state.doc));
    assert!(volume(&state.doc) < before, "a chamfer removes material");
}

// ---------------------------------------------------------------------------
// (1, the other half) Edge picks keep working exactly as they do today.

#[test]
fn fillet_edge_picks_still_round_the_box_top() {
    let _gpu = gpu();
    let mut app = harness(box_document());
    let top: Vec<_> = {
        let doc = &app.state().state.doc;
        doc.edges.iter().filter(|e| (e.midpoint.z - 20.0).abs() < 1e-9).map(|e| e.midpoint).collect()
    };
    assert_eq!(top.len(), 4, "four edges on the box top");
    for (i, midpoint) in top.iter().enumerate() {
        click_world(&mut app, *midpoint);
        let state = &app.state().state;
        if i == 0 {
            assert!(matches!(state.doc.selection, Some(Selection::Edge(_))), "an edge is picked");
            button(&mut app, "Fillet");
        } else {
            assert!(state.editor.is_some(), "the open panel stays up between picks");
        }
    }
    let state = &app.state().state;
    let editor = state.editor.as_ref().expect("Fillet opens on the first edge pick");
    assert_eq!(editor.edges().len(), 4, "four edge picks fill one panel: {:?}", editor.error);
    assert!(editor.error.is_none(), "the panel previews: {:?}", editor.error);
    assert!(editor.preview.result.is_some(), "the preview shows the rounded body");
    let radius = rounding_size(editor);

    ok(&mut app);
    let state = &app.state().state;
    let note = state.note.as_ref().expect("committing leaves a note");
    assert_eq!(note.kind, NoteKind::Ok, "{}", note.text);
    the_rounding(&state.doc, false);
    let expected = rounded_box_volume(60.0, 40.0, 20.0, radius);
    assert!(close(volume(&state.doc), expected), "{} not {expected}", volume(&state.doc));
}

// ---------------------------------------------------------------------------
// (2) and (3) A kernel refusal: the typed reason, and a part that stays usable.

#[test]
fn fillet_face_refusal_on_the_slot_names_the_kernel_reason_and_leaves_the_part_usable() {
    let _gpu = gpu();
    let mut app = harness(slot_document());
    let before = shape(&app.state().state.doc);
    click_world(&mut app, glam::Vec3::new(0.0, 0.0, 10.0));
    let state = &app.state().state;
    let Some(Selection::Face(face)) = state.doc.selection else {
        panic!("the slot's top face is picked, not {:?}", state.doc.selection);
    };
    let expected = face_boundary(&state.doc, face);
    assert_eq!(expected.len(), 4, "the slot's top face has four boundary edges");

    button(&mut app, "Fillet");
    let expected_reason = {
        let state = &app.state().state;
        let editor = state.editor.as_ref().expect("Fillet opens on the slot's face pick");
        assert_eq!(editor.edges().len(), 4, "the face still expands on the body the kernel refuses");
        // The reason is the kernel's own, for the very edges and radius the panel holds.
        let shown = panel_edges(&state.doc, editor.edges());
        let kernel_says = kernel_refusal(&state.doc, &shown, rounding_size(editor), false);
        let expected_reason = typed_reason(&kernel_says);
        let panel_says = editor.error.as_deref().expect("the panel shows the refusal");
        assert_eq!(typed_reason(panel_says), expected_reason, "the panel shows the kernel's reason: {panel_says}");
        expected_reason
    };

    ok(&mut app);
    {
        let state = &app.state().state;
        let note = state.note.as_ref().expect("committing leaves a note");
        assert_eq!(note.kind, NoteKind::Error, "{}", note.text);
        assert_eq!(typed_reason(&note.text), expected_reason, "the note names the kernel's reason: {}", note.text);
        assert!(note.text.contains("fillet"), "the note names what was refused: {}", note.text);
    }
    let after = shape(&app.state().state.doc);
    assert!(same_shape(before, after), "a refusal does not change the shown shape: {before:?} -> {after:?}");
    assert_no_blocking_failure(&app.state().state.doc, "the refused fillet");

    // (2) no red feature gates the names, and (3) the next pick says why again.
    assert_picks_still_work(&mut app, glam::Vec3::new(0.0, 0.0, 10.0), 4, "Fillet");
    {
        let state = &app.state().state;
        let editor = state.editor.as_ref().expect("the second face pick opens a panel");
        let panel_says = editor.error.as_deref().expect("the second panel shows the refusal too");
        assert_eq!(typed_reason(panel_says), expected_reason, "the same refusal, named");
    }
    escape(&mut app);

    // An edge pick after the refusal still resolves to a named edge and joins a panel.
    let edge_midpoint = straight_top_edge(&app.state().state.doc);
    click_world(&mut app, edge_midpoint);
    {
        let state = &app.state().state;
        assert!(matches!(state.doc.selection, Some(Selection::Edge(_))), "an edge is picked after the refusal");
    }
    button(&mut app, "Fillet");
    let state = &app.state().state;
    let editor = state.editor.as_ref().expect("Fillet opens on an edge pick");
    assert_eq!(editor.edges().len(), 1, "the picked edge joins the panel: {:?}", editor.error);
}

#[test]
fn fillet_edge_refusal_on_the_slot_names_the_kernel_reason_and_leaves_the_part_usable() {
    let _gpu = gpu();
    let mut app = harness(slot_document());
    let before = shape(&app.state().state.doc);
    let midpoint = straight_top_edge(&app.state().state.doc);
    click_world(&mut app, midpoint);

    // F is the Fillet shortcut with an edge selected.
    app.key_press(Key::F);
    app.run_steps(4);
    // r = 20 on a 10 mm wide, 10 mm deep slot: the round cannot fit.
    type_field(&mut app, "Radius value", "20");
    let expected_reason = {
        let state = &app.state().state;
        let editor = state.editor.as_ref().expect("F opens Fillet on an edge pick");
        assert_eq!(editor.edges().len(), 1);
        let shown = panel_edges(&state.doc, editor.edges());
        let expected_reason = typed_reason(&kernel_refusal(&state.doc, &shown, rounding_size(editor), false));
        let panel_says = editor.error.as_deref().expect("the panel shows the refusal");
        assert_eq!(typed_reason(panel_says), expected_reason, "the panel shows the kernel's reason: {panel_says}");
        expected_reason
    };

    ok(&mut app);
    {
        let state = &app.state().state;
        let note = state.note.as_ref().expect("committing leaves a note");
        assert_eq!(note.kind, NoteKind::Error, "{}", note.text);
        assert_eq!(typed_reason(&note.text), expected_reason, "the note names the kernel's reason: {}", note.text);
    }
    let after = shape(&app.state().state.doc);
    assert!(same_shape(before, after), "a refusal does not change the shown shape: {before:?} -> {after:?}");
    assert_no_blocking_failure(&app.state().state.doc, "the refused fillet");

    assert_picks_still_work(&mut app, glam::Vec3::new(0.0, 0.0, 10.0), 4, "Fillet");
}

#[test]
fn chamfer_face_refusal_on_the_slot_names_the_kernel_reason_and_leaves_the_part_usable() {
    let _gpu = gpu();
    let mut app = harness(slot_document());
    let before = shape(&app.state().state.doc);
    click_world(&mut app, glam::Vec3::new(0.0, 0.0, 10.0));
    let expected = {
        let state = &app.state().state;
        let Some(Selection::Face(face)) = state.doc.selection else {
            panic!("the slot's top face is picked, not {:?}", state.doc.selection);
        };
        face_boundary(&state.doc, face)
    };

    button(&mut app, "Chamfer");
    let expected_reason = {
        let state = &app.state().state;
        let editor = state.editor.as_ref().expect("Chamfer opens on the slot's face pick");
        let mut shown = panel_edges(&state.doc, editor.edges());
        shown.sort();
        assert_eq!(shown, expected, "the face pick expands into the face's boundary edges");
        let expected_reason = typed_reason(&kernel_refusal(&state.doc, &shown, rounding_size(editor), true));
        let panel_says = editor.error.as_deref().expect("the panel shows the refusal");
        assert_eq!(typed_reason(panel_says), expected_reason, "the panel shows the kernel's reason: {panel_says}");
        expected_reason
    };

    ok(&mut app);
    {
        let state = &app.state().state;
        let note = state.note.as_ref().expect("committing leaves a note");
        assert_eq!(note.kind, NoteKind::Error, "{}", note.text);
        assert_eq!(typed_reason(&note.text), expected_reason, "the note names the kernel's reason: {}", note.text);
        assert!(note.text.contains("chamfer"), "the note names what was refused: {}", note.text);
    }
    let after = shape(&app.state().state.doc);
    assert!(same_shape(before, after), "a refusal does not change the shown shape: {before:?} -> {after:?}");
    assert_no_blocking_failure(&app.state().state.doc, "the refused chamfer");
    assert_picks_still_work(&mut app, glam::Vec3::new(0.0, 0.0, 10.0), 4, "Fillet");
}

// ---------------------------------------------------------------------------
// Recovery end to end: after a refusal, the same gesture with a size the kernel
// accepts must build.

#[test]
fn fillet_refusal_then_retry_rounds_the_box_top() {
    let _gpu = gpu();
    let mut app = harness(box_document());
    let before = volume(&app.state().state.doc);
    click_world(&mut app, glam::Vec3::new(0.0, 0.0, 20.0));
    button(&mut app, "Fillet");
    {
        let state = &app.state().state;
        let editor = state.editor.as_ref().expect("Fillet opens on a face pick");
        assert_eq!(editor.edges().len(), 4, "the face pick expands into its boundary edges: {:?}", editor.error);
    }

    // r = 10 on a 20 mm plate: the round cannot fit, and the kernel says why.
    type_field(&mut app, "Radius value", "10");
    let expected_reason = {
        let state = &app.state().state;
        let editor = state.editor.as_ref().expect("the panel stays open while typing");
        let shown = panel_edges(&state.doc, editor.edges());
        let expected_reason = typed_reason(&kernel_refusal(&state.doc, &shown, rounding_size(editor), false));
        let panel_says = editor.error.as_deref().expect("the panel shows the refusal");
        assert_eq!(typed_reason(panel_says), expected_reason, "the panel shows the kernel's reason: {panel_says}");
        expected_reason
    };
    ok(&mut app);
    {
        let state = &app.state().state;
        let note = state.note.as_ref().expect("committing leaves a note");
        assert_eq!(note.kind, NoteKind::Error, "{}", note.text);
        assert_eq!(typed_reason(&note.text), expected_reason, "the note names the kernel's reason: {}", note.text);
    }
    assert!(close(volume(&app.state().state.doc), before), "a refusal does not change the volume");
    assert_no_blocking_failure(&app.state().state.doc, "the refused fillet");

    // The same gesture again, at a radius that fits, must build with no cleanup first.
    click_world(&mut app, glam::Vec3::new(0.0, 0.0, 20.0));
    button(&mut app, "Fillet");
    let radius = {
        let state = &app.state().state;
        let editor = state.editor.as_ref().expect("the retry opens a panel");
        assert_eq!(editor.edges().len(), 4, "the retry collects the face's boundary edges: {:?}", editor.error);
        rounding_size(editor)
    };
    assert!(radius < 10.0, "the retry uses a smaller radius than the refused one: {radius}");
    ok(&mut app);
    let state = &app.state().state;
    let note = state.note.as_ref().expect("committing leaves a note");
    assert_eq!(note.kind, NoteKind::Ok, "{}", note.text);
    the_rounding(&state.doc, false);
    let expected = rounded_box_volume(60.0, 40.0, 20.0, radius);
    assert!(close(volume(&state.doc), expected), "{} not {expected}", volume(&state.doc));
}
