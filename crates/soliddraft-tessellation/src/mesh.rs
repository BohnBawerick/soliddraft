//! Display meshes: what the viewport draws for a part without its kernel body.
//!
//! A display mesh is in the part's own frame, in millimetres. Its triangles are grouped
//! by face, and each face carries the part's persistent name for it (the model's face
//! reference as canonical JSON, empty when unknown), so a pick on an occurrence maps to
//! a face of the part that survives rebuilds; runtime kernel face ids never get in. The
//! mesh may carry the part's edges as polylines.
//!
//! The binary form (little-endian) is what a part file keeps for its coarse and standard
//! levels (`cache/mesh/coarse.bin`, `cache/mesh/standard.bin`) and what an assembly keeps
//! in `cache/refs` for each part revision it pins (`display.bin`):
//!
//! | Bytes | Field |
//! |---|---|
//! | 4 | magic `DWDM` |
//! | 4 | version, `u32` 1 |
//! | 4 + n | what made it (kernel, tessellation level), length-prefixed UTF-8 |
//! | 4 x 4 | vertex, index, face and edge counts, `u32` |
//! | 12 per vertex | positions, `3 x f32` |
//! | 12 per vertex | unit normals, `3 x f32` |
//! | 4 per index | indices, `u32`, three per triangle, counter-clockwise from outside |
//! | per face | first triangle `u32`, triangle count `u32`, name length-prefixed UTF-8 |
//! | per edge | point count `u32`, points `3 x f32` each |

use std::collections::HashMap;
use std::sync::Arc;

use glam::{DVec3, Vec3};

use soliddraft_kernel_api::{FaceId, Mesh};

use crate::binary::{BinaryError, Reader, Writer};
use crate::cache::Weigh;

/// The entry name of a display copy inside its `cache/refs/<id>/<revision>/` folder.
pub const DISPLAY_MESH: &str = "display.bin";

const MAGIC: &[u8; 4] = b"DWDM";
const VERSION: u32 = 1;

/// One face's triangles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeshFace {
    /// The first triangle.
    pub first: u32,
    /// How many triangles, consecutive from `first`.
    pub count: u32,
    /// The part's persistent name for the face, or empty.
    pub name: String,
}

/// A part's display mesh.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisplayMesh {
    /// Vertex positions.
    pub positions: Vec<[f32; 3]>,
    /// Unit normals, one per position.
    pub normals: Vec<[f32; 3]>,
    /// Three per triangle.
    pub indices: Vec<u32>,
    /// The faces, in triangle order, covering every triangle.
    pub faces: Vec<MeshFace>,
    /// Edges as polylines.
    pub edges: Vec<Vec<[f32; 3]>>,
}

impl DisplayMesh {
    /// The number of triangles.
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// The face a triangle belongs to.
    pub fn face_of_triangle(&self, triangle: u32) -> Option<usize> {
        let i = self.faces.partition_point(|f| f.first + f.count <= triangle);
        (i < self.faces.len() && self.faces[i].first <= triangle).then_some(i)
    }

    /// Roughly how many bytes the mesh holds in memory.
    pub fn heap_bytes(&self) -> usize {
        self.positions.len() * 24
            + self.indices.len() * 4
            + self.faces.iter().map(|f| 8 + f.name.len() + 24).sum::<usize>()
            + self.edges.iter().map(|e| e.len() * 12 + 24).sum::<usize>()
    }

    /// Checks the mesh is consistent: normals per vertex, indices in range, faces in
    /// order covering every triangle.
    pub fn check(&self) -> Result<(), String> {
        if self.normals.len() != self.positions.len() {
            return Err("normals and positions differ in number".into());
        }
        if !self.indices.len().is_multiple_of(3) {
            return Err("the index count is not a multiple of three".into());
        }
        if self.indices.iter().any(|&i| i as usize >= self.positions.len()) {
            return Err("an index is out of range".into());
        }
        let mut next = 0u32;
        for f in &self.faces {
            if f.first != next {
                return Err("faces do not cover the triangles in order".into());
            }
            next = f.first.checked_add(f.count).ok_or("a face is out of range")?;
        }
        if next as usize != self.triangle_count() {
            return Err("faces do not cover every triangle".into());
        }
        if !self.positions.iter().chain(&self.normals).flatten().all(|v| v.is_finite()) {
            return Err("a vertex is not finite".into());
        }
        Ok(())
    }

    /// The binary form. `made_by` says what made it, such as "smlib c9c5979 coarse".
    pub fn encode(&self, made_by: &str) -> Vec<u8> {
        let mut w = Writer::with_capacity(32 + self.heap_bytes());
        w.raw(MAGIC);
        w.u32(VERSION);
        w.str(made_by);
        w.u32(self.positions.len() as u32);
        w.u32(self.indices.len() as u32);
        w.u32(self.faces.len() as u32);
        w.u32(self.edges.len() as u32);
        for v in self.positions.iter().chain(&self.normals).flatten() {
            w.f32(*v);
        }
        for i in &self.indices {
            w.u32(*i);
        }
        for f in &self.faces {
            w.u32(f.first);
            w.u32(f.count);
            w.str(&f.name);
        }
        for e in &self.edges {
            w.u32(e.len() as u32);
            for v in e.iter().flatten() {
                w.f32(*v);
            }
        }
        w.bytes
    }

    /// A mesh from its binary form, with what made it.
    pub fn decode(bytes: &[u8]) -> Result<(DisplayMesh, String), BinaryError> {
        let bad = |what: &str| BinaryError { entry: DISPLAY_MESH, what: what.to_owned() };
        let short = || bad("cut short");
        let mut r = Reader::new(bytes);
        if r.take(4) != Some(MAGIC.as_slice()) {
            return Err(bad("it does not start with DWDM"));
        }
        match r.u32() {
            Some(VERSION) => {}
            Some(v) => return Err(bad(&format!("version {v} needs a newer SolidDraft"))),
            None => return Err(short()),
        }
        let made_by = r.str().ok_or_else(short)?.to_owned();
        let counts = [r.u32(), r.u32(), r.u32(), r.u32()];
        let [Some(nv), Some(ni), Some(nf), Some(ne)] = counts.map(|c| c.map(|c| c as usize)) else {
            return Err(short());
        };
        // Refuse counts the data cannot hold before allocating for them.
        if nv.saturating_mul(24).saturating_add(ni.saturating_mul(4)).saturating_add(nf.saturating_mul(12))
            > r.remaining()
        {
            return Err(short());
        }
        let positions = vec3s(r.take(nv * 12).ok_or_else(short)?);
        let normals = vec3s(r.take(nv * 12).ok_or_else(short)?);
        let indices: Vec<u32> =
            r.take(ni * 4).ok_or_else(short)?.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect();
        let mut faces = Vec::with_capacity(nf);
        for _ in 0..nf {
            let (first, count) = (r.u32().ok_or_else(short)?, r.u32().ok_or_else(short)?);
            faces.push(MeshFace { first, count, name: r.str().ok_or_else(short)?.to_owned() });
        }
        let mut edges = Vec::with_capacity(ne.min(r.remaining() / 4));
        for _ in 0..ne {
            let n = r.u32().ok_or_else(short)? as usize;
            if n.saturating_mul(12) > r.remaining() {
                return Err(short());
            }
            edges.push(vec3s(r.take(n * 12).ok_or_else(short)?));
        }
        if r.remaining() != 0 {
            return Err(bad("it has bytes after its last edge"));
        }
        let mesh = DisplayMesh { positions, normals, indices, faces, edges };
        mesh.check().map_err(|m| bad(&m))?;
        Ok((mesh, made_by))
    }

    /// The same mesh in a canonical order, so equal shapes give equal bytes whatever
    /// order the kernel listed them in. Faces are ordered by name, then by their first
    /// triangle; each face's triangles start at their smallest corner (keeping their
    /// winding) and are sorted; vertices are numbered in order of first use; edges are
    /// sorted by their points. SMLib lists
    /// faces in an order that can change from run to run, so display data is saved in
    /// this order.
    pub fn canonical(&self) -> DisplayMesh {
        let key = |i: u32| {
            let i = i as usize;
            let (p, n) = (self.positions[i], self.normals[i]);
            [p[0], p[1], p[2], n[0], n[1], n[2]].map(f32::to_bits)
        };
        let mut faces: Vec<(&MeshFace, Vec<[u32; 3]>)> = self
            .faces
            .iter()
            .map(|f| {
                let range = f.first as usize * 3..(f.first + f.count) as usize * 3;
                let mut tris: Vec<[u32; 3]> = self.indices[range]
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|t| {
                        let smallest = (0..3).min_by_key(|&k| key(t[k])).expect("three corners");
                        [t[smallest], t[(smallest + 1) % 3], t[(smallest + 2) % 3]]
                    })
                    .collect();
                tris.sort_by_key(|t| t.map(key));
                (f, tris)
            })
            .collect();
        faces.sort_by(|(a, ta), (b, tb)| {
            a.name.cmp(&b.name).then_with(|| ta.first().map(|t| t.map(key)).cmp(&tb.first().map(|t| t.map(key))))
        });
        let mut out = DisplayMesh { edges: self.edges.clone(), ..DisplayMesh::default() };
        let mut renumber: HashMap<u32, u32> = HashMap::new();
        for (f, tris) in faces {
            let first = out.indices.len() as u32 / 3;
            for t in &tris {
                for &old in t {
                    let new = *renumber.entry(old).or_insert_with(|| {
                        out.positions.push(self.positions[old as usize]);
                        out.normals.push(self.normals[old as usize]);
                        out.positions.len() as u32 - 1
                    });
                    out.indices.push(new);
                }
            }
            out.faces.push(MeshFace { first, count: tris.len() as u32, name: f.name.clone() });
        }
        out.edges.sort_by_cached_key(|e| e.iter().map(|p| p.map(f32::to_bits)).collect::<Vec<_>>());
        out
    }

    /// A box from `min` to `max` with six named faces, two triangles each: a stand-in
    /// for tests and generated assemblies.
    pub fn cuboid(min: DVec3, max: DVec3) -> DisplayMesh {
        let (a, b) = (min.as_vec3(), max.as_vec3());
        let mut m = DisplayMesh::default();
        let sides: [(Vec3, [Vec3; 4], &str); 6] = [
            (
                Vec3::NEG_X,
                [
                    Vec3::new(a.x, a.y, a.z),
                    Vec3::new(a.x, a.y, b.z),
                    Vec3::new(a.x, b.y, b.z),
                    Vec3::new(a.x, b.y, a.z),
                ],
                "-x",
            ),
            (
                Vec3::X,
                [
                    Vec3::new(b.x, a.y, a.z),
                    Vec3::new(b.x, b.y, a.z),
                    Vec3::new(b.x, b.y, b.z),
                    Vec3::new(b.x, a.y, b.z),
                ],
                "+x",
            ),
            (
                Vec3::NEG_Y,
                [
                    Vec3::new(a.x, a.y, a.z),
                    Vec3::new(b.x, a.y, a.z),
                    Vec3::new(b.x, a.y, b.z),
                    Vec3::new(a.x, a.y, b.z),
                ],
                "-y",
            ),
            (
                Vec3::Y,
                [
                    Vec3::new(a.x, b.y, a.z),
                    Vec3::new(a.x, b.y, b.z),
                    Vec3::new(b.x, b.y, b.z),
                    Vec3::new(b.x, b.y, a.z),
                ],
                "+y",
            ),
            (
                Vec3::NEG_Z,
                [
                    Vec3::new(a.x, a.y, a.z),
                    Vec3::new(a.x, b.y, a.z),
                    Vec3::new(b.x, b.y, a.z),
                    Vec3::new(b.x, a.y, a.z),
                ],
                "-z",
            ),
            (
                Vec3::Z,
                [
                    Vec3::new(a.x, a.y, b.z),
                    Vec3::new(b.x, a.y, b.z),
                    Vec3::new(b.x, b.y, b.z),
                    Vec3::new(a.x, b.y, b.z),
                ],
                "+z",
            ),
        ];
        for (n, quad, name) in sides {
            let base = m.positions.len() as u32;
            m.positions.extend(quad.map(|p| p.to_array()));
            m.normals.extend([n.to_array(); 4]);
            m.indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
            m.faces.push(MeshFace { first: m.faces.len() as u32 * 2, count: 2, name: name.to_owned() });
        }
        m
    }
}

/// Little-endian `3 x f32` triples.
fn vec3s(bytes: &[u8]) -> Vec<[f32; 3]> {
    bytes
        .as_chunks::<12>()
        .0
        .iter()
        .map(|c| std::array::from_fn(|k| f32::from_le_bytes([c[k * 4], c[k * 4 + 1], c[k * 4 + 2], c[k * 4 + 3]])))
        .collect()
}

/// A kernel mesh as a display mesh: triangles grouped by face in order of first
/// appearance, each face named by `name`.
pub fn display_mesh(mesh: &Mesh, mut name: impl FnMut(FaceId) -> String, edges: Vec<Vec<[f32; 3]>>) -> DisplayMesh {
    let mut order: Vec<FaceId> = Vec::new();
    let mut tris: HashMap<FaceId, Vec<usize>> = HashMap::new();
    for (t, f) in mesh.triangle_faces.iter().enumerate() {
        tris.entry(*f)
            .or_insert_with(|| {
                order.push(*f);
                Vec::new()
            })
            .push(t);
    }
    let mut out = DisplayMesh {
        positions: mesh.positions.clone(),
        normals: mesh.normals.clone(),
        indices: Vec::with_capacity(mesh.indices.len()),
        faces: Vec::with_capacity(order.len()),
        edges,
    };
    for f in order {
        let first = out.indices.len() as u32 / 3;
        let list = &tris[&f];
        for &t in list {
            out.indices.extend_from_slice(&mesh.indices[t * 3..t * 3 + 3]);
        }
        out.faces.push(MeshFace { first, count: list.len() as u32, name: name(f) });
    }
    out
}

impl Weigh for DisplayMesh {
    fn triangles(&self) -> usize {
        self.triangle_count()
    }

    fn bytes(&self) -> usize {
        self.heap_bytes()
    }
}

/// A display mesh shared between the definition store, the renderer and the picker.
pub type SharedMesh = Arc<DisplayMesh>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meshes_round_trip_and_name_their_faces() {
        let mut m = DisplayMesh::cuboid(DVec3::ZERO, DVec3::new(10.0, 4.0, 2.0));
        m.edges.push(vec![[0.0; 3], [10.0, 0.0, 0.0]]);
        m.check().unwrap();
        assert_eq!(m.triangle_count(), 12);
        let bytes = m.encode("test coarse");
        let (back, made_by) = DisplayMesh::decode(&bytes).unwrap();
        assert_eq!((back, made_by.as_str()), (m.clone(), "test coarse"));
        assert_eq!(m.face_of_triangle(0), Some(0));
        assert_eq!(m.face_of_triangle(5), Some(2));
        assert_eq!(m.face_of_triangle(11), Some(5));
        assert_eq!(m.face_of_triangle(12), None);
        assert!(DisplayMesh::decode(&bytes[..bytes.len() - 3]).is_err());
        let mut broken = m.clone();
        broken.indices[0] = 999;
        assert!(DisplayMesh::decode(&broken.encode("")).is_err());
    }

    #[test]
    fn triangles_are_grouped_by_face() {
        let mesh = Mesh {
            unavailable_controls: Vec::new(),
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 4],
            indices: vec![0, 1, 2, 1, 3, 2, 0, 2, 1],
            triangle_faces: vec![FaceId::from_raw(7), FaceId::from_raw(9), FaceId::from_raw(7)],
        };
        let d = display_mesh(&mesh, |f| format!("face {}", f.raw()), Vec::new());
        d.check().unwrap();
        assert_eq!(d.indices, vec![0, 1, 2, 0, 2, 1, 1, 3, 2]);
        assert_eq!(d.faces[0], MeshFace { first: 0, count: 2, name: "face 7".into() });
        assert_eq!(d.faces[1], MeshFace { first: 2, count: 1, name: "face 9".into() });
    }

    #[test]
    fn canonical_order_does_not_depend_on_the_kernels_order() {
        let m = DisplayMesh::cuboid(DVec3::ZERO, DVec3::new(3.0, 2.0, 1.0));
        m.check().unwrap();
        // The same shape listed differently: faces reversed, vertices shuffled, each
        // triangle starting at another corner.
        let mut shuffled = DisplayMesh { edges: m.edges.clone(), ..DisplayMesh::default() };
        let order: Vec<u32> = (0..m.positions.len() as u32).rev().collect();
        let place = |old: u32| order.iter().position(|&o| o == old).unwrap() as u32;
        for &old in &order {
            shuffled.positions.push(m.positions[old as usize]);
            shuffled.normals.push(m.normals[old as usize]);
        }
        for f in m.faces.iter().rev() {
            let first = shuffled.indices.len() as u32 / 3;
            for t in m.indices[f.first as usize * 3..(f.first + f.count) as usize * 3].chunks(3).rev() {
                shuffled.indices.extend([place(t[1]), place(t[2]), place(t[0])]);
            }
            shuffled.faces.push(MeshFace { first, count: f.count, name: f.name.clone() });
        }
        shuffled.check().unwrap();
        assert_ne!(shuffled, m);
        assert_eq!(shuffled.canonical(), m.canonical());
        assert_eq!(m.canonical().encode("x"), shuffled.canonical().encode("x"));
        let c = m.canonical();
        c.check().unwrap();
        assert_eq!(c.triangle_count(), m.triangle_count());
        assert!(c.faces.windows(2).all(|w| w[0].name <= w[1].name));
        // Winding is kept: every triangle still faces out.
        for t in c.indices.chunks(3) {
            let [a, b, d] = [0, 1, 2].map(|k| Vec3::from_array(c.positions[t[k] as usize]));
            assert!((b - a).cross(d - a).dot(Vec3::from_array(c.normals[t[0] as usize])) > 0.0);
        }
        assert_eq!(c.canonical(), c, "canonical is a fixed point");
    }

    #[test]
    fn cuboid_normals_face_outward() {
        let m = DisplayMesh::cuboid(DVec3::ZERO, DVec3::ONE);
        for t in m.indices.chunks(3) {
            let [a, b, c] = [0, 1, 2].map(|k| Vec3::from_array(m.positions[t[k] as usize]));
            let n = Vec3::from_array(m.normals[t[0] as usize]);
            assert!((b - a).cross(c - a).dot(n) > 0.0, "counter-clockwise seen from outside");
        }
    }
}
