//! The drawing queries: hidden-line views, silhouettes, sections, edge curves, face
//! loops and lines-and-arcs conversion, through `cpp/hlr.cpp`.

use soliddraft_kernel_api::drawing::visibility::refine_visibility;
use soliddraft_kernel_api::drawing::{Curve3, CurveSource, ViewCurve, ViewCurves, ViewSpec, deviation, sheet_bounds};
use soliddraft_kernel_api::{BodyId, EdgeId, FaceId, KernelError, KernelResult, Plane, Vec3};

use crate::convert::{invalid, v3};
use crate::ffi::{CurveOut, NurbsIn, ViewIn};
use crate::{SmlibKernel, State, query_failed};

/// The accuracy of the crossing search, as a fraction of the view's size.
const CROSSING_TOL: f64 = 1e-4;

/// SmApiConvertToLinesAndArcs fits to this tolerance, in millimetres, and takes no other.
pub(crate) const LINES_AND_ARCS_TOL: f64 = 1e-3;

fn triples(v: &[f64]) -> Vec<Vec3> {
    v.as_chunks::<3>().0.iter().map(|c| Vec3::new(c[0], c[1], c[2])).collect()
}

/// A curve from the bridge as a [`Curve3`], simplified to a line, arc or circle when it
/// is one. The exact form must reproduce the kernel's own points at the start, middle
/// and end of the curve; a curve that does not is an export bug, reported as an error
/// rather than drawn wrong.
pub(crate) fn curve(out: &CurveOut) -> KernelResult<Curve3> {
    let points = triples(&out.points);
    let exact = match out.kind {
        0 => Curve3::Nurbs { degree: out.degree, knots: out.knots.clone(), points, weights: out.weights.clone() },
        1 if points.len() == 2 => Curve3::Line { a: points[0], b: points[1] },
        2 if points.len() >= 2 => return Ok(Curve3::Polyline { points }),
        _ => return Err(export_error("malformed curve")),
    };
    let check = triples(&out.check);
    let scale = check.iter().map(|p| p.distance(check[0])).fold(1.0, f64::max);
    let tol = 1e-6 * scale;
    let reproduced =
        check.len() == 3 && [0.0, 0.5, 1.0].iter().zip(&check).all(|(t, p)| exact.point_at(*t).distance(*p) <= tol);
    if !reproduced {
        return Err(export_error("a curve's exact form does not reproduce the kernel's points"));
    }
    Ok(exact.simplified(tol))
}

fn export_error(message: &str) -> KernelError {
    KernelError::QueryFailed { query: "curve export".into(), code: -1, message: message.into() }
}

/// `CurveOut::source` of a seam.
const SEAM: u8 = 3;

fn source(out: &CurveOut) -> CurveSource {
    match (out.source, out.tag) {
        (_, 0) => CurveSource::Unknown,
        (1, t) => CurveSource::Edge(EdgeId::from_raw(t)),
        (2, t) => CurveSource::Silhouette(FaceId::from_raw(t)),
        (3, t) => CurveSource::Seam(FaceId::from_raw(t)),
        (4, t) => CurveSource::Section(FaceId::from_raw(t)),
        _ => CurveSource::Unknown,
    }
}

fn view_curve(out: &CurveOut) -> KernelResult<ViewCurve> {
    Ok(ViewCurve { curve: curve(out)?, source: source(out), visible: out.visible, smooth: out.smooth })
}

/// A spline for the bridge, its knots mapped onto [0, 1]: SMLib's lines-and-arcs fit
/// returns a spline on another range unchanged.
fn nurbs_in(degree: u32, knots: &[f64], points: &[Vec3], weights: &[f64]) -> NurbsIn {
    let (lo, hi) = (knots.first().copied().unwrap_or(0.0), knots.last().copied().unwrap_or(1.0));
    let span = if hi > lo { hi - lo } else { 1.0 };
    NurbsIn {
        degree,
        knots: knots.iter().map(|k| (k - lo) / span).collect(),
        points: points.iter().flat_map(|p| [p.x, p.y, p.z]).collect(),
        weights: weights.to_vec(),
    }
}

impl State {
    /// Refuses sheets: SmHCR and the sectioning calls are for closed solids.
    fn drawable(&self, body: BodyId, query: &str) -> KernelResult<()> {
        if self.meta(body)?.snapshot.manifold_solid {
            Ok(())
        } else {
            Err(KernelError::Unsupported(format!("{query} of a sheet body")))
        }
    }
}

pub(crate) fn view_curves(k: &SmlibKernel, body: BodyId, spec: &ViewSpec) -> KernelResult<ViewCurves> {
    let frame = spec.frame().ok_or_else(|| invalid("view direction is zero or parallel to up"))?;
    if !(spec.tolerance.is_finite() && spec.tolerance >= 0.0) {
        return Err(invalid(format!("view tolerance must be zero or positive, got {}", spec.tolerance)));
    }
    let view = ViewIn {
        direction: v3(frame.direction),
        approx_tol: spec.tolerance,
        angle_tol_deg: 0.0,
        seams: spec.seams,
        smooth_edges: spec.smooth_edges,
    };
    k.call(move |s| {
        s.drawable(body, "view_curves")?;
        let raw = s.engine.view_curves(body.raw(), &view).map_err(|e| query_failed("view_curves", &e))?;
        // SmHCR reports seams and smooth edges it is told not to show as hidden curves
        // (unless they are on the outline, where they stay visible). A drawing leaves
        // them out instead.
        let shown = |c: &&CurveOut| c.visible || (view.seams || c.source != SEAM) && (view.smooth_edges || !c.smooth);
        let curves = raw.iter().filter(shown).map(view_curve).collect::<KernelResult<Vec<_>>>()?;
        let size = sheet_bounds(&curves, &frame);
        let tol = CROSSING_TOL * ((size[2] - size[0]).hypot(size[3] - size[1])).max(1.0);
        let curves = refine_visibility(curves, &frame, tol, |points| {
            let flat: Vec<f64> = points.iter().flat_map(|p| [p.x, p.y, p.z]).collect();
            let hidden =
                s.engine.hidden_points(body.raw(), &view, &flat).map_err(|e| query_failed("view_curves", &e))?;
            Ok(hidden.into_iter().map(|h| h != 0).collect())
        })?;
        Ok(ViewCurves::new(curves, &frame))
    })
}

pub(crate) fn silhouette_curves(k: &SmlibKernel, body: BodyId, direction: Vec3) -> KernelResult<Vec<ViewCurve>> {
    let direction = direction.normalized().ok_or_else(|| invalid("silhouette direction is zero"))?;
    k.call(move |s| {
        s.drawable(body, "silhouette_curves")?;
        let raw = s
            .engine
            .silhouette_curves(body.raw(), &v3(direction), 0.0)
            .map_err(|e| query_failed("silhouette_curves", &e))?;
        raw.iter().map(view_curve).collect()
    })
}

pub(crate) fn section_curves(k: &SmlibKernel, body: BodyId, plane: &Plane) -> KernelResult<Vec<ViewCurve>> {
    if !plane.is_well_formed() {
        return Err(invalid("the section plane frame is not orthonormal"));
    }
    let (origin, normal) = (v3(plane.origin), v3(plane.normal));
    k.call(move |s| {
        s.drawable(body, "section_curves")?;
        let raw =
            s.engine.section_curves(body.raw(), &origin, &normal).map_err(|e| query_failed("section_curves", &e))?;
        raw.iter().map(view_curve).collect()
    })
}

pub(crate) fn edge_curve(k: &SmlibKernel, body: BodyId, edge: EdgeId) -> KernelResult<Curve3> {
    k.call(move |s| {
        if !s.meta(body)?.snapshot.edges.iter().any(|e| e.tag == edge.raw()) {
            return Err(KernelError::UnknownEdge { body, edge });
        }
        let out = s.engine.edge_curve(body.raw(), edge.raw()).map_err(|e| query_failed("edge_curve", &e))?;
        curve(&out)
    })
}

pub(crate) fn face_loops(k: &SmlibKernel, body: BodyId, face: FaceId) -> KernelResult<Vec<Vec<Curve3>>> {
    k.call(move |s| {
        if !s.meta(body)?.snapshot.faces.iter().any(|f| f.tag == face.raw()) {
            return Err(KernelError::UnknownFace { body, face });
        }
        let out = s.engine.face_loops(body.raw(), face.raw()).map_err(|e| query_failed("face_loops", &e))?;
        let mut curves = out.curves.iter().zip(&out.reversed).map(|(c, &rev)| {
            let c = curve(c)?;
            Ok(if rev != 0 { c.reversed() } else { c })
        });
        let mut loops = Vec::new();
        for &n in &out.loop_sizes {
            loops.push(curves.by_ref().take(n as usize).collect::<KernelResult<Vec<_>>>()?);
        }
        Ok(loops)
    })
}

pub(crate) fn lines_and_arcs(k: &SmlibKernel, curve: &Curve3, tolerance: f64) -> KernelResult<Vec<Curve3>> {
    if !(tolerance.is_finite() && tolerance > 0.0) {
        return Err(invalid(format!("tolerance must be a positive number, got {tolerance}")));
    }
    if tolerance < LINES_AND_ARCS_TOL {
        return Err(KernelError::Unsupported(format!(
            "lines_and_arcs below {LINES_AND_ARCS_TOL} mm, SMLib's fixed fit tolerance"
        )));
    }
    match curve.simplified(1e-9) {
        c @ (Curve3::Line { .. } | Curve3::Arc { .. } | Curve3::Circle { .. }) => Ok(vec![c]),
        Curve3::Polyline { points } => Ok(points.windows(2).map(|w| Curve3::Line { a: w[0], b: w[1] }).collect()),
        spline => fit_lines_and_arcs(k, &spline, tolerance, 0),
    }
}

/// How many times a spline is halved, at most, to bring the fit within tolerance.
const MAX_HALVINGS: u32 = 6;

/// SmApiConvertToLinesAndArcs aims at its fixed tolerance but can stray a little past
/// it, so each fit is measured and a spline that strays is halved and fitted again.
fn fit_lines_and_arcs(k: &SmlibKernel, spline: &Curve3, tolerance: f64, depth: u32) -> KernelResult<Vec<Curve3>> {
    let Curve3::Nurbs { degree, knots, points, weights } = spline else {
        return Ok(vec![spline.clone()]);
    };
    let input = nurbs_in(*degree, knots, points, weights);
    let raw = k.call(move |s| {
        let raw = s.engine.lines_and_arcs(&input).map_err(|e| query_failed("lines_and_arcs", &e))?;
        raw.iter().map(self::curve).collect::<KernelResult<Vec<_>>>()
    })?;
    // SMLib returns each fitted run of lines and arcs as one spline with a double knot at
    // every joint; split them into separate pieces. A piece it could not fit comes back
    // as it went in; that one becomes chords within half the tolerance.
    let mut pieces = Vec::new();
    for piece in raw.iter().flat_map(Curve3::spans) {
        match piece.simplified(LINES_AND_ARCS_TOL * 1e-3) {
            c @ (Curve3::Line { .. } | Curve3::Arc { .. } | Curve3::Circle { .. }) => pieces.push(c),
            other => {
                let points = other.sample(tolerance * 0.5);
                pieces.extend(points.windows(2).map(|w| Curve3::Line { a: w[0], b: w[1] }));
            }
        }
    }
    let strays = deviation(spline, &pieces, tolerance * 0.05);
    if strays <= tolerance * 0.95 {
        return Ok(pieces);
    }
    if depth >= MAX_HALVINGS {
        return Err(KernelError::QueryFailed {
            query: "lines_and_arcs".into(),
            code: -1,
            message: format!("the fit is {strays} mm off the curve, more than {tolerance} mm"),
        });
    }
    let mut out = fit_lines_and_arcs(k, &spline.between(0.0, 0.5), tolerance, depth + 1)?;
    out.extend(fit_lines_and_arcs(k, &spline.between(0.5, 1.0), tolerance, depth + 1)?);
    Ok(out)
}
