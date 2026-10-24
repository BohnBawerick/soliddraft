//! The drag preview through the interface, driven by egui_kittest against the real wgpu
//! pipeline: select the plate of the sample part, drag its Depth field in the property
//! panel, see the viewport and the tree say "Preview" while the part itself is unchanged,
//! let go and get the final depth as one undo step. Frames are written to
//! `target/exit-test/` for review.
//!
//! Skips itself when no GPU adapter is available (plain CI runners).

use egui::{pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soliddraft_app::app::{Automation, SolidDraftApp};
use soliddraft_app::document::{FeatureKind, Selection};
use soliddraft_app::state::AppAction;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn gpu_available() -> bool {
    let _ = env_logger::try_init();
    let instance = egui_wgpu::wgpu::Instance::default();
    !pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all())).is_empty()
}

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/exit-test");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

type App<'a> = Harness<'a, SolidDraftApp>;

fn save(harness: &mut App<'_>, name: &str) {
    let image = harness.render().expect("render");
    let path = out_dir().join(format!("{name}.png"));
    image.save(&path).expect("save png");
    eprintln!("wrote {}", path.display());
}

fn plate_depth(harness: &App<'_>) -> f64 {
    match &harness.state().state.doc.features[0].kind {
        FeatureKind::Extrude(p) => p.depth,
        other => panic!("the plate is an extrude, not {other:?}"),
    }
}

#[test]
fn dragging_the_depth_field_previews_and_applies_one_undo_step() {
    if !gpu_available() {
        eprintln!("no GPU adapter; skipping");
        return;
    }
    let config = std::env::temp_dir().join(format!("soliddraft-drag-interface-{}", std::process::id()));
    std::fs::create_dir_all(&config).unwrap();
    soliddraft_app::file::set_config_dir(Some(config));
    let mut harness = Harness::builder()
        .with_size(vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .with_render_every_step(true)
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_eframe(|cc| SolidDraftApp::new(cc, soliddraft_app::demo_document(), Automation::default()));
    let plate = harness.state().state.doc.features[0].id;
    harness.state_mut().state.doc.selection = Some(Selection::Feature(plate));
    harness.run_steps(10);
    let undo_before = harness.state().state.doc.undo_depth();
    assert_eq!(plate_depth(&harness), 10.0);

    let start = harness.get_by_label("Depth value").rect().center();
    harness.hover_at(start);
    harness.run_steps(2);
    harness.drag_at(start);
    harness.run_steps(2);
    let mut end = start;
    for step in 1..=20 {
        end = pos2(start.x + 4.0 * step as f32, start.y);
        harness.hover_at(end);
        harness.run_steps(1);
        // egui counts the press as a drag once the pointer has moved past its click distance.
        if step >= 4 {
            assert!(harness.state().state.drag_preview.is_some(), "the drag is under way at step {step}");
        }
        assert_eq!(plate_depth(&harness), 10.0, "the part is unchanged during the drag");
    }
    // Hold still until the worker has answered the value the drag rests on.
    let deadline = Instant::now() + Duration::from_secs(120);
    while harness.state().state.drag_preview.as_ref().is_some_and(|d| d.pending()) {
        assert!(Instant::now() < deadline, "the preview never answered");
        std::thread::sleep(Duration::from_millis(10));
        harness.run_steps(1);
    }
    let dragged = harness.state().state.drag_preview.as_ref().map(|d| d.value()).expect("still dragging");
    assert!(dragged > 10.0);
    let (label, failed) = harness.state().state.drag_preview.as_ref().unwrap().label();
    assert!(!failed && label.starts_with("Preview"), "{label}");
    assert!(harness.state().state.preview_body().is_some(), "the preview is drawn");
    save(&mut harness, "drag-preview");

    harness.drop_at(end);
    harness.run_steps(5);
    let state = &harness.state().state;
    assert!(state.drag_preview.is_none(), "letting go ends the drag");
    assert!(state.preview_body().is_none());
    assert_eq!(plate_depth(&harness), dragged, "the value the drag rested on is applied");
    assert_eq!(harness.state().state.doc.undo_depth(), undo_before + 1, "one drag is one undo step");
    save(&mut harness, "drag-preview-applied");

    harness.state_mut().state.push(AppAction::Undo);
    harness.run_steps(5);
    assert_eq!(plate_depth(&harness), 10.0);
}
