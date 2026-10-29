//! The part document: features in tree order, rebuilt through the kernel trait.

use std::collections::BTreeMap;
use std::sync::Arc;

use soliddraft_document::DocumentId;
use soliddraft_kernel_api::{BodyId, EdgeId, FaceId, Kernel, KernelResult, Plane, Topology};
use soliddraft_sketch::profile::Region;
use soliddraft_sketch::{EntityKind, Sketch, SolveStatus};
use soliddraft_tessellation::{Report, Settings};

use crate::command::UndoEntry;
use crate::evaluate::{Built, kernel_repair};
use crate::feature::{DatumPlane, Feature, FeatureId, FeatureKind, ProfileSource};
use crate::materials::PartMaterials;
use crate::rebuild::BuildCache;
use crate::reference::{
    self, EdgeRef, FaceRef, Generator, NameMap, NamedFace, NamingError, ReferenceSlot, RepairNeeded, View,
};

/// The state of a feature after the last rebuild.
#[derive(Clone, Debug, PartialEq)]
pub enum FeatureStatus {
    /// Built.
    Ok,
    /// Suppressed by the user; the body passes through unchanged.
    Suppressed,
    /// Below the rollback bar; not built.
    RolledBack,
    /// One or more references did not resolve to exactly one matching entity. The
    /// feature is not built and the body passes through unchanged.
    RepairNeeded(Vec<RepairNeeded>),
    /// An input feature, such as the sketch an extrusion sweeps, was not built because
    /// it needs a repair, failed or is suppressed. The repair is reported on that
    /// feature; this one builds again once it does.
    Blocked(FeatureId),
    /// The feature could not be built: bad parameters, a missing input, or a kernel
    /// failure. The body passes through unchanged.
    Failed(String),
}

impl FeatureStatus {
    /// True when the feature was built.
    pub fn is_ok(&self) -> bool {
        *self == FeatureStatus::Ok
    }
}

/// A sketch as the last rebuild placed it.
#[derive(Clone, Debug)]
pub struct BuiltSketch {
    /// The plane in model space.
    pub plane: Plane,
    /// True when the plane is a face of the part.
    pub on_face: bool,
    /// The closed regions, or why the sketch has none.
    pub regions: Result<Vec<Region>, String>,
}

/// How a face is named for people.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceLabel {
    /// The feature that generated the face.
    pub feature: FeatureId,
    /// The part of the feature that made it.
    pub generator: Generator,
    /// Rank among the feature's faces with the same generator.
    pub ordinal: u32,
    /// The face's part of the feature: "End cap", "Side of line 5".
    pub part: String,
    /// The feature's name and the part: "Boss-Extrude1 · End cap".
    pub text: String,
}

/// A part: an ordered feature list, its rebuild, its undo history.
///
/// The document never stores kernel state. It stores features and rebuilds them through
/// [`Kernel`]. A rebuild starts at the first changed feature, and every feature build is
/// kept by its exact inputs, so undo, redo and a value put back reuse the bodies built
/// for them; see [`crate::rebuild`].
pub struct Document {
    /// The document's identity, kept in its file's manifest.
    pub(crate) id: DocumentId,
    pub(crate) name: String,
    pub(crate) features: Vec<Feature>,
    pub(crate) datum_names: BTreeMap<DatumPlane, String>,
    /// The first feature below the rollback bar.
    pub(crate) rollback: Option<FeatureId>,
    pub(crate) next_id: u32,
    /// The material, the appearances and the copies of both the part holds.
    pub(crate) materials: PartMaterials,
    /// Viewport quality, export accuracy and whether to save display data: kept in the
    /// file's `settings.json`, outside the part's truth and hashes.
    pub(crate) settings: Settings,
    pub(crate) kernel: Arc<dyn Kernel>,
    pub(crate) built: Vec<Built>,
    /// Index of the first feature whose cached build is stale.
    pub(crate) dirty: usize,
    /// Feature builds by exact inputs; owns every body the builds made, shared with
    /// forks.
    pub(crate) cache: BuildCache,
    pub(crate) undo: Vec<UndoEntry>,
    pub(crate) redo: Vec<UndoEntry>,
    pub(crate) log: Vec<String>,
    /// The solver's verdict on the last sketch a command solved.
    pub(crate) last_solve: Option<SolveStatus>,
    pub(crate) journal: Option<soliddraft_document::journal::Identity>,
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document")
            .field("name", &self.name)
            .field("features", &self.features)
            .field("rollback", &self.rollback)
            .finish_non_exhaustive()
    }
}

impl Document {
    /// An empty part.
    pub fn new(name: impl Into<String>, kernel: Arc<dyn Kernel>) -> Self {
        Document {
            id: DocumentId::new(),
            name: name.into(),
            features: Vec::new(),
            datum_names: BTreeMap::new(),
            rollback: None,
            next_id: 1,
            materials: PartMaterials::default(),
            settings: Settings::default(),
            kernel,
            built: Vec::new(),
            dirty: 0,
            cache: BuildCache::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            log: Vec::new(),
            last_solve: None,
            journal: Some(Default::default()),
        }
    }

    /// The part's id: minted when the part is made, kept through renames, moves and
    /// edits.
    pub fn id(&self) -> DocumentId {
        self.id
    }

    /// Gives the part a new id, for Save as copy: the copy is a different document.
    pub fn new_identity(&mut self) {
        self.id = DocumentId::new();
    }

    /// The part's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The reference plane's display name. References store the datum, not this label.
    pub fn datum_name(&self, datum: DatumPlane) -> &str {
        self.datum_names.get(&datum).map(String::as_str).unwrap_or_else(|| datum.label())
    }

    /// Renames the part. Not an undoable command: the name is the file's, not the model's.
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    /// The part's display and accuracy settings.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Replaces the part's settings after checking them. Not an undoable command, and
    /// not a change to the part: its revision and geometry hash stay the same, so no
    /// assembly or drawing that pins it sees a change.
    pub fn set_settings(&mut self, settings: Settings) -> Result<(), String> {
        let journal = self.journal_command(|| serde_json::json!({"op":"settings", "settings":settings}));
        journal.finish((|| {
            settings.check()?;
            self.settings = settings;
            Ok(())
        })())
    }

    /// What the part's viewport quality gives its body's certified mesh: each level's
    /// deviation bound in millimetres and triangle count. The screen shows previews at
    /// the same controls, which claim no bound. `None` without a body.
    pub fn quality_report(&self) -> Option<KernelResult<Report>> {
        Some(soliddraft_tessellation::report(self.kernel.as_ref(), self.body()?, &self.settings.viewport))
    }

    /// Exports the part's body as `format`, meshes at the part's export tolerance. A part
    /// with a feature that did not build is refused, so no file shows a body the part does
    /// not have; suppressed and rolled-back features are fine.
    pub fn export(&self, format: soliddraft_exchange::Format) -> Result<soliddraft_exchange::Export, String> {
        if self.is_dirty() {
            return Err("the current part has not been rebuilt; wait for its rebuild before exporting".into());
        }
        if let Some(f) = self.features.iter().find(|f| {
            matches!(
                self.status(f.id),
                Some(FeatureStatus::RepairNeeded(_) | FeatureStatus::Blocked(_) | FeatureStatus::Failed(_))
            )
        }) {
            return Err(format!("{} did not build; repair or suppress it before exporting", f.name));
        }
        let body = self.body().ok_or("the part has no body to export")?;
        soliddraft_exchange::export(self.kernel.as_ref(), body, &self.name, format, &self.settings.export)
            .map_err(|e| e.to_string())
    }

    /// The kernel the document builds on.
    pub fn kernel(&self) -> &Arc<dyn Kernel> {
        &self.kernel
    }

    /// Every feature, in tree order.
    pub fn features(&self) -> &[Feature] {
        &self.features
    }

    /// A feature by id.
    pub fn feature(&self, id: FeatureId) -> Option<&Feature> {
        self.features.iter().find(|f| f.id == id)
    }

    /// A feature's position in the tree.
    pub fn index_of(&self, id: FeatureId) -> Option<usize> {
        self.features.iter().position(|f| f.id == id)
    }

    /// The first feature below the rollback bar, or `None` when every feature is active.
    pub fn rollback(&self) -> Option<FeatureId> {
        self.rollback
    }

    /// Number of features above the rollback bar.
    pub fn active_count(&self) -> usize {
        self.rollback.and_then(|id| self.index_of(id)).unwrap_or(self.features.len())
    }

    /// The command log: one line per executed, undone or redone command.
    pub fn log(&self) -> &[String] {
        &self.log
    }

    /// The solver's verdict on the last sketch a command solved.
    pub fn last_solve(&self) -> Option<&SolveStatus> {
        self.last_solve.as_ref()
    }

    pub(crate) fn built_index(&self, id: FeatureId) -> Option<usize> {
        self.index_of(id).filter(|&i| i < self.built.len())
    }

    /// A feature's state after the last rebuild.
    pub fn status(&self, id: FeatureId) -> Option<&FeatureStatus> {
        self.built_index(id).map(|i| &self.built[i].status)
    }

    /// Every feature's id and state, in tree order.
    pub fn statuses(&self) -> Vec<(FeatureId, &FeatureStatus)> {
        self.features.iter().zip(&self.built).map(|(f, b)| (f.id, &b.status)).collect()
    }

    /// The part body at the end of the tree (or at the rollback bar).
    pub fn body(&self) -> Option<BodyId> {
        self.built.last().and_then(|b| b.output)
    }

    /// The part body right after a feature.
    pub fn body_after(&self, id: FeatureId) -> Option<BodyId> {
        self.built_index(id).and_then(|i| self.built[i].output)
    }

    /// The part body a feature starts from.
    pub fn body_before(&self, id: FeatureId) -> Option<BodyId> {
        let i = self.built_index(id)?;
        i.checked_sub(1).and_then(|p| self.built[p].output)
    }

    /// Where a sketch lies and what regions it has, as the last rebuild placed it.
    pub fn built_sketch(&self, id: FeatureId) -> Option<&BuiltSketch> {
        self.built_index(id).and_then(|i| self.built[i].sketch.as_ref())
    }

    /// The names a feature defined for the faces it generated in the last rebuild.
    pub fn generated_faces(&self, id: FeatureId) -> &[NamedFace] {
        self.built_index(id).map(|i| self.built[i].names.as_slice()).unwrap_or(&[])
    }

    pub(crate) fn name_map(&self) -> NameMap<'_> {
        self.features.iter().zip(&self.built).map(|(f, b)| (f.id, b.names.as_slice())).collect()
    }

    /// The bodies of the last rebuild, latest first, without repeats.
    fn bodies(&self) -> Vec<BodyId> {
        let mut out: Vec<BodyId> = Vec::new();
        for b in self.built.iter().rev() {
            if let Some(body) = b.output
                && !out.contains(&body)
            {
                out.push(body);
            }
        }
        out
    }

    pub(crate) fn view_of(&self, body: BodyId) -> KernelResult<Topology> {
        self.kernel.topology(body)
    }

    /// The latest body of the rebuild that contains a face or edge.
    fn body_with(&self, has: impl Fn(&Topology) -> bool) -> Option<Topology> {
        self.bodies().into_iter().filter_map(|b| self.view_of(b).ok()).find(|t| has(t))
    }

    /// A persistent name for a face of the part, for a feature to store. The face may be
    /// in the final body or in the body after any feature.
    pub fn face_ref(&self, face: FaceId) -> Result<FaceRef, NamingError> {
        let t = self.body_with(|t| t.face(face).is_some()).ok_or(NamingError::NotInModel(face.to_string()))?;
        let mut r = reference::name_face(face, &View { topology: &t }, &self.name_map())?;
        r.label = self.face_label(face).map(|l| l.text).unwrap_or_default();
        Ok(r)
    }

    /// A persistent name for a planar face of the part to sketch on. Unlike
    /// [`face_ref`](Self::face_ref) it accepts a piece of a split face, since every piece
    /// of the face's origin lies in the same plane.
    pub fn plane_ref(&self, face: FaceId) -> Result<FaceRef, NamingError> {
        let t = self.body_with(|t| t.face(face).is_some()).ok_or(NamingError::NotInModel(face.to_string()))?;
        let mut r = reference::name_plane(face, &View { topology: &t }, &self.name_map())?;
        r.label = self.face_label(face).map(|l| l.text).unwrap_or_default();
        Ok(r)
    }

    /// A persistent name for an edge of the part, for a feature to store. Its label
    /// names the two faces it separates: "Base · End cap ∩ Flange · End cap".
    pub fn edge_ref(&self, edge: EdgeId) -> Result<EdgeRef, NamingError> {
        let t = self.body_with(|t| t.edge(edge).is_some()).ok_or(NamingError::NotInModel(edge.to_string()))?;
        let mut r = reference::name_edge(edge, &View { topology: &t }, &self.name_map())?;
        let faces = &t.edge(edge).expect("named edges are in the topology").faces;
        let labels: Vec<String> =
            faces.iter().map(|f| self.face_label(*f).map(|l| l.text).unwrap_or_else(|| f.to_string())).collect();
        r.label = if labels.len() == 2 && labels[0] == labels[1] {
            format!("{} · seam", labels[0])
        } else {
            labels.join(" ∩ ")
        };
        Ok(r)
    }

    /// Finds a face reference in a body, as a feature reading that body would.
    pub fn resolve_face(&self, r: &FaceRef, body: BodyId) -> Result<FaceId, RepairNeeded> {
        let t = self.view_of(body).map_err(|e| kernel_repair(ReferenceSlot::SketchPlane, e))?;
        reference::resolve_face(r, &View { topology: &t }, &self.name_map())
            .map_err(|u| u.into_repair(ReferenceSlot::SketchPlane))
    }

    /// Finds an edge reference in a body, as a feature reading that body would.
    pub fn resolve_edge(&self, r: &EdgeRef, body: BodyId) -> Result<EdgeId, RepairNeeded> {
        let slot = ReferenceSlot::Edge { index: 0 };
        let t = self.view_of(body).map_err(|e| kernel_repair(slot, e))?;
        reference::resolve_edge(r, &View { topology: &t }, &self.name_map()).map_err(|u| u.into_repair(slot))
    }

    /// Moves the rollback bar without an undo entry and rebuilds, for views that show
    /// the part as it was before a feature, such as editing that feature. A command
    /// that rolls back for good is [`Command::RollTo`](crate::Command::RollTo).
    pub fn set_rollback(&mut self, before: Option<FeatureId>) {
        let journal = self.journal_command(|| serde_json::json!({"op":"rollback", "before":before}));
        let _ = journal.finish((|| {
            if before == self.rollback {
                return Ok::<_, String>(());
            }
            let end = |r: Option<FeatureId>| r.and_then(|id| self.index_of(id)).unwrap_or(self.features.len());
            let first = end(self.rollback).min(end(before));
            self.rollback = before;
            self.touch(first);
            self.rebuild();
            Ok(())
        })());
    }

    /// The name of a face for people, such as "Boss-Extrude1 · End cap".
    pub fn face_label(&self, face: FaceId) -> Option<FaceLabel> {
        let t = self.body_with(|t| t.face(face).is_some())?;
        self.label_in(face, &t)
    }

    /// Names every face of a body for people, in topology order, with one kernel query
    /// for the whole body. Faces no feature generated have no label.
    pub fn face_labels(&self, body: BodyId) -> Vec<(FaceId, Option<FaceLabel>)> {
        let Ok(t) = self.view_of(body) else { return Vec::new() };
        t.faces.iter().map(|f| (f.id, self.label_in(f.id, &t))).collect()
    }

    /// The feature whose operation created each edge of a body, in topology order,
    /// following split edges back to the edge they came from.
    pub fn edge_features(&self, body: BodyId) -> Vec<(EdgeId, Option<FeatureId>)> {
        let (Ok(t), Ok(h)) = (self.kernel.topology(body), self.kernel.history(body)) else { return Vec::new() };
        let creator = |edge: EdgeId| {
            let mut x = edge;
            for r in h.iter().rev() {
                if r.edges.created.contains(&x) {
                    return feature_of_tag(r.tag.as_str());
                }
                if let Some(s) = r.edges.split.iter().find(|s| s.into.contains(&x)) {
                    x = s.from;
                }
            }
            None
        };
        t.edges.iter().map(|e| (e.id, creator(e.id))).collect()
    }

    fn label_in(&self, face: FaceId, t: &Topology) -> Option<FaceLabel> {
        let (feature, generator, ordinal) = reference::origin_label(face, &View { topology: t }, &self.name_map())?;
        let f = self.feature(feature)?;
        let what = match generator {
            Generator::Side { entity } => {
                let kind = self.profile_sketch(feature).and_then(|s| s.entity(entity).map(|e| e.kind));
                match kind {
                    Some(EntityKind::Line { .. }) => format!("Side of line {entity}"),
                    Some(EntityKind::Arc { .. }) => format!("Side of arc {entity}"),
                    Some(EntityKind::Circle { .. }) => format!("Wall of circle {entity}"),
                    _ => generator.to_string(),
                }
            }
            _ => generator.to_string(),
        };
        let count = self.generated_faces(feature).iter().filter(|n| n.generator == generator).count();
        let suffix = if count > 1 { format!(" {}", ordinal + 1) } else { String::new() };
        let part = format!("{what}{suffix}");
        Some(FaceLabel { feature, generator, ordinal, text: format!("{} · {part}", f.name), part })
    }

    /// The sketch an extrusion sweeps (a sketch feature's, or its built-in shape's), or
    /// a sketch feature's own.
    fn profile_sketch(&self, id: FeatureId) -> Option<Sketch> {
        match &self.feature(id)?.kind {
            FeatureKind::Sketch(s) => Some(s.sketch.clone()),
            FeatureKind::Revolve(r) => match &self.feature(r.sketch)?.kind {
                FeatureKind::Sketch(s) => Some(s.sketch.clone()),
                _ => None,
            },
            FeatureKind::Extrude(e) => match &e.profile {
                ProfileSource::Sketch { feature } => match &self.feature(*feature)?.kind {
                    FeatureKind::Sketch(s) => Some(s.sketch.clone()),
                    _ => None,
                },
                ProfileSource::Shape { shape, .. } => shape.sketch().ok(),
            },
            _ => None,
        }
    }
}

/// The feature an operation tag belongs to. The document tags every kernel operation
/// `F<id>.<step>`.
pub fn feature_of_tag(tag: &str) -> Option<FeatureId> {
    let rest = tag.strip_prefix('F')?;
    let (id, _) = rest.split_once('.')?;
    id.parse().ok().map(FeatureId)
}
