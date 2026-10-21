//! A document's display and accuracy settings, and the four tolerances kept apart.
//!
//! Four tolerances decide how closely SolidDraft follows exact geometry. Each is its own
//! setting, and changing one never redoes the work of another:
//!
//! | Tolerance | Controls | Lives in | Changing it redoes |
//! |---|---|---|---|
//! | Viewport quality ([`Quality`]) | the display meshes | the document's `settings.json` | display meshes only |
//! | Drawing projection ([`DrawingTolerance`]) | how closely drawing views follow curves | the drawing, which caches its views | that drawing's views |
//! | Export ([`ExportTolerance`]) | meshes written to mesh formats | the document's `settings.json` | the next export |
//! | Modelling | when the kernel treats two points as one | the kernel | the geometry, and everything made from it |
//!
//! `settings.json` is outside both document hashes, so changing a setting never gives
//! the document a new revision: assemblies and drawings that pin it see no change, and
//! no cache keyed by geometry goes stale. The modelling tolerance is the other way
//! round: it changes geometry, so it belongs in the part's truth and its hashes. SMLib
//! fixes its own and takes none through SM_API, so no document carries one yet.
//!
//! The drawing tolerance belongs to the drawing because the drawing caches its views
//! against the revisions they were projected from; a tolerance kept in the model would
//! change what a view shows without telling the drawing. Drawings use
//! [`DrawingTolerance::Auto`] until the drawing file stores one.

use serde::{Deserialize, Serialize};
use soliddraft_kernel_api::TessellationParams;

use crate::quality::Quality;

/// The entry a document keeps its settings in.
pub const ENTRY: &str = "settings.json";
/// The format tag of the settings this version writes.
pub const FORMAT: &str = "soliddraft-settings/1";

/// How closely drawing views of a model follow its curves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum DrawingTolerance {
    /// The kernel's own accuracy for hidden-line removal.
    #[default]
    Auto,
    /// At most this far from the exact curve, in millimetres.
    Fixed {
        /// The distance.
        mm: f64,
    },
}

impl DrawingTolerance {
    /// The value a view request carries
    /// ([`ViewSpec::tolerance`](soliddraft_kernel_api::drawing::ViewSpec::tolerance)):
    /// zero lets the kernel choose.
    pub fn view_tolerance(&self) -> f64 {
        match *self {
            DrawingTolerance::Auto => 0.0,
            DrawingTolerance::Fixed { mm } => mm,
        }
    }

    /// Checks the value is in range.
    pub fn check(&self) -> Result<(), String> {
        match *self {
            DrawingTolerance::Fixed { mm } if !(mm.is_finite() && mm > 0.0) => {
                Err(format!("the drawing tolerance is {mm} mm; it must be more than zero"))
            }
            _ => Ok(()),
        }
    }
}

/// How closely exported meshes follow the surface.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportTolerance {
    /// Largest distance between the mesh and the surface, in millimetres.
    pub chord_height: f64,
    /// Largest angle between neighbouring normals, in degrees.
    pub angle_deg: f64,
}

impl Default for ExportTolerance {
    /// 0.01 mm and 10 degrees: finer than any viewport level, for printing and
    /// downstream tools.
    fn default() -> ExportTolerance {
        ExportTolerance { chord_height: 0.01, angle_deg: 10.0 }
    }
}

impl ExportTolerance {
    /// The kernel parameters for an export.
    pub fn params(&self) -> TessellationParams {
        TessellationParams {
            chord_height: self.chord_height,
            curve_angle_deg: self.angle_deg,
            surface_angle_deg: self.angle_deg,
            ..TessellationParams::default()
        }
    }

    /// Checks the values are in range.
    pub fn check(&self) -> Result<(), String> {
        if !(self.chord_height.is_finite() && self.chord_height > 0.0) {
            return Err(format!("the export chord height is {} mm; it must be more than zero", self.chord_height));
        }
        if !(self.angle_deg > 0.0 && self.angle_deg <= 90.0) {
            return Err(format!("the export angle is {}°; more than 0 and at most 90", self.angle_deg));
        }
        Ok(())
    }
}

/// A document's settings: what `settings.json` holds. Parts and assemblies both keep
/// one; in an assembly, a viewport quality other than auto applies to every part, and
/// `overrides` give components their own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// The format tag, [`FORMAT`].
    pub format: String,
    /// Viewport quality.
    #[serde(default)]
    pub viewport: Quality,
    /// Accuracy of exported meshes.
    #[serde(default)]
    pub export: ExportTolerance,
    /// Whether saving keeps display meshes in the file, so the document opens as saved
    /// without the kernel.
    #[serde(default = "yes")]
    pub save_display_data: bool,
    /// In an assembly: components shown at their own quality, by occurrence path, in
    /// path order. A part ignores them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<QualityOverride>,
}

/// One component shown at its own quality in an assembly, with everything under it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualityOverride {
    /// The occurrence path as text, such as `4/12:9.3`.
    pub path: String,
    /// The quality.
    pub quality: Quality,
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            format: FORMAT.into(),
            viewport: Quality::default(),
            export: ExportTolerance::default(),
            save_display_data: true,
            overrides: Vec::new(),
        }
    }
}

impl Settings {
    /// True when every setting is at its default, so a document need not write the
    /// entry.
    pub fn is_default(&self) -> bool {
        *self == Settings::default()
    }

    /// Checks every value is in range.
    pub fn check(&self) -> Result<(), String> {
        self.viewport.check()?;
        for o in &self.overrides {
            o.quality.check().map_err(|e| format!("the override at {}: {e}", o.path))?;
        }
        if !self.overrides.is_sorted_by(|a, b| a.path < b.path) {
            return Err("the quality overrides are not in path order, or two share a path".into());
        }
        self.export.check()
    }

    /// The override at `path`, if any.
    pub fn override_at(&self, path: &str) -> Option<&Quality> {
        self.overrides.binary_search_by(|o| o.path.as_str().cmp(path)).ok().map(|i| &self.overrides[i].quality)
    }

    /// Shows the component at `path` at `quality`, or at the quality around it again
    /// (`None`). Keeps the overrides in path order.
    pub fn set_override(&mut self, path: &str, quality: Option<Quality>) {
        match (self.overrides.binary_search_by(|o| o.path.as_str().cmp(path)), quality) {
            (Ok(i), Some(q)) => self.overrides[i].quality = q,
            (Ok(i), None) => {
                self.overrides.remove(i);
            }
            (Err(i), Some(q)) => self.overrides.insert(i, QualityOverride { path: path.to_owned(), quality: q }),
            (Err(_), None) => {}
        }
    }

    /// The entry's text: pretty JSON with a final newline, the same bytes for the same
    /// settings.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("settings always serialise") + "\n"
    }

    /// Reads the entry's text. A newer major format, an unknown key or a value out of
    /// range is refused rather than misread.
    pub fn from_json(text: &str) -> Result<Settings, String> {
        let probe: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let found = probe.get("format").and_then(|f| f.as_str()).unwrap_or_default();
        if !matches!(found, FORMAT | "draftwell-settings/1") {
            return Err(format!("the settings are {found:?}; this version reads {FORMAT}"));
        }
        let mut settings: Settings = serde_json::from_value(probe).map_err(|e| e.to_string())?;
        settings.format = FORMAT.into();
        settings.check()?;
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::Level;
    use crate::policy::Levels;

    #[test]
    fn settings_round_trip_and_default_to_auto() {
        let s = Settings::default();
        assert!(s.is_default());
        assert_eq!(Settings::from_json(&s.to_json()).unwrap(), s);
        let minimal = Settings::from_json(r#"{"format": "soliddraft-settings/1"}"#).unwrap();
        assert_eq!(minimal, s);
        let custom = Settings {
            viewport: Quality::Slider { value: 8 },
            export: ExportTolerance { chord_height: 0.005, angle_deg: 5.0 },
            save_display_data: false,
            ..Settings::default()
        };
        assert_eq!(Settings::from_json(&custom.to_json()).unwrap(), custom);
        assert_eq!(custom.to_json(), custom.clone().to_json(), "same bytes");
        let old = custom.to_json().replace(FORMAT, "draftwell-settings/1");
        assert_eq!(Settings::from_json(&old).unwrap(), custom);
        assert!(Settings::from_json(&old.replace("draftwell-settings/1", "draftwell-settings/2")).is_err());
        assert!(Settings::from_json(r#"{"format": "soliddraft-settings/2"}"#).is_err());
        assert!(Settings::from_json(r#"{"format": "soliddraft-settings/1", "sparkle": 1}"#).is_err());
        assert!(
            Settings::from_json(
                r#"{"format": "soliddraft-settings/1", "export": {"chord_height": 0, "angle_deg": 5}}"#
            )
            .is_err()
        );
        assert!(
            Settings::from_json(r#"{"format": "soliddraft-settings/1", "viewport": {"mode": "slider", "value": 0}}"#)
                .is_err()
        );
    }

    #[test]
    fn each_tolerance_drives_only_its_own_output() {
        let base = Settings::default();
        let viewport = |s: &Settings| Levels::new(&s.viewport, 100.0).key(Level::Standard);
        let export = |s: &Settings| crate::policy::ParamsKey::of(&s.export.params());
        let viewport_changed = Settings { viewport: Quality::Slider { value: 9 }, ..base.clone() };
        let export_changed =
            Settings { export: ExportTolerance { chord_height: 0.1, angle_deg: 20.0 }, ..base.clone() };
        assert_ne!(viewport(&viewport_changed), viewport(&base));
        assert_eq!(export(&viewport_changed), export(&base));
        assert_eq!(viewport(&export_changed), viewport(&base));
        assert_ne!(export(&export_changed), export(&base));
        assert_ne!(export(&base), viewport(&base), "export never reuses a viewport mesh");
    }

    #[test]
    fn overrides_stay_in_path_order() {
        let mut s = Settings::default();
        s.set_override("4/12", Some(Quality::Slider { value: 9 }));
        s.set_override("2", Some(Quality::Slider { value: 2 }));
        s.set_override("4/12", Some(Quality::Slider { value: 8 }));
        assert_eq!(s.overrides.iter().map(|o| o.path.as_str()).collect::<Vec<_>>(), ["2", "4/12"]);
        assert_eq!(s.override_at("4/12"), Some(&Quality::Slider { value: 8 }));
        assert_eq!(s.override_at("4"), None);
        s.check().unwrap();
        assert!(!s.is_default());
        assert_eq!(Settings::from_json(&s.to_json()).unwrap(), s);
        s.set_override("2", None);
        s.set_override("9", None);
        assert_eq!(s.overrides.len(), 1);
        s.overrides.reverse();
        s.overrides.push(QualityOverride { path: "1".into(), quality: Quality::Auto });
        assert!(s.check().is_err(), "out of order");
        let bad = r#"{"format": "soliddraft-settings/1", "overrides": [{"path": "2", "quality": {"mode": "slider", "value": 40}}]}"#;
        assert!(Settings::from_json(bad).is_err());
    }

    #[test]
    fn drawing_tolerances_are_view_requests() {
        assert_eq!(DrawingTolerance::Auto.view_tolerance(), 0.0);
        assert_eq!(DrawingTolerance::Fixed { mm: 0.002 }.view_tolerance(), 0.002);
        assert!(DrawingTolerance::Fixed { mm: 0.0 }.check().is_err());
        let text = serde_json::to_string(&DrawingTolerance::Fixed { mm: 0.002 }).unwrap();
        assert_eq!(text, r#"{"mode":"fixed","mm":0.002}"#);
    }
}
