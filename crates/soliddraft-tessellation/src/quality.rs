//! The viewport quality a document asks for.
//!
//! A document saves one [`Quality`]: automatic by default, a position on the quality
//! slider, or advanced raw values. An assembly component can carry its own, which
//! replaces the document's for every part under it. What the setting turns into for a
//! given body is in [`policy`](crate::policy).

use serde::{Deserialize, Serialize};

/// The slider's lowest position, the coarsest.
pub const SLIDER_MIN: u8 = 1;
/// The slider's highest position, the finest.
pub const SLIDER_MAX: u8 = 10;
/// The slider position that gives what [`Quality::Auto`] gives.
pub const SLIDER_AUTO: u8 = 5;

/// How smooth curved faces look in the viewport.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Quality {
    /// The chord height follows the body's size: 0.2 percent of its bounding diagonal
    /// at the standard level.
    #[default]
    Auto,
    /// A slider position from [`SLIDER_MIN`] to [`SLIDER_MAX`]. Each step up divides the
    /// chord height by the square root of two; [`SLIDER_AUTO`] matches `Auto`.
    Slider {
        /// The position.
        value: u8,
    },
    /// Raw values for the standard level, in millimetres and degrees. The coarse and
    /// fine levels scale the chord height and edge length.
    Custom(Advanced),
}

/// Advanced quality values. Zero disables a control.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Advanced {
    /// Largest distance between the mesh and the surface, in millimetres.
    pub chord_height: f64,
    /// Largest angle between neighbouring normals and tangents, in degrees.
    pub angle_deg: f64,
    /// Target triangle edge length, in millimetres.
    #[serde(default)]
    pub max_edge_length: f64,
    /// Largest ratio of a triangle's longest edge to its height.
    #[serde(default)]
    pub max_aspect_ratio: f64,
}

impl Default for Advanced {
    fn default() -> Advanced {
        Advanced { chord_height: 0.1, angle_deg: crate::policy::ANGLE_DEG, max_edge_length: 0.0, max_aspect_ratio: 0.0 }
    }
}

impl Quality {
    /// Checks the values are in range.
    pub fn check(&self) -> Result<(), String> {
        match *self {
            Quality::Auto => Ok(()),
            Quality::Slider { value } if (SLIDER_MIN..=SLIDER_MAX).contains(&value) => Ok(()),
            Quality::Slider { value } => Err(format!("slider {value} is outside {SLIDER_MIN} to {SLIDER_MAX}")),
            Quality::Custom(a) => a.check(),
        }
    }

    /// The setting in words, such as "auto", "slider 7" or "chord 0.05 mm, 15°".
    pub fn describe(&self) -> String {
        match self {
            Quality::Auto => "auto".into(),
            Quality::Slider { value } => format!("slider {value}"),
            Quality::Custom(a) => {
                let mut parts = Vec::new();
                if a.chord_height > 0.0 {
                    parts.push(format!("chord {} mm", a.chord_height));
                }
                if a.angle_deg > 0.0 {
                    parts.push(format!("{}°", a.angle_deg));
                }
                if a.max_edge_length > 0.0 {
                    parts.push(format!("edge {} mm", a.max_edge_length));
                }
                if a.max_aspect_ratio > 0.0 {
                    parts.push(format!("aspect {}", a.max_aspect_ratio));
                }
                parts.join(", ")
            }
        }
    }
}

impl Quality {
    /// The setting as canonical JSON: equal exactly when the settings are, so it keys
    /// meshes and definitions made at this quality.
    pub fn key(&self) -> String {
        serde_json::to_string(self).expect("a quality always serialises")
    }

    /// A short code for file names: sixteen hex digits of a 64-bit FNV-1a hash of
    /// [`Quality::key`].
    pub fn code(&self) -> String {
        let hash =
            self.key().bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3));
        format!("{hash:016x}")
    }
}

impl std::str::FromStr for Quality {
    type Err = String;

    /// Reads "auto", a slider position such as "7", or advanced values as
    /// "chord=0.05,angle=10,edge=2,aspect=4" (any subset; chord and angle default to
    /// [`Advanced::default`]).
    fn from_str(s: &str) -> Result<Quality, String> {
        let s = s.trim();
        if s == "auto" {
            return Ok(Quality::Auto);
        }
        let q = if let Ok(value) = s.parse::<u8>() {
            Quality::Slider { value }
        } else {
            let mut a = Advanced::default();
            for pair in s.split(',') {
                let (key, value) =
                    pair.split_once('=').ok_or_else(|| format!("{pair:?} is not auto, 1 to 10 or key=value"))?;
                let v: f64 = value.trim().parse().map_err(|_| format!("{value:?} is not a number"))?;
                match key.trim() {
                    "chord" => a.chord_height = v,
                    "angle" => a.angle_deg = v,
                    "edge" => a.max_edge_length = v,
                    "aspect" => a.max_aspect_ratio = v,
                    other => return Err(format!("no setting {other:?}: chord, angle, edge or aspect")),
                }
            }
            Quality::Custom(a)
        };
        q.check()?;
        Ok(q)
    }
}

impl Advanced {
    /// Checks the values are finite, not negative, and bound the mesh somehow.
    pub fn check(&self) -> Result<(), String> {
        let values = [
            ("chord height", self.chord_height),
            ("angle", self.angle_deg),
            ("edge length", self.max_edge_length),
            ("aspect ratio", self.max_aspect_ratio),
        ];
        for (name, v) in values {
            if !v.is_finite() || v < 0.0 {
                return Err(format!("the {name} is {v}; it must be zero or more"));
            }
        }
        if self.angle_deg > 90.0 {
            return Err(format!("the angle is {}°; at most 90°", self.angle_deg));
        }
        if self.max_aspect_ratio != 0.0 && self.max_aspect_ratio < 1.0 {
            return Err(format!("the aspect ratio is {}; zero or at least 1", self.max_aspect_ratio));
        }
        if self.chord_height == 0.0 && self.angle_deg == 0.0 && self.max_edge_length == 0.0 {
            return Err("set a chord height, an angle or an edge length".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_read_from_text_and_json() {
        assert_eq!("auto".parse::<Quality>(), Ok(Quality::Auto));
        assert_eq!("7".parse::<Quality>(), Ok(Quality::Slider { value: 7 }));
        assert!("0".parse::<Quality>().is_err());
        assert!("11".parse::<Quality>().is_err());
        let q: Quality = "chord=0.02,aspect=4".parse().unwrap();
        assert_eq!(
            q,
            Quality::Custom(Advanced {
                chord_height: 0.02,
                angle_deg: 15.0,
                max_edge_length: 0.0,
                max_aspect_ratio: 4.0
            })
        );
        assert_eq!(q.describe(), "chord 0.02 mm, 15°, aspect 4");
        assert!("chord=-1".parse::<Quality>().is_err());
        assert!("chord=0,angle=0".parse::<Quality>().is_err());
        assert!("aspect=0.5".parse::<Quality>().is_err());
        assert!("smooth=1".parse::<Quality>().is_err());

        for q in [Quality::Auto, Quality::Slider { value: 3 }, q] {
            let text = serde_json::to_string(&q).unwrap();
            assert_eq!(serde_json::from_str::<Quality>(&text).unwrap(), q);
        }
        assert_eq!(serde_json::to_string(&Quality::Auto).unwrap(), r#"{"mode":"auto"}"#);
        assert_eq!(serde_json::to_string(&Quality::Slider { value: 3 }).unwrap(), r#"{"mode":"slider","value":3}"#);
        assert!(serde_json::from_str::<Quality>(r#"{"mode":"slider","value":3,"step":1}"#).is_err());

        let (a, b) = (Quality::Slider { value: 3 }, Quality::Slider { value: 4 });
        assert_eq!(a.key(), r#"{"mode":"slider","value":3}"#);
        assert_ne!(a.code(), b.code());
        assert_eq!(a.code(), Quality::Slider { value: 3 }.code());
        assert_eq!(a.code().len(), 16);
    }
}
