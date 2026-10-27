//! The drag preview through the interface, driven by egui_kittest against the real wgpu
//! pipeline: select the plate of the sample part, drag its Depth field in the property
//! panel, see preview state in the controls and tree while the part itself is unchanged,
//! let go and get the final depth as one undo step. Frames go to
//! `SOLIDDRAFT_INTERFACE_PROOF_DIR`, or `target/exit-test/` by default.
//!
//! Skips itself when no GPU adapter is available (plain CI runners).

use egui::{Key, Modifiers, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soliddraft_app::app::{Automation, SolidDraftApp};
use soliddraft_app::document::{
    Command, Document, ExtrudeParams, FeatureKind, FeatureStatus, FilletParams, ProfileKind, Selection,
};
use soliddraft_app::drag::Shown;
use soliddraft_app::state::AppAction;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
mod gated;

fn gpu_available() -> bool {
    let _ = env_logger::try_init();
    let instance = egui_wgpu::wgpu::Instance::default();
    !pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all())).is_empty()
}

fn out_dir() -> PathBuf {
    let dir = std::env::var_os("SOLIDDRAFT_INTERFACE_PROOF_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/exit-test"));
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

fn settle_document(harness: &mut App<'_>) {
    let deadline = Instant::now() + Duration::from_secs(120);
    while harness.state().state.doc.proof_pending() {
        assert!(Instant::now() < deadline, "the current document proof never answered");
        std::thread::sleep(Duration::from_millis(10));
        harness.run_steps(1);
    }
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
    let kernel = Arc::new(gated::Gated::default());
    let mut doc = Document::new("Part1", kernel.clone());
    soliddraft_app::populate_demo(&mut doc);
    let mut harness = Harness::builder()
        .with_size(vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .with_render_every_step(true)
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_eframe(|cc| SolidDraftApp::new(cc, doc, Automation::default()));
    let plate = harness.state().state.doc.features[0].id;
    harness.state_mut().state.doc.selection = Some(Selection::Feature(plate));
    harness.run_steps(10);
    let undo_before = harness.state().state.doc.undo_depth();
    assert_eq!(plate_depth(&harness), 10.0);
    kernel.rules.lock().unwrap().hold_next = true;

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
    // The first exact edit is held, but a newer construction must still render.
    let deadline = Instant::now() + Duration::from_secs(120);
    while !harness
        .state()
        .state
        .drag_preview
        .as_ref()
        .is_some_and(|drag| matches!(drag.shown(), Shown::Result(shown) if shown.value == drag.value()))
    {
        assert!(Instant::now() < deadline, "the changed shape was held behind exact proof");
        std::thread::sleep(Duration::from_millis(10));
        harness.run_steps(1);
    }
    let dragged = harness.state().state.drag_preview.as_ref().map(|d| d.value()).expect("still dragging");
    assert!(dragged > 10.0);
    assert!(harness.state().state.preview_body().is_some(), "the preview is drawn");
    assert!(harness.state().state.drag_preview.as_ref().unwrap().pending());
    assert!(harness.state().state.export_part(soliddraft_exchange::Format::Step).is_err());
    save(&mut harness, "drag-preview");

    harness.drop_at(end);
    harness.run_steps(5);
    let state = &harness.state().state;
    assert!(state.drag_preview.is_none(), "letting go ends the drag");
    assert!(state.doc.proof_pending());
    assert!(state.preview_body().is_some());
    assert!(state.export_part(soliddraft_exchange::Format::Step).is_err());
    assert_eq!(plate_depth(&harness), dragged, "the value the drag rested on is applied");
    assert_eq!(harness.state().state.doc.undo_depth(), undo_before + 1, "one drag is one undo step");
    save(&mut harness, "drag-preview-pending-committed");
    let center = harness.get_by_label("3D viewport").rect().center();
    harness.hover_at(center);
    harness.run_steps(8);
    harness.get_by_label("3D viewport").click();
    harness.run_steps(3);
    let Some(Selection::Face(preview_face)) = harness.state().state.doc.selection else {
        panic!("the displayed provisional face is selectable through the real id buffer");
    };
    assert!(harness.state().state.doc.any_face_ref(preview_face).is_none());
    save(&mut harness, "drag-preview-selected-origin");
    kernel.rules.lock().unwrap().hold = None;
    kernel.changed.notify_all();
    settle_document(&mut harness);
    let Some(Selection::Face(proved_face)) = harness.state().state.doc.selection else {
        panic!("the selected origin resolves to the same proved revision");
    };
    assert_ne!(proved_face, preview_face);
    assert!(harness.state().state.doc.any_face_ref(proved_face).is_some());
    assert!(harness.state().state.preview_body().is_none());
    save(&mut harness, "drag-preview-applied");

    harness.state_mut().state.push(AppAction::Undo);
    harness.run_steps(5);
    settle_document(&mut harness);
    assert_eq!(plate_depth(&harness), 10.0);
}

#[test]
fn impossible_fillet_edit_restores_the_proved_shape_and_blocks_export() {
    if !gpu_available() {
        eprintln!("no GPU adapter; skipping");
        return;
    }
    let config = std::env::temp_dir().join(format!("soliddraft-failed-edit-interface-{}", std::process::id()));
    std::fs::create_dir_all(&config).unwrap();
    soliddraft_app::file::set_config_dir(Some(config));
    let mut doc = Document::new("Fillet edit", soliddraft_app::kernel());
    doc.apply(Command::AddExtrude {
        params: ExtrudeParams {
            profile: ProfileKind::Rectangle { width: 40.0, height: 30.0 },
            depth: 10.0,
            ..Default::default()
        },
        via_assistant: false,
    })
    .unwrap();
    let edge = doc.edges.iter().find(|edge| (edge.midpoint.z - 10.0).abs() < 1e-6).unwrap();
    let reference = doc.edge_ref(edge.id).unwrap();
    let fillet = doc
        .apply(Command::AddFillet { params: FilletParams { edges: vec![reference], radius: 1.0 } })
        .unwrap()
        .unwrap();
    let proved_body = doc.display_body();
    let proved_revision = doc.revision();
    doc.selection = Some(Selection::Feature(fillet));
    let mut harness = Harness::builder()
        .with_size(vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .with_render_every_step(true)
        .wgpu()
        .build_eframe(|cc| SolidDraftApp::new(cc, doc, Automation::default()));
    harness.run_steps(10);
    let undo_before = harness.state().state.doc.undo_depth();
    assert_eq!(harness.state().state.doc.revision(), proved_revision);
    save(&mut harness, "edit-proof-before-failure");
    harness.get_by_label("Radius value").click();
    harness.run_steps(2);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.get_by_label("Radius value").type_text("50");
    harness.run_steps(2);
    assert_eq!(harness.state().state.doc.revision(), proved_revision, "typing alone does not author an edit");
    assert_eq!(harness.state().state.doc.undo_depth(), undo_before);
    harness.key_press(Key::Enter);
    harness.run_steps(5);
    settle_document(&mut harness);
    let state = &harness.state().state;
    assert!(matches!(&state.doc.feature(fillet).unwrap().status, FeatureStatus::Error(_)));
    assert_eq!(state.doc.display_body(), proved_body);
    assert_eq!(state.doc.displayed_revision(), proved_revision);
    assert!(state.export_part(soliddraft_exchange::Format::Step).is_err());
    assert_eq!(state.doc.undo_depth(), undo_before + 1);
    save(&mut harness, "edit-proof-failed-snapback");
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    harness.run_steps(5);
    settle_document(&mut harness);
    assert_eq!(harness.state().state.doc.display_body(), proved_body);
    assert_eq!(harness.state().state.doc.feature(fillet).unwrap().status, FeatureStatus::Ok);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Y);
    harness.run_steps(5);
    settle_document(&mut harness);
    assert_eq!(harness.state().state.doc.display_body(), proved_body);
    assert!(harness.state().state.export_part(soliddraft_exchange::Format::Step).is_err());
}
