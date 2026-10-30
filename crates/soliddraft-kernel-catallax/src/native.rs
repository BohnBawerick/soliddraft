//! Catallax recipe operations that `soliddraft-recipe/1` cannot spell.
//!
//! Holes, patterns, mirrors, fillets, drafts and limited extrusions are stored as
//! native Catallax operations. Faces and edges are named by their Catallax origins in
//! the input body, never by numeric handles, so the operation means the same thing on
//! every rebuild of the stored chain.

use catallax_numbers::Rational;
use catallax_recipe::{
    Axis, EdgeRef, ElementId, FaceRef, HoleDepth, HoleHead, HolePlacement, HoleTip, Number, Origin, OriginPattern,
    Plane, Point3, Vector3,
};
use soliddraft_kernel_api::*;

use crate::Solid;

pub(super) fn number(value: f64, what: &str) -> KernelResult<Number> {
    if !value.is_finite() {
        return Err(KernelError::InvalidInput(format!("{what} must be finite")));
    }
    // The shortest round-trip decimal, as the recipe converter reads application numbers.
    Number::parse(&value.to_string()).map_err(|e| KernelError::InvalidInput(format!("{what}: {e}")))
}

pub(super) fn vector(v: Vec3, what: &str) -> KernelResult<Vector3> {
    Ok(Vector3([number(v.x, what)?, number(v.y, what)?, number(v.z, what)?]))
}

pub(super) fn point(v: Vec3, what: &str) -> KernelResult<Point3> {
    Ok(Point3(vector(v, what)?.0))
}

/// A point exactly at a binary64 display position.
fn exact_point(p: [f64; 3]) -> Option<Point3> {
    let n = |v: f64| Rational::from_f64_exact(v).map(Number::new);
    Some(Point3([n(p[0])?, n(p[1])?, n(p[2])?]))
}

fn pattern(origin: &Origin) -> OriginPattern {
    OriginPattern::from(origin.clone())
}

impl Solid {
    fn cat_face(&self, face: FaceId, body: BodyId) -> KernelResult<catallax_topology::FaceId> {
        self.ids
            .faces
            .iter()
            .find_map(|(c, id)| (*id == face).then_some(*c))
            .ok_or(KernelError::UnknownFace { body, face })
    }

    fn cat_edge(&self, edge: EdgeId, body: BodyId) -> KernelResult<catallax_topology::EdgeId> {
        self.ids
            .edges
            .iter()
            .find_map(|(c, id)| (*id == edge).then_some(*c))
            .ok_or(KernelError::UnknownEdge { body, edge })
    }

    /// A point on a face's display triangles, within the display chord bound of the face.
    fn point_on_face(&self, face: catallax_topology::FaceId) -> KernelResult<Option<Point3>> {
        let mesh = self.display(&catallax_mesh::MeshOptions::default(), "face_ref")?;
        let Some(face_mesh) = mesh.faces.iter().find(|f| f.face == face) else { return Ok(None) };
        let triangles = &mesh.triangles[face_mesh.triangles.clone()];
        let area = |t: &catallax_mesh::Triangle| {
            let [a, b, c] = t.vertices.map(|v| Vec3::from(mesh.positions[v]));
            (b - a).cross(c - a).length()
        };
        let Some(largest) = triangles.iter().max_by(|a, b| area(a).total_cmp(&area(b))) else { return Ok(None) };
        let [a, b, c] = largest.vertices.map(|v| Vec3::from(mesh.positions[v]));
        let centre = (a + b + c) * (1.0 / 3.0);
        Ok(exact_point([centre.x, centre.y, centre.z]))
    }

    /// Names one face of this body for a later operation by its first origin, with a
    /// point on it when other faces share that origin.
    pub(super) fn face_ref(&self, face: FaceId, body: BodyId) -> KernelResult<FaceRef> {
        let cat = self.cat_face(face, body)?;
        let origins = &self.app.names.faces[&cat];
        let first = origins.first().ok_or_else(|| KernelError::InvalidInput(format!("{face} has no origin")))?;
        let face_pattern = pattern(first);
        let shared = self.app.names.faces.values().filter(|os| os.iter().any(|o| face_pattern.matches(o))).count();
        let near = if shared > 1 {
            Some(self.point_on_face(cat)?.ok_or_else(|| KernelError::InvalidInput(format!("{face} has no display")))?)
        } else {
            None
        };
        Ok(FaceRef { face: face_pattern, touching: vec![], near })
    }

    /// Names one edge of this body by the origins of the faces it separates, with a
    /// point on it when that pair of origins bounds several edges.
    pub(super) fn edge_ref(&self, edge: EdgeId, body: BodyId) -> KernelResult<EdgeRef> {
        let cat = self.cat_edge(edge, body)?;
        if let Some(name) = &self.app.names.edges[&cat] {
            let [a, b] = name.between();
            return Ok(EdgeRef { between: [pattern(a), pattern(b)], touching: vec![], near: None });
        }
        let model = &self.app.checkpoint.built().model;
        let faces: Vec<_> = model
            .faces_of_body(self.app.checkpoint.built().result)
            .into_iter()
            .filter(|f| {
                model.face(*f).loops.iter().any(|l| match model.loop_(*l) {
                    catallax_topology::Loop::Coedges(cs) => cs.iter().any(|c| model.coedge(*c).edge == cat),
                    catallax_topology::Loop::Vertex(_) => false,
                })
            })
            .collect();
        let first = |f: &catallax_topology::FaceId| self.app.names.faces.get(f).and_then(|os| os.first()).map(pattern);
        let [Some(a), Some(b)] = [faces.first().and_then(first), faces.last().and_then(first)] else {
            return Err(KernelError::InvalidInput(format!("{edge} does not separate two named faces")));
        };
        let mesh = self.display(&catallax_mesh::MeshOptions::default(), "edge_ref")?;
        let line = mesh.edges.iter().find(|e| e.edge == cat).map(|e| &e.vertices);
        let near = line.and_then(|v| exact_point(mesh.positions[v[v.len() / 2]]));
        if near.is_none() {
            return Err(KernelError::InvalidInput(format!("{edge} has no display polyline")));
        }
        Ok(EdgeRef { between: [a, b], touching: vec![], near })
    }
}

pub(super) fn hole(target: catallax_recipe::StepId, params: &Hole) -> KernelResult<catallax_recipe::Operation> {
    let at = params
        .placements
        .iter()
        .enumerate()
        .map(|(i, p)| {
            Ok(HolePlacement {
                id: ElementId::new(format!("h{i}")).expect("placement id"),
                point: point(p.point, "hole point")?,
                direction: vector(p.direction, "hole direction")?,
            })
        })
        .collect::<KernelResult<Vec<_>>>()?;
    if at.is_empty() {
        return Err(KernelError::InvalidInput("a hole needs at least one placement".into()));
    }
    Ok(catallax_recipe::Operation::Hole {
        target,
        at,
        diameter: number(params.diameter, "hole diameter")?,
        depth: match params.depth {
            soliddraft_kernel_api::HoleDepth::Blind { depth } => HoleDepth::Blind(number(depth, "hole depth")?),
            soliddraft_kernel_api::HoleDepth::Through => HoleDepth::Through,
        },
        tip: match params.tip {
            soliddraft_kernel_api::HoleTip::Flat => HoleTip::Flat,
            soliddraft_kernel_api::HoleTip::Cone { angle_deg } => HoleTip::Cone(number(angle_deg, "tip angle")?),
        },
        head: params
            .head
            .map(|h| match h {
                soliddraft_kernel_api::HoleHead::Counterbore { diameter, depth } => {
                    Ok::<_, KernelError>(HoleHead::Counterbore {
                        diameter: number(diameter, "counterbore diameter")?,
                        depth: number(depth, "counterbore depth")?,
                    })
                }
                soliddraft_kernel_api::HoleHead::Countersink { diameter, angle_deg } => Ok(HoleHead::Countersink {
                    diameter: number(diameter, "countersink diameter")?,
                    angle: number(angle_deg, "countersink angle")?,
                }),
            })
            .transpose()?,
    })
}

pub(super) fn pattern_layout(layout: &PatternLayout) -> KernelResult<catallax_recipe::PatternLayout> {
    Ok(match *layout {
        PatternLayout::Linear { step, count } => {
            catallax_recipe::PatternLayout::Linear { vector: vector(step, "pattern step")?, count }
        }
        PatternLayout::Circular { axis_origin, axis_direction, count, angle_deg } => {
            catallax_recipe::PatternLayout::Circular {
                axis: Axis {
                    point: point(axis_origin, "pattern axis")?,
                    direction: vector(axis_direction, "pattern axis")?,
                },
                count,
                angle: number(angle_deg, "pattern angle")?,
            }
        }
    })
}

pub(super) fn plane(origin: Vec3, normal: Vec3, what: &str) -> KernelResult<Plane> {
    Ok(Plane { point: point(origin, what)?, normal: vector(normal, what)? })
}
