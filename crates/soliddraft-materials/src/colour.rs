//! Colours with a declared colour space.
//!
//! Colours are authored and stored in sRGB, eight bits a channel, the way colour pickers,
//! paint charts and other CAD tools give them. Lighting happens in linear RGB, so the
//! renderer converts each colour once with [`Srgb::to_linear`] and never mixes the two.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An sRGB colour, eight bits a channel. Written "#rrggbb".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Srgb {
    /// Red, 0 to 255.
    pub r: u8,
    /// Green, 0 to 255.
    pub g: u8,
    /// Blue, 0 to 255.
    pub b: u8,
}

impl Srgb {
    /// Black, which as an emissive colour means "emits nothing".
    pub const BLACK: Srgb = Srgb::new(0, 0, 0);
    /// White.
    pub const WHITE: Srgb = Srgb::new(255, 255, 255);

    /// The colour with these channels.
    pub const fn new(r: u8, g: u8, b: u8) -> Srgb {
        Srgb { r, g, b }
    }

    /// The colour in linear RGB, each channel 0 to 1, with the exact sRGB transfer
    /// function (IEC 61966-2-1), not a 2.2 power.
    pub fn to_linear(self) -> [f32; 3] {
        [decode(self.r), decode(self.g), decode(self.b)]
    }

    /// The nearest sRGB colour to a linear RGB one. Channels outside 0 to 1 are clamped.
    pub fn from_linear(rgb: [f32; 3]) -> Srgb {
        Srgb::new(encode(rgb[0]), encode(rgb[1]), encode(rgb[2]))
    }

    /// The nearest sRGB colour to channels given as fractions, 0 to 1, of full scale in
    /// sRGB (not linear): what other tools write as "0.78 0.80 0.83".
    pub fn from_unit(rgb: [f64; 3]) -> Srgb {
        let q = |c: f64| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
        Srgb::new(q(rgb[0]), q(rgb[1]), q(rgb[2]))
    }
}

fn decode(c: u8) -> f32 {
    let c = f32::from(c) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn encode(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0).round() as u8
}

impl fmt::Display for Srgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

/// Why a colour did not parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not a colour; write it as \"#rrggbb\"")]
pub struct ParseColourError(String);

impl FromStr for Srgb {
    type Err = ParseColourError;

    /// Parses "#rrggbb", either case.
    fn from_str(s: &str) -> Result<Srgb, ParseColourError> {
        let bad = || ParseColourError(s.to_owned());
        let hex = s.strip_prefix('#').ok_or_else(bad)?;
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad());
        }
        let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| bad());
        Ok(Srgb::new(channel(0)?, channel(2)?, channel(4)?))
    }
}

impl Serialize for Srgb {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Srgb {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Srgb, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_parse_and_print_as_hex() {
        let c: Srgb = "#C7CCD4".parse().unwrap();
        assert_eq!(c, Srgb::new(0xc7, 0xcc, 0xd4));
        assert_eq!(c.to_string(), "#c7ccd4");
        for bad in ["c7ccd4", "#c7ccd", "#c7ccdz", "#c7ccd4ff", ""] {
            assert!(bad.parse::<Srgb>().is_err(), "{bad} parsed");
        }
        assert_eq!(serde_json::to_string(&c).unwrap(), r##""#c7ccd4""##);
        assert_eq!(serde_json::from_str::<Srgb>(r##""#c7ccd4""##).unwrap(), c);
    }

    #[test]
    fn linear_conversion_follows_the_srgb_curve() {
        assert_eq!(Srgb::BLACK.to_linear(), [0.0, 0.0, 0.0]);
        assert_eq!(Srgb::WHITE.to_linear(), [1.0, 1.0, 1.0]);
        // Mid grey is about 0.214 linear, not 0.5 and not the 2.2 power's 0.218.
        let [g, _, _] = Srgb::new(128, 128, 128).to_linear();
        assert!((g - 0.2158).abs() < 1e-4, "{g}");
        for v in 0..=255u8 {
            let c = Srgb::new(v, 255 - v, v / 2);
            assert_eq!(Srgb::from_linear(c.to_linear()), c, "round trip of {c}");
        }
        assert_eq!(Srgb::from_unit([0.78, 0.80, 0.83]), Srgb::new(199, 204, 212));
    }
}
