//! The navigation preset catalogue: exactly the nine offered presets, their rows as the
//! navigation research's recommended table gives them, a schema that rejects malformed
//! rows, and ordered-button rows the schema can express without a preset offering them.

use egui::Key;
use soliddraft_app::navigation::{
    self, Action, Binding, Button, Capability, Chord, DragAxis, EventKind, KeyLocation, LimitKind, Limitation, Mods,
    NavContext, NavKey, Preset, Refusal, SchemaError, Step, WheelPolicy, ZoomAnchor, ZoomDirection,
};
use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

static CONFIG_LOCK: Mutex<()> = Mutex::new(());

/// Points the config directory at an empty directory of its own for the test.
fn config_dir(name: &str) -> (MutexGuard<'static, ()>, PathBuf) {
    let guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("soliddraft-nav-presets-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    soliddraft_app::file::set_config_dir(Some(dir.clone()));
    (guard, dir)
}

const IDS: [&str; 9] =
    ["legacy_soliddraft", "fusion", "solidworks", "onshape", "inventor", "creo", "nx12", "solid_edge", "blender"];

#[test]
fn preset_catalogue_exposes_only_nine_and_excludes_ordered_button_profiles() {
    let ids: Vec<&str> = navigation::presets().iter().map(|p| p.id).collect();
    assert_eq!(ids, IDS, "the nine presets in display order");
    assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len(), "unique ids");
    assert_eq!(navigation::preset("freecad_cad"), None);
    assert_eq!(navigation::preset("catia_profile_experimental"), None);
    for p in navigation::presets() {
        assert!(!p.label.trim().is_empty(), "{}: a label", p.id);
        assert!(!p.release_scope.trim().is_empty(), "{}: a release scope", p.id);
        assert!(matches!(p.wheel, WheelPolicy::SoliddraftDefault | WheelPolicy::Explicit { .. }), "{}", p.id);
        assert!(!p.limitations.is_empty(), "{}: a limitations list", p.id);
        assert!(p.limitations.iter().all(|l| !l.text.trim().is_empty()), "{}: limitations say something", p.id);
    }

    let (_guard, dir) = config_dir("withheld");
    for id in ["freecad_cad", "catia_profile_experimental"] {
        match navigation::set_preset(id) {
            Err(Refusal::Withheld { id: refused, reason }) => {
                assert_eq!(refused, id);
                assert!(reason.contains("ordered button"), "{id}: {reason}");
            }
            other => panic!("{id}: expected a typed refusal, got {other:?}"),
        }
        assert!(!dir.join("settings.json").exists(), "{id}: a refusal writes nothing");
    }
    assert_eq!(navigation::set_preset("not_a_preset"), Err(Refusal::UnknownPreset("not_a_preset".into())));
    assert!(!dir.join("settings.json").exists());
    let _ = std::fs::remove_dir_all(dir);
}

/// One expected row: id, event, buttons, held keys, key, required modifiers, context,
/// action, priority.
type Row = (&'static str, EventKind, &'static [Button], &'static [Key], Option<NavKey>, Mods, NavContext, Action, u16);

const L: Button = Button::Left;
const M: Button = Button::Middle;
const R: Button = Button::Right;
const VP: NavContext = NavContext::Viewport;
const DRAG: EventKind = EventKind::Drag;
const KEY: EventKind = EventKind::Key;
const NONE: Mods = Mods::NONE;

fn key(k: Key) -> Option<NavKey> {
    Some(NavKey::main(k))
}

fn wheel_rows() -> [Row; 2] {
    [
        ("wheel", EventKind::Wheel, &[], &[], None, NONE, VP, Action::WheelZoom, 1),
        ("pinch", EventKind::Pinch, &[], &[], None, NONE, VP, Action::WheelZoom, 2),
    ]
}

fn expected(id: &str) -> Vec<Row> {
    let rows: Vec<Row> = match id {
        "legacy_soliddraft" => vec![
            ("orbit", DRAG, &[L], &[], None, NONE, NavContext::Model, Action::Orbit, 10),
            ("pan_middle", DRAG, &[M], &[], None, NONE, VP, Action::Pan, 11),
            ("pan_right", DRAG, &[R], &[], None, NONE, VP, Action::Pan, 12),
            ("pan_shift_left", DRAG, &[L], &[], None, Mods::SHIFT, VP, Action::Pan, 20),
            ("fit", KEY, &[], &[], key(Key::Z), NONE, VP, Action::Fit, 30),
            ("normal", KEY, &[], &[], key(Key::N), NONE, VP, Action::NormalTo, 31),
        ],
        "fusion" => vec![
            ("pan", DRAG, &[M], &[], None, NONE, VP, Action::Pan, 10),
            ("orbit", DRAG, &[M], &[], None, Mods::SHIFT, VP, Action::Orbit, 20),
            ("fit", KEY, &[], &[], key(Key::F6), NONE, VP, Action::Fit, 30),
        ],
        "solidworks" => vec![
            ("orbit", DRAG, &[M], &[], None, NONE, VP, Action::Orbit, 10),
            ("roll", DRAG, &[M], &[], None, Mods::ALT, VP, Action::Roll, 20),
            ("pan", DRAG, &[M], &[], None, Mods::CTRL, VP, Action::Pan, 21),
            ("zoom", DRAG, &[M], &[], None, Mods::SHIFT, VP, Action::DragZoom, 22),
            ("fit", KEY, &[], &[], key(Key::F), NONE, VP, Action::Fit, 30),
            ("normal", KEY, &[], &[], key(Key::Num8), Mods::CTRL, VP, Action::NormalTo, 31),
        ],
        "onshape" => vec![
            ("orbit", DRAG, &[R], &[], None, NONE, VP, Action::Orbit, 10),
            ("pan", DRAG, &[M], &[], None, NONE, VP, Action::Pan, 11),
            ("pan_ctrl_right", DRAG, &[R], &[], None, Mods::CTRL, VP, Action::Pan, 20),
            ("fit_double_click", EventKind::DoubleClick, &[M], &[], None, NONE, VP, Action::Fit, 30),
            ("fit", KEY, &[], &[], key(Key::F), NONE, VP, Action::Fit, 31),
            ("normal", KEY, &[], &[], key(Key::N), NONE, VP, Action::NormalToggle, 32),
        ],
        "inventor" => vec![
            ("pan", DRAG, &[M], &[], None, NONE, VP, Action::Pan, 10),
            ("orbit", DRAG, &[M], &[], None, Mods::SHIFT, VP, Action::Orbit, 20),
            ("orbit_f4", DRAG, &[L], &[Key::F4], None, NONE, VP, Action::Orbit, 21),
            ("pan_f2", DRAG, &[L], &[Key::F2], None, NONE, VP, Action::Pan, 22),
            ("zoom_f3", DRAG, &[L], &[Key::F3], None, NONE, VP, Action::DragZoom, 23),
            ("fit", KEY, &[], &[], key(Key::Home), NONE, VP, Action::Fit, 30),
            ("look_at", KEY, &[], &[], key(Key::PageUp), NONE, VP, Action::NormalTo, 31),
            ("home_view", KEY, &[], &[], key(Key::F6), NONE, VP, Action::Isometric, 32),
        ],
        "creo" => vec![
            ("orbit", DRAG, &[M], &[], None, NONE, VP, Action::Orbit, 10),
            ("pan", DRAG, &[M], &[], None, Mods::SHIFT, VP, Action::Pan, 20),
            ("zoom", DRAG, &[M], &[], None, Mods::CTRL, VP, Action::DragZoom, 21),
            ("roll", DRAG, &[M], &[], None, Mods::CTRL, VP, Action::Roll, 22),
            ("default_view", KEY, &[], &[], key(Key::D), Mods::CTRL, VP, Action::DefaultView, 30),
        ],
        "nx12" => vec![
            ("orbit", DRAG, &[M], &[], None, NONE, VP, Action::Orbit, 10),
            ("pan", DRAG, &[M], &[], None, Mods::SHIFT, VP, Action::Pan, 20),
            ("zoom", DRAG, &[M], &[], None, Mods::CTRL, VP, Action::DragZoom, 21),
            ("pan_middle_right", DRAG, &[M, R], &[], None, NONE, VP, Action::Pan, 22),
            ("zoom_left_middle", DRAG, &[L, M], &[], None, NONE, VP, Action::DragZoom, 23),
            ("normal", KEY, &[], &[], key(Key::F8), NONE, VP, Action::NormalOrNearest, 30),
            ("isometric", KEY, &[], &[], key(Key::End), NONE, VP, Action::Isometric, 31),
        ],
        "solid_edge" => vec![
            ("orbit", DRAG, &[M], &[], None, NONE, VP, Action::Orbit, 10),
            ("pan", DRAG, &[M], &[], None, Mods::SHIFT, VP, Action::Pan, 20),
            ("zoom_ctrl_right", DRAG, &[R], &[], None, Mods::CTRL, VP, Action::DragZoom, 21),
            ("pan_ctrl_shift_right", DRAG, &[R], &[], None, Mods::CTRL | Mods::SHIFT, VP, Action::Pan, 30),
            ("fit_double_click", EventKind::DoubleClick, &[M], &[], None, NONE, VP, Action::Fit, 40),
        ],
        "blender" => vec![
            ("orbit", DRAG, &[M], &[], None, NONE, VP, Action::Orbit, 10),
            ("pan", DRAG, &[M], &[], None, Mods::SHIFT, VP, Action::Pan, 20),
            ("zoom", DRAG, &[M], &[], None, Mods::CTRL, VP, Action::DragZoom, 21),
            ("fit", KEY, &[], &[], key(Key::Home), NONE, VP, Action::Fit, 30),
            ("fit_selected", KEY, &[], &[], Some(NavKey::numpad(Key::Period)), NONE, VP, Action::FitSelected, 31),
        ],
        other => panic!("no expected rows for {other}"),
    };
    rows.into_iter().chain(wheel_rows()).collect()
}

fn actual(b: &Binding) -> Row {
    let buttons: &'static [Button] = b.buttons.clone().leak();
    let held: &'static [Key] = b.held_keys.iter().map(|k| k.key).collect::<Vec<_>>().leak();
    (b.id, b.event, buttons, held, b.key, b.required, b.context, b.action, b.priority)
}

#[test]
fn preset_rows_match_the_recommended_table() {
    for p in navigation::presets() {
        let want = expected(p.id);
        let got: Vec<Row> = p.bindings.iter().map(actual).collect();
        assert_eq!(got.len(), want.len(), "{}: {got:#?}", p.id);
        for row in &want {
            assert!(got.contains(row), "{}: missing {row:?}; rows are {got:#?}", p.id);
        }
        for b in &p.bindings {
            let extras = b.allow_extra_modifiers;
            if matches!(b.event, EventKind::Drag | EventKind::DoubleClick | EventKind::Sequence | EventKind::Key) {
                assert!(!extras, "{}.{}: exact modifiers", p.id, b.id);
                assert_eq!(b.forbidden, !b.required, "{}.{}: every other modifier forbidden", p.id, b.id);
            }
            if extras {
                let allowed = !(b.required | b.forbidden);
                assert!(!allowed.is_empty(), "{}.{}: the extras allowed are named", p.id, b.id);
                assert!(matches!(b.event, EventKind::Wheel | EventKind::Pinch), "{}.{}", p.id, b.id);
            }
            assert!(b.held_keys.iter().all(|k| k.location == KeyLocation::Main), "{}.{}", p.id, b.id);
        }
        let mut seen = BTreeSet::new();
        for b in &p.bindings {
            assert!(seen.insert((format!("{:?}", b.context), b.priority)), "{}.{}: priority repeats", p.id, b.id);
        }
        assert!(p.bindings.iter().all(|b| b.event != EventKind::Sequence), "{}: no ordered-button rows", p.id);
    }

    let p = |id| navigation::preset(id).unwrap();
    // Rows the research leaves unresolved or undocumented are absent.
    let fusion = p("fusion");
    assert!(
        !fusion.bindings.iter().any(|b| b.buttons == [M] && b.required == Mods::CTRL | Mods::SHIFT),
        "Fusion's Ctrl+Shift+middle zoom is unresolved and not bound"
    );
    assert!(!fusion.bindings.iter().any(|b| matches!(b.action, Action::NormalTo | Action::NormalToggle)));
    let creo = p("creo");
    assert!(!creo.bindings.iter().any(|b| matches!(b.action, Action::Fit | Action::NormalTo)), "Creo: no fit key");
    let solid_edge = p("solid_edge");
    assert!(!solid_edge.bindings.iter().any(|b| b.event == KEY && b.action == Action::NormalTo));
    for id in ["fusion", "inventor", "creo", "nx12", "solid_edge", "blender"] {
        assert!(!p(id).bindings.iter().any(|b| b.key == key(Key::F)), "{id}: no invented F fit key");
    }

    // Axis split, anchors and needs.
    assert_eq!(creo.binding("zoom").unwrap().axis, DragAxis::Vertical);
    assert_eq!(creo.binding("roll").unwrap().axis, DragAxis::Horizontal);
    assert_eq!(creo.binding("roll").unwrap().requires, Capability::CameraRoll);
    let solidworks = p("solidworks");
    assert_eq!(solidworks.binding("zoom").unwrap().anchor, ZoomAnchor::Center, "Shift+M zooms about the screen centre");
    assert_eq!(solidworks.binding("roll").unwrap().requires, Capability::CameraRoll);
    let blender = p("blender");
    assert_eq!(blender.binding("fit_selected").unwrap().requires, Capability::NumpadLocation);
    assert_eq!(blender.binding("fit_selected").unwrap().key.unwrap().location, KeyLocation::Numpad);
    for p in navigation::presets() {
        for b in &p.bindings {
            if !["solidworks.roll", "creo.roll", "blender.fit_selected"]
                .contains(&format!("{}.{}", p.id, b.id).as_str())
            {
                assert_eq!(b.requires, Capability::None, "{}.{}", p.id, b.id);
            }
            if b.id != "zoom" || p.id != "solidworks" {
                assert_eq!(b.anchor, ZoomAnchor::Cursor, "{}.{}", p.id, b.id);
            }
        }
    }

    // Wheel direction is the product's only where it documents one.
    for pr in navigation::presets() {
        match pr.id {
            "inventor" => {
                assert_eq!(pr.wheel, WheelPolicy::Explicit { forward: ZoomDirection::Out }, "forward out, backward in")
            }
            "onshape" => assert_eq!(pr.wheel, WheelPolicy::Explicit { forward: ZoomDirection::In }, "up in, down out"),
            _ => assert_eq!(pr.wheel, WheelPolicy::SoliddraftDefault, "{}", pr.id),
        }
    }
    assert_eq!(WheelPolicy::SoliddraftDefault.forward(), ZoomDirection::In);

    // Limitations say what is historical, unverified or missing.
    assert!(p("nx12").has_limitation(LimitKind::HistoricalProfile));
    assert!(p("nx12").label.contains("NX 12"));
    assert!(creo.has_limitation(LimitKind::HistoricalBinding));
    assert!(creo.limitations.iter().any(|l| l.kind == LimitKind::HistoricalBinding && l.text.contains("Ctrl+D")));
    assert!(solid_edge.has_limitation(LimitKind::SelectionUnverified));
    assert!(blender.has_limitation(LimitKind::NeedsNumpadLocation));
    assert!(solidworks.has_limitation(LimitKind::NeedsCameraRoll));
    assert!(creo.has_limitation(LimitKind::NeedsCameraRoll));
    assert!(fusion.has_limitation(LimitKind::Optional), "Fusion's trackpad profile is optional");
    assert!(fusion.has_limitation(LimitKind::Unresolved), "Fusion's Ctrl+Shift+M is unresolved");
}

fn sample() -> Preset {
    Preset {
        id: "sample",
        label: "Sample",
        release_scope: "Rows for the test.",
        wheel: WheelPolicy::SoliddraftDefault,
        bindings: vec![
            Binding::drag("orbit", &[M], NONE, Action::Orbit, 10),
            Binding::drag("pan", &[M], Mods::SHIFT, Action::Pan, 20),
        ],
        limitations: vec![Limitation { kind: LimitKind::Note, text: "Sample." }],
    }
}

#[test]
fn preset_schema_rejects_malformed_rows() {
    for p in navigation::presets() {
        assert_eq!(navigation::validate(p), Ok(()), "{}", p.id);
    }
    assert_eq!(navigation::validate(&sample()), Ok(()));
    let mut bad = sample();
    bad.bindings[0].buttons.clear();
    bad.bindings[1].priority = 10;
    bad.bindings.push(Binding::drag("roll", &[M], Mods::ALT, Action::Roll, 30).needs(Capability::CameraRoll));
    let errors = navigation::validate(&bad).expect_err("malformed rows are refused");
    assert!(errors.contains(&SchemaError::EmptyButtons("orbit")), "{errors:?}");
    assert!(errors.contains(&SchemaError::DuplicatePriority { priority: 10, first: "orbit", second: "pan" }));
    assert!(errors.contains(&SchemaError::MissingLimitation("roll", Capability::CameraRoll)));
    assert!(errors.iter().all(|e| !e.to_string().is_empty()));
}

#[test]
fn ordered_button_schema_is_expressive_but_not_exposed() {
    let steps = [
        Step { button: M, down: true },
        Step { button: L, down: true },
        Step { button: M, down: false },
        Step { button: L, down: false },
    ];
    let mut p = sample();
    p.id = "ordered_sample";
    p.bindings.push(Binding::sequence("orbit_ordered", &steps, Action::Orbit, 30).needs(Capability::OrderedButtons));
    p.limitations.push(Limitation { kind: LimitKind::NeedsOrderedButtons, text: "Needs ordered button presses." });
    assert_eq!(navigation::validate(&p), Ok(()), "an ordered-button row is valid data");

    let row = p.binding("orbit_ordered").unwrap();
    assert_eq!(row.event, EventKind::Sequence);
    assert_eq!(row.sequence, steps);
    let chord = row.chord();
    let text = serde_json::to_string(&chord).unwrap();
    let back: Chord = serde_json::from_str(&text).unwrap();
    assert_eq!(back, chord, "the row's gesture round-trips: {text}");
    assert_eq!(&row.with_chord(&back), row);

    let mut without = p.clone();
    without.limitations.pop();
    assert_eq!(
        navigation::validate(&without),
        Err(vec![SchemaError::MissingLimitation("orbit_ordered", Capability::OrderedButtons)]),
        "ordered buttons need a stated limitation"
    );

    for offered in navigation::presets() {
        assert!(offered.bindings.iter().all(|b| b.requires != Capability::OrderedButtons), "{}", offered.id);
    }
    assert!(
        navigation::withheld("freecad_cad").is_some() && navigation::withheld("catia_profile_experimental").is_some()
    );
}
