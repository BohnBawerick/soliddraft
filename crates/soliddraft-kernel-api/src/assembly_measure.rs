//! Proven plane equations for assembly mates.
//!
//! A display centroid or a rounded reference point cannot prove a mate. This query reads
//! the plane a face lies on from the kernel's own definition, as an exact equation. It
//! covers axis-aligned planes with rational offsets; anything else is refused, never
//! approximated.

use serde::{Deserialize, Serialize};

/// An exact rational number, in lowest terms with a positive denominator. Written as
/// decimal integers so no precision is lost on the way through this interface.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExactRational {
    /// The numerator.
    pub numerator: String,
    /// The denominator, greater than zero.
    pub denominator: String,
}

/// A coordinate axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    /// The x axis.
    X,
    /// The y axis.
    Y,
    /// The z axis.
    Z,
}

impl Axis {
    /// The axis's index in `[x, y, z]`.
    pub fn index(self) -> usize {
        match self {
            Axis::X => 0,
            Axis::Y => 1,
            Axis::Z => 2,
        }
    }
}

/// The plane `u · p = offset` a face lies on, where `u` is the unit axis the face's
/// outward side looks along. A negative direction measures the offset along that
/// negative axis, so the bottom of a block at `z = 0` has `u = -z` and offset 0, and a
/// face at `x = -20` looking along `-x` has offset 20.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExactAxisPlane {
    /// The axis the outward normal lies on.
    pub axis: Axis,
    /// True when the outward normal points along the positive axis.
    pub positive: bool,
    /// Where the plane lies along the outward normal, in millimetres.
    pub offset: ExactRational,
}

impl ExactAxisPlane {
    /// The outward unit normal as coordinates, each exactly -1, 0 or 1.
    pub fn normal(&self) -> [i8; 3] {
        let mut n = [0; 3];
        n[self.axis.index()] = if self.positive { 1 } else { -1 };
        n
    }
}
