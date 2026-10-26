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

use crate::drawing::{Curve3, CurveSource, ViewCurve, ViewCurves, ViewFrame, ViewSpec};
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
