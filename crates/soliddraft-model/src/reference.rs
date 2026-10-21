//! Persistent references: naming faces and edges by where they came from.
//!
//! Kernel ids change on every rebuild, so a feature never stores one. It stores a name
//! made of the feature that generated the entity, the part of that feature that made it
//! (its [`Generator`]: a cap, the side swept from a given sketch entity, the blend of a
//! given edge) and an ordinal among faces with the same generator. On each rebuild the
//! name is looked up among the faces the generating feature made in this rebuild and
//! followed through the kernel's history: split faces become their pieces, deleted ones
//! drop out. An edge is named by the two faces it separates.
//!
//! The result is checked against a geometric fingerprint taken when the reference was
//! made: the surface or curve type must not change, and a planar face must still face
//! the same way. Position and size are allowed to change, since that is what editing a
//! dimension does; they are used to rank candidates when a reference breaks.
//!
//! A reference that resolves to no entity, to several, or to one that fails the check
//! does not guess. The feature stops with a [`RepairNeeded`] that says which reference
//! broke, why, and which entities could replace it.

use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use soliddraft_kernel_api::{
    CurveKind, EdgeId, EdgeInfo, FaceId, FaceInfo, HistoryRecord, SurfaceKind, Topology, Vec3,
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
    /// Any other face a feature made.
    Other,
}

impl fmt::Display for Generator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Generator::StartCap => f.write_str("Start cap"),
            Generator::EndCap => f.write_str("End cap"),
            Generator::Side { entity } => write!(f, "Side of {entity}"),
            Generator::Blend { edge } => write!(f, "Blend of edge {}", edge + 1),
            Generator::Other => f.write_str("Face"),
        }
    }
}

/// A face as a feature generated it during a rebuild.
#[derive(Clone, Debug, PartialEq)]
pub struct NamedFace {
    /// The kernel face, valid in this rebuild only.
    pub face: FaceId,
    /// What made it.
    pub generator: Generator,
    /// Its rank among the feature's faces with the same generator.
    pub ordinal: u32,
    /// The history record that created it.
    pub(crate) seq: u64,
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
}

impl fmt::Display for ReferenceSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReferenceSlot::SketchPlane => f.write_str("sketch plane"),
            ReferenceSlot::Edge { index } => write!(f, "edge {}", index + 1),
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
    /// No feature generated the entity, or the history does not reach its origin.
    #[error("{0} has no origin in the feature tree")]
    NoOrigin(String),
    /// The name would match other entities too, for example a piece of a face that a
    /// later feature split.
    #[error("{0} cannot be named uniquely: {1}")]
    NotUnique(String, String),
}

/// The faces each feature generated in the current rebuild.
pub(crate) type NameMap<'a> = HashMap<FeatureId, &'a [NamedFace]>;

/// A body as references see it.
pub(crate) struct View<'a> {
    pub topology: &'a Topology,
    pub history: &'a [HistoryRecord],
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

/// The faces the named face became, following splits and deletions after the record
/// that created it.
fn descendants(seed: FaceId, seq: u64, history: &[HistoryRecord]) -> HashSet<FaceId> {
    let mut set: HashSet<FaceId> = [seed].into();
    for r in history.iter().filter(|r| r.seq > seq) {
        for s in &r.faces.split {
            if set.remove(&s.from) {
                set.extend(s.into.iter().copied());
            }
        }
        for d in &r.faces.deleted {
            set.remove(d);
        }
    }
    set
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
    let alive = descendants(seed.face, seed.seq, view.history);
    let present: Vec<FaceId> = view.topology.faces.iter().map(|f| f.id).filter(|id| alive.contains(id)).collect();
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

/// Every current face a feature generated, through any splits since, in topology
/// order.
pub(crate) fn faces_of_feature(feature: FeatureId, view: &View, names: &NameMap) -> Vec<FaceId> {
    let Some(made) = names.get(&feature) else { return Vec::new() };
    let mut alive = HashSet::new();
    for n in made.iter() {
        alive.extend(descendants(n.face, n.seq, view.history));
    }
    view.topology.faces.iter().map(|f| f.id).filter(|id| alive.contains(id)).collect()
}

fn ranked_edges(fp: &EdgeFingerprint, edges: impl Iterator<Item = EdgeId>, view: &View) -> Vec<Candidate> {
    let mut scored: Vec<(f64, EdgeId)> =
        edges.filter_map(|id| view.topology.edge(id).map(|e| (fp.score(e), id))).collect();
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    scored.into_iter().take(CANDIDATES).map(|(_, id)| Candidate::Edge(id)).collect()
}

/// The one edge a reference names: the edge between a face the first name became and a
/// face the second became. Faces may have been split, as long as one edge remains.
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

/// The face a face came from: the face a record created, through any splits since.
fn origin_of(face: FaceId, history: &[HistoryRecord]) -> Option<(FaceId, u64)> {
    let mut x = face;
    for r in history.iter().rev() {
        if r.faces.created.contains(&x) {
            return Some((x, r.seq));
        }
        if let Some(s) = r.faces.split.iter().find(|s| s.into.contains(&x)) {
            x = s.from;
        }
    }
    None
}

/// The origin-based name of a face, without checking that it is unique.
fn origin_name(face: FaceId, view: &View, names: &NameMap) -> Result<FaceRef, NamingError> {
    let info = view.topology.face(face).ok_or_else(|| NamingError::NotInModel(face.to_string()))?;
    let (seed, seq) = origin_of(face, view.history).ok_or_else(|| NamingError::NoOrigin(face.to_string()))?;
    let mut ids: Vec<&FeatureId> = names.keys().collect();
    ids.sort();
    for id in ids {
        if let Some(n) = names[id].iter().find(|n| n.face == seed && n.seq == seq) {
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
    use soliddraft_kernel_api::{EntityChanges, OpKind, OpTag, Split};

    fn record(seq: u64, created: &[u64], deleted: &[u64], split: &[(u64, &[u64])]) -> HistoryRecord {
        let ids = |v: &[u64]| v.iter().map(|&i| FaceId::from_raw(i)).collect();
        HistoryRecord {
            seq,
            tag: OpTag::new(format!("op{seq}")),
            op: OpKind::Union,
            faces: EntityChanges {
                created: ids(created),
                modified: vec![],
                deleted: ids(deleted),
                split: split.iter().map(|(f, i)| Split { from: FaceId::from_raw(*f), into: ids(i) }).collect(),
            },
            edges: EntityChanges::default(),
            vertices: EntityChanges::default(),
            face_roles: vec![],
        }
    }

    #[test]
    fn descendants_follow_splits_after_the_creating_record() {
        let h = vec![record(1, &[1, 2], &[], &[]), record(2, &[], &[], &[(1, &[3, 4])]), record(3, &[], &[3], &[])];
        let d = descendants(FaceId::from_raw(1), 1, &h);
        assert_eq!(d, [FaceId::from_raw(4)].into());
        assert_eq!(origin_of(FaceId::from_raw(4), &h), Some((FaceId::from_raw(1), 1)));
        assert_eq!(origin_of(FaceId::from_raw(2), &h), Some((FaceId::from_raw(2), 1)));
        assert_eq!(origin_of(FaceId::from_raw(9), &h), None);
    }
}
