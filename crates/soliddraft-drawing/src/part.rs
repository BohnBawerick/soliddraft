//! The part a drawing is made from, and how the drawing names its entities.
//!
//! A [`Part`] is a body in a kernel session plus what the drawing needs to know about
//! the part's definition: its digest and the dimensions of its features. A [`PartIndex`]
//! reads the body's topology, history and exact edge curves once and answers the two
//! questions associativity asks: what to call an entity ([`PartIndex::edge_ref`]) and
//! which entity a stored name means after a rebuild ([`PartIndex::resolve`]).
//!
//! A part made in the app carries a [`Namer`]: its model document names entities with
//! its persistent references ([`EntityRef::model`]) and resolves them after a rebuild.
//! Without one, as for a recipe, names follow the fingerprint scheme: the history tag of
//! the operation that generated the entity, the entity's geometric class as its role,
//! and its midpoint (or centroid) and length (or area). Kernel ids never leave a
//! session.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use soliddraft_document::ConfigurationId;
use soliddraft_kernel_api::drawing::Curve3;
use soliddraft_kernel_api::{BodyId, EdgeId, FaceId, HistoryRecord, Kernel, SurfaceKind, Vec3};

use crate::entity::{EntityKind, EntityRef};
use crate::model_items::ModelDimension;
use crate::{ContentHash, DrawingError};

/// The part a drawing shows: a body and what its definition says about it.
pub struct Part<'k> {
    /// The kernel session that owns the body.
    pub kernel: &'k dyn Kernel,
    /// The part's body.
    pub body: BodyId,
    /// The part's revision: the semantic hash of its definition. A view made from
    /// another revision is out of date.
    pub digest: ContentHash,
    /// The dimensions of the part's features, for Model items.
    pub dimensions: Vec<ModelDimension>,
    /// The part document's own names for its entities, when it has them.
    pub namer: Option<&'k dyn Namer>,
    /// Draw the views from the kernel's unproved preview mesh, for the screen. Line work
    /// for an export leaves it off.
    pub preview: bool,
    /// The configuration and text properties this revision resolves to.
    pub source: PartSource,
}

/// What a drawing records of its part, beside the revision.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PartSource {
    /// The selected configuration; `None` for Default.
    pub configuration: Option<ConfigurationId>,
    /// Text properties, overrides over file defaults.
    pub properties: BTreeMap<String, String>,
}

/// What a part document knows about its entities beyond the kernel: persistent names,
/// the words its interface uses for them, and its hole features.
pub trait Namer {
    /// The name of an edge of the body, or `None` to use the fingerprint name.
    fn edge_ref(&self, edge: EdgeId) -> Option<EntityRef>;

    /// The name of a face of the body, or `None` to use the fingerprint name.
    fn face_ref(&self, face: FaceId) -> Option<EntityRef>;

    /// Finds the entity a reference with a model name ([`EntityRef::model`]) names, or
    /// `None` when the reference is not one of this document's.
    fn resolve(&self, r: &EntityRef) -> Option<Resolution>;

    /// The callout of a hole whose edge a view sees face-on, such as "4× Ø9 THRU", in
    /// the drawing's symbols, or `None` when the circle is not a hole.
    fn hole_callout(
        &self,
        circle: &crate::project::CircleInfo,
        projected: &crate::project::Projected,
        symbols: &crate::library::Symbols,
    ) -> Option<String> {
        let _ = (circle, projected, symbols);
        None
    }
}

/// A model entity in the current body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Entity {
    /// An edge.
    Edge(EdgeId),
    /// A face.
    Face(FaceId),
}

/// What a stored reference means in the current body.
#[derive(Clone, Debug, PartialEq)]
pub enum Resolution {
    /// Exactly one entity matches.
    Found(Entity),
    /// No entity matches, or several match equally well.
    Lost(String),
}

#[derive(Clone, Debug)]
struct IndexedEdge {
    id: EdgeId,
    curve: Curve3,
    origin: String,
    role: &'static str,
    point: Vec3,
    size: f64,
}

#[derive(Clone, Debug)]
struct IndexedFace {
    id: FaceId,
    origin: String,
    role: &'static str,
    point: Vec3,
    size: f64,
}

/// The body's entities with their names and exact curves.
#[derive(Clone)]
pub struct PartIndex<'k> {
    edges: Vec<IndexedEdge>,
    faces: Vec<IndexedFace>,
    /// Every history tag that produced the body.
    tags: BTreeSet<String>,
    /// Size of the body, the scale of the matching tolerances.
    size: f64,
    /// The part document's names, when it has them.
    namer: Option<&'k dyn Namer>,
}

/// Tolerance for treating two candidates as equally good, relative to the body's size.
const TIE_TOL: f64 = 1e-6;

impl<'k> PartIndex<'k> {
    /// Reads the part's topology, history and edge curves.
    pub fn build(part: &Part<'k>) -> Result<PartIndex<'k>, DrawingError> {
        let mut index = PartIndex::of_body(part.kernel, part.body)?;
        index.namer = part.namer;
        Ok(index)
    }

    /// Reads a body's topology, history and edge curves, naming by fingerprint.
    pub fn of_body(kernel: &dyn Kernel, body: BodyId) -> Result<PartIndex<'k>, DrawingError> {
        let topology = kernel.topology(body)?;
        let history = kernel.history(body)?;
        let edge_origin = origins(&history, |r| r.edges.generated().collect());
        let face_origin = origins(&history, |r| r.faces.generated().collect());
        let mut edges = Vec::with_capacity(topology.edges.len());
        for e in &topology.edges {
            let curve = kernel.edge_curve(body, e.id)?;
            edges.push(IndexedEdge {
                id: e.id,
                role: curve_role(&curve),
                curve,
                origin: edge_origin.get(&e.id).cloned().unwrap_or_default(),
                point: e.midpoint,
                size: e.length,
            });
        }
        let faces = topology
            .faces
            .iter()
            .map(|f| IndexedFace {
                id: f.id,
                origin: face_origin.get(&f.id).cloned().unwrap_or_default(),
                role: surface_role(f.surface),
                point: f.centroid,
                size: f.area,
            })
            .collect();
        Ok(PartIndex {
            edges,
            faces,
            tags: history.iter().map(|r| r.tag.as_str().to_owned()).collect(),
            size: topology.bbox.diagonal().max(1.0),
            namer: None,
        })
    }

    /// The part document's names, when the part has them.
    pub fn namer(&self) -> Option<&'k dyn Namer> {
        self.namer
    }

    /// The exact curve of an edge of the body.
    pub fn edge_curve(&self, id: EdgeId) -> Option<&Curve3> {
        self.edges.iter().find(|e| e.id == id).map(|e| &e.curve)
    }

    /// Every edge of the body with its exact curve.
    pub fn edges(&self) -> impl Iterator<Item = (EdgeId, &Curve3)> {
        self.edges.iter().map(|e| (e.id, &e.curve))
    }

    /// The name of an edge, or `None` when it is not on the body.
    pub fn edge_ref(&self, id: EdgeId) -> Option<EntityRef> {
        let e = self.edges.iter().find(|e| e.id == id)?;
        if let Some(mut r) = self.namer.and_then(|n| n.edge_ref(id)) {
            r.kernel_hint = Some(id.raw());
            return Some(r);
        }
        let ordinal = self.ordinal(EntityKind::Edge, &e.origin, e.role, e.point);
        let mut r = EntityRef::new(EntityKind::Edge, &e.origin, e.role, ordinal, round_vec(e.point), round(e.size));
        r.kernel_hint = Some(id.raw());
        Some(r)
    }

    /// The name of a face, or `None` when it is not on the body.
    pub fn face_ref(&self, id: FaceId) -> Option<EntityRef> {
        let f = self.faces.iter().find(|f| f.id == id)?;
        if let Some(mut r) = self.namer.and_then(|n| n.face_ref(id)) {
            r.kernel_hint = Some(id.raw());
            return Some(r);
        }
        let ordinal = self.ordinal(EntityKind::Face, &f.origin, f.role, f.point);
        let mut r = EntityRef::new(EntityKind::Face, &f.origin, f.role, ordinal, round_vec(f.point), round(f.size));
        r.kernel_hint = Some(id.raw());
        Some(r)
    }

    /// The name of an entity.
    pub fn entity_ref(&self, entity: Entity) -> Option<EntityRef> {
        match entity {
            Entity::Edge(id) => self.edge_ref(id),
            Entity::Face(id) => self.face_ref(id),
        }
    }

    /// Index among entities of the same origin and role, ordered by position, so a name
    /// reads "the second circle of hole1". Informative only: resolving does not use it.
    fn ordinal(&self, kind: EntityKind, origin: &str, role: &str, point: Vec3) -> u32 {
        let key = |p: Vec3| (round(p.x), round(p.y), round(p.z));
        let mut points: Vec<(f64, f64, f64)> = match kind {
            EntityKind::Edge => {
                self.edges.iter().filter(|e| e.origin == origin && e.role == role).map(|e| key(e.point)).collect()
            }
            _ => self.faces.iter().filter(|f| f.origin == origin && f.role == role).map(|f| key(f.point)).collect(),
        };
        points.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        points.iter().position(|p| *p == key(point)).unwrap_or(0) as u32
    }

    /// Finds the entity a stored reference names: among the entities of the same kind,
    /// origin and role, the one whose fingerprint is nearest. It is lost when there is
    /// none, or when the second best is about as near as the best.
    pub fn resolve(&self, r: &EntityRef) -> Resolution {
        if r.model.is_some()
            && let Some(found) = self.namer.and_then(|n| n.resolve(r))
        {
            return found;
        }
        let feature = feature_name(r);
        let what = match r.kind {
            EntityKind::Edge => "edge",
            EntityKind::Face => "face",
            EntityKind::Vertex => "vertex",
        };
        if !r.feature.is_empty() && !self.tags.contains(&r.feature) {
            return Resolution::Lost(format!("the {what} came from {feature}, which no longer exists"));
        }
        let score = |point: Vec3, size: f64| point.distance(r.fingerprint.point) + (size - r.fingerprint.size).abs();
        let mut scored: Vec<(f64, Entity)> = match r.kind {
            EntityKind::Edge => self
                .edges
                .iter()
                .filter(|e| e.origin == r.feature && e.role == r.role)
                .map(|e| (score(e.point, e.size), Entity::Edge(e.id)))
                .collect(),
            EntityKind::Face => self
                .faces
                .iter()
                .filter(|f| f.origin == r.feature && f.role == r.role)
                .map(|f| (score(f.point, f.size), Entity::Face(f.id)))
                .collect(),
            EntityKind::Vertex => Vec::new(),
        };
        scored.sort_by(|a, b| a.0.total_cmp(&b.0));
        match scored.as_slice() {
            [] => Resolution::Lost(format!("no {} {what} of {feature} is left", r.role)),
            [(_, only)] => Resolution::Found(*only),
            [(best, e), (second, _), ..] => {
                if *second <= 2.0 * best + TIE_TOL * self.size {
                    Resolution::Lost(format!("several {} {what}s of {feature} match equally well", r.role))
                } else {
                    Resolution::Found(*e)
                }
            }
        }
    }
}

/// For each entity, the tag of the first record that generated it.
fn origins<Id: Copy + Eq + std::hash::Hash>(
    history: &[HistoryRecord],
    generated: impl Fn(&HistoryRecord) -> Vec<Id>,
) -> HashMap<Id, String> {
    let mut out = HashMap::new();
    for r in history {
        for id in generated(r) {
            out.entry(id).or_insert_with(|| r.tag.as_str().to_owned());
        }
    }
    out
}

/// The role a fingerprint name gives an edge: its geometric class.
pub(crate) fn curve_role(curve: &Curve3) -> &'static str {
    match curve {
        Curve3::Line { .. } => "line",
        Curve3::Arc { .. } => "arc",
        Curve3::Circle { .. } => "circle",
        Curve3::Nurbs { .. } | Curve3::Polyline { .. } => "curve",
    }
}

fn surface_role(kind: SurfaceKind) -> &'static str {
    match kind {
        SurfaceKind::Plane => "plane",
        SurfaceKind::Cylinder => "cylinder",
        SurfaceKind::Cone => "cone",
        SurfaceKind::Sphere => "sphere",
        SurfaceKind::Torus => "torus",
        SurfaceKind::Spline => "spline",
        SurfaceKind::Other => "surface",
    }
}

/// Rounds to 0.1 micrometre so stored names stay short and stable across runs.
pub(crate) fn round(x: f64) -> f64 {
    let r = (x * 1e4).round() / 1e4;
    if r == 0.0 { 0.0 } else { r }
}

pub(crate) fn round_vec(p: Vec3) -> Vec3 {
    Vec3::new(round(p.x), round(p.y), round(p.z))
}

/// The name of the feature a reference came from, in the words of the part's interface
/// when the reference carries them ("Slot · Side" gives "Slot").
pub fn feature_name(r: &EntityRef) -> String {
    match r.role.split_once(" · ") {
        Some((feature, _)) => feature.to_owned(),
        None if r.feature.is_empty() => "an unknown feature".to_owned(),
        None => r.feature.clone(),
    }
}
