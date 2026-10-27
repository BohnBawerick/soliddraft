//! Section views: a body cut by a plane and seen from the side of the cut.
//!
//! [`by_half_space`] makes one from operations every backend has: it subtracts a box
//! that fills the half-space between the cutting plane and the eye, takes the cut
//! body's hidden-line view, and reads the regions to hatch from the faces the cut left
//! on the plane. Every entity id in the result is mapped back to the uncut body through
//! the subtraction's history, so a drawing names section curves with the same
//! references as the body's other views.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::drawing::{Curve3, CurveSource, ViewCurve, ViewCurves, ViewFrame, ViewSpec, mesh_hlr};
use crate::{
    BodyId, BooleanOp, EdgeId, Extrude, FaceId, Kernel, KernelError, KernelResult, OpTag, Plane, Point2, Profile,
    Topology, Vec3,
};

/// What a section view asks for.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SectionSpec {
    /// How the cut body is seen. The cutting plane is perpendicular to
    /// `view.direction`.
    pub view: ViewSpec,
    /// A point on the cutting plane. The material between the plane and the eye is
    /// removed.
    pub origin: Vec3,
}

/// A region where the cutting plane meets material: what a section view hatches.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CutRegion {
    /// The outer boundary: curves on the plane in loop order, each tagged with the face
    /// of the uncut body it cuts.
    pub outer: Vec<ViewCurve>,
    /// The boundaries of holes in the region, tagged the same way.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holes: Vec<Vec<ViewCurve>>,
}

impl CutRegion {
    /// The region's area in the plane: the outer boundary's less its holes'.
    pub fn area(&self, frame: &ViewFrame) -> f64 {
        let ring = |curves: &[ViewCurve]| {
            let points: Vec<Point2> =
                curves.iter().flat_map(|c| c.curve.sample(0.0)).map(|p| frame.project(p)).collect();
            shoelace(&points).abs()
        };
        ring(&self.outer) - self.holes.iter().map(|h| ring(h)).sum::<f64>()
    }
}

/// A section view: the cut body's line work and the regions to hatch.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SectionView {
    /// The cut body seen from the view, visible and hidden. Curves that lie on the
    /// cutting plane are tagged [`CurveSource::Section`] with the face of the uncut body
    /// they cut; the others carry the uncut body's edge and face ids.
    pub curves: ViewCurves,
    /// Where the plane meets material, one region per face the cut leaves on the plane.
    pub regions: Vec<CutRegion>,
}

impl SectionView {
    /// The region areas, largest first, for golden tests.
    pub fn region_areas(&self, frame: &ViewFrame) -> Vec<f64> {
        let mut areas: Vec<f64> = self.regions.iter().map(|r| r.area(frame)).collect();
        areas.sort_by(|a, b| b.total_cmp(a));
        areas
    }
}

fn shoelace(points: &[Point2]) -> f64 {
    let n = points.len();
    (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a.u * b.v - b.u * a.v
        })
        .sum::<f64>()
        / 2.0
}

/// A section view made with a boolean subtraction: the default every backend gets.
pub fn by_half_space<K: Kernel + ?Sized>(k: &K, body: BodyId, spec: &SectionSpec) -> KernelResult<SectionView> {
    let frame = spec
        .view
        .frame()
        .ok_or_else(|| KernelError::InvalidInput("view direction is zero or parallel to up".into()))?;
    if !spec.origin.is_finite() {
        return Err(KernelError::InvalidInput("the section origin is not a finite point".into()));
    }
    let n = frame.direction;
    let original = k.topology(body)?;
    if original.kind != crate::BodyKind::Solid {
        return Err(KernelError::Unsupported("section_view of a sheet body".into()));
    }
    let bbox = original.bbox;
    let corners: Vec<Vec3> = (0..8)
        .map(|i| {
            Vec3::new(
                if i & 1 == 0 { bbox.min.x } else { bbox.max.x },
                if i & 2 == 0 { bbox.min.y } else { bbox.max.y },
                if i & 4 == 0 { bbox.min.z } else { bbox.max.z },
            )
        })
        .collect();
    let depths: Vec<f64> = corners.iter().map(|p| p.dot(n)).collect();
    let (near, far) = (
        depths.iter().copied().fold(f64::INFINITY, f64::min),
        depths.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    );
    let at = spec.origin.dot(n);
    let size = bbox.diagonal().max(1.0);
    if at >= far - 1e-9 * size {
        // The plane is in front of the body: nothing is cut away.
        let curves = k.view_curves(body, &spec.view)?;
        return Ok(SectionView { curves, regions: Vec::new() });
    }
    if at <= near + 1e-9 * size {
        return Err(KernelError::InvalidInput("the cutting plane is behind the whole body".into()));
    }

    // A box on the plane, wider than the body and reaching past its front.
    let centre = bbox.center();
    let plane_origin = centre - n * (centre.dot(n) - at);
    let plane = Plane { origin: plane_origin, normal: n, x_dir: frame.right };
    let width = 2.0 * size + 2.0;
    let sketch = k.create_body_from_profile(
        &OpTag::new("section_cut_sketch"),
        &Profile::rectangle(plane, Point2::new(0.0, 0.0), width, width),
    )?;
    let tool = k.extrude(
        &OpTag::new("section_cut_tool"),
        sketch.body,
        &Extrude { distance: far - at + size + 1.0, direction: None },
    );
    let _ = k.release(sketch.body);
    let tool = tool?;
    let cut = k.boolean(&OpTag::new("section_cut"), BooleanOp::Subtract, body, tool.body);
    let _ = k.release(tool.body);
    let cut = cut?;
    let result = section_of_cut(k, &original, cut.body, &cut.record, spec, &frame, size);
    let _ = k.release(cut.body);
    result
}

/// The view and regions of the cut body `cut`, its ids mapped back to `original`.
fn section_of_cut<K: Kernel + ?Sized>(
    k: &K,
    original: &Topology,
    cut: BodyId,
    record: &crate::HistoryRecord,
    spec: &SectionSpec,
    frame: &ViewFrame,
    size: f64,
) -> KernelResult<SectionView> {
    let n = frame.direction;
    let tol = 1e-6 * size;
    let on_plane = |p: Vec3| (p - spec.origin).dot(n).abs() <= tol;
    let topology = k.topology(cut)?;
    let faces: HashSet<FaceId> = original.faces.iter().map(|f| f.id).collect();
    let edges: HashSet<EdgeId> = original.edges.iter().map(|e| e.id).collect();
    let face_parent: HashMap<FaceId, FaceId> =
        record.faces.split.iter().flat_map(|s| s.into.iter().map(move |i| (*i, s.from))).collect();
    let edge_parent: HashMap<EdgeId, EdgeId> =
        record.edges.split.iter().flat_map(|s| s.into.iter().map(move |i| (*i, s.from))).collect();
    let map_face = |f: FaceId| {
        let mut id = f;
        for _ in 0..8 {
            if faces.contains(&id) {
                return Some(id);
            }
            id = *face_parent.get(&id)?;
        }
        None
    };
    let map_edge = |e: EdgeId| {
        let mut id = e;
        for _ in 0..8 {
            if edges.contains(&id) {
                return Some(id);
            }
            id = *edge_parent.get(&id)?;
        }
        None
    };
    // The faces the cut left on the plane: planar, facing the eye, lying on the plane.
    let caps: Vec<FaceId> = topology
        .faces
        .iter()
        .filter(|f| f.normal.is_some_and(|m| m.dot(n) > 1.0 - 1e-9) && on_plane(f.centroid))
        .map(|f| f.id)
        .collect();
    // A cap edge's other face is the face of the body the plane cut.
    let cut_face = |e: EdgeId| {
        let info = topology.edge(e)?;
        let other = info.faces.iter().find(|f| !caps.contains(f))?;
        map_face(*other)
    };
    let on_cap = |e: EdgeId| topology.edge(e).is_some_and(|i| i.faces.iter().any(|f| caps.contains(f)));
    let section = |e: Option<FaceId>| e.map_or(CurveSource::Unknown, CurveSource::Section);

    let view = k.view_curves(cut, &spec.view)?;
    let unproved = view.unproved;
    let curves = view
        .curves
        .into_iter()
        .map(|mut c| {
            c.source = match c.source {
                CurveSource::Edge(e) if on_cap(e) => section(cut_face(e)),
                CurveSource::Edge(e) => map_edge(e).map_or(CurveSource::Unknown, CurveSource::Edge),
                CurveSource::Silhouette(f) => map_face(f).map_or(CurveSource::Unknown, CurveSource::Silhouette),
                CurveSource::Seam(f) => map_face(f).map_or(CurveSource::Unknown, CurveSource::Seam),
                CurveSource::Section(f) => section(map_face(f)),
                CurveSource::Unknown => CurveSource::Unknown,
            };
            c
        })
        .collect();
    let curves = ViewCurves { unproved, ..ViewCurves::new(curves, frame) };

    // Each cap face's loops, each curve tagged through the cap edge it runs along.
    let edge_by_mid: Vec<(EdgeId, Vec3)> =
        topology.edges.iter().filter(|e| on_cap(e.id)).map(|e| (e.id, e.midpoint)).collect();
    let tag = |c: Curve3| {
        let mid = c.point_at(0.5);
        let edge = edge_by_mid
            .iter()
            .map(|(id, m)| (*id, m.distance(mid)))
            .filter(|(_, d)| *d <= 1e-4 * size)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id);
        ViewCurve::new(c, section(edge.and_then(cut_face)), true)
    };
    let mut regions = Vec::new();
    for &cap in &caps {
        let mut loops = k.face_loops(cut, cap)?.into_iter();
        let Some(outer) = loops.next() else { continue };
        regions.push(CutRegion {
            outer: outer.into_iter().map(tag).collect(),
            holes: loops.map(|l| l.into_iter().map(tag).collect()).collect(),
        });
    }
    Ok(SectionView { curves, regions })
}

/// A section view cut from the body's mesh, for the screen: polygon mode throughout.
///
/// Triangles in front of the plane are dropped and those the plane crosses are clipped
/// to it. The crossings chain into the regions to hatch, each stretch tagged with the
/// face it cuts. The clipped edge polylines and the cut mesh's silhouettes go through
/// the hidden-line pass, with the regions hiding what lies behind them. No boolean runs
/// and every entity keeps the uncut body's id. Everything is approximate at the mesh's
/// chord tolerance, and the result is unproved when the mesh is, so an export draws its
/// sections with [`by_half_space`] instead.
pub fn by_mesh_slice(kernel: &dyn Kernel, body: BodyId, spec: &SectionSpec) -> KernelResult<SectionView> {
    let frame = spec
        .view
        .frame()
        .ok_or_else(|| KernelError::InvalidInput("view direction is zero or parallel to up".into()))?;
    if !spec.origin.is_finite() {
        return Err(KernelError::InvalidInput("the section origin is not a finite point".into()));
    }
    let topology = kernel.topology(body)?;
    if topology.kind != crate::BodyKind::Solid {
        return Err(KernelError::Unsupported("section_view of a sheet body".into()));
    }
    let n = frame.direction;
    let at = spec.origin.dot(n);
    let size = topology.bbox.diagonal().max(1.0);
    let bbox = topology.bbox;
    let (mut near, mut far) = (f64::INFINITY, f64::NEG_INFINITY);
    for i in 0..8 {
        let corner = Vec3::new(
            if i & 1 == 0 { bbox.min.x } else { bbox.max.x },
            if i & 2 == 0 { bbox.min.y } else { bbox.max.y },
            if i & 4 == 0 { bbox.min.z } else { bbox.max.z },
        );
        near = near.min(corner.dot(n));
        far = far.max(corner.dot(n));
    }
    if at >= far - 1e-9 * size {
        // The plane is in front of the body: nothing is cut away.
        let curves = kernel.view_curves(body, &spec.view)?;
        return Ok(SectionView { curves, regions: Vec::new() });
    }
    if at <= near + 1e-9 * size {
        return Err(KernelError::InvalidInput("the cutting plane is behind the whole body".into()));
    }
    let chord = if spec.view.tolerance > 0.0 { spec.view.tolerance } else { 2e-5 * size };
    let params = crate::TessellationParams {
        chord_height: chord,
        curve_angle_deg: 2.0,
        surface_angle_deg: 2.0,
        ..Default::default()
    };
    let (mesh, polylines) = if spec.view.preview {
        (kernel.preview(body, &params)?, kernel.preview_edges(body, &params)?)
    } else {
        (kernel.tessellate(body, &params)?, kernel.tessellate_edges(body, &params)?)
    };
    // Signed distance past the plane towards the eye: positive is cut away.
    let eps = 1e-6 * size;
    let past = |p: Vec3| p.dot(n) - at;

    let (kept, crossings) = clip_mesh(&mesh, &past, eps);
    let (regions, windows) = regions_of(&crossings, &frame, size);

    let mut lines = Vec::new();
    for edge in &topology.edges {
        let mut faces = edge.faces.clone();
        faces.dedup();
        let source = match faces.as_slice() {
            [face] => {
                if !spec.view.seams {
                    continue;
                }
                CurveSource::Seam(*face)
            }
            _ => CurveSource::Edge(edge.id),
        };
        let points = &polylines
            .iter()
            .find(|p| p.edge == edge.id)
            .ok_or_else(|| KernelError::QueryFailed {
                query: "section_view".into(),
                code: 0,
                message: format!("missing polyline for {}", edge.id),
            })?
            .points;
        for run in clip_polyline(points, &past, eps) {
            lines.push(mesh_hlr::SourcedPolyline { source, points: run });
        }
    }
    lines.extend(mesh_hlr::silhouettes(&kept, &frame));
    // The cut faces are on the plane and face the eye: they hide everything behind them
    // by more than the mesh's own depth tolerance.
    let depth_tol = (mesh_hlr::DEPTH_TOL_CHORDS * chord).max(eps);
    let cap_hides = |p: Vec3| past(p) < -depth_tol && windows.iter().any(|w| w.contains(frame.project(p)));
    let view = mesh_hlr::hidden_lines_with(&kept, &lines, &frame, chord, &cap_hides);
    let mut curves = view.curves;
    // The region boundaries are on the plane, in front of everything kept: visible.
    for region in &regions {
        curves.extend(region.outer.iter().cloned());
        curves.extend(region.holes.iter().flatten().cloned());
    }
    let curves = ViewCurves { unproved: mesh.unproved, ..ViewCurves::new(curves, &frame) };
    Ok(SectionView { curves, regions })
}

/// One triangle's crossing of the cutting plane.
struct Crossing {
    face: FaceId,
    a: Vec3,
    b: Vec3,
}

/// The part of `mesh` behind the plane (`past` not positive), with the triangles the
/// plane crosses clipped to it, and each crossing. A vertex the plane crosses on a mesh
/// edge is made once, so neighbouring triangles share it and the crossings join up.
fn clip_mesh(mesh: &crate::Mesh, past: &dyn Fn(Vec3) -> f64, eps: f64) -> (crate::Mesh, Vec<Crossing>) {
    let has_normals = mesh.normals.len() == mesh.positions.len();
    let mut out = crate::Mesh {
        positions: mesh.positions.clone(),
        normals: mesh.normals.clone(),
        indices: Vec::new(),
        triangle_faces: Vec::new(),
        unavailable_controls: mesh.unavailable_controls.clone(),
        unproved: mesh.unproved,
    };
    let mut cut_vertex: HashMap<(u32, u32), u32> = HashMap::new();
    let mut crossings = Vec::new();
    for t in 0..mesh.triangle_count() {
        let idx = [mesh.indices[3 * t], mesh.indices[3 * t + 1], mesh.indices[3 * t + 2]];
        let p = idx.map(|i| mesh_hlr::vec3(mesh.positions[i as usize]));
        let d = p.map(past);
        let inside = d.map(|x| x <= eps);
        let face = mesh.triangle_faces[t];
        match inside.iter().filter(|k| **k).count() {
            3 => {
                out.indices.extend(idx);
                out.triangle_faces.push(face);
            }
            0 => {}
            _ => {
                let mut polygon: Vec<u32> = Vec::new();
                let mut cut: Vec<Vec3> = Vec::new();
                for k in 0..3 {
                    let (a, b) = (k, (k + 1) % 3);
                    if inside[a] {
                        polygon.push(idx[a]);
                    }
                    if inside[a] != inside[b] {
                        let key = (idx[a].min(idx[b]), idx[a].max(idx[b]));
                        let v = *cut_vertex.entry(key).or_insert_with(|| {
                            let s = (d[a] / (d[a] - d[b])).clamp(0.0, 1.0);
                            let q = p[a] + (p[b] - p[a]) * s;
                            out.positions.push([q.x as f32, q.y as f32, q.z as f32]);
                            if has_normals {
                                out.normals.push(mesh.normals[idx[a] as usize]);
                            }
                            (out.positions.len() - 1) as u32
                        });
                        polygon.push(v);
                        cut.push(mesh_hlr::vec3(out.positions[v as usize]));
                    }
                }
                for i in 1..polygon.len().saturating_sub(1) {
                    out.indices.extend([polygon[0], polygon[i], polygon[i + 1]]);
                    out.triangle_faces.push(face);
                }
                if let [a, b] = cut[..]
                    && a.distance(b) > 0.0
                {
                    crossings.push(Crossing { face, a, b });
                }
            }
        }
    }
    (out, crossings)
}

/// A polyline's stretches behind the plane, each ending where it crosses it.
fn clip_polyline(points: &[Vec3], past: &dyn Fn(Vec3) -> f64, eps: f64) -> Vec<Vec<Vec3>> {
    let keep = |d: f64| d <= eps;
    let mut runs = Vec::new();
    let mut run: Vec<Vec3> = Vec::new();
    let mut previous: Option<(Vec3, f64)> = None;
    for &p in points {
        let d = past(p);
        match previous {
            None if keep(d) => run.push(p),
            Some((_, dq)) if keep(dq) && keep(d) => run.push(p),
            Some((q, dq)) if keep(dq) != keep(d) => {
                let s = (dq / (dq - d)).clamp(0.0, 1.0);
                let x = q + (p - q) * s;
                if keep(dq) {
                    run.push(x);
                    runs.push(std::mem::take(&mut run));
                } else {
                    run.push(x);
                    run.push(p);
                }
            }
            _ => {}
        }
        previous = Some((p, d));
    }
    runs.push(run);
    runs.into_iter().filter(|r| r.len() >= 2).collect()
}

/// A region's boundaries in the view, for hiding what lies behind the cut face.
struct Window {
    outer: Vec<Point2>,
    holes: Vec<Vec<Point2>>,
}

impl Window {
    fn contains(&self, p: Point2) -> bool {
        inside(&self.outer, p) && !self.holes.iter().any(|h| inside(h, p))
    }
}

/// Even-odd point in polygon.
fn inside(polygon: &[Point2], p: Point2) -> bool {
    let mut c = false;
    let n = polygon.len();
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        let (a, b) = (polygon[i], polygon[j]);
        if (a.v > p.v) != (b.v > p.v) {
            let x = a.u + (p.v - a.v) * (b.u - a.u) / (b.v - a.v);
            if p.u < x {
                c = !c;
            }
        }
        j = i;
    }
    c
}

/// Chains the crossings into closed loops, nests them into regions with their holes,
/// and tags each stretch with the face it cuts. An open chain, from a mesh with a crack,
/// hatches nothing.
fn regions_of(crossings: &[Crossing], frame: &ViewFrame, size: f64) -> (Vec<CutRegion>, Vec<Window>) {
    let quantum = 1e-5 * size;
    let mut ids: HashMap<(i64, i64), u32> = HashMap::new();
    let mut points: Vec<(Vec3, Point2)> = Vec::new();
    let mut id_of = |p: Vec3| -> u32 {
        let q = frame.project(p);
        let key = ((q.u / quantum).round() as i64, (q.v / quantum).round() as i64);
        *ids.entry(key).or_insert_with(|| {
            points.push((p, q));
            (points.len() - 1) as u32
        })
    };
    let mut segments: Vec<(u32, u32)> = Vec::new();
    let mut face_of: HashMap<(u32, u32), FaceId> = HashMap::new();
    for c in crossings {
        let (a, b) = (id_of(c.a), id_of(c.b));
        if a != b {
            segments.push((a, b));
            face_of.entry((a.min(b), a.max(b))).or_insert(c.face);
        }
    }
    segments.sort_unstable();
    segments.dedup();
    let loops: Vec<Vec<u32>> = mesh_hlr::chains(segments)
        .into_iter()
        .filter(|chain| chain.len() >= 4 && chain.first() == chain.last())
        .collect();
    let flat: Vec<Vec<Point2>> =
        loops.iter().map(|l| l[..l.len() - 1].iter().map(|&i| points[i as usize].1).collect()).collect();
    let area = |ring: &[Point2]| shoelace(ring).abs();
    // A loop inside an odd number of others is a hole of the innermost one around it.
    let mut outers: Vec<usize> = Vec::new();
    let mut holes: Vec<(usize, usize)> = Vec::new();
    for i in 0..loops.len() {
        let around: Vec<usize> = (0..loops.len()).filter(|&j| j != i && inside(&flat[j], flat[i][0])).collect();
        if around.len().is_multiple_of(2) {
            outers.push(i);
        } else if let Some(&parent) = around.iter().min_by(|&&a, &&b| area(&flat[a]).total_cmp(&area(&flat[b]))) {
            holes.push((parent, i));
        }
    }
    let curves = |l: &[u32]| -> Vec<ViewCurve> {
        let mut out: Vec<ViewCurve> = Vec::new();
        let mut run: Vec<Vec3> = vec![points[l[0] as usize].0];
        let mut current: Option<FaceId> = None;
        for w in l.windows(2) {
            let face = face_of[&(w[0].min(w[1]), w[0].max(w[1]))];
            if current.is_some_and(|f| f != face) {
                let done = std::mem::replace(&mut run, vec![points[w[0] as usize].0]);
                out.push(ViewCurve::new(
                    Curve3::Polyline { points: done },
                    CurveSource::Section(current.expect("set")),
                    true,
                ));
            }
            current = Some(face);
            run.push(points[w[1] as usize].0);
        }
        if let Some(face) = current {
            out.push(ViewCurve::new(Curve3::Polyline { points: run }, CurveSource::Section(face), true));
        }
        out
    };
    let mut regions = Vec::new();
    let mut windows = Vec::new();
    for &o in &outers {
        let mine: Vec<usize> = holes.iter().filter(|(p, _)| *p == o).map(|(_, h)| *h).collect();
        regions.push(CutRegion { outer: curves(&loops[o]), holes: mine.iter().map(|&h| curves(&loops[h])).collect() });
        windows.push(Window { outer: flat[o].clone(), holes: mine.iter().map(|&h| flat[h].clone()).collect() });
    }
    (regions, windows)
}
