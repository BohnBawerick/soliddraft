//! The phase D2 exit test, driven through egui_kittest against the real kernel and
//! the real wgpu pipeline: make an A3 drawing of the bracket with front, top, left and
//! isometric views; place five smart dimensions, a hole callout and a note; export PDF
//! and DXF; change the plate thickness in the part tab and watch the drawing update
//! with the 10 becoming 12; delete the slot and see two dangling dimensions listed;
//! reattach one to a hole edge and delete the other. Frames go to `target/exit-test/`.
//!
//! Skips itself when no GPU adapter is available (plain CI runners).

use egui::{Key, Modifiers, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soliddraft_app::app::{Automation, SolidDraftApp};
use soliddraft_app::document::{Command, CutParams, Document, ExtrudeParams, PlaneKind, SketchPlane};
use soliddraft_app::drawing::{DrawingAction, DrawingSel, DrawingTool};
use soliddraft_app::sketch::{EntityKind, P2, Sketch};
use soliddraft_app::state::AppAction;
use soliddraft_drawing::export::dxf::count_entities;
use soliddraft_drawing::{AnnotationKind, AnnotationState, DimValue, DimensionKind, LinearOrientation, ViewId};
use soliddraft_kernel_api::Vec3;
use std::path::PathBuf;

fn gpu_available() -> bool {
    let _ = env_logger::try_init();
    let instance = egui_wgpu::wgpu::Instance::default();
    !pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all())).is_empty()
}

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/exit-test/drawing");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

type App<'a> = Harness<'a, SolidDraftApp>;

fn harness(doc: Document) -> App<'static> {
    Harness::builder()
        .with_size(vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .with_render_every_step(true)
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_eframe(|cc| SolidDraftApp::new(cc, doc, Automation::default()))
}

fn save(harness: &mut App<'_>, name: &str) {
    let image = harness.render().expect("render");
    let path = out_dir().join(format!("{name}.png"));
    image.save(&path).expect("save png");
    eprintln!("wrote {}", path.display());
}

fn settle(harness: &mut App<'_>) {
    harness.run_steps(24);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while harness.state().state.doc.proof_pending() {
        assert!(std::time::Instant::now() < deadline, "the part proof did not settle");
        std::thread::sleep(std::time::Duration::from_millis(10));
        // Draw only the frames the app asks for, as a window's event loop would.
        if harness.ctx.has_requested_repaint() {
            harness.run_steps(1);
        }
    }
    harness.run_steps(1);
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
    harness.run_steps(4);
}

/// Clicks the first button with this label; ribbon items come before panel buttons.
fn click_button(harness: &mut App<'_>, label: &str) {
    harness
        .get_all_by_role_and_label(egui::accesskit::Role::Button, label)
        .next()
        .unwrap_or_else(|| panic!("no button labelled {label}"))
        .click();
    harness.run_steps(4);
}

/// Clicks a feature row in the tree. Ribbon buttons come first in the tree of nodes and
/// a feature may share its name with one ("Slot"), so the row is the last match.
fn click_row(harness: &mut App<'_>, name: &str) {
    harness.get_all_by_role_and_label(egui::accesskit::Role::Button, name).last().expect("a tree row").scroll_to_me();
    harness.run_steps(3);
    harness.get_all_by_role_and_label(egui::accesskit::Role::Button, name).last().expect("a tree row").click();
    harness.run_steps(3);
}

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

/// The GPU harnesses share one adapter; one test at a time.
static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu_lock() -> std::sync::MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

fn rectangle(a: P2, b: P2) -> Sketch {
    let mut s = Sketch::new();
    let corners = [a, P2::new(b.u, a.v), b, P2::new(a.u, b.v)];
    for i in 0..4 {
        s.add(EntityKind::Line { a: corners[i], b: corners[(i + 1) % 4] }, false);
    }
    s
}

fn circles(list: &[(P2, f64)]) -> Sketch {
    let mut s = Sketch::new();
    for (center, radius) in list {
        s.add(EntityKind::Circle { center: *center, radius: *radius }, false);
    }
    s
}

/// The sample bracket, built through the document's commands: a 120 x 90 x 10
/// plate, a 10 x 90 x 60 flange on its -X end, a 24 mm boss with a 12 mm bore, four
/// 9 mm holes on a 90 x 44 pattern and a 40 x 12 slot through the flange.
fn bracket_document() -> Document {
    let mut doc = soliddraft_app::document::Document::new(
        "Part1",
        std::sync::Arc::new(soliddraft_kernel_catallax::CatallaxKernel::new()),
    );
    doc.name = "Bracket".into();
    let sketch_on = |doc: &mut Document, plane: PlaneKind, sketch: Sketch| -> u64 {
        let id =
            doc.apply(Command::AddSketch { plane: SketchPlane::Reference { plane, offset: 0.0 } }).unwrap().unwrap();
        doc.apply(Command::EditSketch { id, sketch }).unwrap();
        id
    };
    let plate = sketch_on(&mut doc, PlaneKind::Top, rectangle(P2::ZERO, P2::new(120.0, 90.0)));
    let plate = doc
        .apply(Command::AddExtrude {
            params: ExtrudeParams { sketch: Some(plate), depth: 10.0, ..Default::default() },
            via_assistant: false,
        })
        .unwrap()
        .unwrap();
    let flange = sketch_on(&mut doc, PlaneKind::Top, rectangle(P2::ZERO, P2::new(10.0, 90.0)));
    let flange = doc
        .apply(Command::AddExtrude {
            params: ExtrudeParams { sketch: Some(flange), depth: 60.0, ..Default::default() },
            via_assistant: false,
        })
        .unwrap()
        .unwrap();
    let boss = sketch_on(&mut doc, PlaneKind::Top, circles(&[(P2::new(70.0, 45.0), 12.0)]));
    let boss = doc
        .apply(Command::AddExtrude {
            params: ExtrudeParams { sketch: Some(boss), depth: 30.0, ..Default::default() },
            via_assistant: false,
        })
        .unwrap()
        .unwrap();
    let bore = sketch_on(&mut doc, PlaneKind::Top, circles(&[(P2::new(70.0, 45.0), 6.0)]));
    let bore = doc
        .apply(Command::AddCut {
            params: CutParams { sketch: bore, depth: 40.0, through_all: true, flip: true, up_to: None },
        })
        .unwrap()
        .unwrap();
    let holes = sketch_on(
        &mut doc,
        PlaneKind::Top,
        circles(&[
            (P2::new(25.0, 23.0), 4.5),
            (P2::new(25.0, 67.0), 4.5),
            (P2::new(115.0, 23.0), 4.5),
            (P2::new(115.0, 67.0), 4.5),
        ]),
    );
    let holes = doc
        .apply(Command::AddCut {
            params: CutParams { sketch: holes, depth: 12.0, through_all: true, flip: true, up_to: None },
        })
        .unwrap()
        .unwrap();
    // The slot: on the Right plane (u along Y, v along Z), cut 12 mm along +X.
    let slot = sketch_on(&mut doc, PlaneKind::Right, rectangle(P2::new(25.0, 25.0), P2::new(65.0, 37.0)));
    let slot = doc
        .apply(Command::AddCut {
            params: CutParams { sketch: slot, depth: 12.0, through_all: false, flip: true, up_to: None },
        })
        .unwrap()
        .unwrap();
    for (id, name) in
        [(plate, "Plate"), (flange, "Flange"), (boss, "Boss"), (bore, "Bore"), (holes, "Holes"), (slot, "Slot")]
    {
        doc.apply(Command::Rename { id, name: name.into() }).unwrap();
    }
    for f in &doc.features {
        assert_eq!(f.status, soliddraft_app::document::FeatureStatus::Ok, "{}: {:?}", f.name, f.status);
    }
    doc.selection = None;
    doc.dirty = false;
    doc
}

fn session<'a>(harness: &'a App<'_>) -> &'a soliddraft_app::drawing::DrawingSession {
    harness.state().state.drawing.session.as_ref().expect("a drawing is open")
}

/// The view with this label on the first sheet.
fn view_named(harness: &App<'_>, name: &str) -> ViewId {
    let s = session(harness);
    let sheet = s.sheet();
    sheet
        .views
        .iter()
        .find(|v| soliddraft_drawing::project::view_label(sheet, v) == name)
        .unwrap_or_else(|| panic!("no view {name}"))
        .id
}

/// Screen position of a model point seen in a view.
fn screen_of(harness: &App<'_>, view: ViewId, model: Vec3) -> Pos2 {
    session(harness).screen_of_model(view, model).expect("the view has a transform")
}

fn dimensions(harness: &App<'_>) -> Vec<(soliddraft_drawing::AnnotationId, DimensionKind, f64, AnnotationState)> {
    session(harness)
        .sheet()
        .annotations
        .iter()
        .filter_map(|a| match &a.body {
            AnnotationKind::Dimension(d) => {
                let v = match d.value {
                    DimValue::Measured { value } => value,
                    _ => 0.0,
                };
                Some((a.id, d.kind, v, a.state.clone()))
            }
            _ => None,
        })
        .collect()
}

fn value_of(harness: &App<'_>, id: soliddraft_drawing::AnnotationId) -> (f64, AnnotationState) {
    dimensions(harness).into_iter().find(|(i, ..)| *i == id).map(|(_, _, v, s)| (v, s)).expect("the dimension exists")
}

fn push(harness: &mut App<'_>, action: DrawingAction) {
    harness.state_mut().state.push(AppAction::Drawing(action));
    harness.run_steps(3);
}

/// Smart dimension: picks model points seen in a view, then places at a screen offset
/// from the last pick. Returns the new dimension's id.
fn smart_dimension(
    harness: &mut App<'_>,
    view: ViewId,
    picks: &[Vec3],
    place: egui::Vec2,
) -> soliddraft_drawing::AnnotationId {
    let before: Vec<_> = dimensions(harness).into_iter().map(|d| d.0).collect();
    push(harness, DrawingAction::SetTool(DrawingTool::SmartDimension));
    let mut last = Pos2::ZERO;
    for p in picks {
        last = screen_of(harness, view, *p);
        click_at(harness, last);
    }
    assert_eq!(
        session(harness).picks.len(),
        picks.len(),
        "every pick should land on a curve: {:?}",
        session(harness).picks
    );
    let prop = session(harness).proposal.clone().expect("a proposal");
    assert!(prop.complete, "the proposal should be complete");
    click_at(harness, last + place);
    let after = dimensions(harness);
    let new = after.iter().find(|d| !before.contains(&d.0)).expect("a dimension was added");
    new.0
}

#[test]
fn bracket_drawing_dimensioned_exported_updated_and_repaired() {
    if !gpu_available() {
        eprintln!("no GPU adapter; skipping the drawing exit test");
        return;
    }
    let _gpu = gpu_lock();
    let dir = out_dir();
    soliddraft_app::file::set_config_dir(Some(dir.join("config")));
    let doc = bracket_document();
    assert_eq!(doc.features.len(), 12);
    let mut harness = harness(doc);
    harness.run_steps(6);
    harness.key_press(Key::I);
    harness.key_press(Key::Z);
    settle(&mut harness);
    save(&mut harness, "d00-bracket");

    // 1. Make drawing from the Features tab opens the New drawing PropertyManager.
    click_button(&mut harness, "Make drawing");
    assert!(harness.state().state.drawing.new.is_some(), "the New drawing PropertyManager should open");
    save(&mut harness, "d01-new-drawing");
    click_button(&mut harness, "Create drawing (Enter)");
    settle(&mut harness);
    assert!(harness.state().state.drawing.is_active(), "the drawing tab should be active");
    {
        let s = session(&harness);
        let sheet = s.sheet();
        assert_eq!(sheet.views.len(), 4, "front, top, left and isometric");
        assert_eq!(sheet.size.name, "A3");
        assert_eq!(sheet.projection, soliddraft_drawing::Projection::FirstAngle);
        assert_eq!(sheet.scale, soliddraft_drawing::Scale::FULL, "the bracket fits A3 at 1:1");
        for v in &sheet.views {
            assert!(v.geometry.curves.iter().any(|c| c.visible), "{} has visible curves", v.id);
        }
        let marks = sheet.annotations.iter().filter(|a| matches!(a.body, AnnotationKind::CentreMark)).count();
        assert!(marks >= 5, "centre marks for the boss and the four holes in the top view: {marks}");
    }
    let front = view_named(&harness, "Front view");
    let top = view_named(&harness, "Top view");
    let left = view_named(&harness, "Left view");
    let iso = view_named(&harness, "Isometric view");
    {
        let s = session(&harness);
        let pos = |id: ViewId| s.doc.drawing.view(id).unwrap().position;
        assert!(pos(top).v < pos(front).v && (pos(top).u - pos(front).u).abs() < 1e-9, "first angle: top below front");
        assert!(
            pos(left).u > pos(front).u && (pos(left).v - pos(front).v).abs() < 1e-9,
            "first angle: left to the right"
        );
        assert!(pos(iso).u > pos(left).u && pos(iso).v > pos(top).v, "the isometric sits top right");
        assert!(s.doc.drawing.view(top).unwrap().alignment.is_some());
        assert!(s.doc.drawing.view(left).unwrap().alignment.is_some());
    }
    save(&mut harness, "d02-drawing");

    // 2. Five smart dimensions. Picks are model points on edges, seen in a view.
    click_button(&mut harness, "Annotate tab");
    // Plate thickness: the plate's end edge at x = 120, placed to the right, short of
    // the left view (a click within HIT_PX of its edge would pick it instead).
    let thickness = smart_dimension(&mut harness, front, &[Vec3::new(120.0, 0.0, 5.0)], vec2(30.0, 0.0));
    let (v, _) = value_of(&harness, thickness);
    assert!((v - 10.0).abs() < 1e-6, "plate thickness {v}");
    // Overall length: the flange's outer edge and the plate's end edge, placed below.
    let length = smart_dimension(
        &mut harness,
        front,
        &[Vec3::new(0.0, 0.0, 30.0), Vec3::new(120.0, 0.0, 5.0)],
        vec2(-120.0, 50.0),
    );
    let (v, _) = value_of(&harness, length);
    assert!((v - 120.0).abs() < 1e-6, "overall length {v}");
    // Boss diameter in the top view.
    let boss_dia = smart_dimension(&mut harness, top, &[Vec3::new(82.0, 45.0, 30.0)], vec2(24.0, -52.0));
    let (v, _) = value_of(&harness, boss_dia);
    assert!((v - 24.0).abs() < 1e-6, "boss diameter {v}");
    assert!(matches!(dimensions(&harness).iter().find(|d| d.0 == boss_dia).unwrap().1, DimensionKind::Diameter));
    // Slot length and height in the left view.
    // Placements must clear other entities: a click on a curve adds it to the pick.
    let slot_len = smart_dimension(&mut harness, left, &[Vec3::new(0.0, 45.0, 25.0)], vec2(0.0, -30.0));
    let (v, _) = value_of(&harness, slot_len);
    assert!((v - 40.0).abs() < 1e-6, "slot length {v}");
    let slot_h = smart_dimension(&mut harness, left, &[Vec3::new(0.0, 25.0, 31.0)], vec2(25.0, 0.0));
    let (v, _) = value_of(&harness, slot_h);
    assert!((v - 12.0).abs() < 1e-6, "slot height {v}");
    assert_eq!(dimensions(&harness).len(), 5);
    assert!(matches!(
        dimensions(&harness).iter().find(|d| d.0 == slot_h).unwrap().1,
        DimensionKind::Linear { orientation: LinearOrientation::Vertical }
    ));

    // 3. A hole callout on the first hole, read from the feature.
    push(&mut harness, DrawingAction::SetTool(DrawingTool::HoleCallout));
    let hole = screen_of(&harness, top, Vec3::new(29.5, 23.0, 10.0));
    click_at(&mut harness, hole);
    let callout = session(&harness)
        .sheet()
        .annotations
        .iter()
        .find_map(|a| match &a.body {
            AnnotationKind::HoleCallout { text } => Some(text.clone()),
            _ => None,
        })
        .expect("a hole callout");
    assert_eq!(callout, "4× Ø9 THRU");

    // 4. A note, typed in the property panel.
    push(&mut harness, DrawingAction::SetTool(DrawingTool::Note));
    let beside_top = screen_of(&harness, top, Vec3::new(132.0, 84.0, 10.0));
    click_at(&mut harness, beside_top);
    harness.run_steps(3);
    // The multiline editor registers an inner node with the same label; the first is the field.
    // The section caption carries the same label; the editor is the last match.
    harness.get_all_by_label("Note text").last().expect("the note editor").click();
    harness.run_steps(2);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.get_all_by_label("Note text").last().expect("the note editor").type_text("DEBURR AFTER MACHINING");
    harness.run_steps(2);
    click_button(&mut harness, "Apply text");
    let notes: Vec<String> = session(&harness)
        .sheet()
        .annotations
        .iter()
        .filter_map(|a| match &a.body {
            AnnotationKind::Note { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(notes.iter().any(|t| t == "DEBURR AFTER MACHINING"), "{notes:?}");
    push(&mut harness, DrawingAction::Select(None));
    save(&mut harness, "d03-annotated");

    // 5. Export PDF and DXF from the Sheet tab.
    harness.state_mut().state.drawing.session.as_mut().unwrap().export_dir = Some(dir.clone());
    click_button(&mut harness, "Sheet tab");
    click_button(&mut harness, "PDF");
    click_button(&mut harness, "DXF");
    harness.run_steps(3);
    let pdf = std::fs::read(dir.join("bracket.pdf")).expect("the PDF was written");
    let doc = lopdf::Document::load_mem(&pdf).expect("the PDF loads");
    let pages = doc.get_pages();
    assert_eq!(pages.len(), 1, "one page");
    let page = doc.get_dictionary(pages[&1]).unwrap();
    let mbox: Vec<f32> =
        page.get(b"MediaBox").unwrap().as_array().unwrap().iter().map(|v| v.as_float().unwrap()).collect();
    assert!((mbox[2] - 1190.55).abs() < 0.1 && (mbox[3] - 841.89).abs() < 0.1, "one A3 page: {mbox:?}");
    let text = doc.extract_text(&[1]).unwrap();
    assert!(text.contains("Bracket"), "the title block carries the title: {text}");
    let dxf = std::fs::read(dir.join("bracket-sheet1.dxf")).expect("the DXF was written");
    let counts = count_entities(&dxf).expect("the DXF re-reads");
    eprintln!("dxf entities: {counts:?}");
    assert!(counts.get("VISIBLE/LINE").copied().unwrap_or(0) > 20, "{counts:?}");
    assert!(counts.get("VISIBLE/CIRCLE").copied().unwrap_or(0) >= 5, "{counts:?}");
    assert!(counts.get("HIDDEN/LINE").copied().unwrap_or(0) > 4, "{counts:?}");
    assert!(counts.get("DIMENSION/DIMENSION").copied().unwrap_or(0) >= 5, "dimensions stay dimensions: {counts:?}");
    assert!(counts.get("TEXT/TEXT").copied().unwrap_or(0) >= 8, "{counts:?}");
    assert!(counts.get("FORMAT/LINE").copied().unwrap_or(0) >= 8, "{counts:?}");
    assert!(!counts.keys().any(|k| k.ends_with("/SPLINE")), "no splines: {counts:?}");

    // 6. Change the plate thickness in the part tab: the drawing updates, 10 becomes 12.
    click_button(&mut harness, "Views tab");
    click_button(&mut harness, "Show part");
    harness.run_steps(3);
    assert!(!harness.state().state.drawing.is_active(), "the part tab should be active");
    click_row(&mut harness, "Plate");
    click_button(&mut harness, "Edit feature");
    set_field(&mut harness, "Depth", "12");
    harness.key_press(Key::Enter);
    settle(&mut harness);
    let (v, state) = value_of(&harness, thickness);
    assert!((v - 12.0).abs() < 1e-6, "plate thickness after the change: {v}");
    assert_eq!(state, AnnotationState::Changed { previous: "10".into() });
    let (v, state) = value_of(&harness, length);
    assert!((v - 120.0).abs() < 1e-6 && state == AnnotationState::Ok, "overall length unchanged: {v} {state:?}");
    {
        let s = session(&harness);
        let r = s.doc.last_update.as_ref().expect("an update report");
        assert_eq!(r.views, 4);
        assert_eq!(r.changed, vec![thickness]);
        assert!(r.dangling.is_empty());
        assert!(s.is_up_to_date());
    }
    click_button(&mut harness, "Open drawings");
    settle(&mut harness);
    assert!(harness.state().state.drawing.is_active());
    save(&mut harness, "d04-thickness-12");

    // 7. Delete the slot: its two dimensions dangle and are listed.
    click_button(&mut harness, "Show part");
    harness.run_steps(3);
    click_row(&mut harness, "Slot");
    harness.key_press(Key::Delete);
    settle(&mut harness);
    assert!(!harness.state().state.doc.features.iter().any(|f| f.name == "Slot"));
    {
        let s = session(&harness);
        assert_eq!(s.doc.dangling_count(), 2, "{:?}", dimensions(&harness));
        let (_, state) = value_of(&harness, slot_len);
        let AnnotationState::Dangling { lost, cause } = state else { panic!("slot length should dangle") };
        assert_eq!(lost, vec![0, 1]);
        assert!(cause.contains("Slot") && cause.contains("no longer exists"), "{cause}");
        let r = s.doc.last_update.as_ref().unwrap();
        assert_eq!(r.dangling.len(), 2);
    }
    click_button(&mut harness, "Open drawings");
    settle(&mut harness);
    assert!(harness.get_all_by_role_and_label(egui::accesskit::Role::Button, "Reattach…").count() >= 2, "two cards");
    save(&mut harness, "d05-dangling");

    // 8. Reattach the slot length to the hole edges, delete the slot height.
    push(&mut harness, DrawingAction::StartReattach { annotation: slot_len, end: 0 });
    assert!(matches!(session(&harness).tool, DrawingTool::Reattach { end: 0, .. }));
    let hole1 = screen_of(&harness, left, Vec3::new(25.0, 23.0, 10.0));
    click_at(&mut harness, hole1);
    assert!(matches!(session(&harness).tool, DrawingTool::Reattach { end: 1, .. }), "{:?}", session(&harness).tool);
    save(&mut harness, "d06-reattach-pick");
    let hole2 = screen_of(&harness, left, Vec3::new(25.0, 67.0, 10.0));
    click_at(&mut harness, hole2);
    let (v, state) = value_of(&harness, slot_len);
    assert_eq!(state, AnnotationState::Ok, "reattached");
    assert!((v - 44.0).abs() < 1e-6, "hole pitch between the reattached ends: {v}");
    push(&mut harness, DrawingAction::Select(Some(DrawingSel::Annotation(slot_h))));
    harness.key_press(Key::Delete);
    harness.run_steps(3);
    assert_eq!(session(&harness).doc.dangling_count(), 0);
    assert_eq!(dimensions(&harness).len(), 4);
    push(&mut harness, DrawingAction::AcceptChange(thickness));
    push(&mut harness, DrawingAction::Select(None));
    push(&mut harness, DrawingAction::DismissBanner);
    settle(&mut harness);
    save(&mut harness, "d07-repaired");

    // Undo and redo on the drawing's own stack.
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    harness.run_steps(3);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Y);
    harness.run_steps(3);
    assert_eq!(dimensions(&harness).len(), 4);
}

/// The fixture drawing, opened without its part: the sheet with a changed value and a
/// dangling dimension renders, and the Needs attention list is there.
#[test]
fn fixture_drawing_renders_with_attention_list() {
    if !gpu_available() {
        return;
    }
    let _gpu = gpu_lock();
    let mut harness = harness(soliddraft_app::document::Document::new(
        "Part1",
        std::sync::Arc::new(soliddraft_kernel_catallax::CatallaxKernel::new()),
    ));
    harness.run_steps(3);
    let doc = soliddraft_app::drawing::DrawingDoc::new(soliddraft_drawing::fixture::bracket_a3());
    let s = soliddraft_app::drawing::DrawingSession::new(doc);
    harness.state_mut().state.drawing.session = Some(s);
    push(&mut harness, DrawingAction::ShowDrawing);
    settle(&mut harness);
    assert!(harness.state().state.drawing.is_active());
    assert_eq!(session(&harness).doc.attention_count(), 2);
    assert!(harness.get_all_by_role_and_label(egui::accesskit::Role::Button, "Reattach…").count() >= 1);
    save(&mut harness, "d10-fixture");
    push(&mut harness, DrawingAction::Select(Some(DrawingSel::Annotation(soliddraft_drawing::AnnotationId(5)))));
    harness.run_steps(3);
    save(&mut harness, "d11-fixture-dimension-selected");
}

/// The sheet point a screen position shows, through the session's camera.
fn screen_of_sheet(harness: &App<'_>, p: soliddraft_kernel_api::Point2) -> Pos2 {
    session(harness).screen_of(p)
}

fn views_of_kind(harness: &App<'_>, want: &str) -> Vec<soliddraft_drawing::View> {
    session(harness)
        .doc
        .drawing
        .sheets
        .iter()
        .flat_map(|s| &s.views)
        .filter(|v| {
            matches!(
                (&v.kind, want),
                (soliddraft_drawing::ViewKind::Section { .. }, "section")
                    | (soliddraft_drawing::ViewKind::Detail { .. }, "detail")
                    | (soliddraft_drawing::ViewKind::Auxiliary { .. }, "auxiliary")
            )
        })
        .cloned()
        .collect()
}

fn annotations_of_kind(
    harness: &App<'_>,
    pick: impl Fn(&AnnotationKind) -> bool,
) -> Vec<soliddraft_drawing::Annotation> {
    session(harness).doc.drawing.sheets.iter().flat_map(|s| &s.annotations).filter(|a| pick(&a.body)).cloned().collect()
}

fn type_field(harness: &mut App<'_>, label: &str, value: &str) {
    harness.get_by_label(label).click();
    harness.run_steps(2);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.get_by_label(label).type_text(value);
    harness.run_steps(2);
    harness.key_press(Key::Enter);
    harness.run_steps(3);
}

/// The phase D3 exit test: Section A-A and Detail B of the bracket as in frame 6 of the
/// drawing examples; a second sheet with a detail whose location labels point both ways;
/// an ordinate set; a GD&T frame on a datum; a revision row and symbol; a two-page PDF
/// and a DXF with DIMENSION and HATCH entities. Frames go to `target/exit-test/drawing/`.
#[test]
fn bracket_d3_sections_details_sheets_and_symbols() {
    use soliddraft_drawing::{AlignAxis, SetKind, ViewKind};
    use soliddraft_kernel_api::Point2;
    if !gpu_available() {
        eprintln!("no GPU adapter; skipping the D3 exit test");
        return;
    }
    let _gpu = gpu_lock();
    let dir = out_dir();
    soliddraft_app::file::set_config_dir(Some(dir.join("config")));
    let mut harness = harness(bracket_document());
    harness.run_steps(6);
    click_button(&mut harness, "Make drawing");
    click_button(&mut harness, "Create drawing (Enter)");
    settle(&mut harness);
    assert!(harness.state().state.drawing.is_active());
    let front = view_named(&harness, "Front view");
    let top = view_named(&harness, "Top view");
    let position = |harness: &App<'_>, id: ViewId| session(harness).doc.drawing.view(id).unwrap().position;

    // 1. Section A-A: a vertical cutting line through the boss on the top view, flipped
    // to look along +X, placed right of the top view on its alignment (frame 6).
    click_button(&mut harness, "Section");
    assert!(matches!(session(&harness).tool, DrawingTool::Section { replace: None }));
    let boss = screen_of(&harness, top, Vec3::new(70.0, 45.0, 30.0));
    click_at(&mut harness, boss);
    assert_eq!(session(&harness).draft.view, Some(top), "the line starts on the top view");
    harness.hover_at(boss + vec2(2.0, 70.0));
    harness.run_steps(3);
    assert!(!session(&harness).draft.horizontal, "moving down picks a vertical line");
    save(&mut harness, "d20-section-line");
    click_at(&mut harness, boss + vec2(2.0, 70.0));
    assert!(session(&harness).draft.pending.is_some(), "the section is computed once the line is drawn");
    harness.key_press(Key::F);
    harness.run_steps(3);
    assert!(session(&harness).draft.flip, "F flips the direction");
    let tp = position(&harness, top);
    let place = screen_of_sheet(&harness, Point2::new(tp.u + 150.0, tp.v + 30.0));
    harness.hover_at(place);
    harness.run_steps(3);
    save(&mut harness, "d21-section-ghost");
    click_at(&mut harness, place);
    settle(&mut harness);
    let sections = views_of_kind(&harness, "section");
    assert_eq!(sections.len(), 1, "one section view");
    let section = &sections[0];
    let ViewKind::Section { parent, line, flip, label, model_line, .. } = &section.kind else { panic!() };
    assert_eq!((*parent, label.as_str(), *flip), (top, "A", true));
    assert!((line[0].u - line[1].u).abs() < 1e-9, "a vertical cutting line: {line:?}");
    assert_eq!(model_line.len(), 2, "the cut is kept in model space");
    assert!(!section.geometry.hatch.is_empty(), "the cut faces are hatched");
    assert!(section.geometry.curves.iter().any(|c| c.visible));
    let al = section.alignment.expect("aligned with its parent");
    assert_eq!((al.parent, al.axis), (top, AlignAxis::Horizontal));
    assert!((section.position.v - tp.v).abs() < 1e-9 && section.position.u > tp.u + 100.0, "right of the top view");
    assert_eq!(session(&harness).doc.drawing.next_view_label, "B");
    assert_eq!(session(&harness).selection, Some(DrawingSel::View(section.id)));
    assert!(session(&harness).doc.transforms.contains_key(&section.id), "the section's transform is remembered");

    // 2. Detail B at 2:1 on the front view: the top of the flange, as in frame 6.
    click_button(&mut harness, "Detail");
    let centre = screen_of(&harness, front, Vec3::new(5.0, 0.0, 54.0));
    click_at(&mut harness, centre);
    assert_eq!(session(&harness).draft.view, Some(front));
    let rim = screen_of(&harness, front, Vec3::new(5.0, 0.0, 42.0));
    click_at(&mut harness, rim);
    assert!(session(&harness).draft.pending.is_some(), "the detail is computed once the circle is drawn");
    let place = screen_of_sheet(&harness, Point2::new(tp.u + 240.0, tp.v + 30.0));
    click_at(&mut harness, place);
    settle(&mut harness);
    let details = views_of_kind(&harness, "detail");
    assert_eq!(details.len(), 1);
    let detail = &details[0];
    let ViewKind::Detail { parent, radius, factor, label, profile, model_center, .. } = &detail.kind else { panic!() };
    assert_eq!((*parent, label.as_str(), *factor), (front, "B", 2.0));
    assert!((radius - 12.0).abs() < 0.5, "radius from the second click: {radius}");
    assert!(profile.is_empty() && model_center.is_some());
    assert!(detail.geometry.outline.is_some(), "the detail is clipped to its circle");
    {
        let s = session(&harness);
        let sheet = s.sheet();
        let scale = soliddraft_drawing::project::effective_scale(sheet, detail);
        assert!((scale - 2.0).abs() < 1e-9, "2:1 on a 1:1 sheet: {scale}");
        let list = soliddraft_drawing::display::layout_sheet(&s.doc.drawing, 0);
        let texts: Vec<&str> = list
            .items
            .iter()
            .filter_map(|i| match i {
                soliddraft_drawing::display::Item::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect();
        for want in ["SECTION A-A", "DETAIL B", "SCALE 2:1"] {
            assert!(texts.contains(&want), "{want} on the sheet: {texts:?}");
        }
    }
    push(&mut harness, DrawingAction::Select(None));
    save(&mut harness, "d22-section-and-detail");

    // 3. A second sheet with Detail C of a hole in the top view: location labels on
    // both sheets point to each other.
    click_button(&mut harness, "Detail");
    harness.state_mut().state.drawing.session.as_mut().unwrap().draft.target_sheet = Some(1);
    let hole = screen_of(&harness, top, Vec3::new(25.0, 23.0, 10.0));
    click_at(&mut harness, hole);
    let rim = screen_of(&harness, top, Vec3::new(36.0, 23.0, 10.0));
    click_at(&mut harness, rim);
    settle(&mut harness);
    {
        let s = session(&harness);
        assert_eq!(s.doc.drawing.sheets.len(), 2, "the detail made a second sheet");
        assert_eq!(s.sheet, 1, "the canvas shows the new sheet");
        assert_eq!(s.doc.drawing.sheets[1].views.len(), 1);
        let labels = soliddraft_drawing::display::location_labels(&s.doc.drawing);
        let c = labels.iter().find(|l| l.view_sheet == 1).expect("a label for the detail on sheet 2");
        assert_eq!(c.parent, top);
        assert_eq!(c.on_parent().as_deref(), Some(format!("SEE SHEET 2, ZONE {}", c.view_zone).as_str()));
        assert_eq!(c.on_view().as_deref(), Some(format!("FROM SHEET 1, ZONE {}", c.parent_zone).as_str()));
        let list = soliddraft_drawing::display::layout_sheet(&s.doc.drawing, 1);
        assert!(
            list.items
                .iter()
                .any(|i| matches!(i, soliddraft_drawing::display::Item::Text(t) if t.text.starts_with("FROM SHEET 1")))
        );
        let list = soliddraft_drawing::display::layout_sheet(&s.doc.drawing, 0);
        assert!(
            list.items
                .iter()
                .any(|i| matches!(i, soliddraft_drawing::display::Item::Text(t) if t.text.starts_with("SEE SHEET 2")))
        );
    }
    push(&mut harness, DrawingAction::Select(None));
    save(&mut harness, "d23-sheet2-detail");
    // Rename it through the sheet tab strip, then go back to sheet 1.
    push(
        &mut harness,
        DrawingAction::Command(soliddraft_drawing::Command::RenameSheet { sheet: 1, name: "Details".into() }),
    );
    assert_eq!(session(&harness).doc.drawing.sheets[1].name, "Details");
    click_button(&mut harness, "Sheet1 tab");
    settle(&mut harness);
    assert_eq!(session(&harness).sheet, 0);
    save(&mut harness, "d24-sheet1-see-sheet2");

    // 4. An ordinate set in the top view: from the flange's outer edge to the holes and
    // the boss, placed above the view.
    click_button(&mut harness, "Annotate tab");
    click_button(&mut harness, "Ordinate");
    assert_eq!(session(&harness).tool, DrawingTool::DimensionSet(SetKind::Ordinate));
    for p in [
        Vec3::new(0.0, 45.0, 60.0),
        Vec3::new(29.5, 23.0, 10.0),
        Vec3::new(82.0, 45.0, 30.0),
        Vec3::new(119.5, 67.0, 10.0),
    ] {
        let at = screen_of(&harness, top, p);
        click_at(&mut harness, at);
    }
    assert_eq!(session(&harness).draft.anchors.len(), 4, "{:?}", session(&harness).draft.anchors);
    // Placed between the top and front views, clear of the general notes.
    let above = screen_of_sheet(&harness, Point2::new(tp.u, tp.v + 60.0));
    click_at(&mut harness, above);
    let sets = annotations_of_kind(&harness, |b| matches!(b, AnnotationKind::DimensionSet(_)));
    assert_eq!(sets.len(), 1);
    let AnnotationKind::DimensionSet(set) = &sets[0].body else { panic!() };
    assert_eq!((set.kind, set.orientation), (SetKind::Ordinate, LinearOrientation::Horizontal));
    assert_eq!(set.values, vec![25.0, 70.0, 115.0], "{set:?}");

    // 5. Datum A on the plate's bottom edge and a perpendicularity frame on the flange's
    // outer edge, both in the front view.
    click_button(&mut harness, "Datum");
    let bottom = screen_of(&harness, front, Vec3::new(60.0, 0.0, 0.0));
    click_at(&mut harness, bottom);
    assert_eq!(session(&harness).draft.anchors.len(), 1, "the datum needs an edge");
    click_at(&mut harness, bottom + vec2(0.0, 36.0));
    let datums = annotations_of_kind(&harness, |b| matches!(b, AnnotationKind::Datum { .. }));
    assert_eq!(datums.len(), 1);
    assert!(matches!(&datums[0].body, AnnotationKind::Datum { letter } if letter == "A"));
    assert_eq!(session(&harness).doc.drawing.next_datum_label, "B");
    click_button(&mut harness, "GD&T frame");
    {
        let d = &mut harness.state_mut().state.drawing.session.as_mut().unwrap().draft.gdt;
        d.characteristic = soliddraft_drawing::GdtCharacteristic::Perpendicularity;
        d.tolerance = 0.1;
        d.diameter = false;
        d.datums = vec![soliddraft_drawing::DatumRef::new("A")];
    }
    harness.run_steps(2);
    let flange = screen_of(&harness, front, Vec3::new(0.0, 0.0, 30.0));
    click_at(&mut harness, flange);
    assert_eq!(session(&harness).draft.anchors.len(), 1, "the frame is on an edge");
    click_at(&mut harness, flange + vec2(-40.0, -46.0));
    let frames = annotations_of_kind(&harness, |b| matches!(b, AnnotationKind::Gdt { .. }));
    assert_eq!(frames.len(), 1);
    let AnnotationKind::Gdt { characteristic, tolerance, datums, .. } = &frames[0].body else { panic!() };
    assert_eq!(*characteristic, soliddraft_drawing::GdtCharacteristic::Perpendicularity);
    assert!((tolerance - 0.1).abs() < 1e-9 && datums.len() == 1 && datums[0].letter == "A");
    assert_eq!(frames[0].anchors.len(), 1, "with a leader to the edge");
    save(&mut harness, "d25-ordinate-datum-gdt");

    // 6. A revision row through the revision table panel, and a revision symbol on the
    // boss.
    click_button(&mut harness, "Revision table");
    assert_eq!(session(&harness).selection, Some(DrawingSel::Revisions));
    type_field(&mut harness, "Description field", "Added section A-A and detail B");
    type_field(&mut harness, "Approved field", "PP");
    click_button(&mut harness, "Add revision row");
    {
        let s = session(&harness);
        let table = s.sheet().revision_table().expect("the first row makes the table");
        assert_eq!(table.revisions.len(), 1);
        assert_eq!(table.revisions[0].revision, "B");
        assert_eq!(table.revisions[0].description, "Added section A-A and detail B");
        assert_eq!(s.doc.drawing.meta.revision, "B", "the drawing's revision follows the row");
    }
    click_button(&mut harness, "Revision symbol");
    let boss_edge = screen_of(&harness, top, Vec3::new(82.0, 45.0, 30.0));
    click_at(&mut harness, boss_edge);
    click_at(&mut harness, boss_edge + vec2(40.0, -30.0));
    let symbols = annotations_of_kind(&harness, |b| matches!(b, AnnotationKind::RevisionSymbol { .. }));
    assert_eq!(symbols.len(), 1);
    assert!(matches!(&symbols[0].body, AnnotationKind::RevisionSymbol { revision } if revision == "B"));
    push(&mut harness, DrawingAction::Select(None));
    settle(&mut harness);
    save(&mut harness, "d26-revisions");

    // 7. Export: the PDF carries both sheets; the DXF of sheet 1 keeps the ordinate set
    // as DIMENSION entities and the section's hatch as a HATCH.
    harness.state_mut().state.drawing.session.as_mut().unwrap().export_dir = Some(dir.clone());
    click_button(&mut harness, "Sheet tab");
    click_button(&mut harness, "PDF");
    click_button(&mut harness, "DXF");
    harness.run_steps(3);
    let pdf = std::fs::read(dir.join("bracket.pdf")).expect("the PDF was written");
    let doc = lopdf::Document::load_mem(&pdf).expect("the PDF loads");
    assert_eq!(doc.get_pages().len(), 2, "one page per sheet");
    let text = doc.extract_text(&[1]).unwrap();
    assert!(text.contains("SECTION A-A") && text.contains("DETAIL B"), "{text}");
    let text2 = doc.extract_text(&[2]).unwrap();
    assert!(text2.contains("FROM SHEET 1"), "{text2}");
    let dxf = std::fs::read(dir.join("bracket-sheet1.dxf")).expect("the DXF was written");
    let counts = count_entities(&dxf).expect("the DXF re-reads");
    eprintln!("dxf entities: {counts:?}");
    assert!(counts.get("DIMENSION/DIMENSION").copied().unwrap_or(0) >= 3, "the ordinate set: {counts:?}");
    assert!(counts.get("HATCH/HATCH").copied().unwrap_or(0) >= 1, "the section's hatch: {counts:?}");
    assert!(counts.get("CUTTING/LINE").copied().unwrap_or(0) >= 1, "the cutting line: {counts:?}");

    // 8. Undo walks back through the drawing's own stack.
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    harness.run_steps(3);
    assert_eq!(annotations_of_kind(&harness, |b| matches!(b, AnnotationKind::RevisionSymbol { .. })).len(), 0);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Y);
    harness.run_steps(3);
    assert_eq!(annotations_of_kind(&harness, |b| matches!(b, AnnotationKind::RevisionSymbol { .. })).len(), 1);
}
