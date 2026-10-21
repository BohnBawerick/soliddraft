//! What a quality setting means for one body: the kernel parameters of its three levels.
//!
//! - **Auto** sets the standard level's chord height to 0.2 percent of the body's
//!   bounding diagonal (FreeCAD's default), so a washer and an engine block look equally
//!   smooth at the size they fill the screen. A slider position scales that.
//! - **Angles stay at 15 degrees** on every level: at SMLib's own 25, triangles beside
//!   circular fillets can wind against their face's normal.
//! - **Levels** multiply the chord height (and an edge length, when set) by 4, 1 and 1/4.
//!   Small bodies whose curves are bounded by the angle rather than the chord can come
//!   out the same at two levels; the display cache then keeps one mesh for both.
//!
//! The chord height is what the viewport reports as the deviation: the mesh is within
//! that distance of the exact surface.

use soliddraft_kernel_api::TessellationParams;

use crate::level::Level;
use crate::quality::{Advanced, Quality, SLIDER_AUTO};

/// The standard level's chord height under [`Quality::Auto`], as a fraction of the
/// body's bounding diagonal.
pub const BASELINE: f64 = 0.002;
/// The angle control on every level of the automatic and slider settings, in degrees.
pub const ANGLE_DEG: f64 = 15.0;
/// The smallest chord height the policy asks for, in millimetres, so a degenerate or
/// tiny body never asks the kernel for an unbounded mesh.
pub const MIN_CHORD_MM: f64 = 1e-4;

/// The chord height of the standard level under a slider position, as a fraction of the
/// bounding diagonal.
pub fn slider_fraction(value: u8) -> f64 {
    BASELINE * 2f64.powf((f64::from(SLIDER_AUTO) - f64::from(value)) / 2.0)
}

/// The kernel parameters of one definition's three levels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Levels {
    params: [TessellationParams; 3],
}

impl Levels {
    /// The levels `quality` gives a body whose bounding diagonal is `diagonal_mm`.
    pub fn new(quality: &Quality, diagonal_mm: f64) -> Levels {
        let diagonal = if diagonal_mm.is_finite() { diagonal_mm.max(0.0) } else { 0.0 };
        let standard = match *quality {
            Quality::Auto => relative(BASELINE, diagonal),
            Quality::Slider { value } => relative(slider_fraction(value), diagonal),
            Quality::Custom(a) => advanced(&a),
        };
        Levels { params: Level::ALL.map(|l| scaled(&standard, l.chord_factor())) }
    }

    /// The parameters of `level`.
    pub fn params(&self, level: Level) -> &TessellationParams {
        &self.params[level.index()]
    }

    /// Every level's parameters, coarsest first, as [`Kernel::tessellate_levels`] takes
    /// them.
    ///
    /// [`Kernel::tessellate_levels`]: soliddraft_kernel_api::Kernel::tessellate_levels
    pub fn all(&self) -> &[TessellationParams; 3] {
        &self.params
    }

    /// The largest distance from the mesh of `level` to the exact surface, in
    /// millimetres. `None` when no chord height bounds it, as with advanced values that
    /// set only an angle.
    pub fn deviation(&self, level: Level) -> Option<f64> {
        let chord = self.params(level).chord_height;
        (chord > 0.0).then_some(chord)
    }

    /// The cache key of `level`'s parameters.
    pub fn key(&self, level: Level) -> ParamsKey {
        ParamsKey::of(self.params(level))
    }
}

/// Tessellation parameters as an exact, hashable key: the bits of each value. Two keys
/// are equal exactly when the kernel was asked for the same thing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ParamsKey([u64; 5]);

impl ParamsKey {
    /// The key of `params`.
    pub fn of(params: &TessellationParams) -> ParamsKey {
        let p = params;
        ParamsKey(
            [p.chord_height, p.curve_angle_deg, p.surface_angle_deg, p.max_edge_length, p.max_aspect_ratio]
                .map(|v| if v == 0.0 { 0 } else { v.to_bits() }),
        )
    }

    /// The key as text for a cache entry's header, such as "c0.2 a15 s15 e0 r0".
    pub fn describe(&self) -> String {
        let [c, a, s, e, r] = self.0.map(f64::from_bits);
        format!("c{c} a{a} s{s} e{e} r{r}")
    }
}

fn relative(fraction: f64, diagonal: f64) -> TessellationParams {
    TessellationParams {
        chord_height: (fraction * diagonal).max(MIN_CHORD_MM),
        curve_angle_deg: ANGLE_DEG,
        surface_angle_deg: ANGLE_DEG,
        max_edge_length: 0.0,
        max_aspect_ratio: 0.0,
    }
}

fn advanced(a: &Advanced) -> TessellationParams {
    TessellationParams {
        chord_height: if a.chord_height > 0.0 { a.chord_height.max(MIN_CHORD_MM) } else { 0.0 },
        curve_angle_deg: a.angle_deg,
        surface_angle_deg: a.angle_deg,
        max_edge_length: a.max_edge_length,
        max_aspect_ratio: a.max_aspect_ratio,
    }
}

fn scaled(standard: &TessellationParams, factor: f64) -> TessellationParams {
    TessellationParams {
        chord_height: if standard.chord_height > 0.0 {
            (standard.chord_height * factor).max(MIN_CHORD_MM)
        } else {
            0.0
        },
        max_edge_length: standard.max_edge_length * factor,
        ..*standard
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_follows_the_body_size() {
        let small = Levels::new(&Quality::Auto, 10.0);
        let large = Levels::new(&Quality::Auto, 1000.0);
        assert_eq!(small.deviation(Level::Standard), Some(0.02));
        assert_eq!(large.deviation(Level::Standard), Some(2.0));
        assert_eq!(large.deviation(Level::Coarse), Some(8.0));
        assert_eq!(large.deviation(Level::Fine), Some(0.5));
        for l in Level::ALL {
            let p = large.params(l);
            assert_eq!((p.curve_angle_deg, p.surface_angle_deg), (15.0, 15.0), "angles stay at 15 degrees");
            assert_eq!((p.max_edge_length, p.max_aspect_ratio), (0.0, 0.0));
        }
        assert_eq!(Levels::new(&Quality::Slider { value: SLIDER_AUTO }, 250.0), Levels::new(&Quality::Auto, 250.0));
    }

    #[test]
    fn the_slider_steps_by_the_square_root_of_two() {
        let at = |value| Levels::new(&Quality::Slider { value }, 100.0).deviation(Level::Standard).unwrap();
        for v in 1..10 {
            let ratio = at(v) / at(v + 1);
            assert!((ratio - std::f64::consts::SQRT_2).abs() < 1e-12, "step {v}: {ratio}");
        }
        assert!((at(1) - 0.8).abs() < 1e-12, "slider 1 is 0.8 percent: {}", at(1));
    }

    #[test]
    fn degenerate_bodies_get_a_floor() {
        for diagonal in [0.0, -1.0, f64::NAN, f64::INFINITY, 1e-9] {
            let l = Levels::new(&Quality::Auto, diagonal);
            for level in Level::ALL {
                assert!(l.deviation(level).unwrap() >= MIN_CHORD_MM, "{diagonal}: {level}");
            }
        }
    }

    #[test]
    fn advanced_values_are_the_standard_level() {
        let a = Advanced { chord_height: 0.05, angle_deg: 10.0, max_edge_length: 4.0, max_aspect_ratio: 3.0 };
        let l = Levels::new(&Quality::Custom(a), 500.0);
        let s = l.params(Level::Standard);
        assert_eq!((s.chord_height, s.curve_angle_deg, s.max_edge_length, s.max_aspect_ratio), (0.05, 10.0, 4.0, 3.0));
        let c = l.params(Level::Coarse);
        assert_eq!((c.chord_height, c.max_edge_length, c.max_aspect_ratio), (0.2, 16.0, 3.0));
        let angle_only = Levels::new(&Quality::Custom(Advanced { chord_height: 0.0, ..a }), 500.0);
        assert_eq!(angle_only.deviation(Level::Fine), None);
    }

    #[test]
    fn keys_are_exact() {
        let l = Levels::new(&Quality::Auto, 100.0);
        assert_ne!(l.key(Level::Coarse), l.key(Level::Standard));
        assert_eq!(l.key(Level::Fine), ParamsKey::of(l.params(Level::Fine)));
        let negative_zero = TessellationParams { max_edge_length: -0.0, ..TessellationParams::default() };
        assert_eq!(ParamsKey::of(&negative_zero), ParamsKey::of(&TessellationParams::default()));
        assert_eq!(l.key(Level::Standard).describe(), "c0.2 a15 s15 e0 r0");
    }
}
