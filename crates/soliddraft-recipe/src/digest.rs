//! A geometric digest of a body: a hash of its counts, volume, area and the sizes and
//! positions of its faces and edges, rounded so it does not depend on kernel ids, on the
//! order the kernel lists entities in, or on round-off.
//!
//! Values are rounded to a grid whose spacing is a power of ten about a millionth of the
//! body's size (`quantum`). Two runs that agree to well within that grid get the same
//! digest; a value that lands within round-off of a grid boundary can still flip, which
//! is why the canonical text is available for comparing two digests line by line.

use std::fmt::Write as _;

use sha2::{Digest as _, Sha256};
use soliddraft_kernel_api::{BodyKind, MassProperties, SurfaceKind, Topology, Vec3};

/// The body's scale as a power of ten: floor(log10(diagonal)), at least 0.
fn scale_exponent(topology: &Topology) -> i32 {
    topology.bbox.diagonal().max(1.0).log10().floor() as i32
}

/// Grid spacing for lengths: a millionth of the body's scale (the power of ten at or
/// below its bounding-box diagonal). Areas and volumes use the scale's square and cube.
pub fn quantum(topology: &Topology) -> f64 {
    10f64.powi(scale_exponent(topology) - 6)
}

fn q(v: f64, step: f64) -> i64 {
    let r = (v / step).round();
    // -0 and 0 are the same grid point.
    if r == 0.0 { 0 } else { r as i64 }
}

fn qp(p: Vec3, step: f64) -> [i64; 3] {
    [q(p.x, step), q(p.y, step), q(p.z, step)]
}

fn surface_name(s: SurfaceKind) -> &'static str {
    match s {
        SurfaceKind::Plane => "plane",
        SurfaceKind::Cylinder => "cylinder",
        SurfaceKind::Cone => "cone",
        SurfaceKind::Sphere => "sphere",
        SurfaceKind::Torus => "torus",
        SurfaceKind::Spline => "spline",
        SurfaceKind::Other => "other",
    }
}

/// The text the digest hashes, one fact per line, in a fixed order.
pub fn canonical_text(topology: &Topology, mass: &MassProperties) -> String {
    let e = scale_exponent(topology);
    let lq = 10f64.powi(e - 6);
    let aq = 10f64.powi(2 * e - 6);
    let vq = 10f64.powi(3 * e - 6);
    let mut out = String::new();
    let kind = match topology.kind {
        BodyKind::Solid => "solid",
        BodyKind::Sheet => "sheet",
    };
    let _ = writeln!(out, "soliddraft-digest/1 quantum {lq:e}");
    let _ = writeln!(out, "kind {kind}");
    let _ = writeln!(
        out,
        "counts faces {} edges {} vertices {}",
        topology.faces.len(),
        topology.edges.len(),
        topology.vertex_count
    );
    let _ = writeln!(out, "volume {}", q(mass.volume, vq));
    let _ = writeln!(out, "area {}", q(mass.area, aq));
    let _ = writeln!(out, "bbox {:?} {:?}", qp(topology.bbox.min, lq), qp(topology.bbox.max, lq));
    let mut faces: Vec<String> = topology
        .faces
        .iter()
        .map(|f| format!("face {} {} {:?}", surface_name(f.surface), q(f.area, aq), qp(f.centroid, lq)))
        .collect();
    faces.sort();
    let mut edges: Vec<String> =
        topology.edges.iter().map(|e| format!("edge {} {:?}", q(e.length, lq), qp(e.midpoint, lq))).collect();
    edges.sort();
    for line in faces.into_iter().chain(edges) {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// The digest: the first 16 bytes of the SHA-256 of [`canonical_text`], in hex.
pub fn digest(topology: &Topology, mass: &MassProperties) -> String {
    let hash = Sha256::digest(canonical_text(topology, mass).as_bytes());
    hash[..16].iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use soliddraft_kernel_api::{Aabb, CurveKind, EdgeId, EdgeInfo, FaceId, FaceInfo};

    fn cube(offset: f64, reverse: bool) -> Topology {
        let bbox = Aabb { min: Vec3::new(0.0, 0.0, 0.0), max: Vec3::new(10.0, 10.0, 10.0) };
        let mut faces: Vec<FaceInfo> = (0..6)
            .map(|i| FaceInfo {
                id: FaceId::from_raw(i + 100),
                surface: SurfaceKind::Plane,
                area: 100.0 + offset,
                centroid: Vec3::new(i as f64, 5.0, 5.0),
                bbox,
                edges: vec![],
                normal: None,
            })
            .collect();
        if reverse {
            faces.reverse();
        }
        let edges = (0..12)
            .map(|i| EdgeInfo {
                id: EdgeId::from_raw(i),
                curve: CurveKind::Line,
                length: 10.0,
                midpoint: Vec3::new(i as f64, 0.0, -offset),
                bbox,
                faces: vec![],
                vertices: vec![],
            })
            .collect();
        Topology {
            display_chord_height: None,
            kind: BodyKind::Solid,
            faces,
            edges,
            vertex_count: 8,
            vertices: vec![],
            bbox,
        }
    }

    fn mass() -> MassProperties {
        MassProperties { volume: 1000.0, area: 600.0, centroid: Vec3::new(5.0, 5.0, 5.0), inertia: Some([[0.0; 3]; 3]) }
    }

    #[test]
    fn ignores_ids_order_and_round_off() {
        let a = digest(&cube(0.0, false), &mass());
        assert_eq!(a, digest(&cube(1e-12, true), &mass()));
        assert_eq!(a.len(), 32);
    }

    #[test]
    fn sees_real_changes() {
        let a = digest(&cube(0.0, false), &mass());
        assert_ne!(a, digest(&cube(0.01, false), &mass()));
        let mut m = mass();
        m.volume += 1.0;
        assert_ne!(a, digest(&cube(0.0, false), &m));
    }

    #[test]
    fn quantum_follows_size() {
        assert_eq!(quantum(&cube(0.0, false)), 1e-5);
    }
}
