//! The render looks, driven through egui_kittest against the real wgpu pipeline: the
//! sample bracket in Fast, Studio and Realistic, its boss in a red anodised
//! appearance, with hover picking checked in each.
//! Frames are written to `target/exit-test/look-<preset>.png` for review.
//!
//! `WGPU_BACKEND=gl` runs the same frames on the WebGL2 tier (OpenGL, no occlusion
//! pass); the files then end in `-gl`.
//!
//! Skips itself when no GPU adapter is available (plain CI runners).

use egui::{Key, vec2};
use egui_kittest::Harness;
use soliddraft_app::app::{Automation, SolidDraftApp};
use soliddraft_app::state::AppAction;
use soliddraft_app::viewport::look::Preset;
use soliddraft_app::viewport::render::{DisplayMode, Picked};
use soliddraft_materials::{Appearance, Srgb};
use soliddraft_model::AppearanceTarget;
use std::path::PathBuf;

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

/// Mean brightness of a 7 by 7 pixel patch, 0 to 255.
fn brightness(image: &image::RgbaImage, at: egui::Pos2) -> f64 {
    let mut sum = 0.0;
    for y in at.y as u32 - 3..=at.y as u32 + 3 {
        for x in at.x as u32 - 3..=at.x as u32 + 3 {
            let p = image.get_pixel(x, y);
            sum += (p[0] as f64 + p[1] as f64 + p[2] as f64) / 3.0;
        }
    }
    sum / 49.0
}

#[test]
fn the_bracket_in_each_look() {
    if !gpu_available() {
        return;
    }
    let suffix = match std::env::var("WGPU_BACKEND").as_deref() {
        Ok("gl") | Ok("opengl") => "-gl",
        _ => "",
    };
    let mut harness = Harness::builder()
        .with_size(vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .with_render_every_step(true)
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_eframe(|cc| {
            SolidDraftApp::new(
                cc,
                soliddraft_app::command_tools::board_bracket_on(std::sync::Arc::new(
                    soliddraft_kernel_smlib::SmlibKernel::new(),
                )),
                Automation::default(),
            )
        });
    assert_eq!(harness.state().state.viewport.preset, Preset::Studio, "Studio is the default look");
    {
        // Red anodised on every face the boss made, through the part's own assignments.
        let doc = &mut harness.state_mut().state.doc;
        let boss = doc.features.iter().find(|f| f.name == "Boss-Extrude3").expect("the boss").id;
        let red = Appearance {
            metallic: 1.0,
            roughness: 0.35,
            ..Appearance::new("anodised-red", "Anodised red", Srgb::new(196, 48, 40))
        };
        let target = AppearanceTarget::Feature { feature: soliddraft_model::FeatureId(boss as u32) };
        let triangles = doc.meshes.values().map(|m| m.triangle_count()).sum::<usize>();
        doc.set_appearance(target, Some(red)).expect("the boss takes an appearance");
        assert_eq!(doc.meshes.values().map(|m| m.triangle_count()).sum::<usize>(), triangles, "same geometry");
        assert_eq!(doc.undo_label(), Some("set appearance Anodised red on Boss-Extrude3"));
        let shown =
            |doc: &soliddraft_app::document::Document| doc.meshes.values().map(|m| m.appearances.len()).sum::<usize>();
        assert_eq!(shown(doc), 2, "the part's grey and the boss's red");
        doc.undo().expect("undo");
        assert_eq!(shown(doc), 1, "undo takes the red away");
        doc.redo().expect("redo");
        assert_eq!(shown(doc), 2);
    }
    harness.key_press(Key::I);
    harness.run_steps(30);
    harness.key_press(Key::Z);
    harness.run_steps(30);

    let rect = harness.state().state.viewport.rect;
    let mut frames = Vec::new();
    for preset in Preset::ALL {
        harness.state_mut().state.push(AppAction::SetPreset(preset));
        harness.run_steps(4);
        let display = harness.state().state.viewport.display;
        assert_eq!(display == DisplayMode::ShadedWithEdges, preset.shows_edges(), "{preset:?} shows {display:?}");
        // Hover the boss's top face: the id buffer still names it in every look.
        let boss = harness.state().state.viewport.camera.project(glam::Vec3::new(85.0, 55.0, 34.0), rect);
        let boss = boss.expect("the boss is in view") + vec2(9.0, 0.0);
        harness.hover_at(boss);
        harness.run_steps(6);
        let hovered = harness.state().state.viewport.hover;
        assert!(matches!(hovered, Some(Picked::Face(_))), "{preset:?}: hover found {hovered:?}");
        harness.hover_at(rect.left_bottom() + vec2(30.0, -140.0));
        harness.run_steps(4);
        let image = harness.render().expect("render");
        let path = out_dir().join(format!("look-{}{suffix}.png", preset.label().to_lowercase()));
        image.save(&path).expect("save png");
        eprintln!("wrote {}", path.display());
        // Two spots on the base plate's top: one in the boss's shadow (the fixed light
        // is up and to the back left, so shadows fall forward and right) and one in the
        // open. Fast has no shadows, so both read the same there.
        let camera = &harness.state().state.viewport.camera;
        let spot = |x: f32, y: f32| camera.project(glam::Vec3::new(x, y, 10.0), rect).expect("in view");
        let shade = brightness(&image, spot(100.2, 42.3));
        let open = brightness(&image, spot(110.0, 80.0));
        frames.push(shade / open);
        // The boss's top shows its own colour.
        let top = camera.project(glam::Vec3::new(85.0 + 9.0, 55.0, 34.0), rect).expect("in view");
        let p = image.get_pixel(top.x as u32, top.y as u32);
        assert!(p[0] as i32 > p[1] as i32 + 30, "{preset:?}: the boss is red, got {p:?}");
    }
    let (fast, studio, realistic) = (frames[0], frames[1], frames[2]);
    assert!(fast > 0.95, "Fast casts no shadow: {fast}");
    assert!(studio < fast - 0.1, "the boss shades the plate in Studio: {studio} against {fast}");
    assert!(realistic < studio, "Realistic's shadow is darker than Studio's: {realistic} against {studio}");
}
