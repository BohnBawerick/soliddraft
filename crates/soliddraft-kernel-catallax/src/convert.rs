//! Conversion of Catallax names, exact definitions and certified display data.

use std::collections::{BTreeMap, BTreeSet};

use catallax_app::ApplicationBody;
use catallax_recipe::{Origin, Role};
use catallax_shapes::{CurveKind as CatCurve, Surface, SurfaceKind as CatSurface};
use catallax_topology::{Model, Sense};
use soliddraft_kernel_api::*;

use crate::query_error;

type CatFace = catallax_topology::FaceId;
type CatEdge = catallax_topology::EdgeId;
type CatVertex = catallax_topology::VertexId;

pub(super) struct Ids {
    pub faces: BTreeMap<CatFace, FaceId>,
    pub edges: BTreeMap<CatEdge, EdgeId>,
    pub vertices: BTreeMap<CatVertex, VertexId>,
    face_keys: BTreeMap<FaceId, String>,
    edge_keys: BTreeMap<EdgeId, String>,
    vertex_keys: BTreeMap<VertexId, String>,
    origins: BTreeMap<FaceId, Vec<Origin>>,
    supports: BTreeMap<FaceId, String>,
    edge_names: BTreeMap<EdgeId, String>,
}

fn assign<I: Copy + Ord>(key: &str, parents: &[&BTreeMap<I, String>], next: &mut u64, wrap: impl Fn(u64) -> I) -> I {
    if let Some(id) = parents.iter().flat_map(|p| p.iter()).find_map(|(id, value)| (value == key).then_some(*id)) {
        return id;
    }
    *next += 1;
    wrap(*next)
}

fn vertex_key(model: &Model, vertex: CatVertex) -> String {
    format!("{:?}", model.geom().position(model.vertex(vertex).point))
}

fn edge_key(model: &Model, edge: CatEdge) -> String {
    let e = model.edge(edge);
    let mut ends: Vec<_> = [e.start, e.end].into_iter().flatten().map(|v| vertex_key(model, v)).collect();
    ends.sort();
    format!("{:?}:{ends:?}", model.geom().curve(e.curve))
}

impl Ids {
    pub fn new(app: &ApplicationBody, parents: &[&Ids], next: &mut u64) -> Self {
        let model = &app.built.model;
        let mut ids = Self {
            faces: BTreeMap::new(),
            edges: BTreeMap::new(),
            vertices: BTreeMap::new(),
            face_keys: BTreeMap::new(),
            edge_keys: BTreeMap::new(),
            vertex_keys: BTreeMap::new(),
            origins: BTreeMap::new(),
            supports: BTreeMap::new(),
            edge_names: BTreeMap::new(),
        };
        for face in app.names.faces.keys() {
            let f = model.face(*face);
            let support = format!("{:?}", model.geom().surface(f.surface));
            let mut boundary: Vec<_> = f
                .loops
                .iter()
                .flat_map(|l| match model.loop_(*l) {
                    catallax_topology::Loop::Coedges(cs) => {
                        cs.iter().map(|c| edge_key(model, model.coedge(*c).edge)).collect()
                    }
                    catallax_topology::Loop::Vertex(v) => vec![vertex_key(model, *v)],
                })
                .collect();
            boundary.sort();
            let origins = app.names.faces[face].clone();
            let key = format!("{support}:{:?}:{boundary:?}:{origins:?}", f.sense);
            let id = assign(&key, &parents.iter().map(|p| &p.face_keys).collect::<Vec<_>>(), next, FaceId::from_raw);
            ids.faces.insert(*face, id);
            ids.face_keys.insert(id, key);
            ids.origins.insert(id, origins);
            ids.supports.insert(id, support);
        }
        for (edge, name) in &app.names.edges {
            let key = format!("{}:{name:?}", edge_key(model, *edge));
            let id = assign(&key, &parents.iter().map(|p| &p.edge_keys).collect::<Vec<_>>(), next, EdgeId::from_raw);
            ids.edges.insert(*edge, id);
            ids.edge_keys.insert(id, key);
            if let Some(name) = name {
                ids.edge_names.insert(id, serde_json::to_string(name).expect("edge name serializes"));
            }
        }
        for vertex in app.names.vertices.keys() {
            let key = vertex_key(model, *vertex);
            let id =
                assign(&key, &parents.iter().map(|p| &p.vertex_keys).collect::<Vec<_>>(), next, VertexId::from_raw);
            ids.vertices.insert(*vertex, id);
            ids.vertex_keys.insert(id, key);
        }
        ids
    }

    pub fn record(&self, seq: u64, tag: &OpTag, op: OpKind, step: &str, parents: &[&Ids]) -> HistoryRecord {
        let faces = changes(
            self.face_keys.keys().copied(),
            parents.iter().flat_map(|p| p.face_keys.keys().copied()),
            |old, new| {
                parents.iter().any(|p| {
                    p.origins.get(&old).is_some_and(|os| self.origins[&new].iter().any(|o| os.contains(o)))
                        && p.supports.get(&old) == self.supports.get(&new)
                })
            },
        );
        let edges = changes(
            self.edge_keys.keys().copied(),
            parents.iter().flat_map(|p| p.edge_keys.keys().copied()),
            |old, new| {
                self.edge_names
                    .get(&new)
                    .is_some_and(|name| parents.iter().any(|p| p.edge_names.get(&old) == Some(name)))
            },
        );
        let vertices = changes(
            self.vertex_keys.keys().copied(),
            parents.iter().flat_map(|p| p.vertex_keys.keys().copied()),
            |_, _| false,
        );
        let face_roles = faces
            .created
            .iter()
            .map(|face| {
                let role = self.origins[face]
                    .iter()
                    .find(|o| o.step.as_str() == step)
                    .map(|o| face_role(&o.role))
                    .unwrap_or(FaceRole::Other);
                FaceOrigin { face: *face, role }
            })
            .collect();
        HistoryRecord { seq, tag: tag.clone(), op, faces, edges, vertices, face_roles }
    }
}

fn changes<I: Copy + Ord>(
    now: impl Iterator<Item = I>,
    before: impl Iterator<Item = I>,
    descendant: impl Fn(I, I) -> bool,
) -> EntityChanges<I> {
    let now: BTreeSet<_> = now.collect();
    let before: BTreeSet<_> = before.collect();
    let new: Vec<_> = now.difference(&before).copied().collect();
    let mut out = EntityChanges::default();
    let mut children = BTreeSet::new();
    for old in before.difference(&now) {
        let into: Vec<_> = new.iter().copied().filter(|n| descendant(*old, *n)).collect();
        if into.is_empty() {
            out.deleted.push(*old);
        } else {
            children.extend(into.iter().copied());
            out.split.push(Split { from: *old, into });
        }
    }
    out.created = new.into_iter().filter(|n| !children.contains(n)).collect();
    out
}

fn face_role(role: &Role) -> FaceRole {
    match role {
        Role::Start => FaceRole::StartCap,
        Role::End => FaceRole::EndCap,
        Role::Side { segment, .. } => {
            if let Some((loop_index, segment)) = segment
                .as_str()
                .strip_prefix('l')
                .and_then(|s| s.split_once('s'))
                .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
            {
                FaceRole::Side { loop_index, segment }
            } else {
                FaceRole::Other
            }
        }
        _ => FaceRole::Other,
    }
}

pub(super) fn midpoint(value: &catallax_measure::Enclosure) -> f64 {
    ((&value.lo + &value.hi) / catallax_numbers::Rational::from_i64(2)).to_f64_nearest()
}

fn vector(v: &catallax_shapes::Vector3) -> Vec3 {
    Vec3::new(v.x.to_f64(), v.y.to_f64(), v.z.to_f64())
}

fn surface_kind(surface: &Surface) -> SurfaceKind {
    match surface.kind() {
        CatSurface::Plane => SurfaceKind::Plane,
        CatSurface::Cylinder => SurfaceKind::Cylinder,
        CatSurface::Cone => SurfaceKind::Cone,
        CatSurface::Sphere => SurfaceKind::Sphere,
        CatSurface::Torus => SurfaceKind::Torus,
        _ => SurfaceKind::Other,
    }
}

fn bounds(points: impl Iterator<Item = Vec3>, padding: f64) -> Aabb {
    let mut min = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut max = -min;
    let mut any = false;
    for p in points {
        any = true;
        min = Vec3::new(min.x.min(p.x), min.y.min(p.y), min.z.min(p.z));
        max = Vec3::new(max.x.max(p.x), max.y.max(p.y), max.z.max(p.z));
    }
    let pad = Vec3::new(padding, padding, padding);
    if any { Aabb { min: min - pad, max: max + pad } } else { Aabb { min: Vec3::ZERO, max: Vec3::ZERO } }
}

pub(super) fn topology(app: &ApplicationBody, ids: &Ids) -> KernelResult<Topology> {
    let model = &app.built.model;
    let mesh = &app.display.value;
    let chord = catallax_mesh::MeshOptions::default().chord_tolerance.to_f64_nearest();
    let mut faces = Vec::new();
    let mut adjacency: BTreeMap<CatEdge, Vec<FaceId>> = BTreeMap::new();
    for face_mesh in &mesh.faces {
        let face = model.face(face_mesh.face);
        let triangles = &mesh.triangles[face_mesh.triangles.clone()];
        let mut area = 0.0;
        let mut center = Vec3::ZERO;
        for t in triangles {
            let [a, b, c] = t.vertices.map(|v| Vec3::from(mesh.positions[v]));
            let weight = (b - a).cross(c - a).length() * 0.5;
            area += weight;
            center = center + (a + b + c) * (weight / 3.0);
        }
        let bbox = bounds(triangles.iter().flat_map(|t| t.vertices).map(|v| mesh.positions[v].into()), chord);
        let centroid = if area > 0.0 {
            center * (1.0 / area)
        } else {
            return Err(KernelError::QueryFailed {
                query: "topology".into(),
                code: 0,
                message: "Catallax display face has no nondegenerate triangles (Catallax stage 1)".into(),
            });
        };
        let proven_area = catallax_measure::face_area(
            model,
            face_mesh.face,
            &catallax_measure::Accuracy::default(),
            &mut catallax_numbers::Decider::default(),
        )
        .map_err(|e| query_error("face_area", e))?;
        let mut edges = Vec::new();
        for l in &face.loops {
            if let catallax_topology::Loop::Coedges(cs) = model.loop_(*l) {
                for c in cs {
                    let edge = model.coedge(*c).edge;
                    edges.push(ids.edges[&edge]);
                    adjacency.entry(edge).or_default().push(ids.faces[&face_mesh.face]);
                }
            }
        }
        let surface = model.geom().surface(face.surface);
        let normal = if let Surface::Plane(p) = surface {
            vector(&p.normal).normalized().map(|n| if face.sense == Sense::Same { n } else { -n })
        } else {
            None
        };
        faces.push(FaceInfo {
            id: ids.faces[&face_mesh.face],
            surface: surface_kind(surface),
            area: midpoint(&proven_area.value),
            centroid,
            bbox,
            edges,
            normal,
        });
    }
    let mut edges = Vec::new();
    for e in &mesh.edges {
        let edge = model.edge(e.edge);
        let points: Vec<Vec3> = e.vertices.iter().map(|v| mesh.positions[*v].into()).collect();
        let length = points.windows(2).map(|p| p[0].distance(p[1])).sum();
        let midpoint = if points.len() % 2 == 1 {
            points[points.len() / 2]
        } else {
            (points[points.len() / 2 - 1] + points[points.len() / 2]) * 0.5
        };
        let curve = match model.geom().curve(edge.curve).kind() {
            CatCurve::Line => CurveKind::Line,
            CatCurve::Circle => CurveKind::Circle,
            CatCurve::Ellipse => CurveKind::Ellipse,
            _ => CurveKind::Other,
        };
        let vertices: BTreeSet<_> = [edge.start, edge.end].into_iter().flatten().map(|v| ids.vertices[&v]).collect();
        edges.push(EdgeInfo {
            id: ids.edges[&e.edge],
            curve,
            length,
            midpoint,
            bbox: bounds(points.into_iter(), chord),
            faces: adjacency.remove(&e.edge).unwrap_or_default(),
            vertices: vertices.into_iter().collect(),
        });
    }
    let vertices: Vec<_> = ids
        .vertices
        .iter()
        .map(|(v, id)| VertexInfo {
            id: *id,
            position: model.geom().approx_point(model.vertex(*v).point).into(),
            edges: edges.iter().filter(|e| e.vertices.contains(id)).map(|e| e.id).collect(),
        })
        .collect();
    Ok(Topology {
        kind: BodyKind::Solid,
        faces,
        edges,
        vertex_count: vertices.len(),
        vertices,
        bbox: bounds(mesh.positions.iter().copied().map(Vec3::from), chord),
        display_chord_height: Some(chord),
    })
}

pub(super) fn mesh(model: &Model, mesh: &catallax_mesh::Mesh, ids: &Ids, params: &TessellationParams) -> Mesh {
    let mut out = Mesh::default();
    for (value, name) in [
        (params.curve_angle_deg, "curve_angle_deg"),
        (params.surface_angle_deg, "surface_angle_deg"),
        (params.max_edge_length, "max_edge_length"),
        (params.max_aspect_ratio, "max_aspect_ratio"),
    ] {
        if value > 0.0 {
            out.unavailable_controls
                .push(format!("{name}: Catallax stage 1 display entry supports chord tolerance only"));
        }
    }
    let mut vertices = BTreeMap::new();
    for triangle in &mesh.triangles {
        let points = triangle.vertices.map(|v| Vec3::from(mesh.positions[v]));
        let fallback = (points[1] - points[0]).cross(points[2] - points[0]).normalized().unwrap_or(Vec3::Z);
        let f = model.face(triangle.face);
        for (vertex, point) in triangle.vertices.into_iter().zip(points) {
            // Share vertices within each face while keeping sharp-face normals separate.
            if let Some(index) = vertices.get(&(triangle.face, vertex)) {
                out.indices.push(*index);
                continue;
            }
            vertices.insert((triangle.face, vertex), out.positions.len() as u32);
            let real = |v: f64| {
                catallax_numbers::Real::from_rational(
                    catallax_numbers::Rational::from_f64_exact(v).expect("finite Catallax mesh coordinate"),
                )
            };
            let p = catallax_shapes::Point3::new(real(point.x), real(point.y), real(point.z));
            let gradient = model.geom().surface(f.surface).gradient(&p);
            let n =
                vector(&gradient).normalized().map(|n| if f.sense == Sense::Same { n } else { -n }).unwrap_or(fallback);
            out.indices.push(out.positions.len() as u32);
            out.positions.push([point.x as f32, point.y as f32, point.z as f32]);
            out.normals.push([n.x as f32, n.y as f32, n.z as f32]);
        }
        out.triangle_faces.push(ids.faces[&triangle.face]);
    }
    out
}
