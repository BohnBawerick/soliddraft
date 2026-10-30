//! The user's navigation settings: the `navigation` key of `settings.json`.
//!
//! The section carries a `version`. A section from a newer SolidDraft is read for what
//! this build knows and never written back, and keys this build does not know are kept
//! when it saves, so a newer build's settings survive an older one.

use super::{Chord, Preset, Refusal, ZoomAnchor, offered, preset, validate};
use crate::settings::InstallKind;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// The navigation section's version this build writes.
pub const VERSION: u64 = 1;

const KNOWN: [&str; 7] = ["version", "preset_id", "overrides", "wheel", "keyboard_profile", "trackpad", "camera"];

/// The mouse wheel, for every preset.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WheelSettings {
    /// Turns the preset's direction around.
    pub invert: bool,
    /// How far one notch zooms, 1 as SolidDraft always has.
    pub speed: f32,
}

impl Default for WheelSettings {
    fn default() -> Self {
        Self { invert: false, speed: 1.0 }
    }
}

/// The point the view orbits about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pivot {
    /// The view's target, the middle of the screen: the only pivot the camera has now.
    #[default]
    Target,
}

/// How the camera answers the gestures, for every preset.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraSettings {
    pub orbit_sensitivity: f32,
    pub pan_sensitivity: f32,
    pub pivot: Pivot,
    pub zoom_anchor: ZoomAnchor,
    /// How long the move to a standard view takes.
    pub transition_seconds: f32,
}

impl Default for CameraSettings {
    fn default() -> Self {
        Self {
            orbit_sensitivity: 1.0,
            pan_sensitivity: 1.0,
            pivot: Pivot::Target,
            zoom_anchor: ZoomAnchor::Cursor,
            transition_seconds: 0.33,
        }
    }
}

/// The navigation settings.
#[derive(Clone, Debug, PartialEq)]
pub struct NavigationSettings {
    /// The version the section was read at. Saving writes [`VERSION`].
    pub version: u64,
    /// The chosen preset's id, kept as written even when this build has no such preset.
    pub preset_id: String,
    /// The user's changes to presets' rows: preset id, then binding id. Every preset
    /// keeps its own, so switching away and back finds them again.
    pub overrides: BTreeMap<String, BTreeMap<String, Chord>>,
    pub wheel: WheelSettings,
    /// The view keys' profile, chosen apart from the mouse preset.
    pub keyboard_profile: String,
    /// The trackpad's profile, chosen apart from the mouse preset.
    pub trackpad: String,
    pub camera: CameraSettings,
    /// Written by a newer SolidDraft: shown, never saved.
    pub read_only: bool,
    /// The preset used when `preset_id` names none this build offers.
    fallback: &'static str,
    /// Keys a newer SolidDraft wrote, kept as they are.
    extra: Map<String, Value>,
}

/// The view keys' profile that keeps SolidDraft's own keys.
pub const KEYBOARD_PROFILE: &str = "soliddraft";
/// The trackpad profile that keeps SolidDraft's scroll-to-zoom.
pub const TRACKPAD_PROFILE: &str = "soliddraft";

impl NavigationSettings {
    /// The settings a fresh install starts with: Fusion for a new installation, the
    /// original controls for one that already had settings.
    pub fn new(install: InstallKind) -> Self {
        let fallback = default_preset(install);
        Self {
            version: VERSION,
            preset_id: fallback.to_owned(),
            overrides: BTreeMap::new(),
            wheel: WheelSettings::default(),
            keyboard_profile: KEYBOARD_PROFILE.to_owned(),
            trackpad: TRACKPAD_PROFILE.to_owned(),
            camera: CameraSettings::default(),
            read_only: false,
            fallback,
            extra: Map::new(),
        }
    }

    /// Reads the `navigation` value of the settings file. A missing or damaged value, or
    /// field, gives the default for the install.
    pub fn from_json(value: &Value, install: InstallKind) -> Self {
        let mut s = Self::new(install);
        let Some(obj) = value.as_object() else { return s };
        s.version = obj.get("version").and_then(Value::as_u64).unwrap_or(0);
        s.read_only = s.version > VERSION;
        if let Some(id) = obj.get("preset_id").and_then(Value::as_str) {
            s.preset_id = id.to_owned();
        }
        if let Some(Value::Object(presets)) = obj.get("overrides") {
            for (preset, rows) in presets {
                let Some(rows) = rows.as_object() else { continue };
                let rows: BTreeMap<String, Chord> = rows
                    .iter()
                    .filter_map(|(id, chord)| Some((id.clone(), serde_json::from_value(chord.clone()).ok()?)))
                    .collect();
                s.overrides.insert(preset.clone(), rows);
            }
        }
        let field = |key: &str| obj.get(key).cloned().unwrap_or(Value::Null);
        s.wheel = serde_json::from_value(field("wheel")).unwrap_or_default();
        s.camera = serde_json::from_value(field("camera")).unwrap_or_default();
        if let Some(p) = obj.get("keyboard_profile").and_then(Value::as_str) {
            s.keyboard_profile = p.to_owned();
        }
        if let Some(p) = obj.get("trackpad").and_then(Value::as_str) {
            s.trackpad = p.to_owned();
        }
        s.extra =
            obj.iter().filter(|(k, _)| !KNOWN.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
        s
    }

    /// The `navigation` value to write, at [`VERSION`].
    pub fn to_json(&self) -> Value {
        let mut obj = self.extra.clone();
        obj.insert("version".into(), VERSION.into());
        obj.insert("preset_id".into(), self.preset_id.clone().into());
        obj.insert("overrides".into(), serde_json::to_value(&self.overrides).unwrap_or_default());
        obj.insert("wheel".into(), serde_json::to_value(self.wheel).unwrap_or_default());
        obj.insert("keyboard_profile".into(), self.keyboard_profile.clone().into());
        obj.insert("trackpad".into(), self.trackpad.clone().into());
        obj.insert("camera".into(), serde_json::to_value(self.camera).unwrap_or_default());
        Value::Object(obj)
    }

    /// The preset in use: the chosen one, or the install's default when this build does
    /// not offer the one chosen.
    pub fn active_preset(&self) -> &'static Preset {
        preset(&self.preset_id).or_else(|| preset(self.fallback)).expect("the default presets exist")
    }

    /// The preset with the user's changes to its rows.
    pub fn effective(&self, preset: &Preset) -> Preset {
        let mut p = preset.clone();
        if let Some(rows) = self.overrides.get(preset.id) {
            for b in &mut p.bindings {
                if let Some(chord) = rows.get(b.id) {
                    *b = b.with_chord(chord);
                }
            }
        }
        p
    }

    /// Chooses the preset with this id. Its own changes come back with it.
    pub fn select_preset(&mut self, id: &str) -> Result<(), Refusal> {
        self.preset_id = offered(id)?.id.to_owned();
        Ok(())
    }

    /// Drops the changes to preset `id`'s rows, and nothing else.
    pub fn reset_preset(&mut self, id: &str) -> Result<(), Refusal> {
        self.overrides.remove(offered(id)?.id);
        Ok(())
    }

    /// Changes one row's gesture. Refused when the preset or row does not exist, or when
    /// the result would be invalid, such as a gesture another row already has.
    pub fn set_override(&mut self, preset_id: &str, binding: &str, chord: Chord) -> Result<(), Refusal> {
        let preset = offered(preset_id)?;
        let Some(row) = preset.binding(binding) else {
            return Err(Refusal::UnknownBinding { preset: preset_id.to_owned(), binding: binding.to_owned() });
        };
        let mut changed = self.clone();
        changed.overrides.entry(preset.id.to_owned()).or_default().insert(row.id.to_owned(), chord);
        validate(&changed.effective(preset)).map_err(Refusal::Invalid)?;
        *self = changed;
        Ok(())
    }
}

/// Fusion for a new installation; SolidDraft's original controls for an existing one.
pub fn default_preset(install: InstallKind) -> &'static str {
    match install {
        InstallKind::New => "fusion",
        InstallKind::Existing => "legacy_soliddraft",
    }
}

/// Why the navigation settings were not saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveError {
    /// The file's navigation section is from a newer SolidDraft.
    NewerVersion {
        found: u64,
    },
    NoConfigDir,
    Write(String),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::NewerVersion { found } => write!(
                f,
                "the navigation settings were written by a newer SolidDraft (version {found}) and are kept as they are"
            ),
            SaveError::NoConfigDir => write!(f, "no config directory"),
            SaveError::Write(e) => write!(f, "{e}"),
        }
    }
}
