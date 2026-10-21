//! References from a drawing to entities of the model.

use serde::{Deserialize, Serialize};
use soliddraft_kernel_api::Vec3;
use soliddraft_model::{EdgeRef, FaceRef};

/// The part's own persistent name for an entity, from `soliddraft-model`: what an anchor
/// is resolved with after a rebuild.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelEntity {
    /// An edge, named by the two faces it separates.
    Edge(EdgeRef),
    /// A face, named by the feature and the part of it that made the face.
    Face(FaceRef),
}

/// What kind of model entity a reference names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    /// An edge between two faces.
    Edge,
    /// A face.
    Face,
    /// A vertex.
    Vertex,
}

/// Where an entity was when the reference was last resolved, used to pick between
/// candidates when the topology changes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fingerprint {
    /// A characteristic point in model space: an edge's midpoint, a face's centroid or
    /// the vertex itself.
    pub point: Vec3,
    /// A characteristic size in millimetres: an edge's length, a face's area or zero.
    pub size: f64,
}

/// A persistent reference to a model entity: the creating feature, the role the entity
/// plays in that feature, an ordinal among entities with the same role, and a fingerprint.
///
/// The feature, role, ordinal and fingerprint describe the entity in words and numbers;
/// `model` carries the part's own persistent name, which is what resolution uses when it
/// is there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntityRef {
    /// Edge, face or vertex.
    pub kind: EntityKind,
    /// Tag of the feature that created the entity, such as `"plate"`.
    pub feature: String,
    /// The entity's role in that feature, such as `"side"`, `"cap_end"` or `"wall"`.
    pub role: String,
    /// Index among the feature's entities with the same role.
    pub ordinal: u32,
    /// Geometry at the last resolve.
    pub fingerprint: Fingerprint,
    /// The kernel's id for the entity in the current body, a hint that is only valid
    /// until the next rebuild.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel_hint: Option<u64>,
    /// The part's persistent name for the entity, when the drawing was made from a part
    /// that names its entities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelEntity>,
}

impl EntityRef {
    /// A reference without a kernel hint.
    pub fn new(kind: EntityKind, feature: &str, role: &str, ordinal: u32, point: Vec3, size: f64) -> EntityRef {
        EntityRef {
            kind,
            feature: feature.to_owned(),
            role: role.to_owned(),
            ordinal,
            fingerprint: Fingerprint { point, size },
            kernel_hint: None,
            model: None,
        }
    }
}
