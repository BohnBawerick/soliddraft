//! The open part as the drawing sees it: its displayed body as a `soliddraft-drawing`
//! [`Part`], named through the part's own persistent references, with the words the
//! interface uses for its entities, its hole callouts and its feature dimensions.

use crate::document::{Document, FeatureKind};
use soliddraft_drawing::library::{self, Symbols};
use soliddraft_drawing::measure::format_value;
use soliddraft_drawing::project::{CircleInfo, Projected};
use soliddraft_drawing::{ContentHash, DocumentId};
use soliddraft_drawing::{
    Entity, EntityKind, EntityRef, ModelDimension, ModelEntity, ModelMeasure, Namer, Part, Resolution, feature_name,
};
use soliddraft_kernel_api::{BodyId, EdgeId, FaceId, Kernel, Point2, Topology, Vec3};
use std::sync::Arc;

/// The part's displayed body with what the drawing needs to know about it.
pub struct PartContext<'a> {
    pub doc: &'a Document,
    pub kernel: Arc<dyn Kernel>,
    pub body: BodyId,
    pub topology: Topology,
    /// The part's id.
    pub id: DocumentId,
    /// The part's revision now, unsaved edits included.
    pub digest: ContentHash,
    pub name: String,
}

impl<'a> PartContext<'a> {
    /// The context for the part's displayed body, or `None` without a body.
    pub fn new(doc: &'a Document) -> Option<PartContext<'a>> {
        let body = doc.display_body()?;
        let kernel = doc.kernel().clone();
        let topology = kernel.topology(body).ok()?;
        Some(PartContext {
            doc,
            kernel,
            body,
            topology,
            id: doc.model().id(),
            digest: digest(doc),
            name: doc.name.clone(),
        })
    }

    /// The part as `soliddraft-drawing` takes it.
    pub fn part(&self) -> Part<'_> {
        Part {
            kernel: self.kernel.as_ref(),
            body: self.body,
            digest: self.digest.clone(),
            dimensions: feature_dimensions(self.doc),
            namer: Some(self),
        }
    }

    fn feature_name(&self, id: u64) -> String {
        self.doc.feature(id).map(|f| f.name.clone()).unwrap_or_else(|| format!("Feature {id}"))
    }

    /// The feature an entity came from: its id, its name, and whether it is still there
    /// and not suppressed. A feature that is gone is named by what the reference
    /// remembered of it.
    pub fn feature_of(&self, entity: &EntityRef) -> Option<(u64, String, bool)> {
        let id = match &entity.model {
            Some(ModelEntity::Edge(e)) => Some(u64::from(e.faces[0].feature.0)),
            Some(ModelEntity::Face(f)) => Some(u64::from(f.feature.0)),
            None => soliddraft_model::feature_of_tag(&entity.feature).map(|f| u64::from(f.0)),
        }?;
        match self.doc.feature(id) {
            Some(f) if !f.suppressed => Some((id, f.name.clone(), true)),
            _ => Some((id, feature_name(entity), false)),
        }
    }
}

impl Namer for PartContext<'_> {
    /// The model's persistent name for the edge, with the words the interface uses
    /// ("Plate · Edge 3"), so a reference can still be described after its feature is
    /// gone.
    fn edge_ref(&self, id: EdgeId) -> Option<EntityRef> {
        let e = self.topology.edges.iter().find(|e| e.id == id)?;
        let model = self.doc.model().edge_ref(id).ok();
        let named = self.doc.named_edge(id);
        let feature =
            named.map(|n| n.feature).or_else(|| model.as_ref().map(|m| u64::from(m.faces[0].feature.0))).unwrap_or(0);
        let label = named.map(|n| n.label.clone()).unwrap_or_else(|| "Edge".into());
        let role = format!("{} · {label}", self.feature_name(feature));
        let mut r = EntityRef::new(EntityKind::Edge, &format!("f{feature}"), &role, 0, e.midpoint, e.length);
        r.model = model.map(ModelEntity::Edge);
        Some(r)
    }

    fn face_ref(&self, id: FaceId) -> Option<EntityRef> {
        let f = self.topology.faces.iter().find(|f| f.id == id)?;
        let model = self.doc.model().face_ref(id).ok();
        let named = self.doc.named_face(id);
        let feature = named.map(|n| n.feature).or_else(|| model.as_ref().map(|m| u64::from(m.feature.0))).unwrap_or(0);
        let label = named.map(|n| n.label.clone()).unwrap_or_else(|| "Face".into());
        let role = format!("{} · {label}", self.feature_name(feature));
        let ordinal = model.as_ref().map(|m| m.ordinal).unwrap_or(0);
        let mut r = EntityRef::new(EntityKind::Face, &format!("f{feature}"), &role, ordinal, f.centroid, f.area);
        r.model = model.map(ModelEntity::Face);
        Some(r)
    }

    /// Resolves through the model's persistent naming: the feature and generator of the
    /// faces, checked by fingerprint. A reference whose feature is gone or suppressed is
    /// lost with that reason.
    fn resolve(&self, r: &EntityRef) -> Option<Resolution> {
        let what = match r.kind {
            EntityKind::Edge => "edge",
            EntityKind::Face => "face",
            EntityKind::Vertex => "vertex",
        };
        if let Some((_, name, false)) = self.feature_of(r) {
            return Some(Resolution::Lost(format!("the {what} came from {name}, which no longer exists")));
        }
        let found = match r.model.as_ref()? {
            ModelEntity::Edge(e) => self.doc.model().resolve_edge(e, self.body).map(Entity::Edge),
            ModelEntity::Face(f) => self.doc.model().resolve_face(f, self.body).map(Entity::Face),
        };
        Some(match found {
            Ok(e) => Resolution::Found(e),
            Err(repair) => Resolution::Lost(repair.message),
        })
    }

    /// "4× Ø9 THRU", "Ø12 DEEP 20" or "Ø24": holes of the same cut and size are counted
    /// by centre, since the far end of a hole lies under its near end face-on.
    fn hole_callout(&self, circle: &CircleInfo, projected: &Projected, symbols: &Symbols) -> Option<String> {
        let diameter = trim(format_value(circle.model_radius * 2.0, 2));
        let feature = self.feature_of(&circle.entity).and_then(|(id, _, _)| self.doc.feature(id));
        let mut centres: Vec<Point2> = Vec::new();
        for c in projected.circles.iter().filter(|c| {
            c.entity.feature == circle.entity.feature && (c.model_radius - circle.model_radius).abs() < 1e-6
        }) {
            if !centres.iter().any(|k| (k.u - c.centre.u).abs() < 1e-3 && (k.v - c.centre.v).abs() < 1e-3) {
                centres.push(c.centre);
            }
        }
        let count = if centres.len() > 1 { format!("{}{} ", centres.len(), symbols.times) } else { String::new() };
        Some(match feature.map(|f| &f.kind) {
            Some(FeatureKind::Cut(p)) if p.through_all => {
                format!("{count}{}{diameter} {}", symbols.diameter, symbols.through)
            }
            Some(FeatureKind::Cut(p)) => {
                format!("{count}{}{diameter} {} {}", symbols.diameter, symbols.depth, trim(format_value(p.depth, 2)))
            }
            _ => format!("{count}{}{diameter}", symbols.diameter),
        })
    }
}

fn trim(s: String) -> String {
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_owned() } else { s }
}

/// The hole callout of a circle in the drawing's own style.
pub fn hole_callout(ctx: &PartContext<'_>, circle: &CircleInfo, projected: &Projected, template: &str) -> String {
    let symbols = &library::template(template).style_file().symbols;
    ctx.hole_callout(circle, projected, symbols).unwrap_or_default()
}

/// The part's revision: its semantic hash, equal while nothing about the part changed.
pub fn digest(doc: &Document) -> ContentHash {
    doc.model().semantic_hash()
}

/// The dimensions the part's features define, for Model items: each extrusion's depth
/// along its sketch normal, and the diameter of every circle its extrusions and cuts
/// were sketched with.
pub fn feature_dimensions(doc: &Document) -> Vec<ModelDimension> {
    let mut out = Vec::new();
    for f in doc.features.iter().filter(|f| !f.suppressed) {
        let (plane, circles) = match &f.kind {
            FeatureKind::Extrude(p) => {
                let plane = match p.sketch {
                    Some(s) => doc.feature(s).and_then(|sf| sf.plane),
                    None => Some(p.plane.to_kernel()),
                };
                let Some(plane) = plane else { continue };
                let normal = if p.flip { -plane.normal } else { plane.normal };
                out.push(ModelDimension {
                    feature: f.name.clone(),
                    name: "depth".into(),
                    measure: ModelMeasure::Distance {
                        axis: normal,
                        from: plane.origin,
                        to: plane.origin + normal * p.depth,
                        sketch_normal: None,
                    },
                    count: 1,
                });
                let circles = match (p.sketch, &p.profile) {
                    (Some(s), _) => sketch_circles(doc, s),
                    (None, crate::document::ProfileKind::Circle { diameter }) => vec![(plane.origin, diameter / 2.0)],
                    _ => Vec::new(),
                };
                (plane, circles)
            }
            FeatureKind::Cut(p) => {
                let Some(plane) = doc.feature(p.sketch).and_then(|sf| sf.plane) else { continue };
                (plane, sketch_circles(doc, p.sketch))
            }
            _ => continue,
        };
        for (center, radius) in circles {
            out.push(ModelDimension {
                feature: f.name.clone(),
                name: "diameter".into(),
                measure: ModelMeasure::Diameter { center, normal: plane.normal, radius },
                count: 1,
            });
        }
    }
    out
}

/// The circles of a sketch feature, as model centre and radius.
fn sketch_circles(doc: &Document, sketch: u64) -> Vec<(Vec3, f64)> {
    let Some(f) = doc.feature(sketch) else { return Vec::new() };
    let (Some(plane), Some(s)) = (f.plane, f.sketch()) else { return Vec::new() };
    s.sketch
        .drawn()
        .filter_map(|e| match e.kind {
            crate::sketch::EntityKind::Circle { center, radius } => {
                Some((plane.to_world(Point2::new(center.u, center.v)), radius))
            }
            _ => None,
        })
        .collect()
}
