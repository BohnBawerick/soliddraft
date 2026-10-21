//! Screen-space level choice, per occurrence.
//!
//! Each placement of a definition picks one of the definition's three levels from how
//! large its deviation looks on screen. An occurrence moves to a finer level as soon as
//! its error covers more than about a pixel, and back to a coarser one only after its
//! error has stayed under about a tenth of a pixel for a while. The gap between the two
//! thresholds, with levels a factor of four apart, keeps an occurrence from flickering
//! between levels while the camera moves.
//!
//! The device budget sets a ceiling: when the app's meshes fill its budget it stops
//! promoting to the fine level, or to the standard one.

use soliddraft_kernel_api::Vec3;

use crate::level::Level;
use crate::policy::Levels;

/// How many screen pixels one millimetre covers, for the camera of one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Projection {
    /// A perspective camera.
    Perspective {
        /// Where the eye is, in the same frame as the occurrences.
        eye: Vec3,
        /// The vertical field of view, in radians.
        fov_y: f64,
        /// The viewport's height in pixels.
        height_px: f64,
    },
    /// A parallel camera.
    Orthographic {
        /// Pixels per millimetre everywhere in the view.
        pixels_per_mm: f64,
    },
}

impl Projection {
    /// Pixels per millimetre at the part of a bounding sphere nearest the eye. A sphere
    /// around the eye counts as filling the screen at one millimetre's distance.
    pub fn pixels_per_mm(&self, center: Vec3, radius: f64) -> f64 {
        match *self {
            Projection::Orthographic { pixels_per_mm } => pixels_per_mm,
            Projection::Perspective { eye, fov_y, height_px } => {
                let distance = (center.distance(eye) - radius.max(0.0)).max(1.0);
                height_px / (2.0 * (fov_y / 2.0).tan() * distance)
            }
        }
    }
}

/// When to change level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thresholds {
    /// Promote when the current level's error on screen is more than this, in pixels.
    pub promote_px: f64,
    /// Demote when the current level's error has stayed under this, in pixels...
    pub demote_px: f64,
    /// ...for this long, in seconds.
    pub demote_after_s: f32,
}

impl Default for Thresholds {
    fn default() -> Thresholds {
        Thresholds { promote_px: 1.0, demote_px: 0.1, demote_after_s: 2.0 }
    }
}

/// One occurrence's level and how long it has been a candidate for demotion. Eight
/// bytes, so a million occurrences keep their choices in 8 MB.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Choice {
    /// The level drawn.
    pub level: Level,
    /// Seconds the error has stayed under the demotion threshold.
    pub quiet_s: f32,
}

impl Thresholds {
    /// The level a newly shown occurrence starts at: the coarsest whose error is within
    /// the promotion threshold, at most `ceiling`.
    pub fn initial(&self, levels: &Levels, pixels_per_mm: f64, ceiling: Level) -> Choice {
        Choice { level: self.coarsest_sharp(levels, pixels_per_mm, Level::Coarse, ceiling), quiet_s: 0.0 }
    }

    /// Moves `choice` on by one frame `dt_s` seconds long. Returns true when the level
    /// changed.
    pub fn update(&self, choice: &mut Choice, levels: &Levels, pixels_per_mm: f64, dt_s: f32, ceiling: Level) -> bool {
        let before = choice.level;
        let error = |l: Level| levels.deviation(l).map(|d| d * pixels_per_mm);
        let Some(now) = error(choice.level) else {
            // Nothing bounds the error, so the levels are the same mesh.
            *choice = Choice { level: Level::Standard.min(ceiling), quiet_s: 0.0 };
            return choice.level != before;
        };
        if choice.level > ceiling {
            *choice = Choice { level: ceiling, quiet_s: 0.0 };
        } else if now > self.promote_px && choice.level < ceiling {
            *choice = Choice { level: self.coarsest_sharp(levels, pixels_per_mm, choice.level, ceiling), quiet_s: 0.0 };
        } else if now < self.demote_px
            && let Some(coarser) = choice.level.coarser()
        {
            choice.quiet_s += dt_s.max(0.0);
            if choice.quiet_s >= self.demote_after_s && error(coarser).is_some_and(|e| e <= self.promote_px) {
                *choice = Choice { level: coarser, quiet_s: 0.0 };
            }
        } else {
            choice.quiet_s = 0.0;
        }
        choice.level != before
    }

    /// The coarsest level from `from` to `ceiling` whose error is within the promotion
    /// threshold, or `ceiling` when none is. Standard when no chord bounds the error.
    fn coarsest_sharp(&self, levels: &Levels, pixels_per_mm: f64, from: Level, ceiling: Level) -> Level {
        if levels.deviation(Level::Standard).is_none() {
            return Level::Standard.min(ceiling);
        }
        Level::ALL
            .into_iter()
            .filter(|l| *l >= from && *l <= ceiling)
            .find(|l| levels.deviation(*l).is_some_and(|d| d * pixels_per_mm <= self.promote_px))
            .unwrap_or(ceiling)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality::{Advanced, Quality};

    /// A body with a 100 mm diagonal: deviations 0.8, 0.2 and 0.05 mm.
    fn levels() -> Levels {
        Levels::new(&Quality::Auto, 100.0)
    }

    #[test]
    fn perspective_scale_falls_with_distance() {
        let p = Projection::Perspective { eye: Vec3::new(0.0, 0.0, 0.0), fov_y: 90f64.to_radians(), height_px: 1000.0 };
        let near = p.pixels_per_mm(Vec3::new(0.0, 0.0, 100.0), 0.0);
        let far = p.pixels_per_mm(Vec3::new(0.0, 0.0, 1000.0), 0.0);
        assert!((near - 5.0).abs() < 1e-9, "{near}");
        assert!((near / far - 10.0).abs() < 1e-9);
        // The near side of the sphere decides, and the eye inside a sphere is a close-up.
        assert!((p.pixels_per_mm(Vec3::new(0.0, 0.0, 150.0), 50.0) - near).abs() < 1e-9);
        assert!((p.pixels_per_mm(Vec3::new(0.0, 0.0, 1.0), 50.0) - 500.0).abs() < 1e-9);
        assert_eq!(Projection::Orthographic { pixels_per_mm: 3.0 }.pixels_per_mm(Vec3::new(9.0, 9.0, 9.0), 1.0), 3.0);
    }

    #[test]
    fn a_new_occurrence_starts_at_the_coarsest_sharp_level() {
        let t = Thresholds::default();
        let l = levels();
        assert_eq!(t.initial(&l, 1.0, Level::Fine).level, Level::Coarse);
        assert_eq!(t.initial(&l, 2.0, Level::Fine).level, Level::Standard);
        assert_eq!(t.initial(&l, 10.0, Level::Fine).level, Level::Fine);
        assert_eq!(t.initial(&l, 1000.0, Level::Fine).level, Level::Fine, "the finest when none is sharp enough");
        assert_eq!(t.initial(&l, 10.0, Level::Standard).level, Level::Standard, "the ceiling holds");
    }

    #[test]
    fn promotion_is_immediate_and_demotion_waits() {
        let t = Thresholds::default();
        let l = levels();
        let mut c = t.initial(&l, 1.0, Level::Fine);
        assert_eq!(c.level, Level::Coarse);
        // Zooming in: coarse's 0.8 mm covers 8 px, standard's 2 px, fine's 0.5 px.
        assert!(t.update(&mut c, &l, 10.0, 0.016, Level::Fine));
        assert_eq!(c.level, Level::Fine);
        // Zooming out a little stays put: fine is 0.05 px, under the demotion threshold,
        // but only for a moment.
        assert!(!t.update(&mut c, &l, 1.0, 1.0, Level::Fine));
        assert_eq!(c.level, Level::Fine);
        // Back in before the wait is over resets it.
        assert!(!t.update(&mut c, &l, 5.0, 1.5, Level::Fine));
        assert_eq!(c.quiet_s, 0.0);
        // Out for two seconds: one level down.
        assert!(!t.update(&mut c, &l, 1.0, 1.0, Level::Fine));
        assert!(t.update(&mut c, &l, 1.0, 1.0, Level::Fine));
        assert_eq!(c.level, Level::Standard);
        // Standard's 0.2 px is above the demotion threshold, so it stays.
        for _ in 0..100 {
            assert!(!t.update(&mut c, &l, 1.0, 1.0, Level::Fine));
        }
        // Far away it steps down once more.
        assert!(!t.update(&mut c, &l, 0.1, 1.0, Level::Fine));
        assert!(t.update(&mut c, &l, 0.1, 1.0, Level::Fine));
        assert_eq!(c.level, Level::Coarse);
    }

    #[test]
    fn the_ceiling_demotes_at_once() {
        let t = Thresholds::default();
        let l = levels();
        let mut c = t.initial(&l, 10.0, Level::Fine);
        assert_eq!(c.level, Level::Fine);
        assert!(t.update(&mut c, &l, 10.0, 0.0, Level::Coarse));
        assert_eq!(c.level, Level::Coarse);
        assert!(!t.update(&mut c, &l, 10.0, 0.0, Level::Coarse), "no promotion past the ceiling");
        assert!(t.update(&mut c, &l, 10.0, 0.0, Level::Fine));
        assert_eq!(c.level, Level::Fine);
    }

    #[test]
    fn levels_without_a_chord_stay_standard() {
        let t = Thresholds::default();
        let l = Levels::new(&Quality::Custom(Advanced { chord_height: 0.0, ..Advanced::default() }), 100.0);
        let mut c = Choice { level: Level::Fine, quiet_s: 0.0 };
        assert!(t.update(&mut c, &l, 1000.0, 0.0, Level::Fine));
        assert_eq!(c.level, Level::Standard);
        assert_eq!(t.initial(&l, 1000.0, Level::Fine).level, Level::Standard);
    }

    #[test]
    fn a_choice_is_eight_bytes() {
        assert_eq!(std::mem::size_of::<Choice>(), 8);
    }
}
