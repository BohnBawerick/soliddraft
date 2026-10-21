//! Appearances: how a surface looks.
//!
//! An [`Appearance`] is glTF's metallic-roughness model: a base colour, how metallic the
//! surface is, how rough, how opaque, and what light it emits. It says nothing about
//! what the part is made of; that is the [`Material`](crate::Material), and a painted
//! steel part stays steel. Textures and their projection come later.

use serde::{Deserialize, Serialize};

use crate::colour::Srgb;
use crate::problem::Problem;

/// An appearance's id: stable, unique among the appearances an app has loaded, and not
/// the appearance's name. Documents refer to appearances by it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AppearanceId(pub String);

impl AppearanceId {
    /// The id as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AppearanceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// How a surface looks, in glTF's metallic-roughness terms.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Appearance {
    /// The appearance's id.
    pub id: AppearanceId,
    /// The name people see: "Anodised blue".
    pub name: String,
    /// The surface colour, or for a metal the colour of its reflections.
    pub base_color: Srgb,
    /// 0 for a dielectric (paint, plastic, wood), 1 for bare metal.
    #[serde(default)]
    pub metallic: f32,
    /// 0 for a mirror finish, 1 for fully diffuse.
    #[serde(default = "half")]
    pub roughness: f32,
    /// 1 for opaque, 0 for fully transparent.
    #[serde(default = "one")]
    pub opacity: f32,
    /// Light the surface gives off; black for none, which is almost every surface.
    #[serde(default, skip_serializing_if = "is_black")]
    pub emissive: Srgb,
}

fn half() -> f32 {
    0.5
}

fn one() -> f32 {
    1.0
}

fn is_black(c: &Srgb) -> bool {
    *c == Srgb::BLACK
}

impl Appearance {
    /// The id of [`Appearance::default_part`].
    pub const DEFAULT_ID: &str = "soliddraft-default";

    /// An opaque dielectric of this colour: roughness 0.5, nothing emitted.
    pub fn new(id: impl Into<String>, name: impl Into<String>, base_color: Srgb) -> Appearance {
        Appearance {
            id: AppearanceId(id.into()),
            name: name.into(),
            base_color,
            metallic: 0.0,
            roughness: 0.5,
            opacity: 1.0,
            emissive: Srgb::BLACK,
        }
    }

    /// What a face with no appearance assigned looks like: the light blue-grey the
    /// viewport has always drawn parts in.
    pub fn default_part() -> Appearance {
        Appearance { roughness: 0.45, ..Appearance::new(Appearance::DEFAULT_ID, "Default", Srgb::new(199, 204, 212)) }
    }

    /// The values a shader takes, in linear RGB.
    pub fn shading(&self) -> Shading {
        let [r, g, b] = self.base_color.to_linear();
        Shading {
            base_color: [r, g, b, self.opacity.clamp(0.0, 1.0)],
            emissive: self.emissive.to_linear(),
            metallic: self.metallic.clamp(0.0, 1.0),
            roughness: self.roughness.clamp(0.0, 1.0),
        }
    }

    /// What is wrong with the appearance: an empty id or name, or a factor outside 0
    /// to 1 or not a number. Empty when it is usable as it is.
    pub fn problems(&self) -> Vec<Problem> {
        let mut out = Vec::new();
        if self.id.0.trim().is_empty() {
            out.push(Problem::new("id", "is empty"));
        }
        if self.name.trim().is_empty() {
            out.push(Problem::new("name", "is empty"));
        }
        for (field, v) in [("metallic", self.metallic), ("roughness", self.roughness), ("opacity", self.opacity)] {
            if !(0.0..=1.0).contains(&v) {
                out.push(Problem::new(field, format!("is {v}; it must be from 0 to 1")));
            }
        }
        out
    }
}

/// An appearance as a shader reads it: linear RGB, factors clamped to 0 to 1. The
/// layout is plain `f32`s so it can go straight into a GPU buffer.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Shading {
    /// Linear base colour, and opacity as alpha (not premultiplied).
    pub base_color: [f32; 4],
    /// Linear emitted light.
    pub emissive: [f32; 3],
    /// Metallic factor.
    pub metallic: f32,
    /// Roughness factor.
    pub roughness: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearances_round_trip_with_defaults_filled() {
        let a: Appearance = toml::from_str(
            r##"
            id = "anodised-blue"
            name = "Anodised blue"
            base_color = "#1f4e8c"
            metallic = 1.0
            roughness = 0.35
            "##,
        )
        .unwrap();
        assert_eq!(a.opacity, 1.0);
        assert_eq!(a.emissive, Srgb::BLACK);
        assert!(a.problems().is_empty());
        let json = serde_json::to_value(&a).unwrap();
        assert!(json.get("emissive").is_none(), "no emission is not written");
        assert_eq!(json["base_color"], "#1f4e8c");
        assert_eq!(serde_json::from_value::<Appearance>(json).unwrap(), a);

        let plastic: Appearance = toml::from_str("id = \"p\"\nname = \"P\"\nbase_color = \"#202020\"").unwrap();
        assert_eq!((plastic.metallic, plastic.roughness), (0.0, 0.5));
    }

    #[test]
    fn shading_is_linear_and_clamped() {
        let mut a = Appearance::new("glass", "Glass", Srgb::WHITE);
        a.opacity = 0.2;
        a.roughness = 1.4;
        let s = a.shading();
        assert_eq!(s.base_color, [1.0, 1.0, 1.0, 0.2]);
        assert_eq!(s.roughness, 1.0);
        assert_eq!(a.problems(), [Problem::new("roughness", "is 1.4; it must be from 0 to 1")]);
        let d = Appearance::default_part().shading();
        assert!(d.base_color[0] < 0.6 && d.base_color[0] > 0.5, "sRGB 199 is about 0.57 linear");
        a.metallic = f32::NAN;
        assert_eq!(a.problems().len(), 2, "NaN is not a factor");
    }
}
