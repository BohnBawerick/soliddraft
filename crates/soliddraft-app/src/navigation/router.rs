//! The navigation router: one small gesture state machine that turns pointer, wheel and
//! key events into the actions of the active preset's rows. It owns no camera; the
//! viewport applies what it returns.
//!
//! A drag belongs to navigation only when it starts in the viewport and moves past
//! [`NAV_DRAG_THRESHOLD`]; until then the press stays a click. Once past the threshold the
//! gesture is captured until every button is up, wherever the pointer goes. Modifiers
//! match exactly, physical Ctrl is not the platform's command key, held ordinary keys
//! count as modifiers, and the most specific row wins.

use super::{
    Action, Binding, Button, Capability, Chord, DragAxis, EventKind, KeyLocation, Mods, NavContext, NavKey,
    NavigationSettings, Preset, ZoomDirection,
};
use egui::{Pos2, Vec2};

/// How far the pointer moves before a press becomes a drag: egui's click distance, so a
/// press that navigates is never also a click and one that does not stays one.
pub const NAV_DRAG_THRESHOLD: f32 = 6.0;

/// The longest gap between two clicks of a double click, in seconds.
const DOUBLE_CLICK_SECONDS: f64 = 0.3;

/// The zoom of one wheel line: the natural log of its factor, as SolidDraft always had.
const LINE_ZOOM: f32 = 0.1;

/// The zoom of one point of high-resolution scrolling.
const POINT_ZOOM: f32 = 0.0025;

/// What the router reads: pointer, key and wheel input, one event at a time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NavEvent {
    /// The modifier keys now down.
    Modifiers(egui::Modifiers),
    /// `inside`: the press is on the viewport itself, not on a panel or an overlay.
    Press {
        button: Button,
        pos: Pos2,
        inside: bool,
        time: f64,
    },
    Release {
        button: Button,
        pos: Pos2,
        time: f64,
    },
    Move(Pos2),
    KeyDown(NavKey),
    KeyUp(NavKey),
    /// Wheel lines this frame, forward positive.
    Wheel {
        lines: f32,
    },
    /// High-resolution scrolling this frame, in points, as a trackpad sends.
    SmoothScroll {
        delta: Vec2,
    },
    /// A pinch this frame: its zoom factor, above 1 to zoom in.
    Pinch {
        factor: f32,
    },
}

/// What an event did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    None,
    Action(Action),
    /// The row matched, but SolidDraft cannot do it yet. Nothing else runs in its place.
    Unsupported(Action),
}

/// How far an action goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Motion {
    None,
    /// Pointer movement since the last event, and where the drag started.
    Drag {
        delta: Vec2,
        origin: Pos2,
    },
    /// A zoom factor, above 1 to zoom in.
    Zoom {
        factor: f32,
    },
}

/// The result of one event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Routed {
    pub outcome: Outcome,
    /// The row that decided it.
    pub binding: Option<&'static str>,
    /// Navigation used the input: a release that ends a navigation drag is no click,
    /// and a held key used for a drag runs no command of its own.
    pub consumed: bool,
    pub motion: Motion,
}

impl Routed {
    const NONE: Routed = Routed { outcome: Outcome::None, binding: None, consumed: false, motion: Motion::None };

    fn consumed() -> Routed {
        Routed { consumed: true, ..Routed::NONE }
    }
}

/// What the camera and the input layer can do. Rows that need more say so.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Supports {
    pub camera_roll: bool,
    pub numpad_location: bool,
    pub ordered_buttons: bool,
}

impl Supports {
    fn has(self, c: Capability) -> bool {
        match c {
            Capability::None => true,
            Capability::CameraRoll => self.camera_roll,
            Capability::NumpadLocation => self.numpad_location,
            Capability::OrderedButtons => self.ordered_buttons,
        }
    }
}

/// What two-finger scrolling on a trackpad does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrackpadProfile {
    /// Scrolling zooms, as SolidDraft always had.
    #[default]
    Soliddraft,
    /// Fusion's Mac gestures: scrolling pans, Shift and scrolling orbits.
    FusionMac,
}

impl TrackpadProfile {
    pub const IDS: [&'static str; 2] = ["soliddraft", "fusion_mac"];

    /// The profile with this id; SolidDraft's for one this build does not know.
    pub fn from_id(id: &str) -> Self {
        match id {
            "fusion_mac" => TrackpadProfile::FusionMac,
            _ => TrackpadProfile::Soliddraft,
        }
    }
}

/// Everything besides the rows that decides what an event does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RouterConfig {
    /// Command is the platform key, and Ctrl stays Ctrl.
    pub mac: bool,
    /// What a forward wheel turn does, after the user's invert setting.
    pub wheel_forward: ZoomDirection,
    pub wheel_speed: f32,
    pub trackpad: TrackpadProfile,
    pub supports: Supports,
}

impl RouterConfig {
    fn of(preset: &Preset, settings: Option<&NavigationSettings>, mac: bool) -> Self {
        let forward = preset.wheel.forward();
        let invert = settings.is_some_and(|s| s.wheel.invert);
        Self {
            mac,
            wheel_forward: match (forward, invert) {
                (ZoomDirection::In, false) | (ZoomDirection::Out, true) => ZoomDirection::In,
                _ => ZoomDirection::Out,
            },
            wheel_speed: settings.map_or(1.0, |s| s.wheel.speed),
            trackpad: settings.map_or(TrackpadProfile::Soliddraft, |s| TrackpadProfile::from_id(&s.trackpad)),
            supports: Supports::default(),
        }
    }
}

/// The platform's primary modifier for selection: Command on macOS, Ctrl elsewhere.
/// Navigation never uses it for Ctrl rows.
pub fn selection_primary(modifiers: egui::Modifiers, mac: bool) -> bool {
    if mac { modifiers.mac_cmd } else { modifiers.ctrl || modifiers.command }
}

/// Whether the modifiers held are exactly what the row asks, comparing each key. Off
/// macOS Ctrl is also the platform key, so a row's `platform` means Ctrl there.
pub fn modifiers_match(row: &Binding, held: egui::Modifiers, mac: bool) -> bool {
    let (required, forbidden, pressed) = if mac {
        (
            row.required,
            row.forbidden,
            Mods { ctrl: held.ctrl, shift: held.shift, alt: held.alt, platform: held.mac_cmd },
        )
    } else {
        let ctrl = row.required.ctrl || row.required.platform;
        let required = Mods { ctrl, platform: false, ..row.required };
        let forbidden =
            Mods { ctrl: (row.forbidden.ctrl || row.forbidden.platform) && !ctrl, platform: false, ..row.forbidden };
        let pressed = Mods { ctrl: held.ctrl || held.command, shift: held.shift, alt: held.alt, platform: false };
        (required, forbidden, pressed)
    };
    (pressed & required) == required && !pressed.intersects(forbidden)
}

fn same_set<T: PartialEq>(a: &[T], b: &[T]) -> bool {
    a.len() == b.len() && a.iter().all(|x| b.contains(x))
}

/// The drag a router has captured.
#[derive(Clone, Copy, Debug)]
struct Capture {
    origin: Pos2,
    last: Pos2,
    /// Past the threshold: the row it latched, if any.
    latched: Option<Option<&'static str>>,
}

/// The gesture state machine.
#[derive(Clone, Debug)]
pub struct Router {
    preset: Preset,
    pub config: RouterConfig,
    /// The settings the preset and config were made from.
    source: Option<NavigationSettings>,
    /// Where the pointer is: the 3D view outside or inside a sketch.
    pub context: NavContext,
    modifiers: egui::Modifiers,
    buttons: Vec<Button>,
    /// Held ordinary keys the preset navigates with, and whether a drag used them.
    held: Vec<(NavKey, bool)>,
    capture: Option<Capture>,
    /// The last click that did not navigate: button, place and time.
    last_click: Option<(Button, Pos2, f64)>,
}

impl Router {
    /// A router for the user's settings: their preset with their changes.
    pub fn new(settings: &NavigationSettings, mac: bool) -> Self {
        let mut r = Self::for_preset(settings.active_preset().clone(), mac);
        r.configure(settings, mac);
        r
    }

    /// A router for these rows with the default wheel, trackpad and camera.
    pub fn for_preset(preset: Preset, mac: bool) -> Self {
        Self {
            config: RouterConfig::of(&preset, None, mac),
            preset,
            source: None,
            context: NavContext::Model,
            modifiers: egui::Modifiers::NONE,
            buttons: Vec::new(),
            held: Vec::new(),
            capture: None,
            last_click: None,
        }
    }

    /// Follows the settings when they changed. A gesture in progress keeps going.
    pub fn configure(&mut self, settings: &NavigationSettings, mac: bool) {
        if self.source.as_ref() == Some(settings) && self.config.mac == mac {
            return;
        }
        self.preset = settings.effective(settings.active_preset());
        self.config =
            RouterConfig { supports: self.config.supports, ..RouterConfig::of(&self.preset, Some(settings), mac) };
        self.source = Some(settings.clone());
    }

    pub fn preset(&self) -> &Preset {
        &self.preset
    }

    pub fn binding(&self, id: &str) -> Option<&Binding> {
        self.preset.binding(id)
    }

    /// The buttons of the captured gesture.
    pub fn buttons(&self) -> &[Button] {
        &self.buttons
    }

    /// A press in the viewport is being tracked.
    pub fn captured(&self) -> bool {
        self.capture.is_some()
    }

    /// The captured press moved past the threshold onto a row: it is a navigation drag,
    /// not a click. A drag no row takes, such as the left button in a sketch, is left to
    /// the tools.
    pub fn dragging(&self) -> bool {
        self.capture.is_some_and(|c| matches!(c.latched, Some(Some(_))))
    }

    /// The gesture held now, as a row's chord: a drag with these buttons, held keys and
    /// modifiers. For capturing a new gesture.
    pub fn gesture(&self) -> Chord {
        let m = self.modifiers;
        let platform = self.config.mac && m.mac_cmd;
        Chord {
            event: EventKind::Drag,
            buttons: self.buttons.clone(),
            held_keys: self.held.iter().map(|(k, _)| *k).collect(),
            required: Mods { ctrl: m.ctrl || (!self.config.mac && m.command), shift: m.shift, alt: m.alt, platform },
            ..Chord::default()
        }
    }

    /// The captured press moved past the threshold, whether or not a row took it.
    pub fn moved_past_threshold(&self) -> bool {
        self.capture.is_some_and(|c| c.latched.is_some())
    }

    /// The preset navigates while this key is held.
    pub fn is_hold_key(&self, key: NavKey) -> bool {
        self.preset.bindings.iter().any(|b| b.held_keys.iter().any(|k| k.key == key.key))
    }

    /// A drag happened while this key was held, so releasing it runs nothing.
    pub fn held_key_used(&self, key: NavKey) -> bool {
        self.held.iter().any(|(k, used)| k.key == key.key && *used)
    }

    /// Reads one event.
    pub fn handle(&mut self, event: NavEvent) -> Routed {
        match event {
            NavEvent::Modifiers(m) => {
                let added = (m.ctrl && !self.modifiers.ctrl)
                    || (m.shift && !self.modifiers.shift)
                    || (m.alt && !self.modifiers.alt)
                    || (m.mac_cmd && !self.modifiers.mac_cmd)
                    || (m.command && !self.modifiers.command);
                self.modifiers = m;
                if added && self.moved_past_threshold() { self.relatch() } else { Routed::NONE }
            }
            NavEvent::Press { button, pos, inside, .. } => {
                let Some(_) = self.capture else {
                    if !inside {
                        return Routed::NONE;
                    }
                    self.capture = Some(Capture { origin: pos, last: pos, latched: None });
                    self.buttons = vec![button];
                    return Routed::NONE;
                };
                if !self.buttons.contains(&button) {
                    self.buttons.push(button);
                }
                if self.moved_past_threshold() { self.relatch() } else { Routed::NONE }
            }
            NavEvent::Move(pos) => self.moved(pos),
            NavEvent::Release { button, pos, time } => self.released(button, pos, time),
            NavEvent::KeyDown(key) => {
                if !self.is_hold_key(key) || self.held.iter().any(|(k, _)| k.key == key.key) {
                    return Routed::NONE;
                }
                self.held.push((key, false));
                if self.moved_past_threshold() { self.relatch() } else { Routed::NONE }
            }
            NavEvent::KeyUp(key) => {
                let Some(i) = self.held.iter().position(|(k, _)| k.key == key.key) else { return Routed::NONE };
                let (_, used) = self.held.remove(i);
                Routed { consumed: used, ..Routed::NONE }
            }
            NavEvent::Wheel { lines } => {
                let sign = if self.config.wheel_forward == ZoomDirection::In { 1.0 } else { -1.0 };
                let factor = (lines * LINE_ZOOM * self.config.wheel_speed * sign).exp();
                self.simple(EventKind::Wheel, None, Motion::Zoom { factor })
            }
            NavEvent::SmoothScroll { delta } => self.scrolled(delta),
            NavEvent::Pinch { factor } => self.simple(EventKind::Pinch, None, Motion::Zoom { factor }),
        }
    }

    fn moved(&mut self, pos: Pos2) -> Routed {
        let Some(mut c) = self.capture else { return Routed::NONE };
        let routed = match c.latched {
            None if (pos - c.origin).length() > NAV_DRAG_THRESHOLD => {
                let row = self.drag_row(pos - c.origin).cloned();
                c.latched = Some(row.as_ref().map(|b| b.id));
                self.capture = Some(c);
                self.use_held_keys(row.as_ref());
                self.routed(row.as_ref(), Motion::Drag { delta: pos - c.origin, origin: c.origin })
            }
            None => Routed::NONE,
            Some(id) => {
                let row = id.and_then(|id| self.preset.binding(id)).cloned();
                self.routed(row.as_ref(), Motion::Drag { delta: pos - c.last, origin: c.origin })
            }
        };
        if c.latched.is_some() {
            c.last = pos;
        }
        self.capture = Some(c);
        routed
    }

    fn released(&mut self, button: Button, pos: Pos2, time: f64) -> Routed {
        let Some(c) = self.capture else { return Routed::NONE };
        self.buttons.retain(|b| *b != button);
        let dragged = matches!(c.latched, Some(Some(_)));
        let moved = c.latched.is_some();
        if !self.buttons.is_empty() {
            return if dragged { Routed::consumed() } else { Routed::NONE };
        }
        self.capture = None;
        if moved {
            self.last_click = None;
            return if dragged { Routed::consumed() } else { Routed::NONE };
        }
        let double = self.last_click.is_some_and(|(b, p, t)| {
            b == button && time - t <= DOUBLE_CLICK_SECONDS && (pos - p).length() <= NAV_DRAG_THRESHOLD
        });
        if !double {
            self.last_click = Some((button, pos, time));
            return Routed::NONE;
        }
        self.last_click = None;
        let routed = self.simple(EventKind::DoubleClick, Some(button), Motion::None);
        if routed.binding.is_some() { Routed { consumed: true, ..routed } } else { routed }
    }

    fn scrolled(&mut self, delta: Vec2) -> Routed {
        match self.config.trackpad {
            TrackpadProfile::Soliddraft => {
                let factor = (delta.y * POINT_ZOOM * self.config.wheel_speed).exp();
                self.simple(EventKind::Wheel, None, Motion::Zoom { factor })
            }
            TrackpadProfile::FusionMac => {
                let action = if self.modifiers.shift { Action::Orbit } else { Action::Pan };
                // Scrolling moves the content; the camera goes the other way.
                Routed {
                    outcome: Outcome::Action(action),
                    binding: None,
                    consumed: true,
                    motion: Motion::Drag { delta: -delta, origin: Pos2::ZERO },
                }
            }
        }
    }

    /// Re-decides a captured drag after a button, key or modifier went down.
    fn relatch(&mut self) -> Routed {
        let Some(mut c) = self.capture else { return Routed::NONE };
        let row = self.drag_row(c.last - c.origin).cloned();
        c.latched = Some(row.as_ref().map(|b| b.id));
        self.capture = Some(c);
        self.use_held_keys(row.as_ref());
        self.routed(row.as_ref(), Motion::None)
    }

    /// A navigation drag uses every ordinary key held during it: none runs its command
    /// when it comes up.
    fn use_held_keys(&mut self, row: Option<&Binding>) {
        if row.is_some() {
            for (_, used) in &mut self.held {
                *used = true;
            }
        }
    }

    fn context_applies(&self, context: NavContext) -> bool {
        match context {
            NavContext::Viewport => true,
            NavContext::Unknown => false,
            c => c == self.context,
        }
    }

    /// The most specific drag row for the buttons, held keys and modifiers down now.
    fn drag_row(&self, moved: Vec2) -> Option<&Binding> {
        let axis = if moved.y.abs() >= moved.x.abs() { DragAxis::Vertical } else { DragAxis::Horizontal };
        let held: Vec<NavKey> = self.held.iter().map(|(k, _)| *k).collect();
        self.preset
            .bindings
            .iter()
            .filter(|b| b.event == EventKind::Drag)
            .filter(|b| same_set(&b.buttons, &self.buttons))
            .filter(|b| {
                b.held_keys.len() == held.len() && b.held_keys.iter().all(|k| held.iter().any(|h| h.key == k.key))
            })
            .filter(|b| self.context_applies(b.context))
            .filter(|b| b.axis == DragAxis::Any || b.axis == axis)
            .filter(|b| modifiers_match(b, self.modifiers, self.config.mac))
            .max_by_key(|b| b.priority)
    }

    /// The row for a wheel turn, a pinch or a double click.
    fn simple(&self, event: EventKind, button: Option<Button>, motion: Motion) -> Routed {
        let row = self
            .preset
            .bindings
            .iter()
            .filter(|b| b.event == event)
            .filter(|b| button.is_none_or(|x| b.buttons == [x]))
            .filter(|b| self.context_applies(b.context))
            .filter(|b| modifiers_match(b, self.modifiers, self.config.mac))
            .max_by_key(|b| b.priority);
        let routed = self.routed(row, motion);
        Routed { consumed: row.is_some(), ..routed }
    }

    fn routed(&self, row: Option<&Binding>, motion: Motion) -> Routed {
        let Some(b) = row else {
            // A captured drag with no row still belongs to navigation: it selects nothing.
            return Routed { consumed: self.dragging(), ..Routed::NONE };
        };
        let outcome = if self.config.supports.has(b.requires) {
            Outcome::Action(b.action)
        } else {
            Outcome::Unsupported(b.action)
        };
        Routed { outcome, binding: Some(b.id), consumed: true, motion }
    }
}

/// Where a key is, as far as egui says: keys found both on the number row and on the
/// keypad cannot be placed.
pub fn key_location(key: egui::Key) -> KeyLocation {
    use egui::Key::*;
    match key {
        Num0 | Num1 | Num2 | Num3 | Num4 | Num5 | Num6 | Num7 | Num8 | Num9 | Period | Plus | Minus | Enter | Slash => {
            KeyLocation::Unknown
        }
        _ => KeyLocation::Main,
    }
}

fn button_of(b: egui::PointerButton) -> Option<Button> {
    match b {
        egui::PointerButton::Primary => Some(Button::Left),
        egui::PointerButton::Middle => Some(Button::Middle),
        egui::PointerButton::Secondary => Some(Button::Right),
        _ => None,
    }
}

/// The egui button for a navigation button.
pub fn egui_button(b: Button) -> egui::PointerButton {
    match b {
        Button::Left => egui::PointerButton::Primary,
        Button::Middle => egui::PointerButton::Middle,
        Button::Right => egui::PointerButton::Secondary,
    }
}

/// Turns one frame's egui events into navigation events. `inside` says whether a press
/// at a point is on the viewport itself; `hovered` whether the pointer is over it for the
/// wheel and pinches. One physical zoom gesture gives one event: a frame's pinches
/// multiply into one, and a Ctrl wheel beside them, the browser's imitation of a pinch,
/// is dropped; wheel lines and high-resolution scrolling each add up into one.
pub fn gather(events: &[egui::Event], time: f64, inside: impl Fn(Pos2) -> bool, hovered: bool) -> Vec<NavEvent> {
    let pinched = events.iter().any(|e| matches!(e, egui::Event::Zoom(_)));
    let mut out = Vec::new();
    let (mut lines, mut smooth, mut pinch) = (0.0, Vec2::ZERO, 1.0);
    for e in events {
        match e {
            egui::Event::ModifiersChanged(m) => out.push(NavEvent::Modifiers(*m)),
            egui::Event::PointerButton { pos, button, pressed, modifiers } => {
                let Some(button) = button_of(*button) else { continue };
                out.push(NavEvent::Modifiers(*modifiers));
                out.push(if *pressed {
                    NavEvent::Press { button, pos: *pos, inside: inside(*pos), time }
                } else {
                    NavEvent::Release { button, pos: *pos, time }
                });
            }
            egui::Event::PointerMoved(pos) => out.push(NavEvent::Move(*pos)),
            egui::Event::Key { key, physical_key, pressed, repeat, modifiers } => {
                if *repeat {
                    continue;
                }
                let key = physical_key.unwrap_or(*key);
                let k = NavKey { key, location: key_location(key) };
                out.push(NavEvent::Modifiers(*modifiers));
                out.push(if *pressed { NavEvent::KeyDown(k) } else { NavEvent::KeyUp(k) });
            }
            egui::Event::MouseWheel { unit, delta, modifiers, .. } if hovered => {
                if pinched && (modifiers.ctrl || modifiers.command) {
                    continue;
                }
                out.push(NavEvent::Modifiers(*modifiers));
                match unit {
                    egui::MouseWheelUnit::Line => lines += delta.y,
                    egui::MouseWheelUnit::Page => lines += delta.y * 10.0,
                    egui::MouseWheelUnit::Point => smooth += *delta,
                }
            }
            egui::Event::Zoom(f) if hovered => pinch *= f,
            _ => {}
        }
    }
    if pinch != 1.0 {
        out.push(NavEvent::Pinch { factor: pinch });
    }
    if lines != 0.0 {
        out.push(NavEvent::Wheel { lines });
    }
    if smooth != Vec2::ZERO {
        out.push(NavEvent::SmoothScroll { delta: smooth });
    }
    out
}
