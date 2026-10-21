//! Closed 2D profiles on a sketch plane, the input of `create_body_from_profile`.

use serde::{Deserialize, Serialize};

use crate::geometry::{Plane, Point2};

/// One piece of a profile loop, in the coordinates of the profile's plane.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Segment {
    /// A straight segment.
    Line {
        /// Start point.
        from: Point2,
        /// End point.
        to: Point2,
    },
    /// A circular arc from `from` to `to` around `center`, counter-clockwise when seen
    /// from the side the plane normal points to, or clockwise when `clockwise` is set.
    /// Both ends must lie on the same circle.
    Arc {
        /// Arc centre.
        center: Point2,
        /// Start point.
        from: Point2,
        /// End point.
        to: Point2,
        /// Runs clockwise from `from` to `to`. A loop traversed clockwise needs this for
        /// its convex arcs, and a counter-clockwise loop for its concave ones.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        clockwise: bool,
    },
    /// A full circle. It forms a loop on its own.
    Circle {
        /// Circle centre.
        center: Point2,
        /// Radius, greater than zero.
        radius: f64,
    },
}

impl Segment {
    /// Start point of the segment (for a circle, the point at angle zero).
    pub fn start(&self) -> Point2 {
        match *self {
            Segment::Line { from, .. } | Segment::Arc { from, .. } => from,
            Segment::Circle { center, radius } => Point2::new(center.u + radius, center.v),
        }
    }

    /// End point of the segment (for a circle, the same as the start).
    pub fn end(&self) -> Point2 {
        match *self {
            Segment::Line { to, .. } | Segment::Arc { to, .. } => to,
            Segment::Circle { .. } => self.start(),
        }
    }
}

/// A closed chain of segments, each one starting where the previous one ends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Loop(pub Vec<Segment>);

/// A planar region: one outer loop and any number of hole loops.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    /// The sketch plane the loops are drawn on.
    pub plane: Plane,
    /// The outer boundary.
    pub outer: Loop,
    /// Holes inside the outer boundary.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holes: Vec<Loop>,
}

/// Why a profile was rejected before reaching a kernel.
#[derive(Clone, Debug, PartialEq, thiserror::Error, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileError {
    /// The plane frame is not finite, unit length and orthogonal.
    #[error("the sketch plane frame is not orthonormal")]
    BadPlane,
    /// A loop has no segments.
    #[error("loop {0} is empty")]
    EmptyLoop(usize),
    /// A loop mixes a full circle with other segments.
    #[error("loop {0} mixes a full circle with other segments")]
    CircleNotAlone(usize),
    /// Two consecutive segments do not meet.
    #[error("loop {loop_index} is open between segment {segment} and the next one (gap {gap})")]
    Open {
        /// Which loop (0 is the outer loop).
        loop_index: usize,
        /// Index of the segment whose end misses the next start.
        segment: usize,
        /// Distance between the two ends.
        gap: f64,
    },
    /// A segment has zero length, a non-positive radius, or non-finite coordinates.
    #[error("loop {loop_index} segment {segment} is degenerate")]
    Degenerate {
        /// Which loop (0 is the outer loop).
        loop_index: usize,
        /// Index of the bad segment.
        segment: usize,
    },
}

/// Distance under which two loop ends count as joined.
pub const PROFILE_JOIN_TOLERANCE: f64 = 1e-7;

impl Profile {
    /// An axis-aligned rectangle centred at `center`.
    pub fn rectangle(plane: Plane, center: Point2, width: f64, height: f64) -> Profile {
        let (hw, hh) = (width / 2.0, height / 2.0);
        let corners = [
            Point2::new(center.u - hw, center.v - hh),
            Point2::new(center.u + hw, center.v - hh),
            Point2::new(center.u + hw, center.v + hh),
            Point2::new(center.u - hw, center.v + hh),
        ];
        let segments = (0..4).map(|i| Segment::Line { from: corners[i], to: corners[(i + 1) % 4] }).collect();
        Profile { plane, outer: Loop(segments), holes: Vec::new() }
    }

    /// A full circle.
    pub fn circle(plane: Plane, center: Point2, radius: f64) -> Profile {
        Profile { plane, outer: Loop(vec![Segment::Circle { center, radius }]), holes: Vec::new() }
    }

    /// All loops, outer first.
    pub fn loops(&self) -> impl Iterator<Item = &Loop> {
        std::iter::once(&self.outer).chain(self.holes.iter())
    }

    /// Checks the structure every backend relies on: a sound plane, closed loops and
    /// non-degenerate segments. It does not check for self-intersection.
    pub fn check(&self) -> Result<(), ProfileError> {
        if !self.plane.is_well_formed() {
            return Err(ProfileError::BadPlane);
        }
        for (li, lp) in self.loops().enumerate() {
            let segs = &lp.0;
            if segs.is_empty() {
                return Err(ProfileError::EmptyLoop(li));
            }
            let has_circle = segs.iter().any(|s| matches!(s, Segment::Circle { .. }));
            if has_circle && segs.len() > 1 {
                return Err(ProfileError::CircleNotAlone(li));
            }
            for (si, s) in segs.iter().enumerate() {
                if segment_is_degenerate(s) {
                    return Err(ProfileError::Degenerate { loop_index: li, segment: si });
                }
                let next = &segs[(si + 1) % segs.len()];
                let gap = dist(s.end(), next.start());
                if gap > PROFILE_JOIN_TOLERANCE {
                    return Err(ProfileError::Open { loop_index: li, segment: si, gap });
                }
            }
        }
        Ok(())
    }
}

fn dist(a: Point2, b: Point2) -> f64 {
    ((a.u - b.u).powi(2) + (a.v - b.v).powi(2)).sqrt()
}

fn finite(p: Point2) -> bool {
    p.u.is_finite() && p.v.is_finite()
}

fn segment_is_degenerate(s: &Segment) -> bool {
    match *s {
        Segment::Line { from, to } => !finite(from) || !finite(to) || dist(from, to) <= PROFILE_JOIN_TOLERANCE,
        Segment::Arc { center, from, to, .. } => {
            let (r0, r1) = (dist(center, from), dist(center, to));
            !finite(center)
                || !finite(from)
                || !finite(to)
                || r0 <= PROFILE_JOIN_TOLERANCE
                || (r0 - r1).abs() > PROFILE_JOIN_TOLERANCE.max(r0 * 1e-9)
                || dist(from, to) <= PROFILE_JOIN_TOLERANCE
        }
        Segment::Circle { center, radius } => !finite(center) || !(radius.is_finite() && radius > 0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangle_and_circle_are_closed() {
        Profile::rectangle(Plane::XY, Point2::new(1.0, 2.0), 10.0, 4.0).check().unwrap();
        Profile::circle(Plane::XY, Point2::new(0.0, 0.0), 3.0).check().unwrap();
    }

    #[test]
    fn open_loop_is_rejected() {
        let mut p = Profile::rectangle(Plane::XY, Point2::new(0.0, 0.0), 10.0, 4.0);
        p.outer.0.pop();
        assert!(matches!(p.check(), Err(ProfileError::Open { .. })));
    }

    #[test]
    fn bad_plane_and_radius_are_rejected() {
        let mut p = Profile::circle(Plane::XY, Point2::new(0.0, 0.0), 3.0);
        p.plane.normal = crate::geometry::Vec3::new(0.0, 1.0, 1.0);
        assert_eq!(p.check(), Err(ProfileError::BadPlane));
        let p = Profile::circle(Plane::XY, Point2::new(0.0, 0.0), 0.0);
        assert!(matches!(p.check(), Err(ProfileError::Degenerate { .. })));
    }

    #[test]
    fn profile_round_trips_through_json() {
        let p = Profile::rectangle(Plane::XY, Point2::new(0.0, 0.0), 10.0, 4.0);
        let text = serde_json::to_string(&p).unwrap();
        let back: Profile = serde_json::from_str(&text).unwrap();
        assert_eq!(p, back);
    }
}
