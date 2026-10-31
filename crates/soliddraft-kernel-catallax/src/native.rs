//! Catallax recipe operations that `soliddraft-recipe/1` cannot spell.
//!
//! Holes, patterns, mirrors, fillets, drafts and limited extrusions are stored as
//! native Catallax operations. Faces and edges are named by their Catallax origins in
//! the input body, never by numeric handles, so the operation means the same thing on
//! every rebuild of the stored chain.

use catallax_numbers::{Integer, Rational};
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

type Exact = [Rational; 3];

fn dot(a: &Exact, b: &Exact) -> Rational {
    &(&(&a[0] * &b[0]) + &(&a[1] * &b[1])) + &(&a[2] * &b[2])
}

fn times(a: &Exact, k: &Rational) -> Exact {
    [&a[0] * k, &a[1] * k, &a[2] * k]
}

fn plus(a: &Exact, b: &Exact) -> Exact {
    [&a[0] + &b[0], &a[1] + &b[1], &a[2] + &b[2]]
}

fn cross(a: &Exact, b: &Exact) -> Exact {
    [&(&a[1] * &b[2]) - &(&a[2] * &b[1]), &(&a[2] * &b[0]) - &(&a[0] * &b[2]), &(&a[0] * &b[1]) - &(&a[1] * &b[0])]
}

/// The whole square root of `n`, when it has one.
fn whole_root(n: &Integer) -> Option<Integer> {
    if n.is_zero() {
        return Some(Integer::zero());
    }
    let two = Integer::from_i64(2);
    let mut root = Integer::one().mul_pow2(n.bit_length().div_ceil(2));
    loop {
        let (quotient, _) = n.div_rem_floor(&root)?;
        let (next, _) = (&root + &quotient).div_rem_floor(&two)?;
        if next >= root {
            return (&root * &root == *n).then_some(root);
        }
        root = next;
    }
}

/// `v` scaled to unit length, when that length is rational.
fn unit(v: &Exact) -> Option<Exact> {
    let square = dot(v, v);
    let length = Rational::from_integer(whole_root(square.numerator())?)
        / Rational::from_integer(whole_root(square.denominator())?);
    (!length.is_zero()).then(|| times(v, &(Rational::one() / length)))
}

/// A planar face's sketch frame in exact numbers, as `FacePoint` defines it. A face whose
/// unit normal or X axis is irrational has none.
struct FaceFrame {
    origin: Exact,
    x: Exact,
    y: Exact,
    normal: Exact,
}

impl FaceFrame {
    fn of(solid: &Solid, body: BodyId, face: FaceId) -> KernelResult<Option<Self>> {
        let id = solid.cat_face(face, body)?;
        let model = &solid.app.checkpoint.built().model;
        let native = model.face(id);
        let catallax_shapes::Surface::Plane(plane) = model.geom().surface(native.surface) else { return Ok(None) };
        let exact = |x, y, z| -> Option<Exact> {
            let r = |v: &catallax_numbers::Real| v.as_rational();
            Some([r(x)?, r(y)?, r(z)?])
        };
        let normal = exact(&plane.normal.x, &plane.normal.y, &plane.normal.z);
        let origin = exact(&plane.origin.x, &plane.origin.y, &plane.origin.z);
        let (Some(normal), Some(origin)) = (normal, origin) else { return Ok(None) };
        Ok(Self::on(&normal, &origin, native.sense == catallax_topology::Sense::Same))
    }

    fn on(normal: &Exact, point: &Exact, same_sense: bool) -> Option<Self> {
        let sign = Rational::from_i64(if same_sense { 1 } else { -1 });
        let normal = times(&unit(normal)?, &sign);
        let zero = Rational::zero();
        // The test `plane_on_face` makes, on the value it reads.
        let x = if normal[2].to_f64_nearest().abs() < 1.0 - 1e-9 {
            unit(&[&zero - &normal[1], normal[0].clone(), zero])?
        } else {
            let along = times(&normal, &(&zero - &normal[0]));
            unit(&plus(&[Rational::one(), zero.clone(), zero], &along))?
        };
        let y = cross(&normal, &x);
        let origin = times(&normal, &dot(point, &normal));
        Some(Self { origin, x, y, normal })
    }

    fn point(&self, u: &Rational, v: &Rational) -> Exact {
        plus(&plus(&self.origin, &times(&self.x, u)), &times(&self.y, v))
    }
}

/// A hole's centre and direction. One authored on a face is built from the local point and
/// the face's exact frame, so literal inputs stay exact and the centre lies on the plane.
fn hole_axis(solid: &Solid, body: BodyId, p: &crate::HolePlacement) -> KernelResult<(Point3, Vector3)> {
    let frame = match p.on_face {
        Some(on) => FaceFrame::of(solid, body, on.face)?.map(|frame| (on, frame)),
        None => None,
    };
    let Some((on, frame)) = frame else {
        return Ok((point(p.point, "hole point")?, vector(p.direction, "hole direction")?));
    };
    let numbers = |v: Exact| v.map(Number::new);
    let centre = frame.point(number(on.u, "hole position")?.value(), number(on.v, "hole position")?.value());
    let into = times(&frame.normal, &Rational::from_i64(-1));
    Ok((Point3(numbers(centre)), Vector3(numbers(into))))
}

pub(super) fn hole(
    target: catallax_recipe::StepId,
    solid: &Solid,
    body: BodyId,
    params: &Hole,
) -> KernelResult<catallax_recipe::Operation> {
    let at = params
        .placements
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let (point, direction) = hole_axis(solid, body, p)?;
            Ok(HolePlacement { id: ElementId::new(format!("h{i}")).expect("placement id"), point, direction })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn exact(v: [&str; 3]) -> Exact {
        v.map(|text| Rational::parse(text).unwrap())
    }

    #[test]
    fn whole_roots_are_exact_or_absent() {
        for n in [0, 1, 4, 25, 625, 1_000_000] {
            let root = whole_root(&Integer::from_i64(n)).unwrap();
            assert_eq!(&root * &root, Integer::from_i64(n));
        }
        for n in [2, 3, 24, 26, 999_999] {
            assert!(whole_root(&Integer::from_i64(n)).is_none(), "{n}");
        }
    }

    #[test]
    fn a_tilted_frame_is_exact_and_an_irrational_one_is_absent() {
        // Any multiple of a 3-4-5 normal has rational axes. The point fixes the plane at 6.
        let frame = FaceFrame::on(&exact(["-4", "3", "0"]), &exact(["3", "14", "-13"]), true).unwrap();
        assert_eq!(frame.normal, exact(["-4/5", "3/5", "0"]));
        assert_eq!(frame.x, exact(["-3/5", "-4/5", "0"]));
        assert_eq!(frame.y, exact(["0", "0", "1"]));
        assert_eq!(frame.origin, exact(["-24/5", "18/5", "0"]));
        assert_eq!(frame.point(&Rational::from_i64(-13), &Rational::from_i64(4)), exact(["3", "14", "4"]));
        let turned = FaceFrame::on(&exact(["-4", "3", "0"]), &exact(["3", "14", "-13"]), false).unwrap();
        assert_eq!(turned.normal, exact(["4/5", "-3/5", "0"]));
        let top = FaceFrame::on(&exact(["0", "0", "2"]), &exact(["5", "5", "10"]), true).unwrap();
        assert_eq!(
            (top.x, top.y, top.origin),
            (exact(["1", "0", "0"]), exact(["0", "1", "0"]), exact(["0", "0", "10"]))
        );
        assert!(FaceFrame::on(&exact(["1", "1", "0"]), &exact(["0", "0", "0"]), true).is_none());
        assert!(
            FaceFrame::on(&exact(["3", "4", "12"]), &exact(["0", "0", "0"]), true).is_some(),
            "|n| is 13 and |Z x n| is 5"
        );
        assert!(FaceFrame::on(&exact(["1", "2", "2"]), &exact(["0", "0", "0"]), true).is_none(), "|Z x n| is sqrt 5");
    }
}
