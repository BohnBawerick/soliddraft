//! The navigation router without a window: exact modifiers, physical Ctrl, held ordinary
//! keys, the most specific chord, capture past the threshold, clicks left alone, one
//! zoom per gesture, sketches, and the legacy, Fusion, SOLIDWORKS and Onshape presets.
//!
//! Each test writes its event-to-action trace under `target/navigation-test/routing/`.

use egui::{Key, Modifiers, Pos2, Vec2, pos2, vec2};
use soliddraft_app::navigation::{
    self, Action, Binding, Button, LimitKind, Limitation, Mods, Motion, NAV_DRAG_THRESHOLD, NavContext, NavEvent,
    NavKey, NavigationSettings, Outcome, Preset, Routed, Router, WheelPolicy,
};
use soliddraft_app::settings::InstallKind;
use soliddraft_app::viewport::camera::Camera;
use std::path::PathBuf;

const L: Button = Button::Left;
const M: Button = Button::Middle;
const R: Button = Button::Right;
const O: Pos2 = pos2(400.0, 300.0);
const PAST: Vec2 = vec2(0.0, NAV_DRAG_THRESHOLD + 1.0);

/// Keeps what each gesture did, and writes it when the test ends.
struct Trace {
    name: &'static str,
    rows: Vec<String>,
}

impl Trace {
    fn new(name: &'static str) -> Self {
        Self { name, rows: Vec::new() }
    }

    fn note(&mut self, preset: &str, gesture: &str, r: &Routed) {
        self.rows.push(format!("{preset} | {gesture} | {:?} | {:?} | consumed {}", r.outcome, r.binding, r.consumed));
    }
}

impl Drop for Trace {
    fn drop(&mut self) {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/navigation-test/routing");
        let _ = std::fs::create_dir_all(&dir);
        let _ =
            std::fs::write(dir.join(format!("{}.json", self.name)), serde_json::to_string_pretty(&self.rows).unwrap());
    }
}

fn router(id: &str) -> Router {
    Router::for_preset(navigation::preset(id).unwrap().clone(), false)
}

fn mac_router(id: &str) -> Router {
    Router::for_preset(navigation::preset(id).unwrap().clone(), true)
}

fn mods(ctrl: bool, shift: bool, alt: bool) -> Modifiers {
    Modifiers { ctrl, shift, alt, command: ctrl, mac_cmd: false }
}

fn press(r: &mut Router, b: Button) -> Routed {
    r.handle(NavEvent::Press { button: b, pos: O, inside: true, time: 0.0 })
}

/// Holds `m`, presses `buttons` in order at one point and moves past the threshold.
/// Returns what the move that crossed it did.
fn drag(r: &mut Router, buttons: &[Button], m: Modifiers) -> Routed {
    r.handle(NavEvent::Modifiers(m));
    for b in buttons {
        press(r, *b);
    }
    r.handle(NavEvent::Move(O + PAST))
}

/// Releases every button and the modifiers. Returns the last release's result.
fn end(r: &mut Router, buttons: &[Button]) -> Routed {
    let mut last = None;
    for b in buttons {
        last = Some(r.handle(NavEvent::Release { button: *b, pos: O + PAST, time: 0.1 }));
    }
    r.handle(NavEvent::Modifiers(Modifiers::NONE));
    last.unwrap()
}

/// What a drag does, released afterwards.
fn gesture(r: &mut Router, buttons: &[Button], m: Modifiers) -> Routed {
    let out = drag(r, buttons, m);
    end(r, buttons);
    out
}

fn assert_outcome(t: &mut Trace, preset: &str, gesture_name: &str, r: Routed, want: Outcome) {
    t.note(preset, gesture_name, &r);
    assert_eq!(r.outcome, want, "{preset}: {gesture_name} chose {:?}", r.binding);
}

#[test]
fn exact_modifier_matching_prevents_bare_fallback_from_eating_chords() {
    let mut t = Trace::new("solidworks-modifiers");
    let mut sw = router("solidworks");
    for (m, name, want) in [
        (mods(false, false, false), "M drag", Outcome::Action(Action::Orbit)),
        (mods(true, false, false), "Ctrl+M drag", Outcome::Action(Action::Pan)),
        (mods(false, true, false), "Shift+M drag", Outcome::Action(Action::DragZoom)),
        (mods(false, false, true), "Alt+M drag", Outcome::Unsupported(Action::Roll)),
        (mods(true, true, false), "Ctrl+Shift+M drag", Outcome::None),
        (mods(true, false, true), "Ctrl+Alt+M drag", Outcome::None),
    ] {
        assert_outcome(&mut t, "solidworks", name, gesture(&mut sw, &[M], m), want);
    }
    let mut legacy = router("legacy_soliddraft");
    assert_outcome(
        &mut t,
        "legacy",
        "Shift+L drag",
        gesture(&mut legacy, &[L], mods(false, true, false)),
        Outcome::Action(Action::Pan),
    );
    assert_outcome(
        &mut t,
        "legacy",
        "Ctrl+L drag",
        gesture(&mut legacy, &[L], mods(true, false, false)),
        Outcome::None,
    );

    // The row whose exact modifiers match wins, whatever the priorities say.
    let synthetic = |pan: Mods, orbit: Mods| Preset {
        id: "synthetic",
        label: "Synthetic",
        release_scope: "Test rows.",
        wheel: WheelPolicy::SoliddraftDefault,
        bindings: vec![
            Binding::drag("pan", &[M], pan, Action::Pan, 10),
            Binding::drag("orbit", &[M], orbit, Action::Orbit, 90),
        ],
        limitations: vec![Limitation { kind: LimitKind::Note, text: "Test." }],
    };
    let mut r = Router::for_preset(synthetic(Mods::CTRL, Mods::SHIFT), false);
    assert_eq!(gesture(&mut r, &[M], mods(true, false, false)).binding, Some("pan"));
    assert_eq!(gesture(&mut r, &[M], mods(false, true, false)).binding, Some("orbit"));
    let mut r = Router::for_preset(synthetic(Mods::SHIFT, Mods::CTRL), false);
    assert_eq!(gesture(&mut r, &[M], mods(true, false, false)).binding, Some("orbit"));
    assert_eq!(gesture(&mut r, &[M], mods(false, true, false)).binding, Some("pan"));

    // egui's logical matching would let a bare row take Shift; the router does not.
    let bare = Binding::drag("bare", &[M], Mods::NONE, Action::Orbit, 10);
    assert!(Modifiers::SHIFT.matches_logically(Modifiers::NONE), "logical matching ignores the extra Shift");
    assert!(!navigation::modifiers_match(&bare, Modifiers::SHIFT, false), "the router does not");
    let mut only_bare = Router::for_preset(synthetic(Mods::CTRL, Mods::NONE), false);
    assert_eq!(gesture(&mut only_bare, &[M], Modifiers::SHIFT).outcome, Outcome::None);
}

#[test]
fn physical_ctrl_is_not_the_platform_command_modifier() {
    let mut t = Trace::new("onshape-ctrl");
    let physical_ctrl = Modifiers { ctrl: true, command: false, mac_cmd: false, ..Modifiers::NONE };
    let command = Modifiers { ctrl: false, command: true, mac_cmd: true, ..Modifiers::NONE };
    let mut mac = mac_router("onshape");
    assert_outcome(
        &mut t,
        "onshape-mac",
        "Ctrl+R drag",
        gesture(&mut mac, &[R], physical_ctrl),
        Outcome::Action(Action::Pan),
    );
    let cmd = gesture(&mut mac, &[R], command);
    assert_outcome(&mut t, "onshape-mac", "Cmd+R drag", cmd, Outcome::None);
    let mut win = router("onshape");
    assert_outcome(
        &mut t,
        "onshape-win",
        "Ctrl+R drag",
        gesture(&mut win, &[R], mods(true, false, false)),
        Outcome::Action(Action::Pan),
    );

    let onshape = navigation::preset("onshape").unwrap();
    let pan = onshape.binding("pan_ctrl_right").unwrap();
    assert!(pan.required.ctrl && !pan.required.platform, "the data asks for physical Ctrl");
    assert!(pan.text(true).contains("Ctrl") && !pan.text(true).contains("Cmd"), "{}", pan.text(true));

    // Selection's primary modifier is the platform's: Command on macOS, Ctrl elsewhere.
    assert!(navigation::selection_primary(command, true));
    assert!(!navigation::selection_primary(physical_ctrl, true));
    assert!(navigation::selection_primary(mods(true, false, false), false));

    // A row that does mean the platform key gets Command on macOS and Ctrl elsewhere.
    let platform = Binding::drag("platform", &[M], Mods::PLATFORM, Action::Pan, 10);
    assert!(navigation::modifiers_match(&platform, command, true));
    assert!(!navigation::modifiers_match(&platform, physical_ctrl, true));
    assert!(navigation::modifiers_match(&platform, mods(true, false, false), false));
    assert!(platform.text(true).contains("Cmd") && platform.text(false).contains("Ctrl"));
}

fn key_down(r: &mut Router, k: Key) -> Routed {
    r.handle(NavEvent::KeyDown(NavKey::main(k)))
}

fn key_up(r: &mut Router, k: Key) -> Routed {
    r.handle(NavEvent::KeyUp(NavKey::main(k)))
}

#[test]
fn held_ordinary_keys_are_modifiers_and_taps_still_run_their_command() {
    let mut t = Trace::new("inventor-held-keys");
    let mut r = router("inventor");
    for (k, want) in [(Key::F4, Action::Orbit), (Key::F2, Action::Pan), (Key::F3, Action::DragZoom)] {
        key_down(&mut r, k);
        let out = gesture(&mut r, &[L], Modifiers::NONE);
        assert_outcome(&mut t, "inventor", &format!("held {k:?}+L drag"), out, Outcome::Action(want));
        assert!(r.held_key_used(NavKey::main(k)));
        assert!(key_up(&mut r, k).consumed, "{k:?}: the drag used it, so it runs nothing");
    }
    // A key alone does nothing to the view, and its tap stays a tap.
    assert_eq!(key_down(&mut r, Key::F4).outcome, Outcome::None);
    let tap = key_up(&mut r, Key::F4);
    assert_eq!(tap.outcome, Outcome::None);
    key_down(&mut r, Key::F2);
    assert!(r.is_hold_key(NavKey::main(Key::F2)));
    assert!(!r.held_key_used(NavKey::main(Key::F2)));
    assert!(!key_up(&mut r, Key::F2).consumed, "an F2 tap still renames");

    // Let go of F2 mid-drag: the pan holds until the buttons are up.
    key_down(&mut r, Key::F2);
    assert_eq!(drag(&mut r, &[L], Modifiers::NONE).outcome, Outcome::Action(Action::Pan));
    assert!(key_up(&mut r, Key::F2).consumed, "no rename");
    let later = r.handle(NavEvent::Move(O + PAST * 3.0));
    assert_outcome(&mut t, "inventor", "F2 up mid-drag, move", later, Outcome::Action(Action::Pan));
    assert!(end(&mut r, &[L]).consumed);

    // F2 pressed after the button but before the threshold still counts.
    press(&mut r, L);
    key_down(&mut r, Key::F2);
    let out = r.handle(NavEvent::Move(O + PAST));
    assert_outcome(&mut t, "inventor", "L down, F2 down, drag", out, Outcome::Action(Action::Pan));
    end(&mut r, &[L]);
    assert!(key_up(&mut r, Key::F2).consumed);
    // A key the preset does not hold is not navigation's.
    assert!(!r.is_hold_key(NavKey::main(Key::F5)));
    assert!(!key_up(&mut r, Key::F5).consumed);
}

#[test]
fn most_specific_ordered_button_chord_wins() {
    let mut t = Trace::new("nx12-onshape-chords");
    let mut nx = router("nx12");
    let none = Modifiers::NONE;
    for (buttons, m, name, want) in [
        (&[M][..], none, "M", Outcome::Action(Action::Orbit)),
        (&[M, R][..], none, "M then R", Outcome::Action(Action::Pan)),
        (&[L, M][..], none, "L then M", Outcome::Action(Action::DragZoom)),
        (&[M, L][..], none, "M then L", Outcome::Action(Action::DragZoom)),
        (&[M][..], mods(true, false, false), "Ctrl+M", Outcome::Action(Action::DragZoom)),
        (&[M, R][..], mods(true, false, false), "Ctrl+M+R", Outcome::None),
    ] {
        let out = gesture(&mut nx, buttons, m);
        assert_outcome(&mut t, "nx12", name, out, want);
    }
    // R up mid-drag: the pan carries on to the release.
    assert_eq!(drag(&mut nx, &[M, R], none).binding, Some("pan_middle_right"));
    nx.handle(NavEvent::Release { button: R, pos: O + PAST, time: 0.1 });
    let after = nx.handle(NavEvent::Move(O + PAST * 2.0));
    assert_outcome(&mut t, "nx12", "R up mid-drag, move", after, Outcome::Action(Action::Pan));
    assert!(end(&mut nx, &[M]).consumed);
    // A second button pressed after the drag began re-decides it.
    assert_eq!(drag(&mut nx, &[M], none).outcome, Outcome::Action(Action::Orbit));
    let added = press(&mut nx, R);
    assert_outcome(&mut t, "nx12", "M dragging, then R", added, Outcome::Action(Action::Pan));
    end(&mut nx, &[M, R]);

    let mut on = router("onshape");
    assert_outcome(&mut t, "onshape", "R", gesture(&mut on, &[R], none), Outcome::Action(Action::Orbit));
    assert_outcome(
        &mut t,
        "onshape",
        "Ctrl+R",
        gesture(&mut on, &[R], mods(true, false, false)),
        Outcome::Action(Action::Pan),
    );
    // R first, then Ctrl, before the threshold.
    press(&mut on, R);
    on.handle(NavEvent::Modifiers(mods(true, false, false)));
    let r_then_ctrl = on.handle(NavEvent::Move(O + PAST));
    assert_outcome(&mut t, "onshape", "R then Ctrl", r_then_ctrl, Outcome::Action(Action::Pan));
    end(&mut on, &[R]);
    assert_outcome(&mut t, "onshape", "M+R", gesture(&mut on, &[M, R], none), Outcome::None);
}

#[test]
fn drag_capture_requires_viewport_origin_and_threshold() {
    let mut t = Trace::new("capture");
    let mut r = router("legacy_soliddraft");
    // Under the threshold: no navigation, and the release is still a click.
    press(&mut r, L);
    let short = r.handle(NavEvent::Move(O + vec2(0.0, NAV_DRAG_THRESHOLD - 1.0)));
    assert_outcome(&mut t, "legacy", "L moved threshold - 1", short, Outcome::None);
    assert!(!r.dragging());
    assert!(!end(&mut r, &[L]).consumed, "a click");
    // Past it: the first move orbits, with no click before.
    press(&mut r, L);
    let long = r.handle(NavEvent::Move(O + PAST));
    assert_outcome(&mut t, "legacy", "L moved threshold + 1", long, Outcome::Action(Action::Orbit));
    assert_eq!(long.motion, Motion::Drag { delta: PAST, origin: O });
    end(&mut r, &[L]);
    // A press outside the viewport is not navigation's, even when it moves in.
    r.handle(NavEvent::Press { button: L, pos: pos2(10.0, 300.0), inside: false, time: 0.0 });
    let outside = r.handle(NavEvent::Move(O));
    assert_outcome(&mut t, "legacy", "press in the tree, move in", outside, Outcome::None);
    assert!(!r.captured());
    assert!(!r.handle(NavEvent::Release { button: L, pos: O, time: 0.1 }).consumed);
    // Captured, it carries on outside the window and over overlays until the release.
    press(&mut r, L);
    r.handle(NavEvent::Move(O + PAST));
    let gone = r.handle(NavEvent::Move(pos2(-200.0, -50.0)));
    assert_outcome(&mut t, "legacy", "dragged out of the window", gone, Outcome::Action(Action::Orbit));
    assert!(r.dragging());
    assert!(r.handle(NavEvent::Release { button: L, pos: pos2(-200.0, -50.0), time: 0.2 }).consumed);
    assert!(!r.captured() && !r.dragging(), "capture ends at the release");

    // The adapter says which presses are the viewport's.
    let viewport = egui::Rect::from_min_size(pos2(300.0, 100.0), vec2(800.0, 600.0));
    let events = [
        egui::Event::PointerButton {
            pos: pos2(100.0, 300.0),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
        egui::Event::PointerButton {
            pos: O,
            button: egui::PointerButton::Middle,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ];
    let gathered = navigation::gather(&events, 0.0, |p| viewport.contains(p), true);
    let presses: Vec<bool> = gathered
        .iter()
        .filter_map(|e| match e {
            NavEvent::Press { inside, .. } => Some(*inside),
            _ => None,
        })
        .collect();
    assert_eq!(presses, [false, true]);
}

#[test]
fn navigation_drag_does_not_select_or_open_a_secondary_click_path() {
    let mut t = Trace::new("click-paths");
    let mut on = router("onshape");
    // A right drag orbits and its release is navigation's: no secondary click follows.
    let orbit = drag(&mut on, &[R], Modifiers::NONE);
    assert_outcome(&mut t, "onshape", "R drag", orbit, Outcome::Action(Action::Orbit));
    assert!(end(&mut on, &[R]).consumed);
    // A right click without moving is left for a context menu.
    press(&mut on, R);
    let click = on.handle(NavEvent::Release { button: R, pos: O, time: 0.1 });
    t.note("onshape", "R click", &click);
    assert!(!click.consumed && click.outcome == Outcome::None);
    // A primary click is left to selection.
    press(&mut on, L);
    assert!(!on.handle(NavEvent::Release { button: L, pos: O, time: 0.5 }).consumed);
    // In a sketch the left drag is the sketch tool's.
    let mut legacy = router("legacy_soliddraft");
    legacy.context = NavContext::Sketch;
    let sketch = drag(&mut legacy, &[L], Modifiers::NONE);
    assert_outcome(&mut t, "legacy-sketch", "L drag", sketch, Outcome::None);
    assert!(!sketch.consumed && !legacy.dragging());
    assert!(!end(&mut legacy, &[L]).consumed);
}

fn wheel(lines: f32) -> egui::Event {
    egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: vec2(0.0, lines),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
    }
}

fn scroll(dy: f32, phase: egui::TouchPhase, modifiers: Modifiers) -> egui::Event {
    egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, dy), phase, modifiers }
}

/// The zooms one frame of events gives.
fn zooms(r: &mut Router, events: &[egui::Event]) -> Vec<f32> {
    navigation::gather(events, 0.0, |_| true, true)
        .into_iter()
        .map(|e| r.handle(e))
        .filter_map(|r| match (r.outcome, r.motion) {
            (Outcome::Action(Action::WheelZoom), Motion::Zoom { factor }) => Some(factor),
            _ => None,
        })
        .collect()
}

#[test]
fn one_physical_zoom_gesture_produces_one_zoom() {
    let mut t = Trace::new("zoom-gestures");
    let mut r = router("fusion");
    let line = zooms(&mut r, &[wheel(1.0)]);
    assert_eq!(line.len(), 1, "one wheel line, one zoom");
    assert!(line[0] > 1.0, "a forward turn zooms in");
    let back = zooms(&mut r, &[wheel(-1.0)]);
    assert!(back[0] < 1.0);
    use egui::TouchPhase::{End, Move, Start};
    let smooth = zooms(
        &mut r,
        &[scroll(3.0, Start, Modifiers::NONE), scroll(4.0, Move, Modifiers::NONE), scroll(5.0, End, Modifiers::NONE)],
    );
    assert_eq!(smooth.len(), 1, "high-resolution scrolling is one zoom, not one per event");
    let pinch = zooms(&mut r, &[egui::Event::Zoom(1.2)]);
    assert_eq!(pinch, [1.2]);
    let emulated = egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, 20.0),
        phase: Move,
        modifiers: Modifiers::CTRL,
    };
    let both = zooms(&mut r, &[egui::Event::Zoom(1.2), emulated.clone()]);
    assert_eq!(both, [1.2], "a pinch and the browser's Ctrl-wheel copy of it are one zoom");
    t.rows.push(format!("fusion | line {line:?} | smooth {smooth:?} | pinch {pinch:?} | pinch+ctrl wheel {both:?}"));

    // The invert setting turns the wheel around, and only the wheel.
    let mut s = NavigationSettings::new(InstallKind::New);
    s.wheel.invert = true;
    let mut inverted = Router::new(&s, false);
    assert!(zooms(&mut inverted, &[wheel(1.0)])[0] < 1.0, "inverted, a forward turn zooms out");
    assert_eq!(zooms(&mut inverted, &[egui::Event::Zoom(1.2)]), [1.2], "a pinch keeps its direction");

    // A cursor-anchored zoom keeps the point under the cursor; a centred one the target.
    let rect = egui::Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0));
    let aspect = rect.width() / rect.height();
    let cursor = pos2(720.0, 230.0);
    let mut cam = Camera::default();
    let (o, d) = cam.ray(cursor, rect);
    let point = o + d * (cam.distance / d.dot(cam.forward()));
    let ndc = ((cursor.x / rect.width()) * 2.0 - 1.0, 1.0 - (cursor.y / rect.height()) * 2.0);
    cam.zoom(line[0], Some(ndc), aspect);
    let after = cam.project(point, rect).expect("still in view");
    assert!((after - cursor).length() < 1.0, "the point stays under the cursor: {after:?} vs {cursor:?}");
    let mut centred = Camera::default();
    let target = centred.target;
    centred.zoom(line[0], None, aspect);
    assert_eq!(centred.target, target);

    // Fusion's Mac trackpad profile pans with two fingers and orbits with Shift; the
    // wheel's zoom does not also run.
    s.trackpad = "fusion_mac".into();
    let mut pad = Router::new(&s, true);
    let out: Vec<Routed> = navigation::gather(&[scroll(5.0, Move, Modifiers::NONE)], 0.0, |_| true, true)
        .into_iter()
        .map(|e| pad.handle(e))
        .filter(|r| r.outcome != Outcome::None)
        .collect();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].outcome, Outcome::Action(Action::Pan));
    let shifted: Vec<Outcome> = navigation::gather(&[scroll(5.0, Move, Modifiers::SHIFT)], 0.0, |_| true, true)
        .into_iter()
        .map(|e| pad.handle(e).outcome)
        .filter(|o| *o != Outcome::None)
        .collect();
    assert_eq!(shifted, [Outcome::Action(Action::Orbit)]);
}

#[test]
fn navigation_works_with_active_sketch_and_collector() {
    let mut t = Trace::new("sketch");
    let mut fusion = router("fusion");
    fusion.context = NavContext::Sketch;
    assert_outcome(
        &mut t,
        "fusion-sketch",
        "M drag",
        gesture(&mut fusion, &[M], Modifiers::NONE),
        Outcome::Action(Action::Pan),
    );
    assert_outcome(
        &mut t,
        "fusion-sketch",
        "Shift+M drag",
        gesture(&mut fusion, &[M], Modifiers::SHIFT),
        Outcome::Action(Action::Orbit),
    );
    let left = gesture(&mut fusion, &[L], Modifiers::NONE);
    assert_outcome(&mut t, "fusion-sketch", "L drag", left, Outcome::None);
    assert!(!left.consumed, "the left drag stays the sketch tool's");
    for id in ["fusion", "solidworks", "onshape"] {
        let mut r = router(id);
        r.context = NavContext::Sketch;
        let out = gesture(&mut r, &[L], Modifiers::NONE);
        assert!(out.outcome == Outcome::None && !out.consumed, "{id}: L in a sketch is not navigation");
    }
}

#[test]
fn legacy_preset_reproduces_current_navigation() {
    let mut t = Trace::new("legacy");
    let mut r = router("legacy_soliddraft");
    for (buttons, m, name, want) in [
        (&[L][..], Modifiers::NONE, "L drag", Outcome::Action(Action::Orbit)),
        (&[M][..], Modifiers::NONE, "M drag", Outcome::Action(Action::Pan)),
        (&[R][..], Modifiers::NONE, "R drag", Outcome::Action(Action::Pan)),
        (&[L][..], Modifiers::SHIFT, "Shift+L drag", Outcome::Action(Action::Pan)),
    ] {
        assert_outcome(&mut t, "legacy", name, gesture(&mut r, buttons, m), want);
    }
    r.context = NavContext::Sketch;
    assert_outcome(&mut t, "legacy-sketch", "L drag", gesture(&mut r, &[L], Modifiers::NONE), Outcome::None);
    assert_outcome(
        &mut t,
        "legacy-sketch",
        "Shift+L drag",
        gesture(&mut r, &[L], Modifiers::SHIFT),
        Outcome::Action(Action::Pan),
    );
    r.context = NavContext::Model;
    assert_eq!(zooms(&mut r, &[wheel(1.0)]).len(), 1);
    // A click without a drag selects; no double click moves the camera.
    for time in [0.0, 0.1] {
        press(&mut r, L);
        let up = r.handle(NavEvent::Release { button: L, pos: O, time });
        assert!(!up.consumed && up.outcome == Outcome::None);
    }
    // Z and N stay the command registry's fit and normal-to keys.
    let keys: Vec<(&str, Key)> = soliddraft_app::commands::all()
        .iter()
        .flat_map(|c| c.keys.iter().map(move |k| (c.id, k.key)))
        .filter(|(id, _)| matches!(*id, "zoom_fit" | "normal_to"))
        .collect();
    assert!(keys.contains(&("zoom_fit", Key::Z)) && keys.contains(&("normal_to", Key::N)), "{keys:?}");
}

#[test]
fn fusion_solidworks_onshape_end_to_end() {
    let mut t = Trace::new("fusion-solidworks-onshape");
    let none = Modifiers::NONE;
    for (id, buttons, m, name, want) in [
        ("fusion", &[M][..], Modifiers::SHIFT, "Shift+M drag", Action::Orbit),
        ("fusion", &[M][..], none, "M drag", Action::Pan),
        ("solidworks", &[M][..], none, "M drag", Action::Orbit),
        ("solidworks", &[M][..], mods(true, false, false), "Ctrl+M drag", Action::Pan),
        ("solidworks", &[M][..], Modifiers::SHIFT, "Shift+M drag", Action::DragZoom),
        ("onshape", &[R][..], none, "R drag", Action::Orbit),
        ("onshape", &[M][..], none, "M drag", Action::Pan),
        ("onshape", &[R][..], mods(true, false, false), "Ctrl+R drag", Action::Pan),
    ] {
        let mut s = NavigationSettings::new(InstallKind::Existing);
        s.select_preset(id).unwrap();
        let mut r = Router::new(&s, false);
        assert_outcome(&mut t, id, name, gesture(&mut r, buttons, m), Outcome::Action(want));
    }
    // Onshape's middle double click fits.
    let mut on = router("onshape");
    let mut last = None;
    for time in [0.0, 0.15] {
        press(&mut on, M);
        last = Some(on.handle(NavEvent::Release { button: M, pos: O, time }));
    }
    assert_outcome(&mut t, "onshape", "M double-click", last.unwrap(), Outcome::Action(Action::Fit));
    // Two clicks too far apart in time are two clicks.
    for time in [1.0, 2.0] {
        press(&mut on, M);
        let up = on.handle(NavEvent::Release { button: M, pos: O, time });
        assert_eq!(up.outcome, Outcome::None);
    }
}

#[test]
fn inventor_held_keys_and_wheel_direction() {
    let mut t = Trace::new("inventor");
    let mut r = router("inventor");
    let none = Modifiers::NONE;
    assert_outcome(
        &mut t,
        "inventor",
        "Shift+M drag",
        gesture(&mut r, &[M], Modifiers::SHIFT),
        Outcome::Action(Action::Orbit),
    );
    assert_outcome(&mut t, "inventor", "M drag", gesture(&mut r, &[M], none), Outcome::Action(Action::Pan));
    for (k, want) in [(Key::F4, Action::Orbit), (Key::F2, Action::Pan), (Key::F3, Action::DragZoom)] {
        key_down(&mut r, k);
        assert_outcome(
            &mut t,
            "inventor",
            &format!("held {k:?}+L"),
            gesture(&mut r, &[L], none),
            Outcome::Action(want),
        );
        assert!(key_up(&mut r, k).consumed);
    }
    key_down(&mut r, Key::F2);
    assert!(!key_up(&mut r, Key::F2).consumed, "an F2 tap still renames");
    let zoom = |r: &mut Router, lines: f32| match r.handle(NavEvent::Wheel { lines }).motion {
        Motion::Zoom { factor } => factor,
        other => panic!("{other:?}"),
    };
    assert!(zoom(&mut r, 1.0) < 1.0, "forward zooms out");
    assert!(zoom(&mut r, -1.0) > 1.0, "backward zooms in");
}

#[test]
fn creo_axis_split_never_falls_back_to_orbit() {
    let mut t = Trace::new("creo");
    let mut r = router("creo");
    let ctrl = mods(true, false, false);
    assert_outcome(&mut t, "creo", "M drag", gesture(&mut r, &[M], Modifiers::NONE), Outcome::Action(Action::Orbit));
    assert_outcome(
        &mut t,
        "creo",
        "Shift+M drag",
        gesture(&mut r, &[M], Modifiers::SHIFT),
        Outcome::Action(Action::Pan),
    );
    let along = |r: &mut Router, moved: Vec2| {
        r.handle(NavEvent::Modifiers(ctrl));
        press(r, M);
        let out = r.handle(NavEvent::Move(O + moved));
        let rest = r.handle(NavEvent::Move(O + moved * 2.0));
        assert_eq!(rest.outcome, out.outcome, "one action for the whole drag");
        end(r, &[M]);
        out
    };
    let up = along(&mut r, vec2(1.0, -10.0));
    assert_outcome(&mut t, "creo", "Ctrl+M vertical", up, Outcome::Action(Action::DragZoom));
    let side = along(&mut r, vec2(10.0, 1.0));
    assert_outcome(&mut t, "creo", "Ctrl+M horizontal", side, Outcome::Unsupported(Action::Roll));
    let diagonal = along(&mut r, vec2(8.0, 8.0));
    assert_outcome(&mut t, "creo", "Ctrl+M diagonal", diagonal, Outcome::Action(Action::DragZoom));
    assert_eq!(along(&mut r, vec2(8.0, 8.0)).binding, Some("zoom"), "a diagonal decides the same way every time");
    let creo = navigation::preset("creo").unwrap();
    assert!(creo.limitations.iter().any(|l| l.kind == LimitKind::NeedsCameraRoll && l.text.contains("roll")));
    assert!(creo.limitations.iter().any(|l| l.kind == LimitKind::HistoricalBinding && l.text.contains("Ctrl+D")));
}

#[test]
fn nx12_ordered_chords_and_historical_label() {
    use soliddraft_app::navigation::view_keys;
    let mut t = Trace::new("nx12");
    let mut r = router("nx12");
    let none = Modifiers::NONE;
    for (buttons, m, name, want) in [
        (&[M][..], none, "M", Action::Orbit),
        (&[M][..], Modifiers::SHIFT, "Shift+M", Action::Pan),
        (&[M, R][..], none, "M+R", Action::Pan),
        (&[M][..], mods(true, false, false), "Ctrl+M", Action::DragZoom),
        (&[L, M][..], none, "L+M", Action::DragZoom),
    ] {
        assert_outcome(&mut t, "nx12", name, gesture(&mut r, buttons, m), Outcome::Action(want));
    }
    let keys = view_keys::analyse("nx12", None, view_keys::PLATFORM).unwrap();
    assert_eq!(
        keys.row_for(NavKey::main(Key::F8), none, false, false).map(|b| b.action),
        Some(Action::NormalOrNearest)
    );
    assert_eq!(keys.row_for(NavKey::main(Key::End), none, false, false).map(|b| b.action), Some(Action::Isometric));
    let nx = navigation::preset("nx12").unwrap();
    assert_eq!(nx.label, "NX 12");
    assert!(nx.limitations.iter().any(|l| l.kind == LimitKind::HistoricalProfile && l.text.contains("not certified")));
    assert!(!nx.label.to_lowercase().contains("current"));
}

#[test]
fn solid_edge_double_click_and_orientation_keys() {
    use soliddraft_app::navigation::view_keys::{self, RowState};
    let mut t = Trace::new("solid-edge");
    let mut r = router("solid_edge");
    let ctrl = mods(true, false, false);
    assert_outcome(&mut t, "solid_edge", "M", gesture(&mut r, &[M], Modifiers::NONE), Outcome::Action(Action::Orbit));
    assert_outcome(
        &mut t,
        "solid_edge",
        "Shift+M",
        gesture(&mut r, &[M], Modifiers::SHIFT),
        Outcome::Action(Action::Pan),
    );
    assert_outcome(
        &mut t,
        "solid_edge",
        "Ctrl+Shift+R",
        gesture(&mut r, &[R], mods(true, true, false)),
        Outcome::Action(Action::Pan),
    );
    assert_outcome(&mut t, "solid_edge", "Ctrl+R", gesture(&mut r, &[R], ctrl), Outcome::Action(Action::DragZoom));
    // A double click without moving fits; one whose second press drags does not.
    let mut last = None;
    for time in [5.0, 5.1] {
        press(&mut r, M);
        last = Some(r.handle(NavEvent::Release { button: M, pos: O, time }));
    }
    assert_outcome(&mut t, "solid_edge", "M double-click", last.unwrap(), Outcome::Action(Action::Fit));
    press(&mut r, M);
    r.handle(NavEvent::Release { button: M, pos: O, time: 9.0 });
    press(&mut r, M);
    let dragged = r.handle(NavEvent::Move(O + PAST));
    assert_outcome(&mut t, "solid_edge", "M click then M drag", dragged, Outcome::Action(Action::Orbit));
    let up = r.handle(NavEvent::Release { button: M, pos: O + PAST, time: 9.1 });
    assert_ne!(up.outcome, Outcome::Action(Action::Fit), "a drag is not a double click");

    let keys = view_keys::analyse("solid_edge", None, view_keys::PLATFORM).unwrap();
    for (row, key, m, action) in [
        ("top", Key::T, ctrl, Action::Top),
        ("bottom", Key::B, Modifiers::NONE, Action::Bottom),
        ("back", Key::K, Modifiers::NONE, Action::Back),
        ("left", Key::L, Modifiers::NONE, Action::Left),
        ("dimetric", Key::J, Modifiers::NONE, Action::Dimetric),
        ("trimetric", Key::M, Modifiers::NONE, Action::Trimetric),
        ("home", Key::Home, Modifiers::NONE, Action::Isometric),
    ] {
        assert_eq!(keys.row_for(NavKey::main(key), m, false, false).map(|b| b.action), Some(action), "{row}");
    }
    for row in ["front", "right", "isometric"] {
        assert!(matches!(keys.state_of(row), Some(RowState::Duplicate(_))), "{row}: SolidDraft's own key runs it once");
    }
    assert!(keys.row_for(NavKey::main(Key::L), Modifiers::NONE, true, false).is_none(), "L in a sketch is Line");
    let se = navigation::preset("solid_edge").unwrap();
    assert!(se.limitations.iter().any(|l| l.kind == LimitKind::SelectionUnverified));
}

#[test]
fn blender_numpad_location_is_required_and_never_main_row() {
    use soliddraft_app::navigation::KeyLocation;
    use soliddraft_app::navigation::view_keys::{self, RowState};
    let pad = |key| NavKey { key, location: KeyLocation::Numpad };
    let none = Modifiers::NONE;
    let ctrl = mods(true, false, false);
    // The rows as data, where the keypad can be told apart.
    let live =
        view_keys::analyse("blender", None, navigation::Supports { numpad_location: true, ..view_keys::PLATFORM })
            .unwrap();
    let act = |key: NavKey, m| live.row_for(key, m, false, false).map(|b| b.action);
    assert_eq!(act(pad(Key::Num1), none), Some(Action::Front));
    assert_eq!(act(NavKey::main(Key::Num1), none), None, "the number row is not the keypad");
    assert_eq!(act(pad(Key::Num1), ctrl), Some(Action::Back));
    assert_eq!(act(NavKey::main(Key::Num1), ctrl), None);
    assert_eq!(act(pad(Key::Num3), none), Some(Action::Right));
    assert_eq!(act(pad(Key::Num7), none), Some(Action::Top));
    assert_eq!(act(pad(Key::Num5), none), Some(Action::ProjectionToggle));
    assert_eq!(act(pad(Key::Period), none), Some(Action::FitSelected));
    assert_eq!(act(pad(Key::Num7), Modifiers::SHIFT), Some(Action::AlignTopToFace));
    let unknown = NavKey { key: Key::Num1, location: KeyLocation::Unknown };
    assert_eq!(act(unknown, none), None, "a key egui could not place is not the keypad's");

    // What builds can do now: egui places neither, so the rows are unavailable and say why.
    assert_eq!(navigation::key_location(Key::Num1), KeyLocation::Unknown);
    assert_eq!(navigation::key_location(Key::Period), KeyLocation::Unknown);
    let now = view_keys::analyse("blender", None, view_keys::PLATFORM).unwrap();
    for row in ["front", "back", "right", "left", "top", "bottom", "projection", "align_top", "fit_selected"] {
        match now.state_of(row) {
            Some(RowState::Unavailable(why)) => assert!(why.contains("keypad"), "{row}: {why}"),
            other => panic!("{row}: {other:?}"),
        }
    }
    for key in [pad(Key::Num1), unknown, NavKey::main(Key::Num1)] {
        assert!(now.row_for(key, none, false, false).is_none(), "{key:?}: nothing, never the digits");
    }
    assert_eq!(now.row_for(NavKey::main(Key::Home), none, false, false).map(|b| b.action), Some(Action::Fit));

    // The adapter that places keys from the platform's key codes, winit's and the browser's.
    for (code, want) in [
        ("Numpad1", KeyLocation::Numpad),
        ("NumpadDecimal", KeyLocation::Numpad),
        ("Digit1", KeyLocation::Main),
        ("Period", KeyLocation::Main),
        ("", KeyLocation::Unknown),
    ] {
        assert_eq!(view_keys::location_from_code(code), want, "{code}");
    }
}
