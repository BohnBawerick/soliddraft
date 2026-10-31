//! The Studio look in both modes, driven through egui_kittest against the real wgpu
//! pipeline: the board bracket in light and in dark, with the chrome, the panels and
//! the modelling space read back from the frame. Frames are written to
//! `target/exit-test/theme-<mode>.png` for review.
//!
//! The mode is process-wide (`theme::set_mode`), so this file holds the only test that
//! switches it, and runs light then dark in one test.
//!
//! Skips itself when no GPU adapter is available (plain CI runners).

use egui::{Key, vec2};
use egui_kittest::Harness;
use soliddraft_app::app::{Automation, SolidDraftApp};
use soliddraft_app::state::AppAction;
use soliddraft_app::theme::{self, Mode};
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
fn the_bracket_in_light_and_dark() {
    if !gpu_available() {
        return;
    }
    // Choosing a theme saves it, so the test keeps its settings away from the user's.
    let config = std::env::temp_dir().join(format!("soliddraft-theme-test-{}", std::process::id()));
    std::fs::create_dir_all(&config).unwrap();
    // The samples below read the backdrop, which the origin planes would tint.
    std::fs::write(config.join("settings.json"), r#"{"view":{"show_origin_planes":false}}"#).unwrap();
    soliddraft_app::file::set_config_dir(Some(config.clone()));
    let mut harness = Harness::builder()
        .with_size(vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .with_render_every_step(true)
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_eframe(|cc| {
            SolidDraftApp::new(cc, soliddraft_app::command_tools::board_bracket(), Automation::default())
        });
    assert_eq!(theme::mode(), Mode::Light, "light is the default mode");
    harness.key_press(Key::I);
    harness.run_steps(30);
    harness.key_press(Key::Z);
    harness.run_steps(30);

    let rect = harness.state().state.viewport.rect;
    for mode in Mode::ALL {
        harness.state_mut().state.push(AppAction::SetTheme(mode));
        harness.run_steps(6);
        assert_eq!(theme::mode(), mode);
        let image = harness.render().expect("render");
        let path = out_dir().join(format!("theme-{}.png", mode.label().to_lowercase()));
        image.save(&path).expect("save png");
        eprintln!("wrote {}", path.display());

        // The modelling space fades from near the panel tone at the top to a grey floor
        // at the bottom, and the floor is clearly not the chrome. The samples sit right of
        // the heads-up toolbar and between the two bottom pills, clear of the origin axes.
        // The chrome is read from the ribbon tab strip, right of the tabs (the menu bar
        // centre holds the search field).
        let top = brightness(&image, rect.center_top() + vec2(300.0, 12.0));
        let bottom = brightness(&image, rect.center_bottom() + vec2(0.0, -12.0));
        let chrome =
            brightness(&image, egui::pos2(rect.center().x, theme::MENU_HEIGHT + theme::RIBBON_TABS_HEIGHT / 2.0));
        let panel = brightness(&image, egui::pos2(40.0, rect.center().y + 120.0));
        eprintln!("{mode:?}: viewport top {top:.0} bottom {bottom:.0}, chrome {chrome:.0}, panel {panel:.0}");
        assert!(top - bottom > 25.0, "{mode:?}: a long fade, top {top:.0} bottom {bottom:.0}");
        assert!(chrome > bottom, "{mode:?}: the floor is under the chrome, {chrome:.0} and {bottom:.0}");
        assert!(panel > chrome + 3.0, "{mode:?}: the panels sit off the chrome, {panel:.0} and {chrome:.0}");
        match mode {
            Mode::Light => {
                assert!(panel > 230.0 && chrome > 220.0, "{mode:?}: light chrome {chrome:.0} panel {panel:.0}")
            }
            Mode::Dark => assert!(panel < 80.0 && chrome < 80.0, "{mode:?}: dark chrome {chrome:.0} panel {panel:.0}"),
        }
    }
    harness.state_mut().state.push(AppAction::SetTheme(Mode::Light));
    harness.run_steps(2);
    let saved = std::fs::read_to_string(config.join("settings.json")).expect("the choice was saved");
    assert!(saved.contains("\"theme\": \"light\""), "the last choice is in the file: {saved}");
    let _ = std::fs::remove_dir_all(config);
}
