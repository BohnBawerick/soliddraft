//! The part document: features in tree order, rebuilt through the kernel trait.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use soliddraft_document::DocumentId;
use soliddraft_kernel_api::{
    BodyId, BooleanOp, EdgeId, Extrude, FaceId, FaceRole, Kernel, KernelError, KernelResult, OpTag, Origin, Plane,
    Topology, Vec3,
};
use soliddraft_sketch::profile::Region;
use soliddraft_sketch::{EntityKind, Sketch, SolveStatus};
use soliddraft_tessellation::{Report, Settings};

use crate::command::UndoEntry;
use crate::feature::{
    BlendFeature, DatumPlane, EndCondition, ExtrudeFeature, ExtrudeOp, Feature, FeatureId, FeatureKind, ProfileSource,
    SketchFeature, SketchPlane,
};
use crate::materials::PartMaterials;
use crate::rebuild::BuildCache;
use crate::reference::{
    self, EdgeRef, FaceRef, Generator, NameMap, NamedFace, NamingError, ReferenceSlot, RepairNeeded, RepairReason, View,
};

/// How far an extrusion from a sketch on a face of the part starts behind that face, so
/// the tool overlaps the part instead of touching it along a coplanar face. Only where
/// that changes nothing: a boss growing out of the face starts inside the material, a
/// cut going into the face starts in the air. A reversed boss or cut starts on the face.
pub const FACE_OVERLAP: f64 = 0.01;

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

/// One feature's rebuild result.
#[derive(Clone, Debug)]
pub(crate) struct Built {
    pub status: FeatureStatus,
    /// The part body after this feature.
    pub output: Option<BodyId>,
    /// The body this feature made. The rebuild cache owns it and releases it.
    pub owned: Option<BodyId>,
    /// The names of the faces this feature generated, with the origins they stand for.
    pub names: Vec<NamedFace>,
    pub sketch: Option<BuiltSketch>,
    /// Which kept build this is, so a feature that reads it can key on it. Zero for a
    /// result the cache does not keep.
    pub version: u64,
}

impl Built {
    fn pass(status: FeatureStatus, current: Option<BodyId>) -> Built {
        Built { status, output: current, owned: None, names: Vec::new(), sketch: None, version: 0 }
    }

    fn failed(current: Option<BodyId>, message: impl Into<String>) -> Built {
        Built::pass(FeatureStatus::Failed(message.into()), current)
    }
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
        settings.check()?;
        self.settings = settings;
        Ok(())
    }

    /// What the part's viewport quality gives its body: each level's deviation in
    /// millimetres and triangle count. `None` without a body.
    pub fn quality_report(&self) -> Option<KernelResult<Report>> {
        Some(soliddraft_tessellation::report(self.kernel.as_ref(), self.body()?, &self.settings.viewport))
    }

    /// Exports the part's body as `format`, meshes at the part's export tolerance. A part
    /// with a feature that did not build is refused, so no file shows a body the part does
    /// not have; suppressed and rolled-back features are fine.
    pub fn export(&self, format: soliddraft_exchange::Format) -> Result<soliddraft_exchange::Export, String> {
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

    fn built_index(&self, id: FeatureId) -> Option<usize> {
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
        if before == self.rollback {
            return;
        }
        let end = |r: Option<FeatureId>| r.and_then(|id| self.index_of(id)).unwrap_or(self.features.len());
        let first = end(self.rollback).min(end(before));
        self.rollback = before;
        self.touch(first);
        self.rebuild();
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

    /// A copy of the part that shares this one's feature builds and bodies, so the copy
    /// builds only what changes in it. It has the same id, features and settings, and
    /// an empty undo history, as a part reopened from its file has. For dry runs, such
    /// as an assistant proposal tried before it is applied.
    pub fn fork(&self) -> Document {
        Document {
            id: self.id,
            name: self.name.clone(),
            features: self.features.clone(),
            datum_names: self.datum_names.clone(),
            rollback: self.rollback,
            next_id: self.next_id,
            materials: self.materials.clone(),
            settings: self.settings.clone(),
            kernel: self.kernel.clone(),
            built: self.built.clone(),
            dirty: self.dirty,
            cache: self.cache.clone(),
            undo: Vec::new(),
            redo: Vec::new(),
            log: Vec::new(),
            last_solve: None,
        }
    }

    /// Reuses a fork's successful builds on this kernel without changing the document.
    /// Exact input keys decide reuse even if the document changed after the dry run.
    pub fn reuse_builds_from(&mut self, source: &Document) {
        if self.id == source.id && Arc::ptr_eq(&self.kernel, &source.kernel) {
            self.cache.import(&source.cache);
            self.cache.trim(&self.built);
        }
    }

    /// True while the document keeps `body`: a feature build it holds made it. A body
    /// no document keeps is released and does not come back.
    pub fn keeps(&self, body: BodyId) -> bool {
        self.cache.holds(body)
    }

    /// The bytes the kept feature builds retain, and the budget they are trimmed to.
    pub fn rebuild_cache_usage(&self) -> (u64, u64) {
        (self.cache.retained(), self.cache.budget())
    }

    /// Sets how many bytes of feature builds the document keeps for reuse. The builds of
    /// the current tree are always kept. A smaller budget applies at once.
    pub fn set_rebuild_budget(&mut self, bytes: u64) {
        self.cache.set_budget(bytes);
        self.cache.trim(&self.built);
    }

    /// Rebuilds from the first stale feature to the end of the tree, reusing every
    /// feature build whose exact inputs the cache holds.
    pub(crate) fn rebuild(&mut self) {
        let start = self.dirty.min(self.built.len());
        self.built.truncate(start);
        let active = self.active_count();
        let mut cache = std::mem::take(&mut self.cache);
        let mut current = start.checked_sub(1).and_then(|i| self.built[i].output);
        for i in start..self.features.len() {
            let f = self.features[i].clone();
            let built = if i >= active {
                Built::pass(FeatureStatus::RolledBack, current)
            } else if f.suppressed {
                Built::pass(FeatureStatus::Suppressed, current)
            } else {
                self.build(&f, current, &mut cache)
            };
            current = built.output;
            self.built.push(built);
        }
        cache.trim(&self.built);
        self.cache = cache;
        self.dirty = self.built.len();
    }

    /// Marks every feature from `index` down as stale.
    pub(crate) fn touch(&mut self, index: usize) {
        self.dirty = self.dirty.min(index);
    }

    /// A feature's build: the kept one when the cache has its exact inputs, else a new
    /// one, kept. Checks whose messages name another feature run every time and are not
    /// kept, so a rename shows in them without building anything.
    fn build(&self, f: &Feature, current: Option<BodyId>, cache: &mut BuildCache) -> Built {
        let profile = match &f.kind {
            FeatureKind::Extrude(ExtrudeFeature { profile: ProfileSource::Sketch { feature }, .. }) => {
                match self.profile_input(*feature, current) {
                    Ok(input) => Some(input.version),
                    Err(refused) => return *refused,
                }
            }
            _ => None,
        };
        let key = cache.key(f, current, profile);
        if let Some(built) = cache.get(&key) {
            return built;
        }
        let built = match &f.kind {
            FeatureKind::Sketch(s) => self.build_sketch(s, current),
            FeatureKind::Extrude(e) => self.build_extrude(f, e, current),
            FeatureKind::Fillet(b) => self.build_blend(f, b, false, current),
            FeatureKind::Chamfer(b) => self.build_blend(f, b, true, current),
        };
        cache.insert(key, built, &self.kernel)
    }

    /// The built sketch an extrusion sweeps, or the extrusion's result when that sketch
    /// is missing, not built or has no closed region.
    fn profile_input(&self, sketch: FeatureId, current: Option<BodyId>) -> Result<&Built, Box<Built>> {
        let sketch_name = || self.feature(sketch).map(|s| s.name.clone()).unwrap_or_else(|| sketch.to_string());
        let failed = |message: String| Err(Box::new(Built::failed(current, message)));
        let Some(input) = self.built_index(sketch).map(|i| &self.built[i]) else {
            return failed(format!("its sketch {} is missing", sketch_name()));
        };
        let Some(sk) = input.sketch.as_ref() else {
            if input.status.is_ok() {
                return failed(format!("{} is not a sketch", sketch_name()));
            }
            return Err(Box::new(Built::pass(FeatureStatus::Blocked(sketch), current)));
        };
        match &sk.regions {
            Ok(r) if !r.is_empty() => Ok(input),
            Ok(_) => failed(format!("{} has no closed region", sketch_name())),
            Err(err) => failed(format!("{} has no closed region: {err}", sketch_name())),
        }
    }

    fn build_sketch(&self, s: &SketchFeature, current: Option<BodyId>) -> Built {
        let (plane, on_face) = match &s.plane {
            SketchPlane::Datum { datum, offset } => {
                let p = datum.plane();
                (p.with_origin(p.origin + p.normal * *offset), false)
            }
            SketchPlane::Face { face } => {
                let repair = |reason, message: String, candidates| {
                    let r = RepairNeeded { slot: ReferenceSlot::SketchPlane, reason, message, candidates };
                    Built::pass(FeatureStatus::RepairNeeded(vec![r]), current)
                };
                let Some(body) = current else {
                    return repair(RepairReason::Missing, "there is no part body to sketch on".into(), vec![]);
                };
                let t = match self.view_of(body) {
                    Ok(v) => v,
                    Err(e) => return Built::failed(current, e.to_string()),
                };
                let id = match reference::resolve_plane(face, &View { topology: &t }, &self.name_map()) {
                    Ok(id) => id,
                    Err(u) => {
                        return Built::pass(
                            FeatureStatus::RepairNeeded(vec![u.into_repair(ReferenceSlot::SketchPlane)]),
                            current,
                        );
                    }
                };
                let info = t.face(id).expect("resolved faces are in the topology");
                let Some(normal) = info.normal else {
                    let detail = "the face is not planar".to_string();
                    return repair(RepairReason::Mismatch { detail: detail.clone() }, detail, vec![]);
                };
                (plane_on_face(normal, info.centroid), true)
            }
        };
        let regions = s.sketch.regions().map_err(|e| e.to_string());
        Built {
            status: FeatureStatus::Ok,
            output: current,
            owned: None,
            names: Vec::new(),
            sketch: Some(BuiltSketch { plane, on_face, regions }),
            version: 0,
        }
    }

    fn build_extrude(&self, f: &Feature, e: &ExtrudeFeature, current: Option<BodyId>) -> Built {
        let shape_sketch;
        let sk = match &e.profile {
            ProfileSource::Sketch { feature } => match self.profile_input(*feature, current) {
                Ok(input) => input.sketch.as_ref().expect("a profile input is a built sketch"),
                Err(refused) => return *refused,
            },
            ProfileSource::Shape { datum, shape } => {
                let regions = shape.sketch().and_then(|s| s.regions().map_err(|e| e.to_string()));
                shape_sketch = BuiltSketch { plane: datum.plane(), on_face: false, regions };
                &shape_sketch
            }
        };
        let regions = match &sk.regions {
            Ok(r) if !r.is_empty() => r,
            Ok(_) => return Built::failed(current, "the built-in profile has no closed region"),
            Err(err) => return Built::failed(current, format!("the built-in profile has no closed region: {err}")),
        };
        let dir = if e.reverse { -sk.plane.normal } else { sk.plane.normal };
        // Behind the face is material for a boss along the normal and air for a cut
        // against it; only there is starting behind the face harmless.
        let away = (e.operation == ExtrudeOp::Boss) != e.reverse;
        let back = if sk.on_face && away { FACE_OVERLAP } else { 0.0 };
        let start = sk.plane.with_origin(sk.plane.origin - dir * back);
        let distance = match e.end {
            EndCondition::Blind { depth } if depth.is_finite() && depth > 0.0 => depth + back,
            EndCondition::Blind { depth } => {
                return Built::failed(current, format!("depth must be positive, got {depth}"));
            }
            EndCondition::ThroughAll => {
                let Some(body) = current else {
                    return Built::failed(current, "through all needs a body to go through");
                };
                let bbox = match self.kernel.topology(body) {
                    Ok(t) => t.bbox,
                    Err(err) => return Built::failed(current, err.to_string()),
                };
                let (lo, hi) = (bbox.min, bbox.max);
                let reach = (0..8)
                    .map(|c| {
                        let corner = Vec3::new(
                            if c & 1 == 0 { lo.x } else { hi.x },
                            if c & 2 == 0 { lo.y } else { hi.y },
                            if c & 4 == 0 { lo.z } else { hi.z },
                        );
                        (corner - start.origin).dot(dir)
                    })
                    .fold(f64::NEG_INFINITY, f64::max);
                if reach <= back + 1e-9 {
                    return Built::failed(current, "the body lies entirely behind the sketch plane");
                }
                reach + 1.0 + 0.01 * bbox.diagonal()
            }
        };
        let mut mine: Option<BodyId> = None;
        let mut faces: Vec<(Origin, Generator, usize)> = Vec::new();
        let mut run = |mine: &mut Option<BodyId>| -> KernelResult<Option<BodyId>> {
            let k = &self.kernel;
            let mut body = current;
            for (i, region) in regions.iter().enumerate() {
                let tag = |step: &str| OpTag::new(format!("{}.r{i}.{step}", f.id));
                let sheet = k.create_body_from_profile(&tag("profile"), &region.to_profile(start))?;
                let tool = k.extrude(&tag("sweep"), sheet.body, &Extrude { distance, direction: Some(dir) });
                let _ = k.release(sheet.body);
                let tool = tool?;
                for o in &tool.record.face_roles {
                    let generator = match o.role {
                        FaceRole::StartCap => Some(Generator::StartCap),
                        FaceRole::EndCap => Some(Generator::EndCap),
                        FaceRole::Side { loop_index, segment } => region
                            .entity(loop_index as usize, segment as usize)
                            .map(|entity| Generator::Side { entity }),
                        _ => None,
                    };
                    if let Some(generator) = generator {
                        faces.push((Origin { tag: tool.record.tag.clone(), role: o.role }, generator, i));
                    }
                }
                let next = match (e.operation, body) {
                    (ExtrudeOp::Boss, None) => tool.body,
                    (ExtrudeOp::Cut, None) => {
                        let _ = k.release(tool.body);
                        return Err(KernelError::InvalidInput("there is no body to cut".into()));
                    }
                    (op, Some(b)) => {
                        let (kind, step) = if op == ExtrudeOp::Boss {
                            (BooleanOp::Union, "union")
                        } else {
                            (BooleanOp::Subtract, "cut")
                        };
                        let joined = k.boolean(&tag(step), kind, b, tool.body);
                        let _ = k.release(tool.body);
                        joined?.body
                    }
                };
                if let Some(old) = mine.replace(next) {
                    let _ = k.release(old);
                }
                body = Some(next);
            }
            Ok(body)
        };
        match run(&mut mine) {
            Ok(output) => {
                Built { status: FeatureStatus::Ok, output, owned: mine, names: number(faces), sketch: None, version: 0 }
            }
            Err(err) => {
                if let Some(body) = mine {
                    let _ = self.kernel.release(body);
                }
                Built::failed(current, err.to_string())
            }
        }
    }

    fn build_blend(&self, f: &Feature, b: &BlendFeature, chamfer: bool, current: Option<BodyId>) -> Built {
        let what = if chamfer { "chamfer" } else { "fillet" };
        let Some(body) = current else {
            return Built::failed(current, format!("there is no body to {what}"));
        };
        if b.edges.is_empty() {
            return Built::failed(current, format!("the {what} has no edges"));
        }
        let t = match self.view_of(body) {
            Ok(v) => v,
            Err(err) => return Built::failed(current, err.to_string()),
        };
        let names = self.name_map();
        let view = View { topology: &t };
        let mut ids: Vec<EdgeId> = Vec::new();
        let mut repairs = Vec::new();
        for (i, r) in b.edges.iter().enumerate() {
            match reference::resolve_edge(r, &view, &names) {
                Ok(id) => ids.push(id),
                Err(u) => repairs.push(u.into_repair(ReferenceSlot::Edge { index: i as u32 })),
            }
        }
        if !repairs.is_empty() {
            return Built::pass(FeatureStatus::RepairNeeded(repairs), current);
        }
        for (i, id) in ids.iter().enumerate() {
            if let Some(j) = ids[..i].iter().position(|x| x == id) {
                return Built::failed(current, format!("edges {} and {} name the same edge", j + 1, i + 1));
            }
        }
        let tag = OpTag::new(format!("{}.blend", f.id));
        let result = if chamfer {
            self.kernel.chamfer(&tag, body, &ids, b.size)
        } else {
            self.kernel.fillet(&tag, body, &ids, b.size)
        };
        let result = match result {
            Ok(r) => r,
            Err(err) => return Built::failed(current, err.to_string()),
        };
        let faces = result
            .record
            .face_roles
            .iter()
            .filter_map(|o| match o.role {
                FaceRole::Blend { edge } => ids.iter().position(|x| *x == edge).map(|i| {
                    (Origin { tag: result.record.tag.clone(), role: o.role }, Generator::Blend { edge: i as u32 }, 0)
                }),
                _ => None,
            })
            .collect();
        Built {
            status: FeatureStatus::Ok,
            output: Some(result.body),
            owned: Some(result.body),
            names: number(faces),
            sketch: None,
            version: 0,
        }
    }
}

fn kernel_repair(slot: ReferenceSlot, e: KernelError) -> RepairNeeded {
    RepairNeeded { slot, reason: RepairReason::Missing, message: e.to_string(), candidates: Vec::new() }
}

/// The feature an operation tag belongs to. The document tags every kernel operation
/// `F<id>.<step>`.
pub fn feature_of_tag(tag: &str) -> Option<FeatureId> {
    let rest = tag.strip_prefix('F')?;
    let (id, _) = rest.split_once('.')?;
    id.parse().ok().map(FeatureId)
}

/// Ordinals: faces with the same generator are ranked by the region that made them. A
/// region makes each generator once, so the rank never depends on geometry.
fn number(mut faces: Vec<(Origin, Generator, usize)>) -> Vec<NamedFace> {
    faces.sort_by_key(|f| (f.1, f.2));
    let mut counts: HashMap<Generator, u32> = HashMap::new();
    faces
        .into_iter()
        .map(|(origin, generator, _)| {
            let n = counts.entry(generator).or_default();
            let ordinal = *n;
            *n += 1;
            NamedFace { origin, generator, ordinal }
        })
        .collect()
}

/// The sketch plane on a planar face: origin at the world origin projected onto the
/// face, X horizontal for a face that is not horizontal, else the world X axis.
pub fn plane_on_face(normal: Vec3, point: Vec3) -> Plane {
    let n = normal;
    let x_dir = if n.z.abs() < 1.0 - 1e-9 {
        Vec3::Z.cross(n).normalized().expect("not parallel to Z")
    } else {
        (Vec3::X - n * Vec3::X.dot(n)).normalized().expect("parallel to Z")
    };
    Plane { origin: n * point.dot(n), normal: n, x_dir }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_planes_have_upright_frames() {
        let p = plane_on_face(-Vec3::Y, Vec3::new(3.0, -45.0, 7.0));
        assert!(p.is_well_formed());
        assert!(p.origin.distance(Vec3::new(0.0, -45.0, 0.0)) < 1e-12);
        assert!(p.x_dir.distance(Vec3::X) < 1e-12 && p.y_dir().distance(Vec3::Z) < 1e-12);
        let top = plane_on_face(Vec3::Z, Vec3::new(5.0, 5.0, 10.0));
        assert!(top.origin.distance(Vec3::new(0.0, 0.0, 10.0)) < 1e-12);
        assert!(top.x_dir.distance(Vec3::X) < 1e-12 && top.y_dir().distance(Vec3::Y) < 1e-12);
        let right = plane_on_face(Vec3::X, Vec3::new(60.0, 1.0, 1.0));
        assert!(right.x_dir.distance(Vec3::Y) < 1e-12 && right.y_dir().distance(Vec3::Z) < 1e-12);
    }
}
