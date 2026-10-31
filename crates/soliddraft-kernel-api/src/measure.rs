//! Proven quantities in millimetres, square millimetres and degrees.

use serde::{Deserialize, Serialize};

use crate::{EdgeId, FaceId};

/// Whether the kernel returned a point value or an enclosure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Certainty {
    /// A zero-width rational enclosure, represented at the nearest double.
    Exact,
    /// A nonzero enclosure, rounded outward at the interface.
    Bounded,
}

/// A proven quantity. Bounded endpoints enclose the kernel's rational interval.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    /// Nearest double of the enclosure midpoint.
    pub value: f64,
    /// Lower endpoint.
    pub lo: f64,
    /// Upper endpoint.
    pub hi: f64,
    /// The kernel's certainty before conversion to doubles.
    pub certainty: Certainty,
}

/// A proven point in the part frame: one enclosure per coordinate.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PointProof {
    /// x, y and z, each with its own certainty.
    pub coordinates: [Measurement; 3],
}

/// One end of a pair request on a single body. No cursor coordinate is a source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairEndpoint {
    /// The whole trimmed edge.
    Edge(EdgeId),
    /// The whole trimmed face.
    Face(FaceId),
    /// The arc-length midpoint of an edge.
    EdgeMidpoint(EdgeId),
}

/// The scalar a pair request asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairQuantity {
    /// Minimum distance between two finite entities.
    MinimumDistance,
    /// Distance between two proven points.
    PointDistance,
    /// Unoriented angle between two straight carriers, in [0, 90] degrees.
    DirectionAngle,
}

impl PairQuantity {
    /// The quantity's name in requests and results.
    pub fn name(self) -> &'static str {
        match self {
            Self::MinimumDistance => "minimum_distance",
            Self::PointDistance => "point_distance",
            Self::DirectionAngle => "direction_angle",
        }
    }

    /// The quantity's document unit.
    pub fn unit(self) -> &'static str {
        match self {
            Self::DirectionAngle => "deg",
            _ => "mm",
        }
    }
}
