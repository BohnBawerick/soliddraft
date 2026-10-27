//! Definition-native display and reserved work for one scalar edit request.
//! Revision authority and publication remain with the caller's existing lifecycle.

use crate::{FaceId, KernelResult, Mesh, OpTag, Origin};

/// A scalar parameter in an operation already present in a body's history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarParameter {
    /// Positive distance of a blind extrusion or cutting tool.
    ExtrudeDistance,
    /// Radius of an existing fillet.
    FilletRadius,
    /// Distance of an existing chamfer.
    ChamferDistance,
}

/// A replacement value bound to an exact operation tag, not a numeric body handle.
#[derive(Clone, Debug, PartialEq)]
pub struct ScalarEdit {
    /// Exact caller tag of the operation being edited.
    pub tag: OpTag,
    /// Which scalar definition to replace.
    pub parameter: ScalarParameter,
    /// New value in millimetres.
    pub value: f64,
}

/// A displayed construction face. Its id is provisional and never an exact handle.
#[derive(Clone, Debug)]
pub struct ConstructionFace {
    /// Picking id local to this construction display.
    pub id: FaceId,
    /// Semantic origins to resolve only after the same revision's exact proof.
    pub origins: Vec<Origin>,
}

/// Complete unproved screen data, with no body or export certificate.
#[derive(Clone, Debug)]
pub struct ConstructionMesh {
    /// Screen triangles, explicitly marked unproved.
    pub mesh: Mesh,
    /// Names only for faces actually represented in the screen triangles.
    pub faces: Vec<ConstructionFace>,
}

/// Single-use work already reserved for the exact model rebuild.
/// A backend must charge every nested kernel operation to this reservation;
/// it cannot obtain another default allowance when this scope ends or refuses.
pub trait ProofWork: Send {
    /// Runs the caller's existing synchronous rebuild on its worker thread.
    /// Returning success after exhausting the reservation is forbidden.
    fn run(self: Box<Self>, rebuild: &mut dyn FnMut() -> KernelResult<()>) -> KernelResult<()>;
}

/// One construction request and its separately scheduled exact-proof allowance.
/// An unsupported or incomplete early display is explicit, while the retained
/// reservation can still establish whether the requested exact model is valid.
pub struct ConstructionEdit {
    /// A complete display, or the early representation's typed refusal.
    pub display: KernelResult<ConstructionMesh>,
    /// Reserved exact-proof work. Dropping it does not refund or prove anything.
    pub proof: Box<dyn ProofWork>,
}
