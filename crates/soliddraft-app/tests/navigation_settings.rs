//! The navigation section of `settings.json`: it round-trips beside foreign keys, keeps
//! each preset's own overrides, is user settings rather than document state, starts new
//! installations on Fusion and existing ones on SolidDraft's original controls, and
//! leaves a newer build's section as it found it.

use serde_json::{Value, json};
use soliddraft_app::document::Selection;
use soliddraft_app::navigation::{
    self, Action, Button, CameraSettings, Chord, EventKind, Mods, NavigationSettings, Pivot, SaveError, VERSION,
    WheelSettings, ZoomAnchor,
};
use soliddraft_app::settings::{self, InstallKind};
use soliddraft_app::state::AppState;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

static CONFIG_LOCK: Mutex<()> = Mutex::new(());

/// Points the config directory at an empty directory of its own for the test.
fn config_dir(name: &str) -> (MutexGuard<'static, ()>, PathBuf) {
    let guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("soliddraft-nav-settings-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    soliddraft_app::file::set_config_dir(Some(dir.clone()));
    (guard, dir)
}

fn write(dir: &Path, text: &str) {
    std::fs::write(dir.join("settings.json"), text).unwrap();
}

fn file_json(dir: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap()).unwrap()
}

fn bytes(dir: &Path) -> Vec<u8> {
    std::fs::read(dir.join("settings.json")).unwrap()
}

fn drag(buttons: &[Button], required: Mods) -> Chord {
    Chord { event: EventKind::Drag, buttons: buttons.to_vec(), required, ..Default::default() }
}

/// Every field set away from its default.
fn populated() -> NavigationSettings {
    let mut s = NavigationSettings::new(InstallKind::Existing);
    s.select_preset("solidworks").unwrap();
    s.set_override("solidworks", "orbit", drag(&[Button::Right], Mods::NONE)).unwrap();
    s.set_override("fusion", "pan", drag(&[Button::Right], Mods::NONE)).unwrap();
    s.wheel = WheelSettings { invert: true, speed: 1.5 };
    s.keyboard_profile = "solidworks".into();
    s.trackpad = "fusion_mac".into();
    s.camera = CameraSettings {
        orbit_sensitivity: 2.0,
        pan_sensitivity: 0.5,
        pivot: Pivot::Target,
        zoom_anchor: ZoomAnchor::Center,
        transition_seconds: 0.5,
    };
    s
}

#[test]
fn navigation_settings_round_trip_keeps_foreign_keys() {
    let (_guard, dir) = config_dir("round-trip");
    write(&dir, r#"{"theme":"dark","future_top":{"x":1}}"#);
    let s = populated();
    settings::save_navigation(&s).unwrap();
    assert_eq!(settings::navigation(InstallKind::Existing), s, "every field reads back");
    let all = file_json(&dir);
    assert_eq!(all["theme"], "dark");
    assert_eq!(all["future_top"], json!({"x": 1}));
    let nav = &all["navigation"];
    assert_eq!(nav["version"], VERSION);
    assert_eq!(nav["preset_id"], "solidworks", "a stable string id, not an index");
    assert_eq!(nav["keyboard_profile"], "solidworks");
    assert!(nav["overrides"].is_object() && nav["overrides"]["solidworks"]["orbit"].is_object(), "{nav}");
    assert_eq!(nav["overrides"]["solidworks"]["orbit"]["buttons"], json!(["right"]));
    assert_eq!(nav["overrides"]["fusion"]["pan"]["buttons"], json!(["right"]));

    let mut wheel_only = settings::navigation(InstallKind::Existing);
    wheel_only.wheel.invert = false;
    settings::save_navigation(&wheel_only).unwrap();
    let back = settings::navigation(InstallKind::Existing);
    let mut want = s.clone();
    want.wheel.invert = false;
    assert_eq!(back, want, "only the wheel changed");
    let all = file_json(&dir);
    assert_eq!(all["theme"], "dark");
    assert_eq!(all["future_top"], json!({"x": 1}));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn overrides_are_scoped_to_their_preset() {
    let (_guard, dir) = config_dir("overrides");
    let orbit = drag(&[Button::Right], Mods::NONE);
    let pan = drag(&[Button::Right], Mods::NONE);
    let mut s = NavigationSettings::new(InstallKind::Existing);
    s.select_preset("solidworks").unwrap();
    s.set_override("solidworks", "orbit", orbit.clone()).unwrap();
    s.set_override("fusion", "pan", pan.clone()).unwrap();
    s.wheel.invert = true;
    s.keyboard_profile = "onshape".into();
    s.camera.orbit_sensitivity = 3.0;
    settings::save_navigation(&s).unwrap();

    let mut s = settings::navigation(InstallKind::Existing);
    s.select_preset("onshape").unwrap();
    settings::save_navigation(&s).unwrap();
    let s = settings::navigation(InstallKind::Existing);
    assert_eq!(s.active_preset().id, "onshape");
    assert_eq!(s.overrides["solidworks"]["orbit"], orbit, "solidworks keeps its override");
    assert_eq!(s.overrides["fusion"]["pan"], pan, "fusion keeps its override");
    let onshape = navigation::preset("onshape").unwrap();
    assert_eq!(&s.effective(s.active_preset()), onshape, "only onshape's own rows are active");

    let mut s = s;
    s.select_preset("solidworks").unwrap();
    settings::save_navigation(&s).unwrap();
    let s = settings::navigation(InstallKind::Existing);
    let active = s.effective(s.active_preset());
    assert_eq!(active.binding("orbit").unwrap().buttons, [Button::Right], "the override is active again");
    assert_eq!(active.binding("orbit").unwrap().action, Action::Orbit);

    navigation::reset_preset("solidworks").unwrap();
    let after = settings::navigation(InstallKind::Existing);
    assert!(!after.overrides.contains_key("solidworks"), "solidworks' overrides are gone");
    assert_eq!(after.overrides["fusion"]["pan"], pan, "fusion's override stays");
    assert_eq!(after.preset_id, "solidworks", "the active preset stays");
    assert_eq!(after.wheel, s.wheel);
    assert_eq!(after.keyboard_profile, "onshape");
    assert_eq!(after.camera, s.camera);
    assert_eq!(&after.effective(after.active_preset()), navigation::preset("solidworks").unwrap());

    // An override that repeats another row's gesture is refused and changes nothing.
    let mut s = after.clone();
    let refused = s.set_override("solidworks", "orbit", drag(&[Button::Middle], Mods::CTRL));
    assert!(matches!(refused, Err(navigation::Refusal::Invalid(_))), "{refused:?}");
    assert_eq!(s, after);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn navigation_save_does_not_touch_document_or_undo() {
    // Both installs, each started deliberately: an existing settings file keeps SolidDraft's
    // original controls, a fresh directory starts on Fusion.
    for (name, start, kind, preset) in [
        ("document-existing", Some(r#"{"theme":"dark"}"#), InstallKind::Existing, "legacy_soliddraft"),
        ("document-new", None, InstallKind::New, "fusion"),
    ] {
        let (_guard, dir) = config_dir(name);
        if let Some(text) = start {
            write(&dir, text);
        }
        assert_eq!(settings::capture_install_kind(), kind, "{name}: the app starts");
        let mut state = AppState::new(soliddraft_app::demo_document());
        assert_eq!(state.navigation.active_preset().id, preset, "{name}");
        let feature = state.doc.features[0].id;
        state.doc.selection = Some(Selection::Feature(feature));
        let part = dir.join("part.dwprt");
        soliddraft_app::file::save(&mut state.doc, &part).unwrap();
        let saved = std::fs::read(&part).unwrap();
        let hash = state.doc.model().semantic_hash();
        let (revision, depth, selection, dirty) =
            (state.doc.revision(), state.doc.undo_depth(), state.doc.selection, state.doc.dirty);
        assert!(depth > 0, "the demo has undo steps");

        let mut through_settings = settings::navigation(kind);
        through_settings.select_preset("blender").unwrap();
        settings::save_navigation(&through_settings).unwrap();
        let mut through_app = state.navigation.clone();
        through_app.select_preset("onshape").unwrap();
        through_app.wheel.invert = true;
        state.set_navigation(through_app.clone()).unwrap();
        assert_eq!(state.navigation, through_app, "{name}: the app uses the new settings");
        assert_eq!(settings::navigation(settings::install_kind()), through_app, "{name}: and saved them");

        assert_eq!(state.doc.revision(), revision);
        assert_eq!(state.doc.undo_depth(), depth);
        assert_eq!(state.doc.selection, selection);
        assert_eq!(state.doc.dirty, dirty);
        assert_eq!(state.doc.model().semantic_hash(), hash);
        soliddraft_app::file::save(&mut state.doc, &part).unwrap();
        assert_eq!(std::fs::read(&part).unwrap(), saved, "{name}: the part file is the same");
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn new_install_gets_fusion_existing_install_gets_legacy() {
    let (_guard, dir) = config_dir("install");
    assert_eq!(InstallKind::detect(), InstallKind::New);
    assert_eq!(settings::navigation(InstallKind::New).active_preset().id, "fusion");
    assert!(!dir.join("settings.json").exists(), "reading creates no file");

    let check = |text: &str, new: &str, existing: &str| {
        write(&dir, text);
        let before = bytes(&dir);
        assert_eq!(settings::navigation(InstallKind::New).active_preset().id, new, "new install, {text}");
        assert_eq!(settings::navigation(InstallKind::Existing).active_preset().id, existing, "existing, {text}");
        assert_eq!(bytes(&dir), before, "reading rewrites nothing: {text}");
    };
    check(r#"{"theme":"dark"}"#, "fusion", "legacy_soliddraft");
    assert_eq!(InstallKind::detect(), InstallKind::Existing);
    check(r#"{"navigation":{"preset_id":"onshape"}}"#, "onshape", "onshape");
    check(r#"{"navigation":{"version":1}}"#, "fusion", "legacy_soliddraft");
    check(r#"{"navigation":{"preset_id":"not_a_preset"}}"#, "fusion", "legacy_soliddraft");
    check(r#"{"navigation":"oops","theme":"dark"}"#, "fusion", "legacy_soliddraft");
    check(r#"{"navigation":[1,2],"theme":"dark"}"#, "fusion", "legacy_soliddraft");
    check(r#"{"navigation":{"preset_id":7,"wheel":"loud"}}"#, "fusion", "legacy_soliddraft");

    // An unknown preset id survives the next save.
    write(&dir, r#"{"navigation":{"preset_id":"not_a_preset"},"theme":"dark"}"#);
    let mut s = settings::navigation(InstallKind::Existing);
    assert_eq!(s.preset_id, "not_a_preset");
    s.wheel.invert = true;
    settings::save_navigation(&s).unwrap();
    let all = file_json(&dir);
    assert_eq!(all["navigation"]["preset_id"], "not_a_preset");
    assert_eq!(all["navigation"]["wheel"]["invert"], true);
    assert_eq!(all["theme"], "dark");

    // A damaged section gives the default and keeps the other sections.
    write(&dir, r#"{"navigation":"oops","theme":"dark","layout":{"width":300}}"#);
    let mut s = settings::navigation(InstallKind::Existing);
    s.camera.pan_sensitivity = 2.0;
    settings::save_navigation(&s).unwrap();
    let all = file_json(&dir);
    assert_eq!(all["navigation"]["preset_id"], "legacy_soliddraft");
    assert_eq!(all["theme"], "dark");
    assert_eq!(all["layout"], json!({"width": 300}));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn install_kind_is_captured_before_any_settings_write() {
    use soliddraft_app::settings::Appearance;
    use soliddraft_app::theme::Mode;
    let (_guard, dir) = config_dir("captured");
    for (start, kind, preset) in
        [(None, InstallKind::New, "fusion"), (Some(r#"{"theme":"dark"}"#), InstallKind::Existing, "legacy_soliddraft")]
    {
        let _ = std::fs::remove_file(dir.join("settings.json"));
        if let Some(text) = start {
            write(&dir, text);
        }
        assert_eq!(settings::capture_install_kind(), kind, "the app starts");
        let state = AppState::new(soliddraft_app::new_document());
        assert_eq!(state.navigation.active_preset().id, preset);
        settings::save_appearance(&Appearance { theme: Mode::Dark }).unwrap();
        let all = file_json(&dir);
        assert_eq!(all["navigation"]["preset_id"], preset, "the first write records the preset: {all}");
        assert_eq!(all["appearance"]["theme"], "dark");

        assert_eq!(settings::capture_install_kind(), InstallKind::Existing, "the app starts again");
        let state = AppState::new(soliddraft_app::new_document());
        assert_eq!(state.navigation.active_preset().id, preset, "still {preset} after a restart");
        settings::save_appearance(&Appearance { theme: Mode::Light }).unwrap();
        assert_eq!(file_json(&dir)["navigation"]["preset_id"], preset, "a later write keeps it");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn newer_navigation_section_is_preserved_and_read_only() {
    use soliddraft_app::settings::Appearance;
    use soliddraft_app::theme::Mode;
    let (_guard, dir) = config_dir("newer");
    let text = r#"{"navigation":{"version":99,"preset_id":"fusion","future":{"a":1}},"theme":"dark"}"#;
    write(&dir, text);
    let s = settings::navigation(InstallKind::Existing);
    assert_eq!(s.active_preset().id, "fusion");
    assert!(s.read_only);
    assert_eq!(s.version, 99);
    assert_eq!(settings::save_navigation(&s), Err(SaveError::NewerVersion { found: 99 }));
    assert_eq!(bytes(&dir), text.as_bytes(), "the file is untouched");
    let fresh = NavigationSettings::new(InstallKind::Existing);
    assert_eq!(settings::save_navigation(&fresh), Err(SaveError::NewerVersion { found: 99 }), "the file decides too");
    assert_eq!(bytes(&dir), text.as_bytes());
    let mut state = AppState::new(soliddraft_app::new_document());
    assert!(state.set_navigation(fresh).is_err(), "the app's path is refused as well");
    assert!(state.navigation.read_only, "and keeps what it read");

    settings::capture_install_kind();
    settings::save_appearance(&Appearance { theme: Mode::Dark }).unwrap();
    let all = file_json(&dir);
    assert_eq!(all["navigation"], json!({"version": 99, "preset_id": "fusion", "future": {"a": 1}}));
    assert_eq!(all["theme"], "dark");

    for old in [
        r#"{"navigation":{"version":0,"preset_id":"onshape","wheel":{"invert":true},"later":[1]}}"#,
        r#"{"navigation":{"preset_id":"onshape","wheel":{"invert":true},"later":[1]}}"#,
    ] {
        write(&dir, old);
        let s = settings::navigation(InstallKind::Existing);
        assert!(!s.read_only);
        settings::save_navigation(&s).unwrap();
        let nav = &file_json(&dir)["navigation"];
        assert_eq!(nav["version"], 1, "{old} migrates to version 1");
        assert_eq!(nav["preset_id"], "onshape");
        assert_eq!(nav["wheel"]["invert"], true);
        assert_eq!(nav["later"], json!([1]), "a key this build does not know is kept");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_captured_install_kind_belongs_to_its_config_dir() {
    let (guard, existing) = config_dir("captured-existing");
    write(&existing, r#"{"theme":"dark"}"#);
    assert_eq!(settings::capture_install_kind(), InstallKind::Existing);
    drop(guard);
    let (_guard, fresh) = config_dir("captured-fresh");
    assert_eq!(settings::install_kind(), InstallKind::New, "another directory is not the captured one");
    assert_eq!(AppState::new(soliddraft_app::new_document()).navigation.active_preset().id, "fusion");
    settings::save_appearance(&settings::Appearance { theme: soliddraft_app::theme::Mode::Dark }).unwrap();
    assert!(file_json(&fresh).get("navigation").is_none(), "nothing captured here, so nothing is recorded");
    let _ = std::fs::remove_dir_all(existing);
    let _ = std::fs::remove_dir_all(fresh);
}
