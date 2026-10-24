//! Where faces came from, and what each operation did to faces and edges.
//!
//! Every face a backend reports lists its [`Origin`]s: the operations, by the caller's
//! [`OpTag`], that made it and the part each one played ([`FaceRole`]), kept through
//! splits and merges. The document layer names a face by one of its origins and finds
//! it again by that origin; this is the foundation of persistent naming.
//!
//! Every face, edge and vertex also carries an id ([`FaceId`], [`EdgeId`], [`VertexId`])
//! that survives later operations for as long as the entity does. Each operation appends
//! one [`HistoryRecord`] that lists the entities it created, modified, deleted or split,
//! and the role of each face it created. Drawings, the recipe runner and edge labels read
//! these records.

use serde::{Deserialize, Serialize};

use crate::ids::{EdgeId, FaceId, OpTag, VertexId};

/// The operation families of the [`Kernel`](crate::Kernel) trait.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpKind {
    /// `create_body_from_profile`.
    Profile,
    /// `extrude`.
    Extrude,
    /// `revolve`.
    Revolve,
    /// `boolean` with [`BooleanOp::Union`](crate::BooleanOp::Union).
    Union,
    /// `boolean` with [`BooleanOp::Subtract`](crate::BooleanOp::Subtract).
    Subtract,
    /// `boolean` with [`BooleanOp::Intersect`](crate::BooleanOp::Intersect).
    Intersect,
    /// `fillet`.
    Fillet,
    /// `chamfer`.
    Chamfer,
    /// `shell`.
    Shell,
}

impl OpKind {
    /// Lower-case name, as used in recipes and messages.
    pub fn name(self) -> &'static str {
        match self {
            OpKind::Profile => "profile",
            OpKind::Extrude => "extrude",
            OpKind::Revolve => "revolve",
            OpKind::Union => "union",
            OpKind::Subtract => "subtract",
            OpKind::Intersect => "intersect",
            OpKind::Fillet => "fillet",
            OpKind::Chamfer => "chamfer",
            OpKind::Shell => "shell",
        }
    }
}

impl std::fmt::Display for OpKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// One input entity that the operation divided into several.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Split<Id> {
    /// The input entity. It no longer exists in the result.
    pub from: Id,
    /// The result entities it became, each with a new id.
    pub into: Vec<Id>,
}

/// How one operation changed one kind of entity. Entities present before and after
/// with unchanged geometry are not listed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityChanges<Id> {
    /// New entities with no ancestor among the inputs.
    pub created: Vec<Id>,
    /// Entities that kept their id but whose geometry or extent changed.
    pub modified: Vec<Id>,
    /// Input entities with no descendant in the result.
    pub deleted: Vec<Id>,
    /// Input entities divided into several result entities.
    pub split: Vec<Split<Id>>,
}

impl<Id> Default for EntityChanges<Id> {
    fn default() -> Self {
        EntityChanges { created: Vec::new(), modified: Vec::new(), deleted: Vec::new(), split: Vec::new() }
    }
}

impl<Id: Copy + PartialEq> EntityChanges<Id> {
    /// Every result entity this operation produced: created ones and split children.
    pub fn generated(&self) -> impl Iterator<Item = Id> + '_ {
        self.created.iter().copied().chain(self.split.iter().flat_map(|s| s.into.iter().copied()))
    }

    /// True when the operation touched `id` in any way.
    pub fn mentions(&self, id: Id) -> bool {
        self.created.contains(&id)
            || self.modified.contains(&id)
            || self.deleted.contains(&id)
            || self.split.iter().any(|s| s.from == id || s.into.contains(&id))
    }
}

/// What part of an operation made a face it created.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum FaceRole {
    /// The planar region of a profile body.
    Region,
    /// The cap of a sweep in the profile plane.
    StartCap,
    /// The cap of a sweep at its far end: the extrusion distance, or the revolution angle.
    EndCap,
    /// A face swept from one segment of the profile.
    Side {
        /// The profile loop: 0 is the outer loop, 1 and up the holes in order.
        loop_index: u32,
        /// The segment within that loop.
        segment: u32,
    },
    /// A face of a fillet or chamfer.
    Blend {
        /// The input edge it rounds or bevels. For a patch where several blends meet,
        /// the nearest of them.
        edge: EdgeId,
    },
    /// The inner wall of a shell, offset from an input face.
    Offset {
        /// The input face it is offset from.
        face: FaceId,
    },
    /// A face the operation made that fits none of the other roles, such as a patch a
    /// boolean added.
    Other,
}

/// One origin of a face: the operation that made it, by the caller's tag, and the part
/// of that operation that made it. A face keeps its origins through later operations:
/// every piece of a split face keeps the whole set, and a face merged from several
/// carries all of theirs. [`FaceInfo::origins`](crate::FaceInfo::origins) lists them.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Origin {
    /// The operation's tag.
    pub tag: OpTag,
    /// What part of it made the face.
    pub role: FaceRole,
}

/// The role of one created face.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FaceOrigin {
    /// A face listed in the record's `faces.created`.
    pub face: FaceId,
    /// What made it.
    pub role: FaceRole,
}

/// What one operation did, keyed by the caller's tag.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryRecord {
    /// Position of the operation in the kernel session; increases with every operation.
    pub seq: u64,
    /// The caller's tag for the operation, for example a feature id.
    pub tag: OpTag,
    /// Which operation ran.
    pub op: OpKind,
    /// Face changes.
    pub faces: EntityChanges<FaceId>,
    /// Edge changes.
    pub edges: EntityChanges<EdgeId>,
    /// Vertex changes.
    #[serde(default)]
    pub vertices: EntityChanges<VertexId>,
    /// One entry per created face, in the order of `faces.created`.
    #[serde(default)]
    pub face_roles: Vec<FaceOrigin>,
}

impl HistoryRecord {
    /// The role of a face this operation created, if it created it.
    pub fn role_of(&self, face: FaceId) -> Option<FaceRole> {
        self.face_roles.iter().find(|o| o.face == face).map(|o| o.role)
    }
}
