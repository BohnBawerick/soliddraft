//! Conversion of Catallax names, exact definitions and certified display data.

use std::collections::{BTreeMap, BTreeSet};

use catallax_app::EvaluatedApplicationBody;
use catallax_recipe::{Origin, Role};
use catallax_shapes::{CurveKind as CatCurve, Surface, SurfaceKind as CatSurface};
use catallax_topology::Sense;
use soliddraft_kernel_api::*;

use crate::identity::{FaceKey, Keys, NamedEdgeKey, VertexKey, same_surface};
use crate::query_error;

type CatFace = catallax_topology::FaceId;
type CatEdge = catallax_topology::EdgeId;
type CatVertex = catallax_topology::VertexId;

pub(super) struct Ids {
    pub faces: BTreeMap<CatFace, FaceId>,
    pub edges: BTreeMap<CatEdge, EdgeId>,
    pub vertices: BTreeMap<CatVertex, VertexId>,
    face_keys: BTreeMap<FaceId, FaceKey>,
    edge_keys: BTreeMap<EdgeId, NamedEdgeKey>,
    vertex_keys: BTreeMap<VertexId, VertexKey>,
    origins: BTreeMap<FaceId, Vec<Origin>>,
    /// The same origins in the interface's terms, by the caller's operation tags.
    pub face_origins: BTreeMap<FaceId, Vec<soliddraft_kernel_api::Origin>>,
    /// Each Catallax origin with its interface origin, kept for later bodies: a blend's
    /// edge is gone once the fillet has built, but its faces keep the name.
    named: Vec<(Origin, soliddraft_kernel_api::Origin)>,
    edge_names: BTreeMap<EdgeId, String>,
}

fn assign<I: Copy + Ord, K>(
    key: &K,
    same: impl Fn(&K, &K) -> bool,
    parents: &[&BTreeMap<I, K>],
    next: &mut u64,
    wrap: impl Fn(u64) -> I,
) -> I {
    if let Some(id) = parents.iter().flat_map(|p| p.iter()).find_map(|(id, value)| same(value, key).then_some(*id)) {
        return id;
    }
    *next += 1;
    wrap(*next)
}

impl Ids {
    pub fn new(
        app: &EvaluatedApplicationBody,
        parents: &[&Ids],
        tags: &BTreeMap<String, OpTag>,
        next: &mut u64,
    ) -> Self {
        let mut keys = Keys::new(app);
        let mut ids = Self {
            faces: BTreeMap::new(),
            edges: BTreeMap::new(),
            vertices: BTreeMap::new(),
            face_keys: BTreeMap::new(),
            edge_keys: BTreeMap::new(),
            vertex_keys: BTreeMap::new(),
            origins: BTreeMap::new(),
            face_origins: BTreeMap::new(),
            named: Vec::new(),
            edge_names: BTreeMap::new(),
        };
        for face in app.names.faces.keys() {
            let origins = app.names.faces[face].clone();
            let key = keys.faces.remove(face).expect("every named face has a key");
            let parent_keys = parents.iter().map(|p| &p.face_keys).collect::<Vec<_>>();
            let id = assign(&key, FaceKey::same, &parent_keys, next, FaceId::from_raw);
            ids.faces.insert(*face, id);
            ids.face_keys.insert(id, key);
            // An origin whose role the interface cannot express is left out, never guessed.
            let mut named = Vec::new();
            for o in &origins {
                let Some(n) = named_origin(o, parents, tags) else {
                    continue;
                };
                if !ids.named.iter().any(|(c, _)| c == o) {
                    ids.named.push((o.clone(), n.clone()));
                }
                named.push(n);
            }
            named.sort();
            named.dedup();
            ids.face_origins.insert(id, named);
            ids.origins.insert(id, origins);
        }
        for (edge, name) in &app.names.edges {
            let key = keys.edges.remove(edge).expect("every named edge has a key");
            let parent_keys = parents.iter().map(|p| &p.edge_keys).collect::<Vec<_>>();
            let id = assign(&key, NamedEdgeKey::same, &parent_keys, next, EdgeId::from_raw);
            ids.edges.insert(*edge, id);
            ids.edge_keys.insert(id, key);
            if let Some(name) = name {
                ids.edge_names.insert(id, serde_json::to_string(name).expect("edge name serializes"));
            }
        }
        for vertex in app.names.vertices.keys() {
            let key = keys.vertices.remove(vertex).expect("every named vertex has a key");
            let parent_keys = parents.iter().map(|p| &p.vertex_keys).collect::<Vec<_>>();
            let id = assign(&key, VertexKey::same, &parent_keys, next, VertexId::from_raw);
            ids.vertices.insert(*vertex, id);
            ids.vertex_keys.insert(id, key);
        }
        ids
    }

    pub fn record(&self, seq: u64, tag: &OpTag, op: OpKind, step: &str, parents: &[&Ids]) -> HistoryRecord {
        // A drafted face keeps its source origins on a new support, so it descends from
        // the face it tilts. Elsewhere a new support means a new face.
        let tilts = op == OpKind::Draft;
        let faces = changes(
            self.face_keys.keys().copied(),
            parents.iter().flat_map(|p| p.face_keys.keys().copied()),
            |old, new| {
                parents.iter().any(|p| {
                    p.origins.get(&old).is_some_and(|os| self.origins[&new].iter().any(|o| os.contains(o)))
                        && (tilts
                            || p.face_keys
                                .get(&old)
                                .is_some_and(|k| same_surface(&k.support, &self.face_keys[&new].support)))
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
                    .and_then(|o| face_role(&o.role, parents))
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

pub(super) fn named_origin(
    origin: &Origin,
    parents: &[&Ids],
    tags: &BTreeMap<String, OpTag>,
) -> Option<soliddraft_kernel_api::Origin> {
    parents
        .iter()
        .find_map(|parent| parent.named.iter().find_map(|(source, named)| (source == origin).then_some(named)))
        .cloned()
        .or_else(|| {
            Some(soliddraft_kernel_api::Origin {
                tag: tags.get(origin.step.as_str())?.clone(),
                role: face_role(&origin.role, parents)?,
            })
        })
}

/// The interface's role for a Catallax role, when the interface has one.
fn face_role(role: &Role, parents: &[&Ids]) -> Option<FaceRole> {
    match role {
        Role::Start => Some(FaceRole::StartCap),
        Role::End => Some(FaceRole::EndCap),
        Role::Side { segment, .. } => segment
            .as_str()
            .strip_prefix('l')
            .and_then(|s| s.split_once('s'))
            .and_then(|(a, b)| Some(FaceRole::Side { loop_index: a.parse().ok()?, segment: b.parse().ok()? })),
        Role::Hole { at, part } => at
            .as_str()
            .strip_prefix('h')
            .and_then(|i| i.parse().ok())
            .map(|placement| FaceRole::Hole { placement, part: hole_part(*part) }),
        Role::Blend { edge } | Role::Bevel { edge } => {
            let name = serde_json::to_string(edge).expect("edge name serializes");
            parents
                .iter()
                .find_map(|p| p.edge_names.iter().find_map(|(id, n)| (*n == name).then_some(*id)))
                .map(|edge| FaceRole::Blend { edge })
        }
        Role::Inner { of } => parents
            .iter()
            .find_map(|p| p.origins.iter().find_map(|(id, os)| os.contains(of).then_some(*id)))
            .map(|face| FaceRole::Offset { face }),
        Role::Copy { instance, of } => parents
            .iter()
            .find_map(|p| p.origins.iter().find_map(|(id, os)| os.contains(of).then_some(*id)))
            .map(|face| FaceRole::Copy { instance: *instance, face }),
        _ => None,
    }
}

fn hole_part(part: catallax_recipe::HolePart) -> HolePart {
    use catallax_recipe::HolePart as P;
    match part {
        P::Entry => HolePart::Entry,
        P::CounterboreWall => HolePart::CounterboreWall,
        P::CounterboreFloor => HolePart::CounterboreFloor,
        P::Countersink => HolePart::Countersink,
        P::Wall => HolePart::Wall,
        P::Floor => HolePart::Floor,
        P::Tip => HolePart::Tip,
    }
}

pub(super) fn midpoint(value: &catallax_measure::Enclosure) -> f64 {
    ((&value.lo + &value.hi) / catallax_numbers::Rational::from_i64(2)).to_f64_nearest()
}

/// A value to a double's precision. `Real::to_f64` is the midpoint of a cached enclosure,
/// kept for display, and for an irrational such as a draft's slope it is wrong in its
/// first digits.
fn nearest(value: &catallax_numbers::Real) -> Result<f64, catallax_numbers::Refusal> {
    use catallax_numbers::{Decider, Rational};
    match value.rational_ref() {
        Some(exact) => Ok(exact.to_f64_nearest()),
        None => {
            let width = Rational::from_i64(1) / Rational::from_i64(1 << 60);
            Ok(Decider::default().enclose(value, &width)?.mid().to_f64_nearest())
        }
    }
}

/// A plane's origin and normal, to a double's precision.
fn plane_vectors(plane: &catallax_shapes::Plane) -> Result<(Vec3, Vec3), catallax_numbers::Refusal> {
    let (o, n) = (&plane.origin, &plane.normal);
    Ok((
        Vec3::new(nearest(&o.x)?, nearest(&o.y)?, nearest(&o.z)?),
        Vec3::new(nearest(&n.x)?, nearest(&n.y)?, nearest(&n.z)?),
    ))
}

fn surface_kind(surface: &Surface) -> SurfaceKind {
    match surface.kind() {
        CatSurface::Plane => SurfaceKind::Plane,
        CatSurface::Cylinder => SurfaceKind::Cylinder,
        CatSurface::Cone => SurfaceKind::Cone,
        CatSurface::Sphere => SurfaceKind::Sphere,
        CatSurface::Torus => SurfaceKind::Torus,
        CatSurface::BSpline => SurfaceKind::Spline,
        _ => SurfaceKind::Other,
    }
}

// A fitted blend's rails and the faces they trim have no single-entity proof.
// Those entities keep their display estimates, as an unproven topology does.
fn display_only(error: &catallax_numbers::Refusal) -> bool {
    error.code == "measure.unsupported-surface"
}

// A face's display area and area-weighted centre.
fn display_face(mesh: &catallax_mesh::Mesh, triangles: &[catallax_mesh::Triangle]) -> (f64, Vec3) {
    let mut weight_sum = 0.0;
    let mut center = Vec3::ZERO;
    for t in triangles {
        let [a, b, c] = t.vertices.map(|v| Vec3::from(mesh.positions[v]));
        let weight = (b - a).cross(c - a).length() * 0.5;
        weight_sum += weight;
        center = center + (a + b + c) * (weight / 3.0);
    }
    (weight_sum, center * (1.0 / weight_sum))
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

fn enclosed(value: &[catallax_measure::Enclosure; 3]) -> Vec3 {
    Vec3::from(value.each_ref().map(midpoint))
}

fn proven_box(b: &catallax_measure::BoundingBox) -> Aabb {
    let f = |v: &[catallax_numbers::Rational; 3]| Vec3::from(v.each_ref().map(|x| x.to_f64_nearest()));
    Aabb { min: f(&b.min), max: f(&b.max) }
}

/// Faces, edges and vertices. Face areas are proven. Without `proven`, centroids, face
/// and body boxes, edge lengths and midpoints are read from the display mesh, so a
/// rebuild or a display never waits for a measurement. With it, they are Catallax's
/// proven measures, for an explicit measurement request. Edge boxes are the edge's
/// display polyline grown by its chord error either way, so they enclose the edge.
pub(super) fn topology(
    app: &EvaluatedApplicationBody,
    ids: &Ids,
    mesh: &catallax_mesh::Mesh,
    proven: bool,
) -> KernelResult<Topology> {
    let model = &app.checkpoint.built().model;
    let chord = catallax_mesh::MeshOptions::default().chord_tolerance.to_f64_nearest();
    let accuracy = catallax_measure::Accuracy::default();
    let query = |name: &str, e: catallax_numbers::Refusal| query_error(name, e);
    let mut faces = Vec::new();
    let mut adjacency: BTreeMap<CatEdge, Vec<FaceId>> = BTreeMap::new();
    for face_mesh in &mesh.faces {
        let id = face_mesh.face;
        let face = model.face(id);
        let triangles = &mesh.triangles[face_mesh.triangles.clone()];
        let (display_area, display_center) = display_face(mesh, triangles);
        let area = match super::numerical("face_area", |dec| catallax_measure::face_area(model, id, &accuracy, dec)) {
            Ok(area) => midpoint(&area.value),
            Err(e) if display_only(&e) => display_area,
            Err(e) => return Err(query("face_area", e)),
        };
        let measured = if proven {
            let centroid =
                super::numerical("face_centroid", |dec| catallax_measure::face_centroid(model, id, &accuracy, dec));
            let bbox = super::numerical("face_bounding_box", |dec| {
                catallax_measure::face_bounding_box(model, id, &accuracy, dec)
            });
            match (centroid, bbox) {
                (Ok(centroid), Ok(bbox)) => Some((enclosed(&centroid.value), proven_box(&bbox.value))),
                (Err(e), _) | (_, Err(e)) if display_only(&e) => None,
                (Err(e), _) => return Err(query("face_centroid", e)),
                (_, Err(e)) => return Err(query("face_bounding_box", e)),
            }
        } else {
            None
        };
        let (centroid, bbox) = match measured {
            Some(measured) => measured,
            None => {
                if display_area <= 0.0 {
                    return Err(KernelError::QueryFailed {
                        query: "topology".into(),
                        code: 0,
                        message: "Catallax display face has no nondegenerate triangles (Catallax stage 1)".into(),
                    });
                }
                let bbox = bounds(triangles.iter().flat_map(|t| t.vertices).map(|v| mesh.positions[v].into()), chord);
                (display_center, bbox)
            }
        };
        let mut edges = Vec::new();
        for l in &face.loops {
            if let catallax_topology::Loop::Coedges(cs) = model.loop_(*l) {
                for c in cs {
                    let edge = model.coedge(*c).edge;
                    edges.push(ids.edges[&edge]);
                    adjacency.entry(edge).or_default().push(ids.faces[&id]);
                }
            }
        }
        let surface = model.geom().surface(face.surface);
        let plane = match surface {
            Surface::Plane(p) => Some(plane_vectors(p).map_err(|e| query("face_plane", e))?),
            _ => None,
        };
        let normal =
            plane.and_then(|(_, normal)| normal.normalized()).map(|n| if face.sense == Sense::Same { n } else { -n });
        // A sketch on a planar face takes its plane's offset from the centroid, so an
        // estimate is moved onto the face's exact plane: an offset of a hair would make
        // the sketch miss faces it should meet.
        let centroid = match (normal, plane) {
            (Some(n), Some((origin, _))) if !proven => centroid + n * (origin.dot(n) - centroid.dot(n)),
            _ => centroid,
        };
        faces.push(FaceInfo {
            id: ids.faces[&id],
            surface: surface_kind(surface),
            area,
            centroid,
            bbox,
            edges,
            normal,
            origins: ids.face_origins[&ids.faces[&id]].clone(),
        });
    }
    let mut edges = Vec::new();
    for e in &mesh.edges {
        let id = e.edge;
        let edge = model.edge(id);
        let points: Vec<Vec3> = e.vertices.iter().map(|v| mesh.positions[*v].into()).collect();
        let measured = if proven {
            let length =
                super::numerical("edge_length", |dec| catallax_measure::edge_length(model, id, &accuracy, dec));
            let middle =
                super::numerical("edge_midpoint", |dec| catallax_measure::edge_midpoint(model, id, &accuracy, dec));
            match (length, middle) {
                (Ok(length), Ok(middle)) => Some((midpoint(&length.value), enclosed(&middle.value))),
                (Err(e), _) | (_, Err(e)) if display_only(&e) => None,
                (Err(e), _) => return Err(query("edge_length", e)),
                (_, Err(e)) => return Err(query("edge_midpoint", e)),
            }
        } else {
            None
        };
        let (length, middle) = measured.unwrap_or_else(|| {
            let length = points.windows(2).map(|p| p[0].distance(p[1])).sum();
            let middle = if points.len() % 2 == 1 {
                points[points.len() / 2]
            } else {
                (points[points.len() / 2 - 1] + points[points.len() / 2]) * 0.5
            };
            (length, middle)
        });
        let curve = match model.geom().curve(edge.curve).kind() {
            CatCurve::Line => CurveKind::Line,
            CatCurve::Circle => CurveKind::Circle,
            CatCurve::Ellipse => CurveKind::Ellipse,
            _ => CurveKind::Other,
        };
        let vertices: BTreeSet<_> = [edge.start, edge.end].into_iter().flatten().map(|v| ids.vertices[&v]).collect();
        edges.push(EdgeInfo {
            id: ids.edges[&id],
            curve,
            length,
            midpoint: middle,
            bbox: bounds(points.into_iter(), e.max_chord_error.to_f64_nearest()),
            faces: adjacency.remove(&id).unwrap_or_default(),
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
    let measured = if proven {
        match super::numerical("bounding_box", |dec| {
            catallax_measure::bounding_box(model, app.checkpoint.built().result, &accuracy, dec)
        }) {
            Ok(bbox) => Some(bbox.value.map(|b| proven_box(&b)).unwrap_or(Aabb { min: Vec3::ZERO, max: Vec3::ZERO })),
            Err(e) if display_only(&e) => None,
            Err(e) => return Err(query("bounding_box", e)),
        }
    } else {
        None
    };
    let (bbox, display_chord_height) = match measured {
        Some(bbox) => (bbox, None),
        None => (bounds(mesh.positions.iter().copied().map(Vec3::from), chord), Some(chord)),
    };
    Ok(Topology {
        kind: BodyKind::Solid,
        faces,
        edges,
        vertex_count: vertices.len(),
        vertices,
        bbox,
        display_chord_height,
    })
}

pub(super) fn mesh(mesh: &catallax_mesh::Mesh, ids: &Ids, params: &TessellationParams) -> Mesh {
    let mut out = triangles(&mesh.positions, mesh.triangles.iter().map(|t| (t.vertices, ids.faces[&t.face])));
    if params.curve_angle_deg > 0.0 {
        out.unavailable_controls.push(
            "curve_angle_deg: Catallax bounds facet normals (surface_angle_deg) and chords, not the turn along an edge"
                .into(),
        );
    }
    for missing in &mesh.unavailable_controls {
        let name = match missing.control {
            catallax_mesh::MeshControl::AngularDeviation => "surface_angle_deg",
            catallax_mesh::MeshControl::EdgeLength => "max_edge_length",
            catallax_mesh::MeshControl::AspectRatio => "max_aspect_ratio",
        };
        out.unavailable_controls.push(format!("{name} on {}: {}", ids.faces[&missing.face], missing.reason));
    }
    out
}

/// An unproved preview for the screen, with no claimed bound on any control.
pub(super) fn preview(mesh: &catallax_mesh::PreviewMesh, ids: &Ids) -> Mesh {
    Mesh {
        unproved: true,
        ..triangles(&mesh.positions, mesh.triangles.iter().map(|t| (t.vertices, ids.faces[&t.face])))
    }
}

pub(super) fn triangles(positions: &[[f64; 3]], triangles: impl IntoIterator<Item = ([usize; 3], FaceId)>) -> Mesh {
    let mut out = Mesh::default();
    // Shading normals are display data, never a proof, so they come from the facets in
    // f64: each face's vertex takes the area-weighted sum of its triangles' normals.
    // Catallax winds every triangle toward its face's outward normal. Vertices are shared
    // within a face and kept apart across faces, so sharp edges stay sharp.
    let mut vertices = BTreeMap::new();
    let mut sums: Vec<Vec3> = Vec::new();
    for (triangle, face) in triangles {
        let points = triangle.map(|v| Vec3::from(positions[v]));
        let facet = (points[1] - points[0]).cross(points[2] - points[0]);
        for (vertex, point) in triangle.into_iter().zip(points) {
            let index = *vertices.entry((face, vertex)).or_insert_with(|| {
                out.positions.push([point.x as f32, point.y as f32, point.z as f32]);
                sums.push(Vec3::default());
                out.positions.len() as u32 - 1
            });
            sums[index as usize] = sums[index as usize] + facet;
            out.indices.push(index);
        }
        out.triangle_faces.push(face);
    }
    out.normals = sums
        .into_iter()
        .map(|sum| {
            let n = sum.normalized().unwrap_or(Vec3::Z);
            [n.x as f32, n.y as f32, n.z as f32]
        })
        .collect();
    out
}
