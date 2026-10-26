//! Hidden-line removal on a triangle mesh.
//!
//! The approximate path, after TechDraw's polygon mode: project the triangles, then walk
//! each edge polyline and mark a stretch hidden when a triangle covers it in the view and
//! lies in front of it. Silhouettes of curved faces come from the mesh, where a face's
//! triangles turn from facing the eye to facing away. Everything comes out as polylines.
//!
//! It works on any backend that can [`tessellate`](crate::Kernel::tessellate) and supply
//! [`tessellate_edges`](crate::Kernel::tessellate_edges), so it is the fallback for a backend
//! without exact hidden-line removal and the yardstick the exact backends are compared
//! with. It always draws smooth (tangent) edges.

use std::collections::HashMap;

use crate::drawing::{Curve3, CurveSource, ViewCurve, ViewCurves, ViewFrame, ViewSpec};
use crate::{BodyId, BodyKind, FaceId, Kernel, KernelError, KernelResult, Mesh, Point2, TessellationParams, Vec3};

/// A polyline on the body with the entity it comes from: an edge's sampled curve, for
/// example.
#[derive(Clone, Debug, PartialEq)]
pub struct SourcedPolyline {
    /// What the points come from.
    pub source: CurveSource,
    /// The points, in order.
    pub points: Vec<Vec3>,
}

/// Bisection steps used to place each change between visible and hidden.
const REFINE_STEPS: u32 = 16;

/// How far in front of a point, in multiples of the mesh's chord height, a triangle must
/// be to hide it. Near a silhouette a ray from a point a chord height inside the faceted
/// surface runs a long way before it leaves, so a few chord heights would mark visible
/// points hidden there. Real hiding depth grows fast away from a silhouette, so the
/// margin moves the change between visible and hidden very little.
const DEPTH_TOL_CHORDS: f64 = 30.0;

/// Approximate view curves of `body`, from a fine tessellation and edge polylines.
/// Sheet bodies are refused, as on the exact backends.
pub fn view_curves(kernel: &dyn Kernel, body: BodyId, spec: &ViewSpec) -> KernelResult<ViewCurves> {
    let frame =
        spec.frame().ok_or_else(|| KernelError::InvalidInput("view direction is zero or parallel to up".into()))?;
    let topology = kernel.topology(body)?;
    if topology.kind == BodyKind::Sheet {
        return Err(KernelError::Unsupported("view_curves of a sheet body".into()));
    }
    let size = topology.bbox.diagonal().max(1.0);
    let chord = if spec.tolerance > 0.0 { spec.tolerance } else { 2e-5 * size };
    let params =
        TessellationParams { chord_height: chord, curve_angle_deg: 2.0, surface_angle_deg: 2.0, ..Default::default() };
    // A view for the screen may come from the preview mesh; an exported one never does.
    let (mesh, polylines) = if spec.preview {
        (kernel.preview(body, &params)?, kernel.preview_edges(body, &params)?)
    } else {
        (kernel.tessellate(body, &params)?, kernel.tessellate_edges(body, &params)?)
    };
    let mut lines = Vec::new();
    for edge in &topology.edges {
        let mut faces = edge.faces.clone();
        faces.dedup();
        let source = match faces.as_slice() {
            [face] => {
                if !spec.seams {
                    continue;
                }
                CurveSource::Seam(*face)
            }
            _ => CurveSource::Edge(edge.id),
        };
        let points = polylines
            .iter()
            .find(|p| p.edge == edge.id)
            .ok_or_else(|| KernelError::QueryFailed {
                query: "mesh_hlr".into(),
                code: 0,
                message: format!("missing polyline for {}", edge.id),
            })?
            .points
            .clone();
        lines.push(SourcedPolyline { source, points });
    }
    lines.extend(silhouettes(&mesh, &frame));
    Ok(ViewCurves { unproved: mesh.unproved, ..hidden_lines(&mesh, &lines, &frame, chord) })
}

/// The silhouettes of the mesh's curved faces: chains of triangle edges inside one face
/// where the triangles on the two sides face the eye differently.
pub fn silhouettes(mesh: &Mesh, frame: &ViewFrame) -> Vec<SourcedPolyline> {
    let facing: Vec<f64> = (0..mesh.triangle_count())
        .map(|t| {
            let [a, b, c] = triangle(mesh, t);
            (b - a).cross(c - a).dot(frame.direction)
        })
        .collect();
    // Each interior edge of a face, keyed by its sorted vertex pair, with its triangles.
    let mut sides: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for t in 0..mesh.triangle_count() {
        let idx = &mesh.indices[3 * t..3 * t + 3];
        for k in 0..3 {
            let (a, b) = (idx[k], idx[(k + 1) % 3]);
            sides.entry((a.min(b), a.max(b))).or_default().push(t);
        }
    }
    let mut segments: HashMap<FaceId, Vec<(u32, u32)>> = HashMap::new();
    for (&(a, b), tris) in &sides {
        if let [t0, t1] = tris[..] {
            let face = mesh.triangle_faces[t0];
            if face == mesh.triangle_faces[t1] && facing[t0] * facing[t1] < 0.0 {
                segments.entry(face).or_default().push((a, b));
            }
        }
    }
    let mut out = Vec::new();
    let mut faces: Vec<FaceId> = segments.keys().copied().collect();
    faces.sort();
    for face in faces {
        let mut segs = segments.remove(&face).unwrap_or_default();
        segs.sort();
        for chain in chains(segs) {
            let points = chain.iter().map(|&i| vec3(mesh.positions[i as usize])).collect();
            out.push(SourcedPolyline { source: CurveSource::Silhouette(face), points });
        }
    }
    out
}

/// Joins segments that share vertices into chains of vertex indices.
fn chains(segs: Vec<(u32, u32)>) -> Vec<Vec<u32>> {
    let mut next: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(a, b) in &segs {
        next.entry(a).or_default().push(b);
        next.entry(b).or_default().push(a);
    }
    let mut used: HashMap<(u32, u32), bool> = segs.iter().map(|&(a, b)| ((a.min(b), a.max(b)), false)).collect();
    let mut take = |a: u32, b: u32| -> bool {
        let key = (a.min(b), a.max(b));
        match used.get_mut(&key) {
            Some(u) if !*u => {
                *u = true;
                true
            }
            _ => false,
        }
    };
    let mut out = Vec::new();
    // Start at chain ends (vertices of degree one) first, then close any loops left.
    let mut starts: Vec<u32> = next.iter().filter(|(_, n)| n.len() == 1).map(|(v, _)| *v).collect();
    starts.sort();
    starts.extend(segs.iter().map(|s| s.0));
    for start in starts {
        let mut chain = vec![start];
        let mut at = start;
        while let Some(&to) = next[&at].iter().find(|&&to| take(at, to)) {
            chain.push(to);
            at = to;
        }
        if chain.len() > 1 {
            out.push(chain);
        }
    }
    out
}

fn vec3(p: [f32; 3]) -> Vec3 {
    Vec3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]))
}

fn triangle(mesh: &Mesh, t: usize) -> [Vec3; 3] {
    let i = &mesh.indices[3 * t..3 * t + 3];
    [vec3(mesh.positions[i[0] as usize]), vec3(mesh.positions[i[1] as usize]), vec3(mesh.positions[i[2] as usize])]
}

/// A projected triangle: 2D corners and depths towards the eye.
struct Flat {
    p: [Point2; 3],
    depth: [f64; 3],
    /// Twice the signed 2D area.
    area2: f64,
}

/// The projected triangles, bucketed on a uniform grid.
struct Occluders {
    tris: Vec<Flat>,
    lo: Point2,
    cell: f64,
    nx: usize,
    ny: usize,
    cells: Vec<Vec<u32>>,
    /// How far in front of a point a triangle must be to hide it.
    depth_tol: f64,
}

impl Occluders {
    fn new(mesh: &Mesh, frame: &ViewFrame, tol: f64) -> Occluders {
        let mut tris = Vec::new();
        let (mut lo, mut hi) =
            (Point2::new(f64::INFINITY, f64::INFINITY), Point2::new(f64::NEG_INFINITY, f64::NEG_INFINITY));
        for t in 0..mesh.triangle_count() {
            let corners = triangle(mesh, t);
            let p = corners.map(|c| frame.project(c));
            let depth = corners.map(|c| frame.depth(c));
            let area2 = (p[1].u - p[0].u) * (p[2].v - p[0].v) - (p[2].u - p[0].u) * (p[1].v - p[0].v);
            for q in &p {
                lo = Point2::new(lo.u.min(q.u), lo.v.min(q.v));
                hi = Point2::new(hi.u.max(q.u), hi.v.max(q.v));
            }
            // Triangles seen edge on cover nothing.
            if area2.abs() > 1e-12 * (1.0 + (hi.u - lo.u).powi(2)) {
                tris.push(Flat { p, depth, area2 });
            }
        }
        let n = ((tris.len() as f64).sqrt().ceil() as usize).clamp(1, 512);
        let span = (hi.u - lo.u).max(hi.v - lo.v).max(1e-9);
        let cell = span / n as f64;
        let (nx, ny) = (n + 1, n + 1);
        let mut cells = vec![Vec::new(); nx * ny];
        for (i, t) in tris.iter().enumerate() {
            let (u0, u1) = (
                t.p.iter().map(|q| q.u).fold(f64::INFINITY, f64::min),
                t.p.iter().map(|q| q.u).fold(f64::NEG_INFINITY, f64::max),
            );
            let (v0, v1) = (
                t.p.iter().map(|q| q.v).fold(f64::INFINITY, f64::min),
                t.p.iter().map(|q| q.v).fold(f64::NEG_INFINITY, f64::max),
            );
            let cx = |u: f64| (((u - lo.u) / cell).floor().max(0.0) as usize).min(nx - 1);
            let cy = |v: f64| (((v - lo.v) / cell).floor().max(0.0) as usize).min(ny - 1);
            for y in cy(v0)..=cy(v1) {
                for x in cx(u0)..=cx(u1) {
                    cells[y * nx + x].push(i as u32);
                }
            }
        }
        let size = span.max(1.0);
        Occluders { tris, lo, cell, nx, ny, cells, depth_tol: (DEPTH_TOL_CHORDS * tol).max(1e-6 * size) }
    }

    /// True when some triangle covers `p` in the view and lies in front of it.
    fn hides(&self, frame: &ViewFrame, p: Vec3) -> bool {
        let q = frame.project(p);
        let d = frame.depth(p);
        let x = ((q.u - self.lo.u) / self.cell).floor();
        let y = ((q.v - self.lo.v) / self.cell).floor();
        if x < 0.0 || y < 0.0 || x as usize >= self.nx || y as usize >= self.ny {
            return false;
        }
        self.cells[y as usize * self.nx + x as usize].iter().any(|&i| {
            let t = &self.tris[i as usize];
            // Barycentric coordinates. A point on the side two triangles share must count
            // as inside one of them, so the test is inclusive.
            let mut w = [0.0; 3];
            for (k, wk) in w.iter_mut().enumerate() {
                let (a, b) = (t.p[(k + 1) % 3], t.p[(k + 2) % 3]);
                *wk = ((b.u - a.u) * (q.v - a.v) - (q.u - a.u) * (b.v - a.v)) / t.area2;
                if *wk < -1e-12 {
                    return false;
                }
            }
            let depth = w[0] * t.depth[0] + w[1] * t.depth[1] + w[2] * t.depth[2];
            depth > d + self.depth_tol
        })
    }
}

/// Splits each polyline into visible and hidden stretches against the mesh. `tol` is the
/// accuracy of the mesh and polylines, in millimetres: covering triangles must be more
/// than a few times that in front of a point to hide it.
pub fn hidden_lines(mesh: &Mesh, lines: &[SourcedPolyline], frame: &ViewFrame, tol: f64) -> ViewCurves {
    let occluders = Occluders::new(mesh, frame, tol);
    let size = occluders.cell * occluders.nx as f64;
    let step = (size / 400.0).max(tol);
    let mut curves = Vec::new();
    let hides = |p: Vec3| occluders.hides(frame, p);
    for line in lines {
        let Some(&first) = line.points.first() else { continue };
        // The current stretch and its visibility, which is that of the last sample.
        let mut run = vec![first];
        let mut visible = !hides(first);
        for w in line.points.windows(2) {
            let (a, b) = (w[0], w[1]);
            let at = |t: f64| a + (b - a) * t;
            let n = ((a.distance(b) / step).ceil() as usize).max(1);
            let mut t_prev = 0.0;
            for k in 1..=n {
                let t = k as f64 / n as f64;
                if !hides(at(t)) != visible {
                    // Place the change by bisection between the two samples.
                    let (mut lo, mut hi) = (t_prev, t);
                    for _ in 0..REFINE_STEPS {
                        let mid = 0.5 * (lo + hi);
                        if !hides(at(mid)) == visible {
                            lo = mid;
                        } else {
                            hi = mid;
                        }
                    }
                    let cut = at(0.5 * (lo + hi));
                    run.push(cut);
                    push_run(&mut curves, std::mem::replace(&mut run, vec![cut]), line.source, visible);
                    visible = !visible;
                }
                t_prev = t;
            }
            run.push(b);
        }
        push_run(&mut curves, run, line.source, visible);
    }
    ViewCurves::new(curves, frame)
}

fn push_run(curves: &mut Vec<ViewCurve>, run: Vec<Vec3>, source: CurveSource, visible: bool) {
    if run.windows(2).any(|w| w[0].distance(w[1]) > 0.0) {
        curves.push(ViewCurve::new(Curve3::Polyline { points: run }, source, visible));
    }
}

/// Visible length per source, summed over `curves`.
pub fn visible_length_by_source(curves: &ViewCurves) -> HashMap<CurveSource, f64> {
    let mut out: HashMap<CurveSource, f64> = HashMap::new();
    for c in &curves.curves {
        if c.visible {
            *out.entry(c.source).or_default() += c.curve.length();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawing::StandardView;
    use crate::{EdgeId, FaceId};

    /// Two unit squares facing +Z: one at z = 1 covering x in [0, 1], one at z = 0
    /// covering x in [0, 2]. Seen from the top, the upper one hides half of a line drawn
    /// across the lower one.
    fn two_squares() -> Mesh {
        let mut mesh = Mesh::default();
        for (z, w, face) in [(1.0f32, 1.0f32, 1u64), (0.0, 2.0, 2)] {
            let base = mesh.positions.len() as u32;
            mesh.positions.extend([[0.0, 0.0, z], [w, 0.0, z], [w, 1.0, z], [0.0, 1.0, z]]);
            mesh.normals.extend([[0.0, 0.0, 1.0]; 4]);
            mesh.indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
            mesh.triangle_faces.extend([FaceId::from_raw(face); 2]);
        }
        mesh
    }

    #[test]
    fn a_covering_triangle_hides_what_is_behind_it() {
        let mesh = two_squares();
        let frame = ViewSpec::standard(StandardView::Top).frame().unwrap();
        let line = SourcedPolyline {
            source: CurveSource::Edge(EdgeId::from_raw(9)),
            points: vec![Vec3::new(0.2, 0.5, -0.0), Vec3::new(1.8, 0.5, 0.0)],
        };
        let out = hidden_lines(&mesh, &[line], &frame, 1e-6);
        assert_eq!(out.counts(), (1, 1), "{out:?}");
        let hidden: f64 = out.curves.iter().filter(|c| !c.visible).map(|c| c.curve.length()).sum();
        let visible: f64 = out.curves.iter().filter(|c| c.visible).map(|c| c.curve.length()).sum();
        assert!((hidden - 0.8).abs() < 1e-3, "hidden {hidden}");
        assert!((visible - 0.8).abs() < 1e-3, "visible {visible}");
        // From below, the lower square hides the whole line, which lies on its far side.
        let below = ViewSpec::standard(StandardView::Bottom).frame().unwrap();
        let line = SourcedPolyline {
            source: CurveSource::Unknown,
            points: vec![Vec3::new(0.2, 0.5, 1.0), Vec3::new(0.8, 0.5, 1.0)],
        };
        let out = hidden_lines(&mesh, &[line], &below, 1e-6);
        assert_eq!(out.counts(), (0, 1));
    }

    #[test]
    fn a_line_on_a_face_is_not_hidden_by_it() {
        let mesh = two_squares();
        let frame = ViewSpec::standard(StandardView::Top).frame().unwrap();
        let on_top = SourcedPolyline {
            source: CurveSource::Unknown,
            points: vec![Vec3::new(0.0, 0.0, 1.0), Vec3::new(1.0, 1.0, 1.0)],
        };
        let out = hidden_lines(&mesh, &[on_top], &frame, 1e-6);
        assert_eq!(out.counts(), (1, 0));
        assert!(matches!(&out.curves[0].curve, Curve3::Polyline { points } if points.len() >= 2));
    }

    #[test]
    fn silhouettes_follow_the_facing_change() {
        // A triangulated half cylinder of radius 1 about Z, z in [0, 1], seen from the top
        // has no silhouette; seen from the front its rim at x = +-1 is one.
        let mut mesh = Mesh::default();
        let n = 12;
        for i in 0..=n {
            let a = std::f32::consts::PI * i as f32 / n as f32;
            mesh.positions.push([a.cos(), -a.sin(), 0.0]);
            mesh.positions.push([a.cos(), -a.sin(), 1.0]);
            mesh.normals.extend([[a.cos(), -a.sin(), 0.0]; 2]);
        }
        // The far half too, so the front view sees the facing flip.
        for i in 1..n {
            let a = std::f32::consts::PI * i as f32 / n as f32;
            mesh.positions.push([-a.cos(), a.sin(), 0.0]);
            mesh.positions.push([-a.cos(), a.sin(), 1.0]);
            mesh.normals.extend([[-a.cos(), a.sin(), 0.0]; 2]);
        }
        let ring: Vec<u32> = (0..=n as u32).chain((n as u32 + 1)..(2 * n as u32)).chain([0]).collect();
        for w in ring.windows(2) {
            let (a, b) = (2 * w[0], 2 * w[1]);
            mesh.indices.extend([a, b, b + 1, a, b + 1, a + 1]);
            mesh.triangle_faces.extend([FaceId::from_raw(1); 2]);
        }
        let front = ViewSpec::standard(StandardView::Front).frame().unwrap();
        let sil = silhouettes(&mesh, &front);
        assert_eq!(sil.len(), 2, "{sil:?}");
        for s in &sil {
            assert!(s.points.iter().all(|p| (p.x.abs() - 1.0).abs() < 1e-6), "{s:?}");
        }
        let top = ViewSpec::standard(StandardView::Top).frame().unwrap();
        assert!(silhouettes(&mesh, &top).is_empty());
    }
}
