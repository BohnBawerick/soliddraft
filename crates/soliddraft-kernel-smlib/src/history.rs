//! Entity tags and history records.
//!
//! The C++ engine keeps a tag attribute on every face, edge and vertex. SMLib copies attributes
//! when it copies, splits or merges an entity, so a tag found on a result entity names
//! the input entity it came from. After each operation this module turns the raw tags
//! into unique ones:
//!
//! - an entity with no tag is new and gets a fresh tag;
//! - a tag found on one entity stays;
//! - a tag found on several entities means the input entity was copied or split. If
//!   exactly one of the pieces has the input's geometry unchanged, it keeps the tag and
//!   the others are new pieces derived from it (a shell's inner skin, for example).
//!   Otherwise every piece gets a fresh tag that remembers the old one: a split.
//!
//! The history record then compares the inputs' entities with the result's.

use std::collections::{BTreeMap, HashMap, HashSet};

use soliddraft_kernel_api::{EntityChanges, Split};

use crate::ffi::{EdgeRec, FaceRec, Snapshot, V3, VertexRec};

/// The geometry used to decide whether an entity changed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Fingerprint {
    /// Area of a face, length of an edge, zero for a vertex.
    pub size: f64,
    /// Area centroid of a face, midpoint of an edge, position of a vertex.
    pub point: V3,
    pub bbox_min: V3,
    pub bbox_max: V3,
    /// Surface kind of a face; 0 for edges.
    pub kind: u8,
}

fn far(a: V3, b: V3, tol: f64) -> bool {
    (a.x - b.x).abs() > tol || (a.y - b.y).abs() > tol || (a.z - b.z).abs() > tol
}

impl Fingerprint {
    /// True when the two agree to within `tol` (absolute, and relative for the size).
    pub fn same(&self, o: &Fingerprint, tol: f64) -> bool {
        self.kind == o.kind
            && (self.size - o.size).abs() <= tol * (1.0 + o.size.abs())
            && !far(self.point, o.point, tol)
            && !far(self.bbox_min, o.bbox_min, tol)
            && !far(self.bbox_max, o.bbox_max, tol)
    }
}

/// A face, edge or vertex with its final, unique tag.
#[derive(Clone, Debug)]
pub(crate) struct Entity {
    pub tag: u64,
    /// For a piece of a copied or split entity, the tag that entity had; otherwise 0.
    pub parent: u64,
    pub geom: Fingerprint,
    /// For a face, the tags of its edges; for an edge, the tags of its faces; for a
    /// vertex, the tags of its edges.
    pub adjacent: Vec<u64>,
    /// For an edge, the tags of its start and end vertex (one for a closed edge).
    pub ends: Vec<u64>,
    /// For a planar face, its outward unit normal; zero otherwise.
    pub normal: V3,
    /// For an edge, its curve kind (see `EdgeRec::curve`).
    pub curve: u8,
}

/// A body's faces, edges and vertices, with final tags.
#[derive(Clone, Debug)]
pub(crate) struct Shot {
    pub manifold_solid: bool,
    pub faces: Vec<Entity>,
    pub edges: Vec<Entity>,
    pub vertices: Vec<Entity>,
    pub bbox_min: V3,
    pub bbox_max: V3,
}

fn face_fp(f: &FaceRec) -> Fingerprint {
    Fingerprint { size: f.area, point: f.centroid, bbox_min: f.bbox_min, bbox_max: f.bbox_max, kind: f.surface }
}

fn edge_fp(e: &EdgeRec) -> Fingerprint {
    Fingerprint { size: e.length, point: e.midpoint, bbox_min: e.bbox_min, bbox_max: e.bbox_max, kind: 0 }
}

fn vertex_fp(v: &VertexRec) -> Fingerprint {
    Fingerprint { size: 0.0, point: v.point, bbox_min: v.point, bbox_max: v.point, kind: 0 }
}

/// Final (tag, parent) for each entity, from the raw tags, the inputs' entities and
/// the next free tag.
fn assign(
    raw: &[(u64, Fingerprint)],
    before: &HashMap<u64, Fingerprint>,
    next_tag: &mut u64,
    tol: f64,
) -> Vec<(u64, u64)> {
    let mut groups: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, (tag, _)) in raw.iter().enumerate() {
        if *tag != 0 {
            groups.entry(*tag).or_default().push(i);
        }
    }
    let keeper = |tag: u64| -> Option<usize> {
        let old = before.get(&tag)?;
        let mut same = groups[&tag].iter().copied().filter(|&i| raw[i].1.same(old, tol));
        let first = same.next();
        if same.next().is_some() { None } else { first }
    };
    raw.iter()
        .enumerate()
        .map(|(i, &(tag, _))| {
            if tag != 0 && (groups[&tag].len() == 1 || keeper(tag) == Some(i)) {
                (tag, 0)
            } else {
                let fresh = *next_tag;
                *next_tag += 1;
                (fresh, tag)
            }
        })
        .collect()
}

/// Turns a raw kernel snapshot into a [`Shot`] with unique tags, given the entities of
/// the operation's inputs. Returns the shot and whether any tag changed (so the new
/// tags must be written back to the kernel body).
pub(crate) fn tag_snapshot(raw: &Snapshot, before: &[&Shot], next_tag: &mut u64, tol: f64) -> (Shot, bool) {
    let before_faces: HashMap<u64, Fingerprint> =
        before.iter().flat_map(|s| s.faces.iter()).map(|e| (e.tag, e.geom)).collect();
    let before_edges: HashMap<u64, Fingerprint> =
        before.iter().flat_map(|s| s.edges.iter()).map(|e| (e.tag, e.geom)).collect();
    let before_vertices: HashMap<u64, Fingerprint> =
        before.iter().flat_map(|s| s.vertices.iter()).map(|e| (e.tag, e.geom)).collect();
    let face_raw: Vec<_> = raw.faces.iter().map(|f| (f.tag, face_fp(f))).collect();
    let edge_raw: Vec<_> = raw.edges.iter().map(|e| (e.tag, edge_fp(e))).collect();
    let vertex_raw: Vec<_> = raw.vertices.iter().map(|v| (v.tag, vertex_fp(v))).collect();
    let face_tags = assign(&face_raw, &before_faces, next_tag, tol);
    let edge_tags = assign(&edge_raw, &before_edges, next_tag, tol);
    let vertex_tags = assign(&vertex_raw, &before_vertices, next_tag, tol);
    let changed = face_tags.iter().zip(&face_raw).any(|(a, r)| a.0 != r.0)
        || edge_tags.iter().zip(&edge_raw).any(|(a, r)| a.0 != r.0)
        || vertex_tags.iter().zip(&vertex_raw).any(|(a, r)| a.0 != r.0);
    let zero = V3::default();
    let faces = raw
        .faces
        .iter()
        .zip(&face_tags)
        .map(|(f, &(tag, parent))| Entity {
            tag,
            parent,
            geom: face_fp(f),
            adjacent: f.edge_indices.iter().map(|&i| edge_tags[i as usize].0).collect(),
            ends: Vec::new(),
            normal: f.normal,
            curve: 0,
        })
        .collect();
    let edges = raw
        .edges
        .iter()
        .zip(&edge_tags)
        .map(|(e, &(tag, parent))| Entity {
            tag,
            parent,
            geom: edge_fp(e),
            adjacent: e.face_indices.iter().map(|&i| face_tags[i as usize].0).collect(),
            ends: e.vertex_indices.iter().map(|&i| vertex_tags[i as usize].0).collect(),
            normal: zero,
            curve: e.curve,
        })
        .collect();
    let vertices = raw
        .vertices
        .iter()
        .zip(&vertex_tags)
        .map(|(v, &(tag, parent))| Entity {
            tag,
            parent,
            geom: vertex_fp(v),
            adjacent: v.edge_indices.iter().map(|&i| edge_tags[i as usize].0).collect(),
            ends: Vec::new(),
            normal: zero,
            curve: 0,
        })
        .collect();
    let shot = Shot {
        manifold_solid: raw.manifold_solid,
        faces,
        edges,
        vertices,
        bbox_min: raw.bbox_min,
        bbox_max: raw.bbox_max,
    };
    (shot, changed)
}

/// Compares the inputs' entities (`before`, possibly from several bodies) with the
/// result's. `tol` is the absolute geometric tolerance for "unchanged".
pub(crate) fn diff<Id: Copy>(
    before: &[&Entity],
    after: &[Entity],
    tol: f64,
    id: impl Fn(u64) -> Id,
) -> EntityChanges<Id> {
    let mut by_tag: HashMap<u64, &Entity> = HashMap::new();
    let mut before_order = Vec::new();
    for e in before {
        if by_tag.insert(e.tag, e).is_none() {
            before_order.push(e.tag);
        }
    }
    let present: HashSet<u64> = after.iter().map(|e| e.tag).collect();
    let mut changes = EntityChanges::default();
    let mut splits: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for e in after {
        if e.parent != 0 && !present.contains(&e.parent) && by_tag.contains_key(&e.parent) {
            splits.entry(e.parent).or_default().push(e.tag);
        } else if e.parent == 0
            && let Some(old) = by_tag.get(&e.tag)
        {
            if !e.geom.same(&old.geom, tol) {
                changes.modified.push(id(e.tag));
            }
        } else {
            changes.created.push(id(e.tag));
        }
    }
    for tag in before_order {
        if !present.contains(&tag) && !splits.contains_key(&tag) {
            changes.deleted.push(id(tag));
        }
    }
    changes.split = splits
        .into_iter()
        .map(|(from, into)| Split { from: id(from), into: into.into_iter().map(&id).collect() })
        .collect();
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(size: f64) -> Fingerprint {
        Fingerprint { size, point: V3::default(), bbox_min: V3::default(), bbox_max: V3::default(), kind: 0 }
    }

    fn ent(tag: u64, parent: u64, size: f64) -> Entity {
        Entity { tag, parent, geom: fp(size), adjacent: Vec::new(), ends: Vec::new(), normal: V3::default(), curve: 0 }
    }

    #[test]
    fn duplicates_keep_the_tag_on_the_unchanged_piece() {
        let before: HashMap<u64, Fingerprint> = [(5, fp(1.0)), (6, fp(1.0))].into();
        // 5 appears twice, one copy unchanged: it keeps 5. 6 appears twice, both changed: split.
        let raw = [(5, fp(1.0)), (5, fp(0.5)), (6, fp(0.3)), (6, fp(0.7)), (0, fp(2.0))];
        let mut next = 100;
        let tags = assign(&raw, &before, &mut next, 1e-9);
        assert_eq!(tags, vec![(5, 0), (100, 5), (101, 6), (102, 6), (103, 0)]);
    }

    #[test]
    fn classifies_created_modified_deleted_and_split() {
        let before = [ent(1, 0, 1.0), ent(2, 0, 1.0), ent(3, 0, 1.0), ent(4, 0, 1.0), ent(5, 0, 1.0)];
        let after = [
            ent(1, 0, 1.0),  // unchanged
            ent(2, 0, 0.5),  // modified
            ent(10, 3, 0.4), // split of 3
            ent(11, 3, 0.6), // split of 3
            ent(12, 0, 2.0), // created
            ent(5, 0, 1.0),  // unchanged
            ent(13, 5, 0.5), // derived from 5, which survives: created
        ];
        let refs: Vec<&Entity> = before.iter().collect();
        let c = diff(&refs, &after, 1e-9, |t| t);
        assert_eq!(c.created, vec![12, 13]);
        assert_eq!(c.modified, vec![2]);
        assert_eq!(c.deleted, vec![4]);
        assert_eq!(c.split, vec![Split { from: 3, into: vec![10, 11] }]);
    }
}
