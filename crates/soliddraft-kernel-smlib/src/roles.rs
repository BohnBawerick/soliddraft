//! The role of every face an operation creates.
//!
//! SMLib does not say which part of a sweep made a face, so the roles come from the
//! geometry of the result against the operation's own inputs:
//!
//! - a sweep's caps are the planar faces parallel to the profile plane, at the start or
//!   at the far end;
//! - a side face has an edge lying on one profile segment in the profile plane, and that
//!   segment named it;
//! - a blend face rounds the selected edge whose box it sits closest to;
//! - a shell's inner wall is a copy of the outer face it was offset from, which the tag
//!   attribute already records as the piece's parent.

use soliddraft_kernel_api::{EdgeId, FaceId, FaceOrigin, FaceRole, Plane, Vec3};

use crate::convert::{to_local_point, vec3};
use crate::ffi::CurveIn;
use crate::history::{Entity, Shot};

/// What an operation's faces are measured against.
pub(crate) enum Roles {
    /// A profile body: every face is the region.
    Region,
    /// A linear sweep of a profile.
    Extrude { plane: Plane, curves: Vec<CurveIn>, loop_sizes: Vec<u32>, local_dir: Vec3, distance: f64 },
    /// A rotational sweep of a profile about a world axis.
    Revolve { plane: Plane, curves: Vec<CurveIn>, loop_sizes: Vec<u32>, origin: Vec3, axis: Vec3, angle_deg: f64 },
    /// A fillet or chamfer of the edges with these tags.
    Blend { edges: Vec<u64> },
    /// A shell: new faces are offsets of the faces they were copied from.
    Offset,
    /// Anything else, such as a boolean.
    Other,
}

/// One role per created face, in the order of `created`.
pub(crate) fn assign(roles: &Roles, created: &[FaceId], shot: &Shot, inputs: &[&Shot], tol: f64) -> Vec<FaceOrigin> {
    let faces = created.iter().filter_map(|id| shot.faces.iter().find(|f| f.tag == id.raw()));
    faces.map(|f| FaceOrigin { face: FaceId::from_raw(f.tag), role: role(roles, f, shot, inputs, tol) }).collect()
}

fn role(roles: &Roles, face: &Entity, shot: &Shot, inputs: &[&Shot], tol: f64) -> FaceRole {
    match roles {
        Roles::Region => FaceRole::Region,
        Roles::Extrude { plane, curves, loop_sizes, local_dir, distance } => {
            if let Some(n) = planar_normal(face)
                && n.dot(plane.normal).abs() > 1.0 - 1e-9
            {
                let z = to_local_point(plane, vec3(face.geom.point)).z;
                if z.abs() <= tol {
                    return FaceRole::StartCap;
                }
                if (z - distance * local_dir.z).abs() <= tol {
                    return FaceRole::EndCap;
                }
            }
            side(face, shot, plane, curves, loop_sizes, tol)
        }
        Roles::Revolve { plane, curves, loop_sizes, origin, axis, angle_deg } => {
            if let Some(n) = planar_normal(face) {
                let c = vec3(face.geom.point);
                if n.dot(plane.normal).abs() > 1.0 - 1e-9 && to_local_point(plane, c).z.abs() <= tol {
                    return FaceRole::StartCap;
                }
                let back = |p: Vec3| rotate(p - *origin, *axis, -angle_deg.to_radians()) + *origin;
                let n_back = rotate(n, *axis, -angle_deg.to_radians());
                if *angle_deg < 360.0
                    && n_back.dot(plane.normal).abs() > 1.0 - 1e-9
                    && to_local_point(plane, back(c)).z.abs() <= tol
                {
                    return FaceRole::EndCap;
                }
            }
            side(face, shot, plane, curves, loop_sizes, tol)
        }
        Roles::Blend { edges } => {
            let center = mid(face.geom.bbox_min, face.geom.bbox_max);
            let chosen =
                inputs.iter().flat_map(|s| s.edges.iter()).filter(|e| edges.contains(&e.tag)).min_by(|a, b| {
                    let d = |e: &Entity| mid(e.geom.bbox_min, e.geom.bbox_max).distance(center);
                    d(a).total_cmp(&d(b))
                });
            match chosen {
                Some(e) => FaceRole::Blend { edge: EdgeId::from_raw(e.tag) },
                None => FaceRole::Other,
            }
        }
        Roles::Offset if face.parent != 0 => FaceRole::Offset { face: FaceId::from_raw(face.parent) },
        Roles::Offset | Roles::Other => FaceRole::Other,
    }
}

fn planar_normal(face: &Entity) -> Option<Vec3> {
    crate::convert::normal(face.normal)
}

fn mid(a: crate::ffi::V3, b: crate::ffi::V3) -> Vec3 {
    (vec3(a) + vec3(b)) * 0.5
}

/// The side role of a swept face: the profile segment one of its edges lies on, in the
/// profile plane.
fn side(face: &Entity, shot: &Shot, plane: &Plane, curves: &[CurveIn], loop_sizes: &[u32], tol: f64) -> FaceRole {
    for edge_tag in &face.adjacent {
        let Some(edge) = shot.edges.iter().find(|e| e.tag == *edge_tag) else { continue };
        let mut points = vec![vec3(edge.geom.point)];
        for v in &edge.ends {
            if let Some(vertex) = shot.vertices.iter().find(|x| x.tag == *v) {
                points.push(vec3(vertex.geom.point));
            }
        }
        let local: Vec<Vec3> = points.into_iter().map(|p| to_local_point(plane, p)).collect();
        if local.iter().any(|p| p.z.abs() > tol) {
            continue;
        }
        let mut index = 0usize;
        for (loop_index, &size) in loop_sizes.iter().enumerate() {
            for segment in 0..size as usize {
                let curve = &curves[index + segment];
                if local.iter().all(|p| on_curve(curve, p.x, p.y, tol)) {
                    return FaceRole::Side { loop_index: loop_index as u32, segment: segment as u32 };
                }
            }
            index += size as usize;
        }
    }
    FaceRole::Other
}

/// True when the local point (u, v) lies on the profile curve.
fn on_curve(c: &CurveIn, u: f64, v: f64, tol: f64) -> bool {
    match c.kind {
        0 => {
            let (ax, ay, bx, by) = (c.a.x, c.a.y, c.b.x, c.b.y);
            let (dx, dy) = (bx - ax, by - ay);
            let len2 = dx * dx + dy * dy;
            let t = if len2 > 0.0 { (((u - ax) * dx + (v - ay) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
            let (px, py) = (ax + t * dx - u, ay + t * dy - v);
            (px * px + py * py).sqrt() <= tol
        }
        1 | 2 => {
            let (du, dv) = (u - c.center.x, v - c.center.y);
            let r = (du * du + dv * dv).sqrt();
            if (r - c.radius).abs() > tol {
                return false;
            }
            if c.kind == 2 {
                return true;
            }
            // Inside the arc's sweep, allowing the tolerance at its ends.
            let slack = (tol / c.radius.max(tol)).to_degrees();
            let mut a = dv.atan2(du).to_degrees();
            while a < c.start_deg - slack {
                a += 360.0;
            }
            a <= c.end_deg + slack
        }
        _ => false,
    }
}

/// Rotates a vector about a unit axis by `angle` radians (Rodrigues).
fn rotate(v: Vec3, axis: Vec3, angle: f64) -> Vec3 {
    let (s, c) = angle.sin_cos();
    v * c + axis.cross(v) * s + axis * (axis.dot(v) * (1.0 - c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffi::V3;

    fn line(a: (f64, f64), b: (f64, f64)) -> CurveIn {
        let zero = V3::default();
        CurveIn {
            kind: 0,
            a: V3 { x: a.0, y: a.1, z: 0.0 },
            b: V3 { x: b.0, y: b.1, z: 0.0 },
            center: zero,
            radius: 0.0,
            start_deg: 0.0,
            end_deg: 0.0,
        }
    }

    #[test]
    fn points_on_lines_and_arcs() {
        let l = line((0.0, 0.0), (10.0, 0.0));
        assert!(on_curve(&l, 5.0, 0.0, 1e-9));
        assert!(!on_curve(&l, 11.0, 0.0, 1e-9));
        let zero = V3::default();
        let arc = CurveIn { kind: 1, a: zero, b: zero, center: zero, radius: 2.0, start_deg: 350.0, end_deg: 460.0 };
        assert!(on_curve(&arc, 2.0, 0.0, 1e-9), "angle 0 is inside 350..460");
        assert!(on_curve(&arc, 0.0, 2.0, 1e-9), "angle 90 is inside");
        assert!(!on_curve(&arc, -2.0, 0.0, 1e-9), "angle 180 is outside");
    }

    #[test]
    fn rotation_turns_about_the_axis() {
        let r = rotate(Vec3::X, Vec3::Z, std::f64::consts::FRAC_PI_2);
        assert!(r.distance(Vec3::Y) < 1e-12);
    }
}
