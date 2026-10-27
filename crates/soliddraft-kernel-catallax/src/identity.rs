//! Identity keys that carry over persistent ids from a parent body's entities.
//!
//! A key holds the exact definitions of a vertex, an edge or a face. Two keys are equal
//! exactly when their `{:?}` text, printed for both at once, is equal, without printing
//! it in the common cases. Exact coordinates are expression DAGs whose printed form
//! expands every shared node, so a drafted vertex alone prints hundreds of kilobytes.
//! The text also shows each algebraic root's refinement cache, so a parent's keys
//! printed when it was built would not match its unchanged entities printed after a
//! later refinement; holding the values keeps those entities' ids.
//!
//! [`Real::same_definition`] and its curve and surface forms hold for every pair with
//! equal text, so a false result is final. A pair whose every exact value is the same
//! node prints the same text. Only a remaining pair, built separately with agreeing
//! definitions, is printed and compared, because `same_definition` also accepts
//! differently built expressions of one rational value, whose text differs.

use std::collections::BTreeMap;
use std::fmt::Debug;

use catallax_app::IncrementalApplicationBody;
use catallax_numbers::Real;
use catallax_shapes::{
    Circle, Cone, Curve, Cylinder, Ellipse, EllipticCylinder, Line, Plane, Point3, Sphere, Surface, Torus, Vector3,
};
use catallax_topology::{Model, Sense};

type CatFace = catallax_topology::FaceId;
type CatEdge = catallax_topology::EdgeId;
type CatVertex = catallax_topology::VertexId;

/// The keys of every named entity of a build.
pub(super) struct Keys {
    pub faces: BTreeMap<CatFace, FaceKey>,
    pub edges: BTreeMap<CatEdge, NamedEdgeKey>,
    pub vertices: BTreeMap<CatVertex, VertexKey>,
}

impl Keys {
    pub fn new(app: &IncrementalApplicationBody) -> Self {
        let model = &app.checkpoint.built().model;
        // Each exact position and curve is read once; faces share their edges' keys.
        let vertices: BTreeMap<_, _> = app.names.vertices.keys().map(|v| (*v, VertexKey::new(model, *v))).collect();
        let vertex = |v| vertices.get(&v).cloned().unwrap_or_else(|| VertexKey::new(model, v));
        let mut curves = BTreeMap::new();
        let mut edge = |e| curves.entry(e).or_insert_with(|| EdgeKey::new(model, e, vertex)).clone();
        let faces = app
            .names
            .faces
            .iter()
            .map(|(face, origins)| {
                let f = model.face(*face);
                let boundary = f
                    .loops
                    .iter()
                    .flat_map(|l| match model.loop_(*l) {
                        catallax_topology::Loop::Coedges(cs) => {
                            cs.iter().map(|c| Bound::Edge(edge(model.coedge(*c).edge))).collect()
                        }
                        catallax_topology::Loop::Vertex(v) => vec![Bound::Vertex(vertex(*v))],
                    })
                    .collect();
                (*face, FaceKey::new(model.geom().surface(f.surface).clone(), f.sense, boundary, origins))
            })
            .collect();
        let edges = app.names.edges.iter().map(|(e, name)| (*e, NamedEdgeKey::new(edge(*e), name))).collect();
        Self { faces, edges, vertices }
    }
}

/// A vertex's exact position.
#[derive(Clone)]
pub(super) struct VertexKey(Point3);

/// An edge's exact curve and end positions, in either order.
#[derive(Clone)]
struct EdgeKey {
    curve: Curve,
    ends: Vec<VertexKey>,
}

/// One entry of a face boundary: an edge, or the vertex of a vertex loop.
#[derive(Clone)]
enum Bound {
    Edge(EdgeKey),
    Vertex(VertexKey),
}

/// A face's exact support, its sense, its boundary in any order and its origin sets.
pub(super) struct FaceKey {
    pub support: Surface,
    sense: Sense,
    boundary: Vec<Bound>,
    /// The origins' text; it is short.
    origins: String,
}

/// An edge's geometry and its origin-pair name's text.
pub(super) struct NamedEdgeKey {
    edge: EdgeKey,
    name: String,
}

impl VertexKey {
    fn new(model: &Model, vertex: CatVertex) -> Self {
        Self(model.geom().position(model.vertex(vertex).point))
    }

    pub fn same(&self, other: &Self) -> bool {
        let (a, b) = (point(&self.0), point(&other.0));
        exact(
            &self.0,
            &other.0,
            a.iter().zip(&b).all(|(a, b)| a.same_definition(b)),
            Some(a.to_vec()),
            Some(b.to_vec()),
        )
    }
}

impl EdgeKey {
    fn new(model: &Model, edge: CatEdge, vertex: impl Fn(CatVertex) -> VertexKey) -> Self {
        let e = model.edge(edge);
        Self {
            curve: model.geom().curve(e.curve).clone(),
            ends: [e.start, e.end].into_iter().flatten().map(vertex).collect(),
        }
    }

    fn same(&self, other: &Self) -> bool {
        same_curve(&self.curve, &other.curve) && same_multiset(&self.ends, &other.ends, VertexKey::same)
    }
}

impl Bound {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Edge(a), Self::Edge(b)) => a.same(b),
            (Self::Vertex(a), Self::Vertex(b)) => a.same(b),
            _ => false,
        }
    }
}

impl FaceKey {
    fn new(support: Surface, sense: Sense, boundary: Vec<Bound>, origins: &impl Debug) -> Self {
        Self { support, sense, boundary, origins: format!("{origins:?}") }
    }

    pub fn same(&self, other: &Self) -> bool {
        self.origins == other.origins
            && self.sense == other.sense
            && same_surface(&self.support, &other.support)
            && same_multiset(&self.boundary, &other.boundary, Bound::same)
    }
}

impl NamedEdgeKey {
    fn new(edge: EdgeKey, name: &impl Debug) -> Self {
        Self { edge, name: format!("{name:?}") }
    }

    pub fn same(&self, other: &Self) -> bool {
        self.name == other.name && self.edge.same(&other.edge)
    }
}

/// Supports compare as their text does.
pub(super) fn same_surface(a: &Surface, b: &Surface) -> bool {
    exact(a, b, a.same_definition(b), surface(a), surface(b))
}

fn same_curve(a: &Curve, b: &Curve) -> bool {
    if let (Curve::Intersection(a), Curve::Intersection(b)) = (a, b) {
        // Store handles and a witness, compared as printed.
        return a == b;
    }
    exact(a, b, a.same_definition(b), curve(a), curve(b))
}

/// The text comparison, decided without printing where `agree` (a necessary condition)
/// fails or every exact value of `a` and `b` is the same node.
fn exact<T: Debug>(a: &T, b: &T, agree: bool, a_reals: Option<Vec<&Real>>, b_reals: Option<Vec<&Real>>) -> bool {
    if !agree {
        return false;
    }
    if let (Some(a), Some(b)) = (&a_reals, &b_reals)
        && a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| a.same_node(b))
    {
        return true;
    }
    format!("{a:?}") == format!("{b:?}")
}

/// Equal as multisets, under an equivalence.
fn same_multiset<T>(a: &[T], b: &[T], same: impl Fn(&T, &T) -> bool) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut used = vec![false; b.len()];
    a.iter().all(|x| match (0..b.len()).find(|&i| !used[i] && same(x, &b[i])) {
        Some(i) => {
            used[i] = true;
            true
        }
        None => false,
    })
}

// Every exact value each definition prints. The struct patterns name every field, so a
// field Catallax adds stops this build rather than escaping the comparison.

fn point(p: &Point3) -> [&Real; 3] {
    let Point3 { x, y, z } = p;
    [x, y, z]
}

fn vector(v: &Vector3) -> [&Real; 3] {
    let Vector3 { x, y, z } = v;
    [x, y, z]
}

fn reals<'a>(points: &[&'a Point3], vectors: &[&'a Vector3], scalars: &[&'a Real]) -> Vec<&'a Real> {
    points
        .iter()
        .flat_map(|p| point(p))
        .chain(vectors.iter().flat_map(|v| vector(v)))
        .chain(scalars.iter().copied())
        .collect()
}

/// None for a definition this list does not cover, which is then printed.
fn curve(c: &Curve) -> Option<Vec<&Real>> {
    Some(match c {
        Curve::Line(Line { origin, dir }) => reals(&[origin], &[dir], &[]),
        Curve::Circle(Circle { center, normal, radius_sq, ref_dir }) => {
            reals(&[center], &[normal, ref_dir], &[radius_sq])
        }
        Curve::Ellipse(Ellipse { center, normal, major_dir, major_sq, minor_sq }) => {
            reals(&[center], &[normal, major_dir], &[major_sq, minor_sq])
        }
        _ => return None,
    })
}

fn surface(s: &Surface) -> Option<Vec<&Real>> {
    Some(match s {
        Surface::Plane(Plane { origin, normal, ref_dir }) => reals(&[origin], &[normal, ref_dir], &[]),
        Surface::Cylinder(Cylinder { origin, axis, radius_sq, ref_dir }) => {
            reals(&[origin], &[axis, ref_dir], &[radius_sq])
        }
        Surface::Cone(Cone { apex, axis, slope, ref_dir }) => reals(&[apex], &[axis, ref_dir], &[slope]),
        Surface::Sphere(Sphere { center, radius, axis, ref_dir }) => reals(&[center], &[axis, ref_dir], &[radius]),
        Surface::Torus(Torus { center, axis, major, minor, ref_dir }) => {
            reals(&[center], &[axis, ref_dir], &[major, minor])
        }
        Surface::EllipticCylinder(EllipticCylinder { origin, axis, major_dir, major_sq, minor_sq }) => {
            reals(&[origin], &[axis, major_dir], &[major_sq, minor_sq])
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CatallaxKernel;
    use soliddraft_kernel_api::{
        BooleanOp, CurveKind, Draft, Extrude, Kernel, OpTag, Plane as SketchPlane, Point2, Profile, Vec3,
    };

    fn text_equal<T: Debug>(a: &T, b: &T) -> bool {
        format!("{a:?}") == format!("{b:?}")
    }

    #[test]
    fn exact_values_compare_as_their_text_does() {
        let two = Real::from_i64(2);
        let sum = &Real::one() + &Real::one();
        let root = Real::from_i64(2).sqrt();
        // Shared nodes, a separately built leaf, the same value built differently, a
        // shared and a separately built irrational, and a different value.
        let xs = [
            two.clone(),
            two.clone(),
            Real::from_i64(2),
            sum.clone(),
            root.clone(),
            root,
            two.sqrt(),
            Real::from_i64(3),
        ];
        assert!(two.same_definition(&sum), "the definitions agree on 1 + 1 and 2");
        assert!(!text_equal(&two, &sum), "but their text does not");
        let points: Vec<_> = xs.iter().map(|x| Point3::new(x.clone(), Real::zero(), x.clone())).collect();
        for a in &points {
            for b in &points {
                assert_eq!(VertexKey(a.clone()).same(&VertexKey(b.clone())), text_equal(a, b), "{a:?}\n{b:?}");
            }
        }
        let planes: Vec<_> = points
            .iter()
            .map(|origin| {
                let normal = Vector3::new(Real::zero(), Real::zero(), origin.x.clone());
                let ref_dir = Vector3::new(Real::one(), Real::zero(), Real::zero());
                Surface::Plane(Plane { origin: origin.clone(), normal, ref_dir })
            })
            .collect();
        for a in &planes {
            for b in &planes {
                assert_eq!(same_surface(a, b), text_equal(a, b), "{a:?}\n{b:?}");
            }
        }
    }

    // The string keys these replace, verbatim.
    fn vertex_text(model: &Model, vertex: CatVertex) -> String {
        format!("{:?}", model.geom().position(model.vertex(vertex).point))
    }

    fn edge_text(model: &Model, edge: CatEdge) -> String {
        let e = model.edge(edge);
        let mut ends: Vec<_> = [e.start, e.end].into_iter().flatten().map(|v| vertex_text(model, v)).collect();
        ends.sort();
        format!("{:?}:{ends:?}", model.geom().curve(e.curve))
    }

    fn face_text(app: &IncrementalApplicationBody, face: CatFace) -> String {
        let model = &app.checkpoint.built().model;
        let f = model.face(face);
        let support = format!("{:?}", model.geom().surface(f.surface));
        let mut boundary: Vec<_> = f
            .loops
            .iter()
            .flat_map(|l| match model.loop_(*l) {
                catallax_topology::Loop::Coedges(cs) => {
                    cs.iter().map(|c| edge_text(model, model.coedge(*c).edge)).collect()
                }
                catallax_topology::Loop::Vertex(v) => vec![vertex_text(model, *v)],
            })
            .collect();
        boundary.sort();
        format!("{support}:{:?}:{boundary:?}:{:?}", f.sense, app.names.faces[&face])
    }

    /// Every pair of `(text, key)` entries agrees, and the set has both outcomes.
    fn agree<K>(entries: &[(String, K)], same: impl Fn(&K, &K) -> bool) -> (usize, usize) {
        let (mut equal, mut different) = (0, 0);
        for (i, (a_text, a)) in entries.iter().enumerate() {
            for (b_text, b) in &entries[i + 1..] {
                let text = a_text == b_text;
                assert_eq!(same(a, b), text, "key {i} disagrees with its text");
                if text { equal += 1 } else { different += 1 }
            }
        }
        (equal, different)
    }

    fn block(k: &CatallaxKernel, tag: &str, w: f64, h: f64, z0: f64, d: f64) -> soliddraft_kernel_api::BodyId {
        let plane = SketchPlane::XY.with_origin(Vec3::new(0.0, 0.0, z0));
        let sheet =
            k.create_body_from_profile(&OpTag::new(tag), &Profile::rectangle(plane, Point2::new(0.0, 0.0), w, h));
        k.extrude(&OpTag::new(tag), sheet.unwrap().body, &Extrude { distance: d, direction: None }).unwrap().body
    }

    fn post(k: &CatallaxKernel, tag: &str, base: Vec3, r: f64, h: f64) -> soliddraft_kernel_api::BodyId {
        let profile = Profile::circle(SketchPlane::XY.with_origin(base), Point2::new(0.0, 0.0), r);
        let sheet = k.create_body_from_profile(&OpTag::new(tag), &profile).unwrap().body;
        k.extrude(&OpTag::new(tag), sheet, &Extrude { distance: h, direction: None }).unwrap().body
    }

    #[test]
    fn identity_keys_match_the_string_keys_on_drafted_cut_and_filleted_bodies() {
        let k = CatallaxKernel::new();
        // The draft scenario: two drafted sides, then a boss on the shrunken top, built
        // at 3 degrees and rebuilt at 5 as an edit does.
        let box_ = block(&k, "box", 40.0, 30.0, 0.0, 20.0);
        let sides: Vec<_> = k
            .topology(box_)
            .unwrap()
            .faces
            .iter()
            .filter(|f| f.normal.is_some_and(|n| n.distance(Vec3::X) < 1e-12 || n.distance(-Vec3::X) < 1e-12))
            .map(|f| f.id)
            .collect();
        for angle_deg in [3.0, 5.0] {
            let draft = Draft { pull: Vec3::Z, neutral_origin: Vec3::ZERO, neutral_normal: Vec3::Z, angle_deg };
            let drafted = k.draft(&OpTag::new("draft"), box_, &sides, &draft).unwrap().body;
            let boss = post(&k, "boss", Vec3::new(0.0, 0.0, 20.0), 5.0, 5.0);
            k.boolean(&OpTag::new("join"), BooleanOp::Union, drafted, boss).unwrap();
        }
        let plate = block(&k, "plate", 60.0, 40.0, 0.0, 10.0);
        let hole = post(&k, "hole", Vec3::new(10.0, 0.0, 0.0), 4.0, 10.0);
        let cut = k.boolean(&OpTag::new("cut"), BooleanOp::Subtract, plate, hole).unwrap().body;
        let edge = k
            .topology(cut)
            .unwrap()
            .edges
            .iter()
            .find(|e| e.curve == CurveKind::Line && e.midpoint.distance(Vec3::new(0.0, 20.0, 10.0)) < 1e-9)
            .expect("the top edge along x at y = 20")
            .id;
        k.fillet(&OpTag::new("round"), cut, &[edge], 2.0).unwrap();

        let state = k.state().unwrap();
        let solids: Vec<_> = state.bodies.values().filter_map(|s| s.solid.clone()).collect();
        let (mut faces, mut edges, mut vertices) = (Vec::new(), Vec::new(), Vec::new());
        for solid in &solids {
            let model = &solid.app.checkpoint.built().model;
            let keys = Keys::new(&solid.app);
            for (face, key) in keys.faces {
                faces.push((face_text(&solid.app, face), key));
            }
            for (edge, key) in keys.edges {
                edges.push((format!("{}:{:?}", edge_text(model, edge), solid.app.names.edges[&edge]), key));
            }
            for (vertex, key) in keys.vertices {
                vertices.push((vertex_text(model, vertex), key));
            }
        }
        // Across and within bodies, so parents meet their children as the ids do.
        for (equal, different) in
            [agree(&faces, FaceKey::same), agree(&edges, NamedEdgeKey::same), agree(&vertices, VertexKey::same)]
        {
            assert!(equal > 0 && different > 0, "{equal} equal and {different} different pairs");
        }
    }
}
