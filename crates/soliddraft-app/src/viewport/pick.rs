//! CPU ray queries against the display mesh. The id buffer decides what was
//! hit; these give the exact 3D point for the status bar and for later
//! features such as placing a sketch on a face.

use crate::document::{DisplayMesh, Document};
use crate::viewport::render::Picked;
use glam::Vec3;

pub fn hit_point(doc: &Document, picked: Picked, origin: Vec3, dir: Vec3) -> Option<Vec3> {
    match picked {
        Picked::Face(pick) => doc
            .meshes
            .values()
            .filter_map(|m| ray_face(m, pick, origin, dir))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, p)| p),
        Picked::Edge(pick) => doc
            .meshes
            .values()
            .filter_map(|m| ray_edge(m, pick, origin, dir))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, p)| p),
        Picked::Vertex(pick) => {
            doc.meshes.values().flat_map(|m| m.corners.iter()).find(|c| c.pick == pick).map(|c| c.position)
        }
    }
}

fn ray_face(mesh: &DisplayMesh, pick: u32, origin: Vec3, dir: Vec3) -> Option<(f32, Vec3)> {
    let mut best: Option<(f32, Vec3)> = None;
    for tri in mesh.indices.as_chunks::<3>().0 {
        let i0 = tri[0] as usize;
        if mesh.face_picks[i0] != pick {
            continue;
        }
        let (a, b, c) = (mesh.positions[i0], mesh.positions[tri[1] as usize], mesh.positions[tri[2] as usize]);
        if let Some(t) = ray_triangle(origin, dir, a, b, c)
            && best.is_none_or(|(bt, _)| t < bt)
        {
            best = Some((t, origin + dir * t));
        }
    }
    best
}

/// Möller–Trumbore, both-sided.
fn ray_triangle(o: Vec3, d: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 0.0).then_some(t)
}

fn ray_edge(mesh: &DisplayMesh, pick: u32, origin: Vec3, dir: Vec3) -> Option<(f32, Vec3)> {
    let edge = mesh.edges.iter().find(|e| e.pick == pick)?;
    let mut best: Option<(f32, Vec3)> = None;
    for seg in edge.segments.as_chunks::<2>().0 {
        let (d, p) = closest_on_segment(origin, dir, seg[0], seg[1]);
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, p));
        }
    }
    best
}

/// Distance between a ray and a segment, and the point on the segment.
fn closest_on_segment(o: Vec3, d: Vec3, a: Vec3, b: Vec3) -> (f32, Vec3) {
    let u = b - a;
    let w = a - o;
    let (aa, bb, cc) = (d.dot(d), d.dot(u), u.dot(u));
    let (dd, ee) = (d.dot(w), u.dot(w));
    let denom = aa * cc - bb * bb;
    let t = if denom.abs() < 1e-9 { ee / cc.max(1e-9) } else { (aa * ee - bb * dd) / denom };
    let t = t.clamp(0.0, 1.0);
    let s = ((bb * t + dd) / aa.max(1e-9)).max(0.0);
    let p_ray = o + d * s;
    let p_seg = a + u * t;
    ((p_ray - p_seg).length(), p_seg)
}

/// Position of a picked corner.
pub fn vertex_position(doc: &Document, pick: u32) -> Option<Vec3> {
    doc.meshes.values().flat_map(|m| m.corners.iter()).find(|c| c.pick == pick).map(|c| c.position)
}
