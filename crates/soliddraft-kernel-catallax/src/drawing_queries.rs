//! Polygon drawing queries over Catallax's display and topology.

use std::collections::BTreeMap;

use catallax_shapes::Curve;
use catallax_topology::{Loop, Sense};
use soliddraft_kernel_api::drawing::{Curve3, CurveSource, ViewCurves, ViewSpec, mesh_hlr};
use soliddraft_kernel_api::*;

use crate::{CatallaxKernel, Solid};

pub(super) fn view_curves(k: &CatallaxKernel, body: BodyId, spec: &ViewSpec) -> KernelResult<ViewCurves> {
    let mut view = mesh_hlr::view_curves(k, body, spec)?;
    let mut counts = BTreeMap::new();
    for curve in &view.curves {
        *counts.entry(curve.source).or_insert(0) += 1;
    }
    // Keep whole lines and circles for dimensions and exporters after polygon visibility.
    // Split edges and silhouettes keep the mesh result.
    let solid = k.solid(body)?;
    for curve in &mut view.curves {
        if let CurveSource::Edge(edge) = curve.source
            && counts[&curve.source] == 1
        {
            let geometry = edge_curve(&solid, body, edge)?;
            if matches!(geometry, Curve3::Line { .. } | Curve3::Circle { .. }) {
                curve.curve = geometry;
            }
        }
    }
    Ok(ViewCurves::new(view.curves, &spec.frame().expect("mesh_hlr checked the frame")))
}

pub(super) fn edge_curve(solid: &Solid, body: BodyId, edge: EdgeId) -> KernelResult<Curve3> {
    let id = solid
        .ids
        .edges
        .iter()
        .find_map(|(id, e)| (*e == edge).then_some(*id))
        .ok_or(KernelError::UnknownEdge { body, edge })?;
    let model = &solid.app.built.model;
    let e = model.edge(id);
    let polyline =
        solid.app.display.value.edges.iter().find(|p| p.edge == id).ok_or_else(|| KernelError::QueryFailed {
            query: "edge_curve".into(),
            code: 0,
            message: "Catallax edge has no display polyline".into(),
        })?;
    let points: Vec<Vec3> = polyline.vertices.iter().map(|v| solid.app.display.value.positions[*v].into()).collect();
    match model.geom().curve(e.curve) {
        Curve::Line(_) => Ok(Curve3::Line { a: points[0], b: *points.last().expect("edge has endpoints") }),
        Curve::Circle(c) if e.start.is_none() && e.end.is_none() => {
            let center = Vec3::new(c.center.x.to_f64(), c.center.y.to_f64(), c.center.z.to_f64());
            let normal = Vec3::new(c.normal.x.to_f64(), c.normal.y.to_f64(), c.normal.z.to_f64())
                .normalized()
                .ok_or_else(|| KernelError::InvalidInput("Catallax circle has a zero normal".into()))?;
            Ok(Curve3::Circle { center, normal, radius: c.radius_sq.to_f64().sqrt() })
        }
        // shortcut: trimmed circles and intersection curves use display polylines until Catallax exposes curve conversion.
        _ => Ok(Curve3::Polyline { points }),
    }
}

pub(super) fn face_loops(solid: &Solid, body: BodyId, face: FaceId) -> KernelResult<Vec<Vec<Curve3>>> {
    let id = solid
        .ids
        .faces
        .iter()
        .find_map(|(id, f)| (*f == face).then_some(*id))
        .ok_or(KernelError::UnknownFace { body, face })?;
    let model = &solid.app.built.model;
    let mesh = &solid.app.display.value;
    let edges: BTreeMap<_, _> = mesh.edges.iter().map(|e| (e.edge, e)).collect();
    let mut loops = Vec::new();
    for boundary in &model.face(id).loops {
        let Loop::Coedges(coedges) = model.loop_(*boundary) else { continue };
        let mut curves = Vec::new();
        for coedge in coedges {
            let c = model.coedge(*coedge);
            let edge = edges.get(&c.edge).ok_or_else(|| KernelError::QueryFailed {
                query: "face_loops".into(),
                code: 0,
                message: "Catallax coedge has no display polyline".into(),
            })?;
            let mut points: Vec<Vec3> = edge.vertices.iter().map(|v| mesh.positions[*v].into()).collect();
            if c.sense == Sense::Reversed {
                points.reverse();
            }
            curves.push(Curve3::Polyline { points });
        }
        if !curves.is_empty() {
            loops.push(curves);
        }
    }
    // Catallax does not order loops outer-first. The largest planar loop encloses the holes.
    let area = |curves: &[Curve3]| {
        curves
            .iter()
            .map(|c| c.sample(0.0).windows(2).fold(Vec3::ZERO, |sum, p| sum + p[0].cross(p[1])))
            .fold(Vec3::ZERO, |sum, area| sum + area)
            .length()
    };
    loops.sort_by(|a, b| area(b).total_cmp(&area(a)));
    Ok(loops)
}
