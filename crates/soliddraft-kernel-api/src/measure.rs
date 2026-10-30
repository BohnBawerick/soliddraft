//! Proven single-entity quantities in millimetres and square millimetres.

use serde::{Deserialize, Serialize};

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
