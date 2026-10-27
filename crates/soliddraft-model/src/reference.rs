//! Persistent references: naming faces and edges by where they came from.
//!
//! Kernel ids change on every rebuild, so a feature never stores one. It stores a name
//! made of the feature that generated the face, the part of that feature that made it
//! (its [`Generator`]: a cap, the side swept from a given sketch entity, the blend of a
//! given edge) and an ordinal among faces with the same generator. An edge is named by
//! the two faces it separates.
//!
//! The name stands for an origin. On each rebuild the generating feature says which
//! kernel origin (operation tag and role) each of its names stands for, and the kernel
//! reports every face's origins: the pieces of a split face all keep the origin, and a
//! face merged from several carries all of theirs. A name resolves to the faces that
//! carry its origin, so it survives renames, splits, merges and dimension edits without
//! following history or comparing geometry.
//!
//! The result is checked against a geometric fingerprint taken when the reference was
//! made: the surface or curve type must not change, and a planar face must still face
//! the same way. The fingerprint never chooses a face; its position and size only rank
//! the candidates offered when a reference breaks.
//!
//! A reference that resolves to no entity, to several, or to one that fails the check
//! does not guess. The feature stops with a [`RepairNeeded`] that says which reference
//! broke, why, and which entities could replace it.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use soliddraft_kernel_api::{
    CurveKind, EdgeId, EdgeInfo, FaceId, FaceInfo, HolePart, Origin, SurfaceKind, Topology, Vec3,
};
use soliddraft_sketch::EntityId;

use crate::feature::FeatureId;

/// The part of a feature that made a face.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Generator {
    /// An extrusion's cap in the sketch plane.
    StartCap,
    /// An extrusion's cap at its far end.
    EndCap,
    /// A side swept from a sketch entity.
    Side {
        /// The sketch line, arc or circle.
        entity: EntityId,
    },
    /// A fillet or chamfer face.
    Blend {
        /// Index of the edge in the feature's edge list.
        edge: u32,
    },
    /// A face of a hole.
    Hole {
        /// Index of the hole in the feature's point list.
        placement: u32,
        /// Which face of the hole.
        part: HolePart,
    },
    /// A face of a pattern's or mirror's copy. Faces of one copy are ranked by the
    /// names of the faces they copy.
    Copy {
        /// Which copy: 1 and up for a pattern, 1 for a mirror image.
        instance: u32,
    },
    /// Any other face a feature made.
    Other,
    /// An inset wall of a shell, ranked by its source face name.
    ShellWall,
}

impl fmt::Display for Generator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Generator::StartCap => f.write_str("Start cap"),
            Generator::EndCap => f.write_str("End cap"),
            Generator::Side { entity } => write!(f, "Side of {entity}"),
            Generator::Blend { edge } => write!(f, "Blend of edge {}", edge + 1),
            Generator::Hole { placement, part } => write!(f, "Hole {} {}", placement + 1, part.label().to_lowercase()),
            Generator::Copy { instance } => write!(f, "Copy {instance} face"),
            Generator::Other => f.write_str("Face"),
            Generator::ShellWall => f.write_str("Inner wall"),
        }
    }
}

/// A name a feature defined during a rebuild, with the kernel origin it stands for.
#[derive(Clone, Debug, PartialEq)]
pub struct NamedFace {
    /// The operation and role the kernel reports on the faces this name finds. Valid in
    /// this rebuild only: operation tags are the document's, but a rebuild may rename them.
    pub origin: Origin,
    /// What made it.
    pub generator: Generator,
    /// Its rank among the feature's faces with the same generator.
    pub ordinal: u32,
}

/// What must stay true of a referenced face.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FaceFingerprint {
    /// Surface type; must not change.
    pub surface: SurfaceKind,
    /// Outward normal of a planar face; must not change direction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normal: Option<Vec3>,
    /// Area centroid when the reference was made; ranks candidates.
    pub centroid: Vec3,
    /// Area when the reference was made; ranks candidates.
    pub area: f64,
}

impl FaceFingerprint {
    /// The fingerprint of a face as it is now.
    pub fn of(face: &FaceInfo) -> Self {
        FaceFingerprint { surface: face.surface, normal: face.normal, centroid: face.centroid, area: face.area }
    }

    /// Why `face` is not the face this fingerprint describes, if it is not.
    fn mismatch(&self, face: &FaceInfo) -> Option<String> {
        if face.surface != self.surface {
            return Some(format!("the face became {:?} instead of {:?}", face.surface, self.surface));
        }
        match (self.normal, face.normal) {
            (Some(a), Some(b)) if a.dot(b) < 1.0 - 1e-6 => {
                Some(format!("the face now faces {:?} instead of {:?}", round3(b), round3(a)))
            }
            _ => None,
        }
    }

    fn score(&self, face: &FaceInfo) -> f64 {
        let kind = if face.surface == self.surface { 0.0 } else { 1e6 };
        kind + face.centroid.distance(self.centroid) + (face.area - self.area).abs().sqrt()
    }
}

/// What must stay true of a referenced edge.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EdgeFingerprint {
    /// Curve type; must not change.
    pub curve: CurveKind,
    /// Midpoint when the reference was made; ranks candidates.
    pub midpoint: Vec3,
    /// Length when the reference was made; ranks candidates.
    pub length: f64,
}

impl EdgeFingerprint {
    /// The fingerprint of an edge as it is now.
    pub fn of(edge: &EdgeInfo) -> Self {
        EdgeFingerprint { curve: edge.curve, midpoint: edge.midpoint, length: edge.length }
    }

    fn score(&self, edge: &EdgeInfo) -> f64 {
        let kind = if edge.curve == self.curve { 0.0 } else { 1e6 };
        kind + edge.midpoint.distance(self.midpoint) + (edge.length - self.length).abs()
    }
}

fn round3(v: Vec3) -> [f64; 3] {
    let r = |x: f64| (x * 1000.0).round() / 1000.0 + 0.0;
    [r(v.x), r(v.y), r(v.z)]
}

/// A face, named by origin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FaceRef {
    /// The feature that generated it.
    pub feature: FeatureId,
    /// The part of the feature that made it.
    pub generator: Generator,
    /// Rank among the feature's faces with the same generator.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub ordinal: u32,
    /// The check.
    pub fingerprint: FaceFingerprint,
    /// What the face was called when the reference was made, such as
    /// "Boss-Extrude1 · End cap", for repair prompts. Resolution never reads it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

/// An edge, named by the two faces it separates (the same face twice for a seam).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EdgeRef {
    /// The faces on either side.
    pub faces: [FaceRef; 2],
    /// The check.
    pub fingerprint: EdgeFingerprint,
    /// What the edge was called when the reference was made, for repair prompts.
    /// Resolution never reads it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
}

/// Which reference of a feature broke.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "slot", rename_all = "snake_case")]
pub enum ReferenceSlot {
    /// A sketch's plane face.
    SketchPlane,
    /// One of a fillet's or chamfer's edges.
    Edge {
        /// Index in the feature's edge list.
        index: u32,
    },
    /// One of a draft's faces.
    Face {
        /// Index in the feature's face list.
        index: u32,
    },
    /// The face a hole starts on, or the face of a mirror's or draft's plane.
    Plane,
    /// The face an extrusion goes up to.
    Limit,
}

impl fmt::Display for ReferenceSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReferenceSlot::SketchPlane => f.write_str("sketch plane"),
            ReferenceSlot::Edge { index } => write!(f, "edge {}", index + 1),
            ReferenceSlot::Face { index } => write!(f, "face {}", index + 1),
            ReferenceSlot::Plane => f.write_str("plane"),
            ReferenceSlot::Limit => f.write_str("end face"),
        }
    }
}

/// Why a reference broke.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum RepairReason {
    /// Nothing in the body matches the name any more.
    Missing,
    /// Several entities match; the reference does not choose among them.
    Ambiguous,
    /// One entity matches but its geometry fails the fingerprint check.
    Mismatch {
        /// What changed.
        detail: String,
    },
}

/// An entity a repair could point a reference at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum Candidate {
    /// A face of the feature's input body.
    Face(FaceId),
    /// An edge of the feature's input body.
    Edge(EdgeId),
}

/// A reference that did not resolve to exactly one matching entity. The feature that
/// holds it is not built until the reference is repaired.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepairNeeded {
    /// Which reference.
    pub slot: ReferenceSlot,
    /// Why.
    pub reason: RepairReason,
    /// The problem, for people.
    pub message: String,
    /// Entities that could replace it, best match first: the several matches when the
    /// name is ambiguous, the nearest by fingerprint otherwise.
    pub candidates: Vec<Candidate>,
}

/// Why an entity could not be given a persistent name.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum NamingError {
    /// The entity is not in any body of the document's current rebuild.
    #[error("{0} is not part of the model")]
    NotInModel(String),
    /// No feature defines a name for any origin the kernel reports on the entity.
    #[error("{0} has no origin in the feature tree")]
    NoOrigin(String),
    /// The name would match other entities too, for example a piece of a face that a
    /// later feature split.
    #[error("{0} cannot be named uniquely: {1}")]
    NotUnique(String, String),
}

/// The names each feature defined in the current rebuild.
pub(crate) type NameMap<'a> = HashMap<FeatureId, &'a [NamedFace]>;

/// A body as references see it.
pub(crate) struct View<'a> {
    pub topology: &'a Topology,
}

/// A reference that did not resolve.
#[derive(Debug)]
pub(crate) struct Unresolved {
    pub reason: RepairReason,
    pub message: String,
    pub candidates: Vec<Candidate>,
}

impl Unresolved {
    pub fn into_repair(self, slot: ReferenceSlot) -> RepairNeeded {
        RepairNeeded { slot, reason: self.reason, message: self.message, candidates: self.candidates }
    }
}

const CANDIDATES: usize = 5;

/// How far apart, in millimetres, two planar pieces of one origin may lie and still
/// count as one sketch plane. The pieces of a split face share their surface exactly;
/// this only absorbs the display mesh's round-off in their centroids.
const SAME_PLANE: f64 = 1e-6;

/// The faces of the body that carry an origin, in topology order.
fn carriers<'a>(origin: &'a Origin, view: &'a View) -> impl Iterator<Item = FaceId> + 'a {
    view.topology.faces.iter().filter(move |f| f.origins.contains(origin)).map(|f| f.id)
}

fn ranked_faces(fp: &FaceFingerprint, faces: impl Iterator<Item = FaceId>, view: &View) -> Vec<Candidate> {
    let mut scored: Vec<(f64, FaceId)> =
        faces.filter_map(|id| view.topology.face(id).map(|f| (fp.score(f), id))).collect();
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    scored.into_iter().take(CANDIDATES).map(|(_, id)| Candidate::Face(id)).collect()
}

/// The current faces a face reference names, in topology order. Never empty.
fn face_set(r: &FaceRef, view: &View, names: &NameMap) -> Result<Vec<FaceId>, Unresolved> {
    let missing = |message: String| Unresolved {
        reason: RepairReason::Missing,
        message,
        candidates: ranked_faces(&r.fingerprint, view.topology.faces.iter().map(|f| f.id), view),
    };
    let Some(made) = names.get(&r.feature) else {
        return Err(missing(format!("{} is not built before this feature", r.feature)));
    };
    let Some(seed) = made.iter().find(|n| n.generator == r.generator && n.ordinal == r.ordinal) else {
        return Err(missing(format!("{} no longer makes its {} #{}", r.feature, r.generator, r.ordinal + 1)));
    };
    let present: Vec<FaceId> = carriers(&seed.origin, view).collect();
    if present.is_empty() {
        return Err(missing(format!("the {} of {} is gone", r.generator, r.feature)));
    }
    Ok(present)
}

/// The one face a reference names.
pub(crate) fn resolve_face(r: &FaceRef, view: &View, names: &NameMap) -> Result<FaceId, Unresolved> {
    let set = face_set(r, view, names)?;
    if set.len() > 1 {
        return Err(Unresolved {
            reason: RepairReason::Ambiguous,
            message: format!("the {} of {} was split into {} faces", r.generator, r.feature, set.len()),
            candidates: ranked_faces(&r.fingerprint, set.into_iter(), view),
        });
    }
    let face = view.topology.face(set[0]).expect("face_set returns faces of the topology");
    if let Some(detail) = r.fingerprint.mismatch(face) {
        return Err(Unresolved {
            reason: RepairReason::Mismatch { detail: detail.clone() },
            message: format!("the {} of {}: {detail}", r.generator, r.feature),
            candidates: vec![Candidate::Face(face.id)],
        });
    }
    Ok(face.id)
}

/// The plane a sketch-plane reference names: the one face it names, or the pieces of a
/// split planar face when they all lie in one plane, since any of them gives the same
/// sketch plane. Returns the first such face in topology order.
pub(crate) fn resolve_plane(r: &FaceRef, view: &View, names: &NameMap) -> Result<FaceId, Unresolved> {
    let set = face_set(r, view, names)?;
    let face = |id: FaceId| view.topology.face(id).expect("face_set returns faces of the topology");
    let first = face(set[0]);
    let mismatch = set.iter().find_map(|id| r.fingerprint.mismatch(face(*id)));
    let coplanar = first.normal.is_some_and(|n| {
        set.iter().all(|id| {
            let f = face(*id);
            f.normal == Some(n) && (f.centroid - first.centroid).dot(n).abs() <= SAME_PLANE
        })
    });
    match mismatch {
        Some(detail) => Err(Unresolved {
            reason: RepairReason::Mismatch { detail: detail.clone() },
            message: format!("the {} of {}: {detail}", r.generator, r.feature),
            candidates: ranked_faces(&r.fingerprint, set.into_iter(), view),
        }),
        None if set.len() > 1 && !coplanar => Err(Unresolved {
            reason: RepairReason::Ambiguous,
            message: format!(
                "the {} of {} was split into {} faces that do not share a plane",
                r.generator,
                r.feature,
                set.len()
            ),
            candidates: ranked_faces(&r.fingerprint, set.into_iter(), view),
        }),
        None => Ok(first.id),
    }
}

/// Every current face a face reference names, for an assignment that follows a face
/// through later features, such as an appearance: every piece of a split face matches.
/// Only a missing face, or one whose surface type or facing changed, breaks it.
pub(crate) fn resolve_faces(r: &FaceRef, view: &View, names: &NameMap) -> Result<Vec<FaceId>, Unresolved> {
    let set = face_set(r, view, names)?;
    let face = |id: &FaceId| view.topology.face(*id).expect("face_set returns faces of the topology");
    let fits: Vec<FaceId> = set.iter().copied().filter(|id| r.fingerprint.mismatch(face(id)).is_none()).collect();
    if fits.is_empty() {
        let detail = r.fingerprint.mismatch(face(&set[0])).unwrap_or_default();
        return Err(Unresolved {
            reason: RepairReason::Mismatch { detail: detail.clone() },
            message: format!("the {} of {}: {detail}", r.generator, r.feature),
            candidates: ranked_faces(&r.fingerprint, set.into_iter(), view),
        });
    }
    Ok(fits)
}

/// Every current face that carries an origin of a feature's names, in topology order.
pub(crate) fn faces_of_feature(feature: FeatureId, view: &View, names: &NameMap) -> Vec<FaceId> {
    let Some(made) = names.get(&feature) else { return Vec::new() };
    let origins: Vec<&Origin> = made.iter().map(|n| &n.origin).collect();
    view.topology.faces.iter().filter(|f| f.origins.iter().any(|o| origins.contains(&o))).map(|f| f.id).collect()
}

fn ranked_edges(fp: &EdgeFingerprint, edges: impl Iterator<Item = EdgeId>, view: &View) -> Vec<Candidate> {
    let mut scored: Vec<(f64, EdgeId)> =
        edges.filter_map(|id| view.topology.edge(id).map(|e| (fp.score(e), id))).collect();
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    scored.into_iter().take(CANDIDATES).map(|(_, id)| Candidate::Edge(id)).collect()
}

/// The one edge a reference names: the edge between a face carrying the first name's
/// origin and a face carrying the second's. Faces may have been split or merged, as long
/// as one edge remains.
pub(crate) fn resolve_edge(r: &EdgeRef, view: &View, names: &NameMap) -> Result<EdgeId, Unresolved> {
    let all_edges = || view.topology.edges.iter().map(|e| e.id);
    let sides = [face_set(&r.faces[0], view, names), face_set(&r.faces[1], view, names)];
    let mut sets = Vec::new();
    for (side, fr) in sides.into_iter().zip(&r.faces) {
        let set = side.map_err(|u| Unresolved {
            candidates: ranked_edges(&r.fingerprint, all_edges(), view),
            message: format!("its face: {}", u.message),
            ..u
        })?;
        let fits: Vec<FaceId> = set
            .iter()
            .copied()
            .filter(|id| fr.fingerprint.mismatch(view.topology.face(*id).unwrap()).is_none())
            .collect();
        if fits.is_empty() {
            let detail = fr.fingerprint.mismatch(view.topology.face(set[0]).unwrap()).unwrap_or_default();
            return Err(Unresolved {
                reason: RepairReason::Mismatch { detail: detail.clone() },
                message: format!("its face, the {} of {}: {detail}", fr.generator, fr.feature),
                candidates: ranked_edges(&r.fingerprint, all_edges(), view),
            });
        }
        sets.push(fits);
    }
    let (a, b) = (&sets[0], &sets[1]);
    let between = |e: &&EdgeInfo| match e.faces.as_slice() {
        [p, q] => (a.contains(p) && b.contains(q)) || (a.contains(q) && b.contains(p)),
        _ => false,
    };
    let found: Vec<&EdgeInfo> = view.topology.edges.iter().filter(between).collect();
    let label = format!(
        "{} of {} and {} of {}",
        r.faces[0].generator, r.faces[0].feature, r.faces[1].generator, r.faces[1].feature
    );
    match found.as_slice() {
        [] => Err(Unresolved {
            reason: RepairReason::Missing,
            message: format!("no edge is left between the {label}"),
            candidates: ranked_edges(&r.fingerprint, all_edges(), view),
        }),
        [e] if e.curve != r.fingerprint.curve => {
            let detail = format!("the edge became {:?} instead of {:?}", e.curve, r.fingerprint.curve);
            Err(Unresolved {
                reason: RepairReason::Mismatch { detail: detail.clone() },
                message: format!("the edge between the {label}: {detail}"),
                candidates: vec![Candidate::Edge(e.id)],
            })
        }
        [e] => Ok(e.id),
        many => Err(Unresolved {
            reason: RepairReason::Ambiguous,
            message: format!("{} edges lie between the {label}", many.len()),
            candidates: ranked_edges(&r.fingerprint, many.iter().map(|e| e.id), view),
        }),
    }
}

/// The origin-based name of a face, without checking that it is unique. A face merged
/// from several carries several names; the earliest feature's comes first.
fn origin_name(face: FaceId, view: &View, names: &NameMap) -> Result<FaceRef, NamingError> {
    let info = view.topology.face(face).ok_or_else(|| NamingError::NotInModel(face.to_string()))?;
    let mut ids: Vec<&FeatureId> = names.keys().collect();
    ids.sort();
    for id in ids {
        if let Some(n) = names[id].iter().find(|n| info.origins.contains(&n.origin)) {
            return Ok(FaceRef {
                feature: *id,
                generator: n.generator,
                ordinal: n.ordinal,
                fingerprint: FaceFingerprint::of(info),
                label: String::new(),
            });
        }
    }
    Err(NamingError::NoOrigin(face.to_string()))
}

/// A persistent name for a face of the body, checked to resolve back to that face.
pub(crate) fn name_face(face: FaceId, view: &View, names: &NameMap) -> Result<FaceRef, NamingError> {
    let r = origin_name(face, view, names)?;
    match resolve_face(&r, view, names) {
        Ok(back) if back == face => Ok(r),
        Ok(other) => Err(NamingError::NotUnique(face.to_string(), format!("the name finds {other}"))),
        Err(u) => Err(NamingError::NotUnique(face.to_string(), u.message)),
    }
}

/// A persistent name for a planar face to sketch on, checked to resolve back to that
/// face's plane. A piece of a split face can carry a sketch, since all the pieces of
/// its origin share one plane; [`name_face`] refuses it.
pub(crate) fn name_plane(face: FaceId, view: &View, names: &NameMap) -> Result<FaceRef, NamingError> {
    let r = origin_name(face, view, names)?;
    let plane_of = |id: FaceId| view.topology.face(id).and_then(|f| f.normal.map(|n| (n, f.centroid.dot(n))));
    let (Some((n, d)), Ok(back)) = (plane_of(face), resolve_plane(&r, view, names)) else {
        let why =
            if plane_of(face).is_none() { "it is not planar".to_owned() } else { "its name finds no plane".into() };
        return Err(NamingError::NotUnique(face.to_string(), why));
    };
    match plane_of(back) {
        Some((m, e)) if m == n && (e - d).abs() <= SAME_PLANE => Ok(r),
        _ => Err(NamingError::NotUnique(face.to_string(), format!("the name finds {back}"))),
    }
}

/// A persistent name for an edge of the body, checked to resolve back to that edge.
pub(crate) fn name_edge(edge: EdgeId, view: &View, names: &NameMap) -> Result<EdgeRef, NamingError> {
    let info = view.topology.edge(edge).ok_or_else(|| NamingError::NotInModel(edge.to_string()))?;
    let [a, b] = info.faces.as_slice() else {
        return Err(NamingError::NotUnique(edge.to_string(), format!("it bounds {} faces", info.faces.len())));
    };
    let r = EdgeRef {
        faces: [origin_name(*a, view, names)?, origin_name(*b, view, names)?],
        fingerprint: EdgeFingerprint::of(info),
        label: String::new(),
    };
    match resolve_edge(&r, view, names) {
        Ok(back) if back == edge => Ok(r),
        Ok(other) => Err(NamingError::NotUnique(edge.to_string(), format!("the name finds {other}"))),
        Err(u) => Err(NamingError::NotUnique(edge.to_string(), u.message)),
    }
}

/// The generator and ordinal of a face a feature made, for labels.
pub(crate) fn origin_label(face: FaceId, view: &View, names: &NameMap) -> Option<(FeatureId, Generator, u32)> {
    origin_name(face, view, names).ok().map(|r| (r.feature, r.generator, r.ordinal))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soliddraft_kernel_api::{Aabb, FaceRole, OpTag};

    fn origin(tag: &str, role: FaceRole) -> Origin {
        Origin { tag: OpTag::new(tag), role }
    }

    fn face(id: u64, z: f64, origins: Vec<Origin>) -> FaceInfo {
        FaceInfo {
            id: FaceId::from_raw(id),
            surface: SurfaceKind::Plane,
            area: 1.0,
            centroid: Vec3::new(id as f64, 0.0, z),
            bbox: Aabb { min: Vec3::ZERO, max: Vec3::ZERO },
            edges: Vec::new(),
            normal: Some(Vec3::Z),
            origins,
        }
    }

    fn topology(faces: Vec<FaceInfo>) -> Topology {
        Topology {
            kind: soliddraft_kernel_api::BodyKind::Solid,
            faces,
            edges: Vec::new(),
            vertex_count: 0,
            vertices: Vec::new(),
            bbox: Aabb { min: Vec3::ZERO, max: Vec3::new(1.0, 1.0, 1.0) },
            display_chord_height: None,
        }
    }

    #[test]
    fn names_resolve_by_origin_through_splits_and_merges() {
        let plate = origin("F2.r0.sweep", FaceRole::EndCap);
        let tab = origin("F4.r0.sweep", FaceRole::EndCap);
        let named = [NamedFace { origin: plate.clone(), generator: Generator::EndCap, ordinal: 0 }];
        let tab_named = [NamedFace { origin: tab.clone(), generator: Generator::EndCap, ordinal: 0 }];
        let names: NameMap = [(FeatureId(2), &named[..]), (FeatureId(4), &tab_named[..])].into();
        // The plate's top merged with the tab's: one face with both origins.
        let merged = topology(vec![face(1, 10.0, vec![plate.clone(), tab.clone()]), face(2, 0.0, vec![])]);
        let view = View { topology: &merged };
        let r = name_face(FaceId::from_raw(1), &view, &names).unwrap();
        assert_eq!((r.feature, r.generator), (FeatureId(2), Generator::EndCap), "the earliest feature names it");
        assert_eq!(resolve_face(&r, &view, &names).unwrap(), FaceId::from_raw(1));
        // A later cut split it into two coplanar pieces: one sketch plane, two faces.
        let split = topology(vec![face(3, 10.0, vec![plate.clone()]), face(4, 10.0, vec![plate.clone(), tab])]);
        let view = View { topology: &split };
        assert_eq!(resolve_plane(&r, &view, &names).unwrap(), FaceId::from_raw(3));
        assert!(matches!(resolve_face(&r, &view, &names).unwrap_err().reason, RepairReason::Ambiguous));
        assert_eq!(resolve_faces(&r, &view, &names).unwrap().len(), 2);
        // Pieces in different planes are not one sketch plane.
        let apart = topology(vec![face(3, 10.0, vec![plate.clone()]), face(4, 12.0, vec![plate])]);
        let view = View { topology: &apart };
        assert!(matches!(resolve_plane(&r, &view, &names).unwrap_err().reason, RepairReason::Ambiguous));
        // Nothing carries the origin any more.
        let gone = topology(vec![face(5, 10.0, vec![])]);
        let view = View { topology: &gone };
        assert!(matches!(resolve_plane(&r, &view, &names).unwrap_err().reason, RepairReason::Missing));
    }
}
