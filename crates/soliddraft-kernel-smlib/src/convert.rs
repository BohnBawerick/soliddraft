//! Conversions between kernel-api values and bridge values.

use soliddraft_kernel_api::{
    Aabb, BodyKind, CurveKind, EdgeId, EdgeInfo, EdgePolyline, FaceId, FaceInfo, KernelError, MassProperties, Mesh,
    Plane, Point2, Profile, Segment, SurfaceKind, TessellationParams, Topology, Vec3, VertexId, VertexInfo,
};

use crate::ffi::{CurveIn, EdgeSamples, Frame, MassOut, MeshOut, TessIn, V3};
use crate::history::Shot;

pub(crate) fn v3(v: Vec3) -> V3 {
    V3 { x: v.x, y: v.y, z: v.z }
}

pub(crate) fn vec3(v: V3) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

pub(crate) fn frame(plane: &Plane) -> Frame {
    Frame { origin: v3(plane.origin), x_dir: v3(plane.x_dir), y_dir: v3(plane.y_dir()) }
}

/// A world vector in the plane's local frame.
pub(crate) fn to_local_dir(plane: &Plane, v: Vec3) -> Vec3 {
    Vec3::new(v.dot(plane.x_dir), v.dot(plane.y_dir()), v.dot(plane.normal))
}

/// A world point in the plane's local frame.
pub(crate) fn to_local_point(plane: &Plane, p: Vec3) -> Vec3 {
    to_local_dir(plane, p - plane.origin)
}

fn local(p: Point2) -> V3 {
    V3 { x: p.u, y: p.v, z: 0.0 }
}

fn angle_deg(center: Point2, p: Point2) -> f64 {
    (p.v - center.v).atan2(p.u - center.u).to_degrees()
}

/// The profile's loops as bridge curves in the plane's local XY coordinates, plus the
/// number of curves in each loop. The profile must already have passed `check`.
pub(crate) fn profile_curves(profile: &Profile) -> (Vec<CurveIn>, Vec<u32>) {
    let mut curves = Vec::new();
    let mut sizes = Vec::new();
    for lp in profile.loops() {
        sizes.push(lp.0.len() as u32);
        for seg in &lp.0 {
            let zero = V3::default();
            curves.push(match *seg {
                Segment::Line { from, to } => CurveIn {
                    kind: 0,
                    a: local(from),
                    b: local(to),
                    center: zero,
                    radius: 0.0,
                    start_deg: 0.0,
                    end_deg: 0.0,
                },
                Segment::Arc { center, from, to, clockwise } => {
                    // SMLib arcs run counter-clockwise, so a clockwise arc is built from its
                    // end to its start.
                    let (ccw_from, ccw_to) = if clockwise { (to, from) } else { (from, to) };
                    let start = angle_deg(center, ccw_from);
                    let mut end = angle_deg(center, ccw_to);
                    while end <= start {
                        end += 360.0;
                    }
                    let radius = ((from.u - center.u).powi(2) + (from.v - center.v).powi(2)).sqrt();
                    CurveIn {
                        kind: 1,
                        a: local(from),
                        b: local(to),
                        center: local(center),
                        radius,
                        start_deg: start,
                        end_deg: end,
                    }
                }
                Segment::Circle { center, radius } => {
                    CurveIn { kind: 2, a: zero, b: zero, center: local(center), radius, start_deg: 0.0, end_deg: 360.0 }
                }
            });
        }
    }
    (curves, sizes)
}

fn surface_kind(code: u8) -> SurfaceKind {
    match code {
        0 => SurfaceKind::Plane,
        1 => SurfaceKind::Cylinder,
        2 => SurfaceKind::Cone,
        3 => SurfaceKind::Sphere,
        4 => SurfaceKind::Torus,
        5 => SurfaceKind::Spline,
        _ => SurfaceKind::Other,
    }
}

fn curve_kind(code: u8) -> CurveKind {
    match code {
        0 => CurveKind::Line,
        1 => CurveKind::Circle,
        2 => CurveKind::Ellipse,
        3 => CurveKind::Spline,
        _ => CurveKind::Other,
    }
}

/// The normal stored on a planar face, or `None` for a curved face (stored as zero).
pub(crate) fn normal(n: V3) -> Option<Vec3> {
    let n = vec3(n);
    (n.length() > 0.5).then_some(n)
}

fn aabb(min: V3, max: V3) -> Aabb {
    Aabb { min: vec3(min), max: vec3(max) }
}

pub(crate) fn body_kind(shot: &Shot) -> BodyKind {
    if shot.manifold_solid { BodyKind::Solid } else { BodyKind::Sheet }
}

pub(crate) fn topology(shot: &Shot) -> Topology {
    Topology {
        display_chord_height: None,
        kind: body_kind(shot),
        faces: shot
            .faces
            .iter()
            .map(|f| FaceInfo {
                id: FaceId::from_raw(f.tag),
                surface: surface_kind(f.geom.kind),
                area: f.geom.size,
                centroid: vec3(f.geom.point),
                bbox: aabb(f.geom.bbox_min, f.geom.bbox_max),
                edges: f.adjacent.iter().map(|&t| EdgeId::from_raw(t)).collect(),
                normal: normal(f.normal),
            })
            .collect(),
        edges: shot
            .edges
            .iter()
            .map(|e| EdgeInfo {
                id: EdgeId::from_raw(e.tag),
                curve: curve_kind(e.curve),
                length: e.geom.size,
                midpoint: vec3(e.geom.point),
                bbox: aabb(e.geom.bbox_min, e.geom.bbox_max),
                faces: e.adjacent.iter().map(|&t| FaceId::from_raw(t)).collect(),
                vertices: e.ends.iter().map(|&t| VertexId::from_raw(t)).collect(),
            })
            .collect(),
        vertex_count: shot.vertices.len(),
        vertices: shot
            .vertices
            .iter()
            .map(|v| VertexInfo {
                id: VertexId::from_raw(v.tag),
                position: vec3(v.geom.point),
                edges: v.adjacent.iter().map(|&t| EdgeId::from_raw(t)).collect(),
            })
            .collect(),
        bbox: aabb(shot.bbox_min, shot.bbox_max),
    }
}

pub(crate) fn tess_params(p: &TessellationParams) -> TessIn {
    TessIn {
        chord_height: p.chord_height,
        curve_angle_deg: p.curve_angle_deg,
        surface_angle_deg: p.surface_angle_deg,
        max_edge_length: p.max_edge_length,
        max_aspect_ratio: p.max_aspect_ratio,
    }
}

pub(crate) fn mesh(out: MeshOut) -> Mesh {
    let triples = |v: &[f32]| v.as_chunks::<3>().0.to_vec();
    Mesh {
        unavailable_controls: Vec::new(),
        positions: triples(&out.positions),
        normals: triples(&out.normals),
        indices: out.indices,
        triangle_faces: out.triangle_face_tags.into_iter().map(FaceId::from_raw).collect(),
    }
}

pub(crate) fn polylines(out: EdgeSamples) -> Vec<EdgePolyline> {
    let mut points = out.points.as_chunks::<3>().0.iter().map(|p| Vec3::new(p[0], p[1], p[2]));
    out.tags
        .iter()
        .zip(&out.counts)
        .map(|(&tag, &n)| EdgePolyline {
            edge: EdgeId::from_raw(tag),
            points: points.by_ref().take(n as usize).collect(),
        })
        .collect()
}

pub(crate) fn mass(out: MassOut) -> MassProperties {
    let (m, p) = (out.moments, out.products);
    // SM_API reports products as positive integrals; the tensor's off-diagonals are their negatives.
    MassProperties {
        volume: out.volume,
        area: out.area,
        centroid: vec3(out.centroid),
        inertia: Some([[m.x, -p.z, -p.y], [-p.z, m.y, -p.x], [-p.y, -p.x, m.z]]),
    }
}

/// Turns a C++ exception from the bridge, "<status>|<message>", into a status and text.
pub(crate) fn exception(e: &cxx::Exception) -> (i64, String) {
    let what = e.what();
    match what.split_once('|') {
        Some((code, msg)) => (code.trim().parse().unwrap_or(-1), msg.to_string()),
        None => (-1, what.to_string()),
    }
}

pub(crate) fn invalid(msg: impl Into<String>) -> KernelError {
    KernelError::InvalidInput(msg.into())
}
