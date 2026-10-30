//! View-key profiles without a window: kept apart from the modelling shortcuts, checked
//! against them so F keeps one owner, and running fit, normal-to, reverse normal and the
//! standard views. Apply and reset touch only the view keys.

use egui::{Key, Modifiers};
use glam::Vec3;
use soliddraft_app::commands::{self, Scope};
use soliddraft_app::document::{Command, Document, ExtrudeParams, ProfileKind, Selection};
use soliddraft_app::navigation::view_keys::{self, Conflict, PLATFORM, RowState, ViewKeys};
use soliddraft_app::navigation::{Action, KeyLocation, NavKey, NavigationSettings, Refusal, Supports};
use soliddraft_app::settings::InstallKind;
use soliddraft_app::state::{AppAction, AppState, NoteKind};
use soliddraft_app::viewport::camera::StandardView;

fn keys(id: &str) -> ViewKeys {
    view_keys::analyse(id, None, PLATFORM).unwrap()
}

fn conflicts_of<'a>(v: &'a ViewKeys, row: &str) -> Vec<&'a Conflict> {
    v.conflicts.iter().filter(|c| c.row == row).collect()
}

/// What a key press runs under a profile, outside a sketch on Windows or Linux.
fn press(v: &ViewKeys, key: Key, m: Modifiers) -> Option<Action> {
    v.row_for(NavKey { key, location: KeyLocation::Main }, m, false, false).map(|b| b.action)
}

fn ctrl() -> Modifiers {
    Modifiers { ctrl: true, command: true, ..Modifiers::NONE }
}

#[test]
fn view_key_profile_separation_and_conflict_list() {
    assert_eq!(view_keys::validate_all(), Ok(()));
    let ids: Vec<&str> = view_keys::profiles().iter().map(|p| p.id).collect();
    assert_eq!(ids[0], view_keys::SOLIDDRAFT, "SolidDraft's own keys come first");
    assert!(view_keys::profile("solidworks").is_some() && view_keys::profile("blender").is_some());
    let registry_before: Vec<(String, Vec<String>)> =
        commands::all().iter().map(|c| (c.id.to_owned(), c.keys.iter().map(|k| format!("{k:?}")).collect())).collect();

    let sw = keys("solidworks");
    let f: Vec<(&str, Scope)> = conflicts_of(&sw, "fit").iter().map(|c| (c.command, c.scope)).collect();
    assert!(f.contains(&("view_front", Scope::Model)), "{f:?}");
    assert!(f.contains(&("fillet", Scope::Edge)), "{f:?}");
    assert_eq!(sw.state_of("fit"), Some(RowState::Blocked), "the imported F = Fit is off");
    for row in ["normal", "front", "back", "left", "right", "top", "bottom", "isometric"] {
        assert_eq!(sw.state_of(row), Some(RowState::Active), "solidworks {row} applies");
    }
    assert_eq!(press(&sw, Key::Num8, ctrl()), Some(Action::NormalTo));
    assert_eq!(press(&sw, Key::Num1, ctrl()), Some(Action::Front));
    assert_eq!(press(&sw, Key::Num7, ctrl()), Some(Action::Isometric));
    assert_eq!(press(&sw, Key::F, Modifiers::NONE), None, "F is not taken from Front or Fillet");

    let on = keys("onshape");
    let f: Vec<&str> = conflicts_of(&on, "fit").iter().map(|c| c.command).collect();
    assert!(f.contains(&"view_front") && f.contains(&"fillet"), "{f:?}");
    assert_eq!(on.state_of("normal"), Some(RowState::Supersedes("normal_to")), "N normal-to-and-reverse takes N");
    assert_eq!(press(&on, Key::Num1, Modifiers::SHIFT), Some(Action::Front));
    assert_eq!(press(&on, Key::N, Modifiers::NONE), Some(Action::NormalToggle));

    let fu = keys("fusion");
    assert!(fu.conflicts.is_empty(), "Fusion's view keys take no command's key: {:?}", fu.conflicts);
    assert_eq!(press(&fu, Key::F6, Modifiers::NONE), Some(Action::Fit));
    assert!(fu.rows.iter().all(|(b, _)| b.key.map(|k| k.key) != Some(Key::F)), "Fusion's profile does not touch F");

    let se = keys("solid_edge");
    for (row, command) in [("front", "view_front"), ("right", "view_right"), ("isometric", "view_isometric")] {
        assert_eq!(se.state_of(row), Some(RowState::Duplicate(command)), "{row} is SolidDraft's own key already");
        assert!(conflicts_of(&se, row).is_empty(), "{row}: no false conflict");
    }
    assert_eq!(press(&se, Key::F, Modifiers::NONE), None, "the registry runs F once, not the profile too");
    let l: Vec<(&str, Scope)> = conflicts_of(&se, "left").iter().map(|c| (c.command, c.scope)).collect();
    assert_eq!(l, [("sketch_line", Scope::Sketch)], "L is the sketch's Line");
    assert_eq!(se.state_of("left"), Some(RowState::OutsideSketch));
    let left = NavKey::main(Key::L);
    assert_eq!(se.row_for(left, Modifiers::NONE, false, false).map(|b| b.action), Some(Action::Left));
    assert!(se.row_for(left, Modifiers::NONE, true, false).is_none(), "in a sketch L stays Line");

    let bl = keys("blender");
    assert!(bl.rows.iter().any(|(_, s)| matches!(s, RowState::Unavailable(_))), "keypad rows say they cannot work");
    match bl.state_of("front") {
        Some(RowState::Unavailable(why)) => assert!(why.contains("keypad"), "{why}"),
        other => panic!("{other:?}"),
    }

    for c in sw.conflicts.iter().chain(&on.conflicts).chain(&se.conflicts) {
        let command = commands::all().iter().find(|x| x.id == c.command).expect("a real command");
        assert_eq!(c.label, command.label, "the conflict names the command's button and menu entry");
        assert!(!c.key.is_empty());
    }
    let registry_after: Vec<(String, Vec<String>)> =
        commands::all().iter().map(|c| (c.id.to_owned(), c.keys.iter().map(|k| format!("{k:?}")).collect())).collect();
    assert_eq!(registry_before, registry_after, "no profile edits the command registry");
}

#[test]
fn f_conflict_does_not_steal_front_or_fillet() {
    let sw = keys("solidworks");
    for sketching in [false, true] {
        assert!(
            sw.row_for(NavKey::main(Key::F), Modifiers::NONE, sketching, false).is_none(),
            "F has no view-key owner"
        );
    }
    assert!(sw.superseded().next().is_none(), "no registry key is taken over");
    // So F goes to the registry: Front in the model, Fillet with an edge selected, once.
    let model = commands::Context::default();
    let edge = commands::Context { edge: true, ..Default::default() };
    let first_f = |c: commands::Context| {
        commands::bindings(c)
            .into_iter()
            .find(|(_, k)| k.key == Key::F && k.modifiers == Modifiers::NONE)
            .map(|(c, _)| c.id)
    };
    assert_eq!(first_f(model), Some("view_front"));
    assert_eq!(first_f(edge), Some("fillet"));
    assert_eq!(sw.state_of("fit"), Some(RowState::Blocked), "the imported row is shown blocked");
    // Every command named in the conflict keeps its button: the registry still has it with a ribbon place.
    for c in &sw.conflicts {
        let command = commands::all().iter().find(|x| x.id == c.command).unwrap();
        assert!(!command.ribbon.is_empty(), "{} keeps its button", c.command);
    }
    let fit = commands::all().iter().find(|c| c.id == "zoom_fit").unwrap();
    assert!(!fit.ribbon.is_empty(), "the View ribbon's Fit stays");
}

fn box_document() -> Document {
    let mut doc = Document::new("Box", soliddraft_app::kernel());
    doc.apply(Command::AddExtrude {
        params: ExtrudeParams {
            profile: ProfileKind::Rectangle { width: 60.0, height: 40.0 },
            depth: 20.0,
            ..Default::default()
        },
        via_assistant: false,
    })
    .unwrap();
    doc
}

fn settle(state: &mut AppState) {
    for _ in 0..240 {
        state.viewport.camera.tick(1.0 / 60.0);
    }
}

fn top_face(state: &AppState) -> Selection {
    Selection::Face(state.doc.faces.iter().find(|f| (f.centroid.z - 20.0).abs() < 1e-9).unwrap().id)
}

#[test]
fn view_keys_fit_normal_reverse_and_principal_views() {
    let mut state = AppState::new(box_document());
    let run = |state: &mut AppState, profile: &str, key: Key, m: Modifiers| {
        let v = keys(profile);
        let row = v.row_for(NavKey::main(key), m, false, false).unwrap_or_else(|| panic!("{profile}: {key:?}"));
        view_keys::run(state, row.action);
        state.take_actions()
    };

    // SOLIDWORKS Ctrl+8 is Normal to; with no face the command says so and moves nothing.
    assert_eq!(run(&mut state, "solidworks", Key::Num8, ctrl()), [AppAction::NormalTo]);
    for (i, view) in [
        StandardView::Front,
        StandardView::Back,
        StandardView::Left,
        StandardView::Right,
        StandardView::Top,
        StandardView::Bottom,
        StandardView::Isometric,
    ]
    .into_iter()
    .enumerate()
    {
        let digit = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7][i];
        assert_eq!(run(&mut state, "solidworks", digit, ctrl()), [AppAction::SetView(view)], "Ctrl+{}", i + 1);
        assert_eq!(run(&mut state, "onshape", digit, Modifiers::SHIFT), [AppAction::SetView(view)], "Shift+{}", i + 1);
    }
    // Shift+1 arrives as "!" with the digit's physical key; the router reads the physical one.
    let event = egui::Event::Key {
        key: Key::Exclamationmark,
        physical_key: Some(Key::Num1),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::SHIFT,
    };
    let nav = soliddraft_app::navigation::gather(&[event], 0.0, |_| true, true);
    let pressed = nav.iter().find_map(|e| match e {
        soliddraft_app::navigation::NavEvent::KeyDown(k) => Some(*k),
        _ => None,
    });
    assert_eq!(pressed.map(|k| k.key), Some(Key::Num1));
    assert_eq!(
        keys("onshape").row_for(pressed.unwrap(), Modifiers::SHIFT, false, false).map(|b| b.action),
        Some(Action::Front)
    );

    // Onshape N: normal, then again the other way round. The registry's normal_to is off N.
    state.doc.selection = Some(top_face(&state));
    assert!(
        run(&mut state, "onshape", Key::N, Modifiers::NONE).is_empty(),
        "the profile's row turns the camera itself"
    );
    settle(&mut state);
    let first = state.viewport.camera.direction();
    assert!(first.dot(Vec3::Z) > 0.999, "{first:?}");
    run(&mut state, "onshape", Key::N, Modifiers::NONE);
    settle(&mut state);
    let second = state.viewport.camera.direction();
    assert!(first.dot(second) < -0.99, "N again reverses: {first:?} then {second:?}");
    assert_eq!(keys("onshape").superseded().collect::<Vec<_>>(), [("normal_to", Key::N)]);

    // Fit keys.
    assert_eq!(run(&mut state, "fusion", Key::F6, Modifiers::NONE), [AppAction::ZoomFit]);
    assert_eq!(run(&mut state, "inventor", Key::Home, Modifiers::NONE), [AppAction::ZoomFit]);
    assert_eq!(run(&mut state, "blender", Key::Home, Modifiers::NONE), [AppAction::ZoomFit]);
    assert_eq!(run(&mut state, "inventor", Key::PageUp, Modifiers::NONE), [AppAction::NormalTo]);
    assert_eq!(run(&mut state, "inventor", Key::F6, Modifiers::NONE), [AppAction::SetView(StandardView::Isometric)]);
    // Creo's default view, with its historical flag.
    assert_eq!(run(&mut state, "creo", Key::D, ctrl()), [AppAction::SetView(StandardView::Isometric)]);
    let creo = view_keys::profile("creo").unwrap();
    assert!(creo.limitations.iter().any(|l| l.text.contains("Ctrl+D") && l.text.contains("Creo 9")));
    // NX: End isometric; F8 normal to a face, or with none the nearest standard view.
    assert_eq!(run(&mut state, "nx12", Key::End, Modifiers::NONE), [AppAction::SetView(StandardView::Isometric)]);
    state.doc.selection = Some(top_face(&state));
    assert!(run(&mut state, "nx12", Key::F8, Modifiers::NONE).is_empty());
    settle(&mut state);
    assert!(state.viewport.camera.direction().dot(Vec3::Z) > 0.999);
    state.doc.selection = None;
    state.viewport.camera.look_along(Vec3::new(0.2, -0.9, 0.1));
    settle(&mut state);
    assert_eq!(run(&mut state, "nx12", Key::F8, Modifiers::NONE), [AppAction::SetView(StandardView::Front)]);

    // Fit selected frames the selection; with none it says so and the camera stays.
    let before = (state.viewport.camera.target, state.viewport.camera.distance);
    view_keys::run(&mut state, Action::FitSelected);
    settle(&mut state);
    assert_eq!((state.viewport.camera.target, state.viewport.camera.distance), before);
    assert!(state.note.as_ref().is_some_and(|n| n.kind == NoteKind::Info && n.text.contains("select")));
    state.viewport.camera.distance = 2000.0;
    state.doc.selection = Some(Selection::Body);
    view_keys::run(&mut state, Action::FitSelected);
    settle(&mut state);
    assert!(state.viewport.camera.distance < 500.0, "the body is framed: {}", state.viewport.camera.distance);
}

#[test]
fn apply_and_reset_view_keys_are_independent() {
    let mut s = NavigationSettings::new(InstallKind::Existing);
    s.select_preset("solidworks").unwrap();
    s.wheel.invert = true;
    s.camera.orbit_sensitivity = 2.0;
    let applied = view_keys::apply_to(&mut s, "solidworks").unwrap();
    assert_eq!(s.keyboard_profile, "solidworks");
    assert!(!applied.conflicts.is_empty(), "the F conflict comes back with the profile");
    assert!(applied.rows.iter().any(|(_, st)| *st == RowState::Active), "the other rows apply");
    assert_eq!(s.preset_id, "solidworks");
    assert!(s.wheel.invert && s.camera.orbit_sensitivity == 2.0, "the mouse side is untouched");

    s.select_preset("onshape").unwrap();
    assert_eq!(s.keyboard_profile, "solidworks", "changing the mouse preset keeps the view keys");

    let before = s.clone();
    assert_eq!(
        view_keys::apply_to(&mut s, "not_a_profile").unwrap_err(),
        Refusal::UnknownProfile("not_a_profile".into())
    );
    assert_eq!(s, before, "an unknown profile changes nothing");

    s.view_key_overrides.entry("solidworks".into()).or_default().insert(
        "normal".into(),
        soliddraft_app::navigation::Chord {
            event: soliddraft_app::navigation::EventKind::Key,
            key: Some(NavKey::main(Key::F7)),
            ..Default::default()
        },
    );
    s.overrides.entry("onshape".into()).or_default().insert("orbit".into(), Default::default());
    view_keys::reset_in(&mut s);
    assert_eq!(s.keyboard_profile, view_keys::SOLIDDRAFT);
    assert!(s.view_key_overrides.is_empty(), "view-key changes go");
    assert!(s.overrides.contains_key("onshape"), "mouse changes stay");
    assert_eq!(s.preset_id, "onshape");
    assert!(s.wheel.invert);

    // A changed row is what the profile uses.
    let mut c = NavigationSettings::new(InstallKind::Existing);
    view_keys::apply_to(&mut c, "solidworks").unwrap();
    c.view_key_overrides.entry("solidworks".into()).or_default().insert(
        "normal".into(),
        soliddraft_app::navigation::Chord {
            event: soliddraft_app::navigation::EventKind::Key,
            key: Some(NavKey::main(Key::F7)),
            ..Default::default()
        },
    );
    let v = view_keys::in_use(&c);
    assert_eq!(
        v.row_for(NavKey::main(Key::F7), Modifiers::NONE, false, false).map(|b| b.action),
        Some(Action::NormalTo)
    );
    assert!(v.row_for(NavKey::main(Key::Num8), ctrl(), false, false).is_none());

    // With keypad support the Blender rows would be live; the platform has none.
    let live = view_keys::analyse("blender", None, Supports { numpad_location: true, ..PLATFORM }).unwrap();
    assert!(live.rows.iter().all(|(_, st)| !matches!(st, RowState::Unavailable(_))));
}
