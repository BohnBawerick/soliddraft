//! The viewport's display choices in `settings.json`: the `view` section's keys, how each
//! falls back on its own, and that saving one thing keeps everything else.
//!
//! The design is `sdc-d-ux-viewport`, the unit-level checks and merge A.

#[path = "support/viewport_acceptance.rs"]
mod support;

use serde_json::json;
use soliddraft_app::settings::{self, InstallKind, ReferenceDisplay};
use soliddraft_app::viewport::look::Preset;
use support::Case;

const DEFAULTS: ReferenceDisplay =
    ReferenceDisplay { show_grid: true, show_origin_planes: true, always_show_origin_axes: false };

/// Reads the settings and checks that reading wrote nothing.
fn view_unchanged(case: &Case) -> settings::ViewSettings {
    let before = std::fs::read(case.settings_file()).ok();
    let view = settings::view();
    assert_eq!(std::fs::read(case.settings_file()).ok(), before, "reading leaves the file as it was");
    view
}

#[test]
fn an_absent_or_legacy_view_section_gets_the_defaults() {
    let case = Case::settings("view_absent_or_legacy");
    let view = view_unchanged(&case);
    assert_eq!((view.preset, view.references), (Preset::Studio, DEFAULTS), "no file");
    assert!(!case.settings_file().exists(), "reading does not create the file");

    case.seed_settings(r#"{"theme":"dark"}"#);
    let view = view_unchanged(&case);
    assert_eq!((view.preset, view.references), (Preset::Studio, DEFAULTS), "no view section");

    case.seed_settings(r#"{"view":{"preset":"realistic"}}"#);
    let view = view_unchanged(&case);
    assert_eq!((view.preset, view.references), (Preset::Realistic, DEFAULTS), "a file from before the keys");

    case.seed_settings(r#"{"view":"not an object"}"#);
    let view = view_unchanged(&case);
    assert_eq!((view.preset, view.references), (Preset::Studio, DEFAULTS), "an unreadable section");
}

#[test]
fn a_missing_show_grid_falls_back_alone() {
    let case = Case::settings("view_missing_show_grid");
    case.seed_settings(r#"{"view":{"preset":"fast","show_origin_planes":false,"always_show_origin_axes":true}}"#);
    let view = view_unchanged(&case);
    let expected = ReferenceDisplay { show_grid: true, show_origin_planes: false, always_show_origin_axes: true };
    assert_eq!((view.preset, view.references), (Preset::Fast, expected));
}

#[test]
fn a_missing_show_origin_planes_falls_back_alone() {
    let case = Case::settings("view_missing_show_origin_planes");
    case.seed_settings(r#"{"view":{"preset":"fast","show_grid":false,"always_show_origin_axes":true}}"#);
    let view = view_unchanged(&case);
    let expected = ReferenceDisplay { show_grid: false, show_origin_planes: true, always_show_origin_axes: true };
    assert_eq!((view.preset, view.references), (Preset::Fast, expected));
}

#[test]
fn a_missing_always_show_origin_axes_falls_back_alone() {
    let case = Case::settings("view_missing_always_show_origin_axes");
    case.seed_settings(r#"{"view":{"preset":"fast","show_grid":false,"show_origin_planes":false}}"#);
    let view = view_unchanged(&case);
    let expected = ReferenceDisplay { show_grid: false, show_origin_planes: false, always_show_origin_axes: false };
    assert_eq!((view.preset, view.references), (Preset::Fast, expected));
}

#[test]
fn a_wrong_typed_show_grid_falls_back_alone() {
    let case = Case::settings("view_wrong_show_grid");
    case.seed_settings(
        r#"{"view":{"preset":"fast","show_grid":"no","show_origin_planes":false,"always_show_origin_axes":true}}"#,
    );
    let view = view_unchanged(&case);
    let expected = ReferenceDisplay { show_grid: true, show_origin_planes: false, always_show_origin_axes: true };
    assert_eq!((view.preset, view.references), (Preset::Fast, expected));
}

#[test]
fn a_wrong_typed_show_origin_planes_falls_back_alone() {
    let case = Case::settings("view_wrong_show_origin_planes");
    case.seed_settings(
        r#"{"view":{"preset":"fast","show_grid":false,"show_origin_planes":0,"always_show_origin_axes":true}}"#,
    );
    let view = view_unchanged(&case);
    let expected = ReferenceDisplay { show_grid: false, show_origin_planes: true, always_show_origin_axes: true };
    assert_eq!((view.preset, view.references), (Preset::Fast, expected));
}

#[test]
fn a_wrong_typed_always_show_origin_axes_falls_back_alone() {
    let case = Case::settings("view_wrong_always_show_origin_axes");
    case.seed_settings(
        r#"{"view":{"preset":"fast","show_grid":false,"show_origin_planes":false,"always_show_origin_axes":"yes"}}"#,
    );
    let view = view_unchanged(&case);
    let expected = ReferenceDisplay { show_grid: false, show_origin_planes: false, always_show_origin_axes: false };
    assert_eq!((view.preset, view.references), (Preset::Fast, expected));
}

#[test]
fn an_unknown_look_falls_back_without_costing_a_display_choice() {
    let case = Case::settings("view_unknown_look");
    case.seed_settings(r#"{"view":{"preset":"cinema","always_show_origin_axes":true}}"#);
    let view = view_unchanged(&case);
    assert_eq!(view.preset, Preset::Studio);
    assert!(view.references.always_show_origin_axes);
}

#[test]
fn look_and_display_writes_preserve_each_other() {
    let case = Case::settings("look_and_display_writes");
    let navigation = json!({"version": 99, "preset_id": "from_the_future", "future": "keep"});
    let seed = json!({
        "view": {
            "preset": "studio",
            "show_grid": false,
            "show_origin_planes": false,
            "always_show_origin_axes": true,
            "future": {"nested": [1, 2]},
        },
        "navigation": navigation,
        "assistant": {"provider": "openrouter"},
        "appearance": {"theme": "dark"},
        "documents": {"autosave": {"interval_seconds": 30}},
        "command_line": {"session_height": 410.0},
        "unknown_section": [true],
    });
    case.seed_settings(&seed.to_string());

    // Changing the look keeps the display choices.
    settings::save_default_preset(Preset::Realistic).unwrap();
    let mut expected = seed.clone();
    expected["view"]["preset"] = json!("realistic");
    assert_eq!(case.settings_json(), expected, "only the look changed");
    let view = view_unchanged(&case);
    assert_eq!(view.preset, Preset::Realistic);
    let seeded = ReferenceDisplay { show_grid: false, show_origin_planes: false, always_show_origin_axes: true };
    assert_eq!(view.references, seeded);

    // Changing a display choice keeps the look and everything foreign.
    settings::save_reference_display(&ReferenceDisplay { always_show_origin_axes: false, ..seeded }).unwrap();
    expected["view"]["always_show_origin_axes"] = json!(false);
    assert_eq!(case.settings_json(), expected, "only the display choice changed");

    // And back the other way.
    settings::save_default_preset(Preset::Fast).unwrap();
    settings::save_reference_display(&seeded).unwrap();
    expected["view"]["preset"] = json!("fast");
    expected["view"]["always_show_origin_axes"] = json!(true);
    assert_eq!(case.settings_json(), expected, "interleaved writes keep each other's keys");
}

#[test]
fn the_first_display_write_of_a_new_install_makes_the_view_section() {
    let case = Case::settings("first_display_write");
    let chosen = ReferenceDisplay { show_grid: false, ..DEFAULTS };
    settings::save_reference_display(&chosen).unwrap();
    assert_eq!(view_unchanged(&case).references, chosen);
    assert_eq!(case.settings_json()["view"]["show_grid"], json!(false));
}

#[test]
fn navigation_and_view_writes_keep_each_other() {
    let case = Case::settings("navigation_and_view_writes");
    case.seed_settings(
        r#"{"view":{"preset":"studio","show_grid":false,"future":1},"navigation":{"preset_id":"onshape","future":"keep"}}"#,
    );

    let mut navigation = settings::navigation(InstallKind::Existing);
    navigation.select_preset("solidworks").unwrap();
    settings::save_navigation(&navigation).unwrap();
    let saved = case.settings_json();
    assert_eq!(saved["navigation"]["preset_id"], json!("solidworks"));
    assert_eq!(saved["navigation"]["future"], json!("keep"));
    assert_eq!(saved["view"], json!({"preset":"studio","show_grid":false,"future":1}), "a navigation write");

    settings::save_reference_display(&ReferenceDisplay { show_grid: true, ..DEFAULTS }).unwrap();
    settings::save_default_preset(Preset::Fast).unwrap();
    let written = case.settings_json();
    assert_eq!(written["navigation"], saved["navigation"], "display and look writes");
    assert_eq!(written["view"]["future"], json!(1));
    assert_eq!(written["view"]["preset"], json!("fast"));
    assert_eq!(written["view"]["show_grid"], json!(true));
}

#[test]
fn a_display_write_does_not_stop_at_a_newer_navigation_section() {
    let case = Case::settings("newer_navigation");
    let navigation = json!({"version": 99, "preset_id": "from_the_future", "future": {"keep": [1]}});
    case.seed_settings(&json!({ "navigation": navigation }).to_string());
    settings::capture_install_kind();
    settings::save_reference_display(&ReferenceDisplay { always_show_origin_axes: true, ..DEFAULTS }).unwrap();
    let written = case.settings_json();
    assert_eq!(written["navigation"], navigation, "exactly as it was");
    assert_eq!(written["view"]["always_show_origin_axes"], json!(true));
}

#[test]
fn a_new_install_that_writes_display_first_keeps_its_starting_preset() {
    let case = Case::settings("new_install_display_first");
    assert_eq!(settings::capture_install_kind(), InstallKind::New);
    settings::save_reference_display(&ReferenceDisplay { show_grid: false, ..DEFAULTS }).unwrap();

    // The next start finds a file, so it detects an existing install.
    let restarted = settings::capture_install_kind();
    assert_eq!(restarted, InstallKind::Existing);
    assert_eq!(
        settings::navigation(restarted).active_preset().id,
        soliddraft_app::navigation::default_preset(InstallKind::New)
    );
    assert!(!view_unchanged(&case).references.show_grid);
}
