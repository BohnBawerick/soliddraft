//! The real window shared by the V2 batch 2 G1 and G2 tests: actual egui input on the
//! wgpu viewport, with targets projected through the current camera.

#![allow(dead_code)]

use egui::{Pos2, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use glam::Vec3;
use soliddraft_app::{
    app::{Automation, SolidDraftApp},
    document::Document,
    sketch::P2,
    sketcher::Frame,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

/// The one hang guard: a wait this long means stuck, not slow.
pub const HANG: Duration = Duration::from_secs(600);

pub type App<'a> = Harness<'a, SolidDraftApp>;

/// A window on `doc`, or `None` when no adapter exists and none is required. With
/// `SOLIDDRAFT_REQUIRE_GPU` set, a missing adapter fails rather than skips.
pub fn window<'a>(doc: Document, name: &str) -> Option<App<'a>> {
    let instance = egui_wgpu::wgpu::Instance::default();
    let adapters = pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all()));
    if adapters.is_empty() {
        assert!(std::env::var_os("SOLIDDRAFT_REQUIRE_GPU").is_none(), "required GPU unavailable");
        eprintln!("no GPU adapter; skipping");
        return None;
    }
    let config = std::env::temp_dir().join(format!("soliddraft-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&config).unwrap();
    soliddraft_app::file::set_config_dir(Some(config));
    let mut h = Harness::builder()
        .with_size(vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .with_render_every_step(true)
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_eframe(|cc| SolidDraftApp::new(cc, doc, Automation::default()));
    settle(&mut h);
    Some(h)
}

/// Runs frames until no proof is pending.
pub fn settle(h: &mut App<'_>) {
    h.run_steps(5);
    let until = Instant::now() + HANG;
    while h.state().state.doc.proof_pending() {
        assert!(Instant::now() < until, "proof did not settle");
        std::thread::sleep(Duration::from_millis(10));
        h.run_steps(1);
    }
    h.run_steps(3);
}

/// Rests the pointer at `pos` long enough for the id-buffer readback to catch up.
pub fn hover(h: &mut App<'_>, pos: Pos2) {
    h.hover_at(pos);
    h.run_steps(4);
}

pub fn click_at(h: &mut App<'_>, pos: Pos2) {
    hover(h, pos);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        });
        h.run_steps(2);
    }
}

/// A part-frame point on screen, through the current camera.
pub fn screen_of(h: &App<'_>, world: Vec3) -> Pos2 {
    let st = &h.state().state;
    st.viewport.camera.project(world, st.viewport.rect).unwrap()
}

/// A point of the open sketch's plane on screen.
pub fn sketch_screen(h: &App<'_>, p: P2) -> Pos2 {
    let st = &h.state().state;
    let s = st.sketch.as_ref().expect("a sketch is open");
    Frame { plane: s.plane, camera: &st.viewport.camera, rect: st.viewport.rect }.screen(p).unwrap()
}

pub fn button(h: &mut App<'_>, name: &str) {
    h.get_all_by_role_and_label(egui::accesskit::Role::Button, name).next().unwrap().click();
    h.run_steps(3);
}

/// Whether some widget carries exactly this label.
pub fn shows(h: &App<'_>, label: &str) -> bool {
    h.query_all_by_label(label).next().is_some()
}

pub fn snapshot(h: &mut App<'_>, name: &str) {
    let dir = std::env::var_os("SOLIDDRAFT_MEASURE_PROOF_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/exit-test"));
    std::fs::create_dir_all(&dir).unwrap();
    h.render().unwrap().save(dir.join(format!("measure-{name}.png"))).unwrap();
}
