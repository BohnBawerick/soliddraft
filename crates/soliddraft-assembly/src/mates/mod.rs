//! Mates: authored relationships that decide where components sit.
//!
//! A mate names faces by occurrence path and the part's persistent [`FaceRef`], never by
//! label or kernel handle. Distances are typed expressions; one that reads an assembly
//! value is driven by it. Definitions are the assembly's truth. Proofs, solver status and
//! diagnostics are derived and stay out of it.
//!
//! - [`solver`] places components from proven plane equations and counts what stays free.
//! - [`reference`] finds faces and proves their planes at a pinned revision.
//! - [`command`] runs every change as one candidate: preview, prove, solve, then commit.
//! - [`bodies`] reads the mates of the root and of every sub-assembly in it, and the
//!   part occurrences they move, by path.
//!
//! This batch has two families: an Origin mate and a distance between planar faces. It
//! covers axis-aligned planes with exact offsets; see `docs/mates.md`.

pub(crate) mod bodies;
pub(crate) mod command;
mod reference;
pub(crate) mod solver;

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use soliddraft_document::{ConfigurationId, DocumentId};
use soliddraft_model::{Expression, FaceRef};

use crate::path::{ComponentId, OccurrencePath};

pub use command::{MateEdit, PlaneMeasurement, ProofIdentity};
pub use reference::{EndpointSelector, FaceSelector, GeneratorChoice, PlaneProof};
pub use solver::{FreeMotion, SolverState, SolverStatus};

pub(crate) use reference::Proofs;

/// A mate, stable within its assembly. Mate ids come from the assembly's one counter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MateId(pub u32);

impl fmt::Display for MateId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// How two planar faces' outward normals relate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Alignment {
    /// The outward normals agree.
    Aligned,
    /// The outward normals oppose.
    Opposed,
}

impl Alignment {
    pub(crate) fn sign(self) -> i8 {
        match self {
            Alignment::Aligned => 1,
            Alignment::Opposed => -1,
        }
    }
}

/// One side of a mate: a face of one placed part.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaceEndpoint {
    /// The occurrence, from the assembly that holds the mate.
    pub path: OccurrencePath,
    /// The part document.
    pub document: DocumentId,
    /// The part's configuration; absent for the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<ConfigurationId>,
    /// The part's persistent name for the face.
    pub face: FaceRef,
}

/// A mate this version can solve.
#[allow(clippy::large_enum_variant, reason = "a mate list is short and read one by one")]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Mate {
    /// A component's origin on the assembly's. With `align_axes` its axes match too.
    Origin {
        /// The mate's id.
        id: MateId,
        /// A label; commands find a mate by it.
        name: String,
        /// The component.
        component: ComponentId,
        /// Whether the component's axes match the assembly's as well.
        align_axes: bool,
    },
    /// The signed distance from `a`'s plane to `b`'s, along `a`'s outward normal. It fixes
    /// the normals' relation and one translation; the faces need not overlap.
    Distance {
        /// The mate's id.
        id: MateId,
        /// A label; commands find a mate by it.
        name: String,
        /// The first face.
        a: FaceEndpoint,
        /// The second face.
        b: FaceEndpoint,
        /// How their outward normals relate.
        alignment: Alignment,
        /// The distance. An expression that reads values is driven by them.
        distance: Expression,
    },
}

impl Mate {
    /// The mate's id.
    pub fn id(&self) -> MateId {
        match self {
            Mate::Origin { id, .. } | Mate::Distance { id, .. } => *id,
        }
    }

    /// The mate's label.
    pub fn name(&self) -> &str {
        match self {
            Mate::Origin { name, .. } | Mate::Distance { name, .. } => name,
        }
    }

    /// The root's components it ties: those its faces are on, or the sub-assemblies that
    /// hold them.
    pub fn components(&self) -> Vec<ComponentId> {
        match self {
            Mate::Origin { component, .. } => vec![*component],
            Mate::Distance { a, b, .. } => [a, b].iter().filter_map(|e| e.holder()).collect(),
        }
    }

    /// The distance's expression, when it has one.
    pub fn expression(&self) -> Option<&Expression> {
        match self {
            Mate::Distance { distance, .. } => Some(distance),
            Mate::Origin { .. } => None,
        }
    }
}

impl FaceEndpoint {
    /// The root component it is on, when it is on one of the root's own parts.
    pub fn component(&self) -> Option<ComponentId> {
        match self.path.steps() {
            [step] if step.member.is_none() => Some(step.component),
            _ => None,
        }
    }

    /// The root component that holds its part: the part itself, or the sub-assembly it is
    /// placed in. `None` for a path through a pattern member, which mates do not reach.
    pub fn holder(&self) -> Option<ComponentId> {
        let steps = self.path.steps();
        (!steps.is_empty() && steps.iter().all(|s| s.member.is_none())).then(|| steps[0].component)
    }
}

/// An entry of the assembly's mate list.
#[allow(clippy::large_enum_variant, reason = "a mate list is short and read one by one")]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MateEntry {
    /// A mate this version understands.
    Known(Mate),
    /// A mate of a family or shape this version does not. It is kept as read and never
    /// counted as solved: while one is there, nothing that would solve can commit.
    Future(Value),
}

impl MateEntry {
    /// The mate, when this version understands it.
    pub fn known(&self) -> Option<&Mate> {
        match self {
            MateEntry::Known(m) => Some(m),
            MateEntry::Future(_) => None,
        }
    }

    /// Its place in the canonical order: known mates by id, then the rest as read.
    pub(crate) fn order(&self) -> (u8, u32) {
        match self {
            MateEntry::Known(m) => (0, m.id().0),
            MateEntry::Future(_) => (1, 0),
        }
    }
}

/// Why a mate command was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalCode {
    /// The candidate leaves freedom a strict request forbids.
    UnderConstrained,
    /// The candidate's mates cannot all hold.
    OverConstrained,
    /// A stored face no longer exists at the proposed revision.
    DanglingReference,
    /// A face selector or reference finds several faces.
    AmbiguousReference,
    /// No face matches a selector.
    NoSuchFace,
    /// A mate or the source it reads names a configuration other than the default.
    UnsupportedConfiguration,
    /// The kernel will not prove this face's plane.
    UnsupportedPlane,
    /// A placed plane's normal is not on a world axis.
    UnsupportedPlacement,
    /// The two faces' planes are not parallel.
    NotParallel,
    /// The part bytes or kernel needed to prove a plane are not available.
    ProofUnavailable,
    /// The assembly holds a mate this version cannot solve.
    UnknownMate,
    /// A literal edit of a value-driven input.
    DrivenInput,
    /// A second driver for a driven input, without asking to replace the first.
    AlreadyDriven,
    /// An expression of the wrong dimension.
    UnitMismatch,
    /// A value that depends on itself.
    ValueCycle,
    /// An expression or value that cannot be used.
    InvalidValue,
    /// A request that names nothing, or names something unusable.
    Invalid,
    /// More components tied together than this solver takes.
    TooLarge,
}

impl RefusalCode {
    /// The code as text.
    pub fn as_str(self) -> &'static str {
        match self {
            RefusalCode::UnderConstrained => "under_constrained",
            RefusalCode::OverConstrained => "over_constrained",
            RefusalCode::DanglingReference => "dangling_reference",
            RefusalCode::AmbiguousReference => "ambiguous_reference",
            RefusalCode::NoSuchFace => "no_such_face",
            RefusalCode::UnsupportedConfiguration => "unsupported_configuration",
            RefusalCode::UnsupportedPlane => "unsupported_plane",
            RefusalCode::UnsupportedPlacement => "unsupported_placement",
            RefusalCode::NotParallel => "not_parallel",
            RefusalCode::ProofUnavailable => "proof_unavailable",
            RefusalCode::UnknownMate => "unknown_mate",
            RefusalCode::DrivenInput => "driven_input",
            RefusalCode::AlreadyDriven => "already_driven",
            RefusalCode::UnitMismatch => "unit_mismatch",
            RefusalCode::ValueCycle => "value_cycle",
            RefusalCode::InvalidValue => "invalid_value",
            RefusalCode::Invalid => "invalid",
            RefusalCode::TooLarge => "too_large",
        }
    }
}

/// A refused mate command: what kind of refusal, what to tell the user, and what it
/// involved. The accepted assembly is exactly as it was.
#[derive(Clone, Debug, PartialEq, Serialize, thiserror::Error)]
#[error("{}: {message}", code.as_str())]
pub struct MateRefusal {
    /// The kind of refusal.
    #[serde(rename = "refused")]
    pub code: RefusalCode,
    /// The problem, for people.
    pub message: String,
    /// The mates involved, by name.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mates: Vec<String>,
    /// How many freedoms the candidate would leave, when that is the problem.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_dof: Option<usize>,
    /// The free motions, when that is the problem.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub free: Vec<FreeMotion>,
}

impl MateRefusal {
    /// A refusal with nothing but a code and a message.
    pub fn new(code: RefusalCode, message: impl Into<String>) -> MateRefusal {
        MateRefusal { code, message: message.into(), mates: Vec::new(), remaining_dof: None, free: Vec::new() }
    }

    pub(crate) fn invalid(message: impl Into<String>) -> MateRefusal {
        MateRefusal::new(RefusalCode::Invalid, message)
    }
}
