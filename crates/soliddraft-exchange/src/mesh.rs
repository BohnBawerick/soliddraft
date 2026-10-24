//! A kernel mesh with one vertex per position, as mesh files want it.
//!
//! The kernel's display mesh repeats a position on every face that meets there, so each
//! face keeps its own normals. Catallax's meshes share edges exactly: the triangles on
//! both sides of an edge use the same positions. Merging vertices whose 32-bit
//! coordinates are bit for bit equal therefore restores the kernel's shared mesh without
//! any distance tolerance.

use std::collections::HashMap;

use soliddraft_kernel_api::Mesh;

/// Triangles over shared vertices.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Welded {
    /// Distinct positions, in millimetres.
    pub positions: Vec<[f32; 3]>,
    /// Three indices into `positions` per triangle, counter-clockwise seen from outside.
    pub triangles: Vec<[u32; 3]>,
}

/// Merges `mesh`'s vertices with equal coordinates. Triangles left with a repeated
/// vertex have no area and are dropped.
pub fn weld(mesh: &Mesh) -> Welded {
    let mut out = Welded::default();
    let mut index: HashMap<[u32; 3], u32> = HashMap::new();
    let mut remap = Vec::with_capacity(mesh.positions.len());
    for p in &mesh.positions {
        // -0.0 and 0.0 are the same point.
        let key = p.map(|v| if v == 0.0 { 0 } else { v.to_bits() });
        let i = *index.entry(key).or_insert_with(|| {
            out.positions.push(*p);
            out.positions.len() as u32 - 1
        });
        remap.push(i);
    }
    for t in mesh.indices.chunks_exact(3) {
        let t = [remap[t[0] as usize], remap[t[1] as usize], remap[t[2] as usize]];
        if t[0] != t[1] && t[1] != t[2] && t[2] != t[0] {
            out.triangles.push(t);
        }
    }
    out
}

impl Welded {
    /// True when every edge is used once in each direction: the mesh is closed, has no
    /// edge shared by more than two triangles, and its triangles agree on which side is
    /// outside.
    pub fn is_closed(&self) -> bool {
        let mut uses: HashMap<(u32, u32), i32> = HashMap::new();
        for t in &self.triangles {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *uses.entry((a, b)).or_default() += 1;
            }
        }
        !self.triangles.is_empty() && uses.iter().all(|(&(a, b), &n)| n == 1 && uses.get(&(b, a)) == Some(&1))
    }

    /// The enclosed volume in cubic millimetres, by the divergence theorem. Meaningful for
    /// a closed mesh.
    pub fn volume(&self) -> f64 {
        let p = |i: u32| self.positions[i as usize].map(f64::from);
        self.triangles
            .iter()
            .map(|t| {
                let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
                let cross = [b[1] * c[2] - b[2] * c[1], b[2] * c[0] - b[0] * c[2], b[0] * c[1] - b[1] * c[0]];
                (a[0] * cross[0] + a[1] * cross[1] + a[2] * cross[2]) / 6.0
            })
            .sum()
    }

    /// The unit normal of a triangle from its corners, or zero for a sliver too thin to
    /// have one.
    pub fn normal(&self, t: [u32; 3]) -> [f32; 3] {
        let p = |i: u32| self.positions[i as usize].map(f64::from);
        let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
        let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 0.0 { n.map(|x| (x / len) as f32) } else { [0.0; 3] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soliddraft_kernel_api::FaceId;

    /// A unit cube with its vertices repeated per face, as the kernel gives it.
    fn cube() -> Mesh {
        let c = |i: u32| [(i & 1) as f32, ((i >> 1) & 1) as f32, ((i >> 2) & 1) as f32];
        let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
        let mut mesh = Mesh::default();
        for (f, q) in quads.iter().enumerate() {
            let base = mesh.positions.len() as u32;
            mesh.positions.extend(q.iter().map(|&i| c(i)));
            mesh.normals.extend([[0.0; 3]; 4]);
            mesh.indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
            mesh.triangle_faces.extend([FaceId::from_raw(f as u64); 2]);
        }
        mesh
    }

    #[test]
    fn welding_a_per_face_cube_closes_it() {
        let w = weld(&cube());
        assert_eq!((w.positions.len(), w.triangles.len()), (8, 12));
        assert!(w.is_closed());
        assert!((w.volume() - 1.0).abs() < 1e-12, "{}", w.volume());
        assert_eq!(w.normal(w.triangles[0]), [0.0, 0.0, -1.0]);
    }

    #[test]
    fn an_open_or_inside_out_mesh_is_not_closed() {
        let mut open = cube();
        open.indices.truncate(30);
        assert!(!weld(&open).is_closed());

        let mut flipped = cube();
        flipped.indices.swap(0, 1);
        assert!(!weld(&flipped).is_closed());

        assert!(!Welded::default().is_closed());
    }

    #[test]
    fn negative_zero_is_zero() {
        let mut mesh = cube();
        mesh.positions[0] = [-0.0, -0.0, -0.0];
        assert!(weld(&mesh).is_closed());
    }
}
