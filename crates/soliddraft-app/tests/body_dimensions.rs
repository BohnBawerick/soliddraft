//! Selecting the solid body shows every dimension that builds it, driven through
//! egui_kittest against the real wgpu pipeline: the tree row and a double-click on the
//! part select the body, a double-click on a shown dimension edits it, and clearing the
//! selection hides them. Frames are written to `target/body-dimensions/` for review.
//!
//! Skips itself when no GPU adapter is available (plain CI runners).

use egui::{Key, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soliddraft_app::app::{Automation, SolidDraftApp};
use soliddraft_app::document::{Command, Document, ExtrudeParams, PlaneKind, Selection, SketchPlane};
use soliddraft_app::state::AppAction;
use soliddraft_app::viewport::dimensions::DimensionTarget;
use soliddraft_sketch::{P2, Sketch};
use std::path::PathBuf;

fn gpu_available() -> bool {
    let _ = env_logger::try_init();
    let instance = egui_wgpu::wgpu::Instance::default();
    !pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all())).is_empty()
}

type App<'a> = Harness<'a, SolidDraftApp>;

fn harness(doc: Document) -> App<'static> {
    let mut doc = Some(doc);
    Harness::builder()
        .with_size(vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .with_render_every_step(true)
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_eframe(move |cc| SolidDraftApp::new(cc, doc.take().unwrap(), Automation::default()))
}

fn save(harness: &mut App<'_>, name: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/body-dimensions");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.png"));
    harness.render().expect("render").save(&path).expect("save png");
    eprintln!("wrote {}", path.display());
}

/// Waits out egui's triple-click window first, so an earlier click does not turn the
/// pair into a triple-click.
fn double_click(harness: &mut App<'_>, pos: Pos2) {
    harness.hover_at(pos);
    harness.run_steps(40);
    for pressed in [true, false, true, false] {
        harness.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        });
        harness.step();
    }
    harness.run_steps(4);
}

fn shown(harness: &App<'_>) -> Vec<(DimensionTarget, String)> {
    harness.state().state.viewport.dimensions.iter().map(|d| (d.target, d.tip.clone())).collect()
}

#[test]
fn the_selected_body_shows_its_feature_dimensions() {
    if !gpu_available() {
        eprintln!("no GPU adapter; skipping");
        return;
    }
    let mut harness = harness(soliddraft_app::demo_document());
    harness.run_steps(5);
    harness.key_press(Key::Z);
    harness.run_steps(30);
    assert!(shown(&harness).is_empty(), "nothing is selected, so no dimension shows");

    harness.get_by_label("Solid body").click();
    harness.run_steps(6);
    assert_eq!(harness.state().state.doc.selection, Some(Selection::Body));
    save(&mut harness, "demo-body-selected");
    let dims = shown(&harness);
    // Three extrudes: width, height and depth twice, then a circle's diameter and depth.
    assert_eq!(dims.len(), 8, "{dims:?}");
    let ids: Vec<_> = harness.state().state.doc.features.iter().map(|f| f.id).collect();
    for id in &ids {
        assert!(dims.iter().any(|(t, tip)| *t == DimensionTarget::Feature(*id) && tip.ends_with("Depth")));
    }

    // A double-click on the boss's depth opens its in-place value editor.
    let boss = ids[2];
    let depth = harness
        .state()
        .state
        .viewport
        .dimensions
        .iter()
        .find(|d| d.target == DimensionTarget::Feature(boss) && d.tip.ends_with("Depth"))
        .unwrap()
        .rect
        .center();
    double_click(&mut harness, depth);
    assert!(
        harness.state().state.dimension_edit.is_some(),
        "the double-click opens the boss's Depth value editor in the viewport"
    );
    harness.key_press(Key::Escape);
    harness.run_steps(4);
    assert!(harness.state().state.dimension_edit.is_none(), "Escape closes the in-place value editor");

    // Clearing the selection hides the dimensions.
    harness.state_mut().state.push(AppAction::Select(None));
    harness.run_steps(4);
    assert!(shown(&harness).is_empty());
    save(&mut harness, "demo-cleared");

    // A double-click on the part selects the body again.
    let state = &harness.state().state;
    let plate_top = state.viewport.camera.project(glam::Vec3::new(40.0, -30.0, 10.0), state.viewport.rect).unwrap();
    double_click(&mut harness, plate_top);
    assert_eq!(harness.state().state.doc.selection, Some(Selection::Body));
    assert_eq!(shown(&harness).len(), 8);
}

#[test]
fn a_sketch_dimension_of_the_body_edits_in_its_sketch() {
    if !gpu_available() {
        eprintln!("no GPU adapter; skipping");
        return;
    }
    let mut doc = soliddraft_app::new_document();
    let sketch_id = doc
        .apply(Command::AddSketch { plane: SketchPlane::Reference { plane: PlaneKind::Top, offset: 0.0 } })
        .unwrap()
        .unwrap();
    let mut sketch = Sketch::new();
    sketch.rectangle(P2::new(10.0, 5.0), 80.0, 50.0).unwrap();
    soliddraft_sketch::solve::solve(&mut sketch, None, false);
    doc.apply(Command::EditSketch { id: sketch_id, sketch }).unwrap();
    doc.apply(Command::AddExtrude {
        params: ExtrudeParams { sketch: Some(sketch_id), depth: 15.0, ..Default::default() },
        via_assistant: false,
    })
    .unwrap();
    doc.selection = None;
    let mut harness = harness(doc);
    harness.run_steps(5);
    harness.key_press(Key::Z);
    harness.run_steps(30);
    harness.state_mut().state.push(AppAction::Select(Some(Selection::Body)));
    harness.run_steps(6);
    save(&mut harness, "sketch-body-selected");
    let dims = harness.state().state.viewport.dimensions.clone();
    let sketch_dims: Vec<_> = dims
        .iter()
        .filter(|d| matches!(d.target, DimensionTarget::Sketch { sketch, .. } if sketch == sketch_id))
        .collect();
    assert_eq!(sketch_dims.len(), 2, "the rectangle's width and height: {dims:?}");
    assert_eq!(dims.len(), 3, "and the extrude's depth: {dims:?}");

    let DimensionTarget::Sketch { constraint, .. } = sketch_dims[0].target else { unreachable!() };
    double_click(&mut harness, sketch_dims[0].rect.center());
    save(&mut harness, "sketch-dimension-editing");
    let session = harness.state().state.sketch.as_ref().expect("the double-click opens the sketch");
    assert_eq!(session.feature, sketch_id);
    assert_eq!(session.dim_edit.as_ref().map(|e| e.constraint), Some(constraint));
}
