//! Navigation presets as data: which mouse buttons, held keys and modifiers orbit, pan
//! and zoom the 3D view, and which keys fit it or turn it normal to a face, in the
//! style of another CAD application. The user picks a preset in the settings
//! ([`NavigationSettings`]); the rows say what each gesture does, and the limitations
//! say plainly what SolidDraft cannot match yet.
//!
//! The rows follow the navigation research's recommended preset table. They describe
//! the named product's documented defaults, not a complete copy of its behaviour. FreeCAD
//! and the CATIA profile need ordered button presses and are not offered; the schema can
//! still describe them ([`EventKind::Sequence`]).

mod presets;
mod router;
mod settings;
pub mod view_keys;

pub use router::{
    Motion, NAV_DRAG_THRESHOLD, NavEvent, Outcome, Routed, Router, RouterConfig, Supports, TrackpadProfile,
    egui_button, gather, key_location, modifiers_match, selection_primary,
};
pub use settings::{
    CameraSettings, KEYBOARD_PROFILE, NavigationSettings, Pivot, SaveError, TRACKPAD_PROFILE, VERSION, WheelSettings,
    default_preset,
};

use egui::Key;
use serde::{Deserialize, Serialize};

/// A mouse button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Left,
    Middle,
    Right,
}

/// What the user does to trigger a binding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// Holding the buttons down together and moving past the drag threshold.
    #[default]
    Drag,
    /// The buttons pressed and released in the order the steps give.
    Sequence,
    /// A double click without moving.
    DoubleClick,
    /// A line-by-line mouse wheel.
    Wheel,
    /// A high-resolution scroll, as a trackpad sends.
    SmoothScroll,
    /// A pinch on a trackpad or touch screen.
    Pinch,
    /// A key press.
    Key,
}

/// Where a key is on the keyboard. The numeric keypad's digits are not the number row's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyLocation {
    #[default]
    Main,
    Numpad,
    /// The platform did not say.
    Unknown,
}

/// A key and where it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NavKey {
    pub key: Key,
    pub location: KeyLocation,
}

impl NavKey {
    pub const fn main(key: Key) -> Self {
        Self { key, location: KeyLocation::Main }
    }

    pub const fn numpad(key: Key) -> Self {
        Self { key, location: KeyLocation::Numpad }
    }

    /// As the interface writes it: "F4", "Num .".
    pub fn text(&self) -> String {
        match self.location {
            KeyLocation::Numpad => format!("Num {}", self.key.symbol_or_name()),
            _ => self.key.symbol_or_name().to_owned(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct NavKeyRepr {
    key: String,
    #[serde(default)]
    location: KeyLocation,
}

impl Serialize for NavKey {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        NavKeyRepr { key: self.key.name().to_owned(), location: self.location }.serialize(s)
    }
}

impl<'de> Deserialize<'de> for NavKey {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let r = NavKeyRepr::deserialize(d)?;
        let key = Key::from_name(&r.key).ok_or_else(|| serde::de::Error::custom(format!("unknown key {}", r.key)))?;
        Ok(Self { key, location: r.location })
    }
}

/// Modifier keys. `ctrl` is the physical Control key on every platform; `platform` is
/// the platform's command modifier, Command on macOS and Control elsewhere.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub platform: bool,
}

impl Mods {
    pub const NONE: Mods = Mods { ctrl: false, shift: false, alt: false, platform: false };
    pub const CTRL: Mods = Mods { ctrl: true, ..Mods::NONE };
    pub const SHIFT: Mods = Mods { shift: true, ..Mods::NONE };
    pub const ALT: Mods = Mods { alt: true, ..Mods::NONE };
    pub const PLATFORM: Mods = Mods { platform: true, ..Mods::NONE };
    pub const ALL: Mods = Mods { ctrl: true, shift: true, alt: true, platform: true };

    const NAMES: [&'static str; 4] = ["ctrl", "shift", "alt", "platform"];

    fn bits(self) -> [bool; 4] {
        [self.ctrl, self.shift, self.alt, self.platform]
    }

    fn from_bits(b: [bool; 4]) -> Self {
        Mods { ctrl: b[0], shift: b[1], alt: b[2], platform: b[3] }
    }

    pub fn is_empty(self) -> bool {
        self == Mods::NONE
    }

    pub fn intersects(self, other: Mods) -> bool {
        !(self & other).is_empty()
    }

    /// As the interface writes it, "Ctrl+Shift", empty for none. `platform` is written
    /// "Cmd" on macOS.
    pub fn text(self, mac: bool) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.platform {
            parts.push(if mac { "Cmd" } else { "Ctrl" });
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.alt {
            parts.push(if mac { "Option" } else { "Alt" });
        }
        parts.join("+")
    }
}

impl std::ops::BitOr for Mods {
    type Output = Mods;
    fn bitor(self, o: Mods) -> Mods {
        let (a, b) = (self.bits(), o.bits());
        Mods::from_bits(std::array::from_fn(|i| a[i] || b[i]))
    }
}

impl std::ops::BitAnd for Mods {
    type Output = Mods;
    fn bitand(self, o: Mods) -> Mods {
        let (a, b) = (self.bits(), o.bits());
        Mods::from_bits(std::array::from_fn(|i| a[i] && b[i]))
    }
}

impl std::ops::Not for Mods {
    type Output = Mods;
    fn not(self) -> Mods {
        Mods::from_bits(self.bits().map(|b| !b))
    }
}

/// Written as a list of names, `["ctrl", "shift"]`.
impl Serialize for Mods {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let names: Vec<&str> = self.bits().iter().zip(Mods::NAMES).filter(|(b, _)| **b).map(|(_, n)| n).collect();
        names.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Mods {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let names = Vec::<String>::deserialize(d)?;
        let mut bits = [false; 4];
        for name in names {
            let i = Mods::NAMES
                .iter()
                .position(|n| *n == name)
                .ok_or_else(|| serde::de::Error::custom(format!("unknown modifier {name}")))?;
            bits[i] = true;
        }
        Ok(Mods::from_bits(bits))
    }
}

/// Where in the application a binding applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NavContext {
    /// The 3D view, in a sketch or not.
    Viewport,
    /// The 3D view outside a sketch.
    Model,
    /// The 3D view inside a sketch.
    Sketch,
    /// A context a newer SolidDraft named. Never matches.
    #[serde(other)]
    Unknown,
}

/// What a binding does to the view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Orbit,
    Pan,
    /// Zoom by dragging.
    DragZoom,
    /// Turn the view about the line of sight.
    Roll,
    /// Zoom by the wheel or a pinch.
    WheelZoom,
    /// Fit everything.
    Fit,
    /// Fit the selection.
    FitSelected,
    /// Look straight at the selected face or plane.
    NormalTo,
    /// Normal to the selection; again, from the other side.
    NormalToggle,
    /// Normal to the selected planar face; with none, the nearest principal view.
    NormalOrNearest,
    Isometric,
    /// The application's default orientation.
    DefaultView,
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
    Dimetric,
    Trimetric,
    /// Between perspective and parallel projection.
    ProjectionToggle,
    /// Top view turned so the selected face's normal points at the viewer.
    AlignTopToFace,
    /// An action a newer SolidDraft named. Never runs.
    #[serde(other)]
    Unknown,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Orbit => "Orbit",
            Action::Pan => "Pan",
            Action::DragZoom | Action::WheelZoom => "Zoom",
            Action::Roll => "Roll",
            Action::Fit => "Fit",
            Action::FitSelected => "Fit selected",
            Action::NormalTo => "Normal to",
            Action::NormalToggle => "Normal to, again to reverse",
            Action::NormalOrNearest => "Normal to, or nearest standard view",
            Action::Isometric => "Isometric",
            Action::DefaultView => "Default view",
            Action::Front => "Front",
            Action::Back => "Back",
            Action::Left => "Left",
            Action::Right => "Right",
            Action::Top => "Top",
            Action::Bottom => "Bottom",
            Action::Dimetric => "Dimetric",
            Action::Trimetric => "Trimetric",
            Action::ProjectionToggle => "Perspective or parallel",
            Action::AlignTopToFace => "Align view to the active face",
            Action::Unknown => "Unknown",
        }
    }
}

/// Which way a drag must mostly go for the binding to apply. Creo zooms on a vertical
/// Ctrl+middle drag and rolls on a horizontal one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DragAxis {
    #[default]
    Any,
    Vertical,
    Horizontal,
}

/// The point a zoom keeps still.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZoomAnchor {
    #[default]
    Cursor,
    Center,
}

/// What a binding needs beyond the table. A binding that needs something must come with
/// a limitation of the matching kind, so the interface can say it does not work yet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    #[default]
    None,
    /// Keypad keys told from the number row.
    NumpadLocation,
    /// A camera that turns about the line of sight.
    CameraRoll,
    /// Recognising buttons pressed and released in order.
    OrderedButtons,
}

/// One step of a [`EventKind::Sequence`]: a button going down or up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Step {
    pub button: Button,
    pub down: bool,
}

/// One gesture and what it does.
#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    /// Stable within its preset: overrides refer to it.
    pub id: &'static str,
    pub event: EventKind,
    /// Held together, in any order. Empty for the wheel, pinches and keys.
    pub buttons: Vec<Button>,
    /// The key of a [`EventKind::Key`] binding.
    pub key: Option<NavKey>,
    /// Ordinary keys held during the gesture, as Inventor's F2, F3 and F4.
    pub held_keys: Vec<NavKey>,
    pub required: Mods,
    /// Modifiers that stop the binding. Every modifier not required, unless the row
    /// allows extras.
    pub forbidden: Mods,
    /// Whether modifiers beyond `required` may be held: the ones neither required nor
    /// forbidden.
    pub allow_extra_modifiers: bool,
    pub context: NavContext,
    pub action: Action,
    /// Higher is tried first. Unique within a preset.
    pub priority: u16,
    pub axis: DragAxis,
    pub anchor: ZoomAnchor,
    /// The ordered steps of a [`EventKind::Sequence`].
    pub sequence: Vec<Step>,
    pub requires: Capability,
}

impl Binding {
    fn new(id: &'static str, event: EventKind, action: Action, priority: u16) -> Self {
        Self {
            id,
            event,
            buttons: Vec::new(),
            key: None,
            held_keys: Vec::new(),
            required: Mods::NONE,
            forbidden: Mods::ALL,
            allow_extra_modifiers: false,
            context: NavContext::Viewport,
            action,
            priority,
            axis: DragAxis::Any,
            anchor: ZoomAnchor::Cursor,
            sequence: Vec::new(),
            requires: Capability::None,
        }
    }

    /// A drag with `buttons` held and exactly the `required` modifiers.
    pub fn drag(id: &'static str, buttons: &[Button], required: Mods, action: Action, priority: u16) -> Self {
        Self {
            buttons: buttons.to_vec(),
            required,
            forbidden: !required,
            ..Self::new(id, EventKind::Drag, action, priority)
        }
    }

    /// A double click of `button` with no modifiers.
    pub fn double_click(id: &'static str, button: Button, action: Action, priority: u16) -> Self {
        Self { buttons: vec![button], ..Self::new(id, EventKind::DoubleClick, action, priority) }
    }

    /// A press of `key` on the main keyboard with exactly the `required` modifiers.
    pub fn key(id: &'static str, key: Key, required: Mods, action: Action, priority: u16) -> Self {
        Self {
            key: Some(NavKey::main(key)),
            required,
            forbidden: !required,
            ..Self::new(id, EventKind::Key, action, priority)
        }
    }

    /// Buttons pressed and released in order.
    pub fn sequence(id: &'static str, steps: &[Step], action: Action, priority: u16) -> Self {
        let mut buttons: Vec<Button> = Vec::new();
        for s in steps {
            if !buttons.contains(&s.button) {
                buttons.push(s.button);
            }
        }
        Self { buttons, sequence: steps.to_vec(), ..Self::new(id, EventKind::Sequence, action, priority) }
    }

    /// Zooming with the wheel, whatever modifiers are held.
    pub fn wheel(id: &'static str, priority: u16) -> Self {
        Self {
            forbidden: Mods::NONE,
            allow_extra_modifiers: true,
            ..Self::new(id, EventKind::Wheel, Action::WheelZoom, priority)
        }
    }

    /// Zooming with a pinch, whatever modifiers are held.
    pub fn pinch(id: &'static str, priority: u16) -> Self {
        Self {
            forbidden: Mods::NONE,
            allow_extra_modifiers: true,
            ..Self::new(id, EventKind::Pinch, Action::WheelZoom, priority)
        }
    }

    /// Only while these ordinary keys are held.
    pub fn held(mut self, keys: &[Key]) -> Self {
        self.held_keys = keys.iter().map(|k| NavKey::main(*k)).collect();
        self
    }

    /// The key is the numeric keypad's.
    pub fn on_numpad(mut self) -> Self {
        self.key = self.key.map(|k| NavKey::numpad(k.key));
        self
    }

    pub fn in_context(mut self, context: NavContext) -> Self {
        self.context = context;
        self
    }

    pub fn along(mut self, axis: DragAxis) -> Self {
        self.axis = axis;
        self
    }

    pub fn anchored(mut self, anchor: ZoomAnchor) -> Self {
        self.anchor = anchor;
        self
    }

    pub fn needs(mut self, capability: Capability) -> Self {
        self.requires = capability;
        self
    }

    /// The part of the row a user can change.
    pub fn chord(&self) -> Chord {
        Chord {
            event: self.event,
            buttons: self.buttons.clone(),
            key: self.key,
            held_keys: self.held_keys.clone(),
            required: self.required,
            sequence: self.sequence.clone(),
        }
    }

    /// The row with its gesture replaced. A row that allows no extras forbids every
    /// modifier the new chord does not require.
    pub fn with_chord(&self, chord: &Chord) -> Binding {
        let forbidden = if self.allow_extra_modifiers { self.forbidden & !chord.required } else { !chord.required };
        Binding {
            event: chord.event,
            buttons: chord.buttons.clone(),
            key: chord.key,
            held_keys: chord.held_keys.clone(),
            required: chord.required,
            forbidden,
            sequence: chord.sequence.clone(),
            ..self.clone()
        }
    }

    /// As the interface writes the gesture: "Ctrl+Middle drag", "Held F4+Left drag",
    /// "F6".
    pub fn text(&self, mac: bool) -> String {
        let mut parts = Vec::new();
        for k in &self.held_keys {
            parts.push(format!("Held {}", k.text()));
        }
        let mods = self.required.text(mac);
        if !mods.is_empty() {
            parts.push(mods);
        }
        let buttons = self.buttons.iter().map(|b| format!("{b:?}")).collect::<Vec<_>>().join("+");
        parts.push(match self.event {
            EventKind::Drag => format!("{buttons} drag"),
            EventKind::DoubleClick => format!("{buttons} double-click"),
            EventKind::Sequence => self
                .sequence
                .iter()
                .map(|s| format!("{:?} {}", s.button, if s.down { "down" } else { "up" }))
                .collect::<Vec<_>>()
                .join(", "),
            EventKind::Wheel => "Wheel".into(),
            EventKind::SmoothScroll => "Scroll".into(),
            EventKind::Pinch => "Pinch".into(),
            EventKind::Key => self.key.map(|k| k.text()).unwrap_or_default(),
        });
        parts.join("+")
    }
}

/// The gesture of a binding, as an override saves it in the settings.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Chord {
    pub event: EventKind,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub buttons: Vec<Button>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<NavKey>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub held_keys: Vec<NavKey>,
    pub required: Mods,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sequence: Vec<Step>,
}

/// Which way a forward (away from the user) wheel turn zooms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZoomDirection {
    In,
    Out,
}

/// A preset's wheel direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelPolicy {
    /// SolidDraft's own direction, forward zooms in, because the product's documentation
    /// does not say.
    SoliddraftDefault,
    /// The direction the product documents.
    Explicit { forward: ZoomDirection },
}

impl WheelPolicy {
    /// What a forward turn does before the user's invert setting.
    pub fn forward(self) -> ZoomDirection {
        match self {
            WheelPolicy::SoliddraftDefault => ZoomDirection::In,
            WheelPolicy::Explicit { forward } => forward,
        }
    }
}

/// What a limitation is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LimitKind {
    /// Background on the profile; nothing is missing.
    Note,
    /// The whole profile follows an older release's documentation.
    HistoricalProfile,
    /// One binding comes from an older release's documentation.
    HistoricalBinding,
    /// A documented binding is left out because the sources disagree.
    Unresolved,
    /// Selection does not follow the product; only navigation does.
    SelectionUnverified,
    /// Part of the product's controls is a separate, optional profile.
    Optional,
    /// Needs [`Capability::CameraRoll`].
    NeedsCameraRoll,
    /// Needs [`Capability::NumpadLocation`].
    NeedsNumpadLocation,
    /// Needs [`Capability::OrderedButtons`].
    NeedsOrderedButtons,
}

impl LimitKind {
    fn covers(self, capability: Capability) -> bool {
        matches!(
            (self, capability),
            (LimitKind::NeedsCameraRoll, Capability::CameraRoll)
                | (LimitKind::NeedsNumpadLocation, Capability::NumpadLocation)
                | (LimitKind::NeedsOrderedButtons, Capability::OrderedButtons)
        )
    }
}

/// What the preset does not match, said plainly.
#[derive(Clone, Debug, PartialEq)]
pub struct Limitation {
    pub kind: LimitKind,
    pub text: &'static str,
}

/// A navigation preset: a product's default mouse and view controls.
#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    /// Stable: the settings file stores it.
    pub id: &'static str,
    pub label: &'static str,
    /// What this preset covers and what it claims.
    pub release_scope: &'static str,
    pub wheel: WheelPolicy,
    pub bindings: Vec<Binding>,
    pub limitations: Vec<Limitation>,
}

impl Preset {
    pub fn binding(&self, id: &str) -> Option<&Binding> {
        self.bindings.iter().find(|b| b.id == id)
    }

    pub fn has_limitation(&self, kind: LimitKind) -> bool {
        self.limitations.iter().any(|l| l.kind == kind)
    }
}

/// The presets offered, in the order the settings list them.
pub fn presets() -> &'static [Preset] {
    &presets::PRESETS
}

/// The offered preset with this id.
pub fn preset(id: &str) -> Option<&'static Preset> {
    presets().iter().find(|p| p.id == id)
}

/// Why a preset with this id is known but not offered.
pub fn withheld(id: &str) -> Option<&'static str> {
    presets::WITHHELD.iter().find(|(w, _)| *w == id).map(|(_, why)| *why)
}

/// What is wrong with a preset's rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaError {
    DuplicateBindingId(&'static str),
    /// A drag, double click or sequence with no button.
    EmptyButtons(&'static str),
    /// A key binding with no key.
    MissingKey(&'static str),
    UnknownAction(&'static str),
    UnknownContext(&'static str),
    DuplicatePriority {
        priority: u16,
        first: &'static str,
        second: &'static str,
    },
    DuplicateButton(&'static str, Button),
    DuplicateHeldKey(&'static str, NavKey),
    /// A modifier both required and forbidden.
    ModifierOverlap(&'static str),
    /// `allow_extra_modifiers` disagrees with the modifiers the row leaves free.
    ExtrasMismatch(&'static str),
    /// A sequence of fewer than two steps.
    SequenceTooShort(&'static str),
    /// A row needs a capability and the preset has no limitation saying so.
    MissingLimitation(&'static str, Capability),
    /// Two rows with the same gesture in the same context.
    DuplicateChord {
        first: &'static str,
        second: &'static str,
    },
}

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchemaError::DuplicateBindingId(id) => write!(f, "{id}: the id is used twice"),
            SchemaError::EmptyButtons(id) => write!(f, "{id}: no mouse button"),
            SchemaError::MissingKey(id) => write!(f, "{id}: no key"),
            SchemaError::UnknownAction(id) => write!(f, "{id}: unknown action"),
            SchemaError::UnknownContext(id) => write!(f, "{id}: unknown context"),
            SchemaError::DuplicatePriority { priority, first, second } => {
                write!(f, "{first} and {second} share priority {priority}")
            }
            SchemaError::DuplicateButton(id, b) => write!(f, "{id}: {b:?} is listed twice"),
            SchemaError::DuplicateHeldKey(id, k) => write!(f, "{id}: {} is listed twice", k.text()),
            SchemaError::ModifierOverlap(id) => write!(f, "{id}: a modifier is both required and forbidden"),
            SchemaError::ExtrasMismatch(id) => write!(f, "{id}: the extra modifiers allowed are not stated"),
            SchemaError::SequenceTooShort(id) => write!(f, "{id}: a sequence needs at least two steps"),
            SchemaError::MissingLimitation(id, c) => write!(f, "{id}: needs {c:?} and no limitation says so"),
            SchemaError::DuplicateChord { first, second } => write!(f, "{first} and {second} are the same gesture"),
        }
    }
}

/// Checks a preset's rows. Returns every problem found.
pub fn validate(preset: &Preset) -> Result<(), Vec<SchemaError>> {
    let mut errors = Vec::new();
    for (i, b) in preset.bindings.iter().enumerate() {
        let earlier = &preset.bindings[..i];
        if earlier.iter().any(|e| e.id == b.id) {
            errors.push(SchemaError::DuplicateBindingId(b.id));
        }
        if let Some(e) = earlier.iter().find(|e| e.priority == b.priority) {
            errors.push(SchemaError::DuplicatePriority { priority: b.priority, first: e.id, second: b.id });
        }
        if matches!(b.event, EventKind::Drag | EventKind::DoubleClick | EventKind::Sequence) && b.buttons.is_empty() {
            errors.push(SchemaError::EmptyButtons(b.id));
        }
        if b.event == EventKind::Key && b.key.is_none() {
            errors.push(SchemaError::MissingKey(b.id));
        }
        if b.action == Action::Unknown {
            errors.push(SchemaError::UnknownAction(b.id));
        }
        if b.context == NavContext::Unknown {
            errors.push(SchemaError::UnknownContext(b.id));
        }
        for (j, button) in b.buttons.iter().enumerate() {
            if b.buttons[..j].contains(button) {
                errors.push(SchemaError::DuplicateButton(b.id, *button));
            }
        }
        for (j, key) in b.held_keys.iter().enumerate() {
            if b.held_keys[..j].contains(key) {
                errors.push(SchemaError::DuplicateHeldKey(b.id, *key));
            }
        }
        if b.required.intersects(b.forbidden) {
            errors.push(SchemaError::ModifierOverlap(b.id));
        }
        if b.allow_extra_modifiers == ((b.required | b.forbidden) == Mods::ALL) {
            errors.push(SchemaError::ExtrasMismatch(b.id));
        }
        if b.event == EventKind::Sequence && b.sequence.len() < 2 {
            errors.push(SchemaError::SequenceTooShort(b.id));
        }
        if b.requires != Capability::None && !preset.limitations.iter().any(|l| l.kind.covers(b.requires)) {
            errors.push(SchemaError::MissingLimitation(b.id, b.requires));
        }
        if let Some(e) = earlier.iter().find(|e| same_gesture(e, b)) {
            errors.push(SchemaError::DuplicateChord { first: e.id, second: b.id });
        }
    }
    if errors.is_empty() { Ok(()) } else { Err(errors) }
}

/// Whether two rows react to the same input in overlapping contexts.
fn same_gesture(a: &Binding, b: &Binding) -> bool {
    let overlap = a.context == b.context || a.context == NavContext::Viewport || b.context == NavContext::Viewport;
    let mut ab = a.buttons.clone();
    let mut bb = b.buttons.clone();
    ab.sort();
    bb.sort();
    let held = |x: &Binding| {
        let mut k: Vec<_> = x.held_keys.iter().map(|k| (k.key.name(), k.location as u8)).collect();
        k.sort();
        k
    };
    overlap
        && a.event == b.event
        && ab == bb
        && a.key == b.key
        && held(a) == held(b)
        && a.required == b.required
        && a.sequence == b.sequence
        && (a.axis == b.axis || a.axis == DragAxis::Any || b.axis == DragAxis::Any)
}

/// Why a preset change was refused.
#[derive(Clone, Debug, PartialEq)]
pub enum Refusal {
    /// No preset has this id.
    UnknownPreset(String),
    /// The preset is known but not offered yet, for the reason given.
    Withheld { id: String, reason: &'static str },
    /// No view-key profile has this id.
    UnknownProfile(String),
    /// The preset has no row with this id.
    UnknownBinding { preset: String, binding: String },
    /// The change would leave the preset's rows invalid.
    Invalid(Vec<SchemaError>),
    /// The settings could not be saved.
    Save(SaveError),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::UnknownPreset(id) => write!(f, "there is no navigation preset {id}"),
            Refusal::Withheld { id, reason } => write!(f, "the {id} preset is not offered yet: {reason}"),
            Refusal::UnknownProfile(id) => write!(f, "there is no view-key profile {id}"),
            Refusal::UnknownBinding { preset, binding } => write!(f, "the {preset} preset has no {binding} binding"),
            Refusal::Invalid(errors) => {
                let text: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
                write!(f, "{}", text.join("; "))
            }
            Refusal::Save(e) => e.fmt(f),
        }
    }
}

/// An offered preset, or why not.
pub(crate) fn offered(id: &str) -> Result<&'static Preset, Refusal> {
    preset(id).ok_or_else(|| match withheld(id) {
        Some(reason) => Refusal::Withheld { id: id.to_owned(), reason },
        None => Refusal::UnknownPreset(id.to_owned()),
    })
}

/// Makes `id` the active preset in the user's settings and saves them.
pub fn set_preset(id: &str) -> Result<(), Refusal> {
    let mut settings = crate::settings::navigation(crate::settings::install_kind());
    settings.select_preset(id)?;
    crate::settings::save_navigation(&settings).map_err(Refusal::Save)
}

/// Drops the user's changes to preset `id`'s rows and saves the settings. Nothing else
/// changes: not the active preset, other presets' changes, the wheel or the camera.
pub fn reset_preset(id: &str) -> Result<(), Refusal> {
    let mut settings = crate::settings::navigation(crate::settings::install_kind());
    settings.reset_preset(id)?;
    crate::settings::save_navigation(&settings).map_err(Refusal::Save)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn baseline() -> Preset {
        Preset {
            id: "test",
            label: "Test",
            release_scope: "Test rows.",
            wheel: WheelPolicy::SoliddraftDefault,
            bindings: vec![
                Binding::drag("orbit", &[Button::Middle], Mods::NONE, Action::Orbit, 10),
                Binding::drag("pan", &[Button::Middle], Mods::SHIFT, Action::Pan, 20),
                Binding::wheel("wheel", 1),
                Binding::key("fit", Key::F6, Mods::NONE, Action::Fit, 30),
            ],
            limitations: vec![Limitation { kind: LimitKind::Note, text: "A test preset." }],
        }
    }

    fn errors_with(change: impl FnOnce(&mut Preset)) -> Vec<SchemaError> {
        let mut p = baseline();
        change(&mut p);
        validate(&p).expect_err("the change is invalid")
    }

    #[test]
    fn a_valid_preset_validates() {
        assert_eq!(validate(&baseline()), Ok(()));
        for p in presets() {
            assert_eq!(validate(p), Ok(()), "{}", p.id);
        }
    }

    #[test]
    fn malformed_rows_give_typed_errors() {
        let e = errors_with(|p| p.bindings[0].buttons.clear());
        assert!(e.contains(&SchemaError::EmptyButtons("orbit")), "{e:?}");
        let e = errors_with(|p| p.bindings[0].action = Action::Unknown);
        assert!(e.contains(&SchemaError::UnknownAction("orbit")), "{e:?}");
        let e = errors_with(|p| p.bindings[0].context = NavContext::Unknown);
        assert!(e.contains(&SchemaError::UnknownContext("orbit")), "{e:?}");
        let e = errors_with(|p| p.bindings[1].priority = 10);
        assert!(e.contains(&SchemaError::DuplicatePriority { priority: 10, first: "orbit", second: "pan" }), "{e:?}");
        let e = errors_with(|p| p.bindings[0].buttons.push(Button::Middle));
        assert!(e.contains(&SchemaError::DuplicateButton("orbit", Button::Middle)), "{e:?}");
        let e = errors_with(|p| p.bindings[0] = p.bindings[0].clone().held(&[Key::F4, Key::F4]));
        assert!(e.contains(&SchemaError::DuplicateHeldKey("orbit", NavKey::main(Key::F4))), "{e:?}");
        let e = errors_with(|p| p.bindings[1].forbidden = Mods::ALL);
        assert!(e.contains(&SchemaError::ModifierOverlap("pan")), "{e:?}");
        let e = errors_with(|p| p.bindings.push(Binding::sequence("seq", &[], Action::Orbit, 40)));
        assert!(e.contains(&SchemaError::SequenceTooShort("seq")), "{e:?}");
        let one = [Step { button: Button::Middle, down: true }];
        let e = errors_with(|p| p.bindings.push(Binding::sequence("seq", &one, Action::Orbit, 40)));
        assert!(e.contains(&SchemaError::SequenceTooShort("seq")), "{e:?}");
        let roll = Binding::drag("roll", &[Button::Middle], Mods::ALT, Action::Roll, 40).needs(Capability::CameraRoll);
        let e = errors_with(|p| p.bindings.push(roll));
        assert!(e.contains(&SchemaError::MissingLimitation("roll", Capability::CameraRoll)), "{e:?}");
        let e = errors_with(|p| p.bindings[0].allow_extra_modifiers = true);
        assert!(e.contains(&SchemaError::ExtrasMismatch("orbit")), "{e:?}");
        let e =
            errors_with(|p| p.bindings.push(Binding::drag("again", &[Button::Middle], Mods::NONE, Action::Pan, 50)));
        assert!(e.contains(&SchemaError::DuplicateChord { first: "orbit", second: "again" }), "{e:?}");
        let e = errors_with(|p| p.bindings[3].key = None);
        assert!(e.contains(&SchemaError::MissingKey("fit")), "{e:?}");
    }

    #[test]
    fn unknown_names_from_a_newer_build_read_as_unknown() {
        assert_eq!(serde_json::from_str::<Action>("\"spin_forever\"").unwrap(), Action::Unknown);
        assert_eq!(serde_json::from_str::<NavContext>("\"drawing\"").unwrap(), NavContext::Unknown);
        assert_eq!(serde_json::from_str::<Action>("\"orbit\"").unwrap(), Action::Orbit);
    }
}
