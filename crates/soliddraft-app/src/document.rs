//! The document as the interface sees it: the feature tree, the displayed body
//! with its named faces and edges, picking, selection and previews.
//!
//! The part itself lives in [`soliddraft_model::Document`]: the features, the
//! commands with undo and redo, the rebuild, persistent references and part
//! files. This layer turns the panels' commands into model commands, one undo
//! entry each, and mirrors the model's tree after every change in the shape
//! the panels read.

use crate::display_quality::{self, DisplayQuality};
use crate::sketch::{self, Sketch};
use glam::Vec3;
use serde::{Deserialize, Serialize};
use soliddraft_kernel_api::{
    BodyId, BooleanOp, EdgeId, EdgePolyline, Extrude, FaceId, Kernel, KernelError, Mesh, OpTag, Plane, Point2, Profile,
    SurfaceKind, TessellationParams, Topology,
};
use soliddraft_materials::{Appearance, Shading};
use soliddraft_model as model;
use soliddraft_tessellation::{Projection, Quality, Settings};
use std::collections::HashMap;
use std::sync::Arc;

pub use soliddraft_model::{EdgeRef, FaceRef};

pub type FeatureId = u64;

fn model_id(id: FeatureId) -> model::FeatureId {
    model::FeatureId(id as u32)
}

fn app_id(id: model::FeatureId) -> FeatureId {
    id.0 as FeatureId
}

/// Built-in profiles: a quick extrude without a sketch.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ProfileKind {
    Rectangle { width: f64, height: f64 },
    Circle { diameter: f64 },
}

impl ProfileKind {
    pub fn to_kernel(self, plane: Plane) -> Profile {
        match self {
            ProfileKind::Rectangle { width, height } => Profile::rectangle(plane, Point2::new(0.0, 0.0), width, height),
            ProfileKind::Circle { diameter } => Profile::circle(plane, Point2::new(0.0, 0.0), diameter / 2.0),
        }
    }

    pub fn summary(&self) -> String {
        match self {
            ProfileKind::Rectangle { width, height } => format!("Rectangle {} × {}", fmt_mm(*width), fmt_mm(*height)),
            ProfileKind::Circle { diameter } => format!("Circle Ø{}", fmt_mm(*diameter)),
        }
    }

    fn to_model(self) -> model::BuiltinShape {
        match self {
            ProfileKind::Rectangle { width, height } => model::BuiltinShape::Rectangle { width, height },
            ProfileKind::Circle { diameter } => model::BuiltinShape::Circle { diameter },
        }
    }

    fn from_model(shape: model::BuiltinShape) -> Self {
        match shape {
            model::BuiltinShape::Rectangle { width, height } => ProfileKind::Rectangle { width, height },
            model::BuiltinShape::Circle { diameter } => ProfileKind::Circle { diameter },
        }
    }
}

/// The three reference planes. Z is up; Front faces the viewer standing on -Y.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaneKind {
    Top,
    Front,
    Right,
}

impl PlaneKind {
    pub const ALL: [PlaneKind; 3] = [PlaneKind::Top, PlaneKind::Front, PlaneKind::Right];

    pub fn label(&self) -> &'static str {
        self.to_model().label()
    }

    pub fn to_kernel(self) -> Plane {
        self.to_model().plane()
    }

    pub(crate) fn to_model(self) -> model::DatumPlane {
        match self {
            PlaneKind::Top => model::DatumPlane::Top,
            PlaneKind::Front => model::DatumPlane::Front,
            PlaneKind::Right => model::DatumPlane::Right,
        }
    }

    fn from_model(datum: model::DatumPlane) -> Self {
        match datum {
            model::DatumPlane::Top => PlaneKind::Top,
            model::DatumPlane::Front => PlaneKind::Front,
            model::DatumPlane::Right => PlaneKind::Right,
        }
    }
}

/// Where a sketch lies: a reference plane or a planar face of the body so far.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SketchPlane {
    Reference { plane: PlaneKind },
    Face { face: FaceRef },
}

impl SketchPlane {
    pub fn label(&self) -> String {
        match self {
            SketchPlane::Reference { plane } => plane.label().to_owned(),
            SketchPlane::Face { face } if face.label.is_empty() => "Face".to_owned(),
            SketchPlane::Face { face } => face.label.clone(),
        }
    }

    fn to_model(&self) -> model::SketchPlane {
        match self {
            SketchPlane::Reference { plane } => model::SketchPlane::Datum { datum: plane.to_model(), offset: 0.0 },
            SketchPlane::Face { face } => model::SketchPlane::Face { face: face.clone() },
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SketchFeature {
    pub plane: SketchPlane,
    pub sketch: Sketch,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExtrudeParams {
    /// The sketch to extrude; when `None`, the built-in profile on `plane`.
    pub sketch: Option<FeatureId>,
    pub profile: ProfileKind,
    pub plane: PlaneKind,
    pub depth: f64,
    pub flip: bool,
}

impl Default for ExtrudeParams {
    fn default() -> Self {
        Self {
            sketch: None,
            profile: ProfileKind::Rectangle { width: 60.0, height: 40.0 },
            plane: PlaneKind::Top,
            depth: 20.0,
            flip: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CutParams {
    pub sketch: FeatureId,
    pub depth: f64,
    pub through_all: bool,
    /// Cuts go against the sketch normal (into a face) unless flipped.
    pub flip: bool,
}

impl CutParams {
    pub fn new(sketch: FeatureId) -> Self {
        CutParams { sketch, depth: 10.0, through_all: true, flip: false }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FilletParams {
    pub edges: Vec<EdgeRef>,
    pub radius: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChamferParams {
    pub edges: Vec<EdgeRef>,
    pub distance: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FeatureKind {
    Sketch(SketchFeature),
    Extrude(ExtrudeParams),
    Cut(CutParams),
    Fillet(FilletParams),
    Chamfer(ChamferParams),
}

impl FeatureKind {
    pub fn prefix(&self) -> &'static str {
        match self {
            FeatureKind::Sketch(_) => "Sketch",
            FeatureKind::Extrude(_) => "Boss-Extrude",
            FeatureKind::Cut(_) => "Cut-Extrude",
            FeatureKind::Fillet(_) => "Fillet",
            FeatureKind::Chamfer(_) => "Chamfer",
        }
    }

    /// The sketch this feature consumes, if any.
    pub fn sketch(&self) -> Option<FeatureId> {
        match self {
            FeatureKind::Extrude(p) => p.sketch,
            FeatureKind::Cut(p) => Some(p.sketch),
            _ => None,
        }
    }

    /// The model's definition of the feature. A sketch's plane comes from the
    /// panels; a sketch is stored as the panels solved it.
    pub(crate) fn to_model(&self) -> model::FeatureKind {
        match self {
            FeatureKind::Sketch(s) => {
                model::FeatureKind::Sketch(model::SketchFeature { plane: s.plane.to_model(), sketch: s.sketch.clone() })
            }
            FeatureKind::Extrude(p) => model::FeatureKind::Extrude(model::ExtrudeFeature {
                profile: match p.sketch {
                    Some(id) => model::ProfileSource::Sketch { feature: model_id(id) },
                    None => model::ProfileSource::Shape { datum: p.plane.to_model(), shape: p.profile.to_model() },
                },
                operation: model::ExtrudeOp::Boss,
                end: model::EndCondition::Blind { depth: p.depth },
                reverse: p.flip,
            }),
            FeatureKind::Cut(p) => model::FeatureKind::Extrude(model::ExtrudeFeature {
                profile: model::ProfileSource::Sketch { feature: model_id(p.sketch) },
                operation: model::ExtrudeOp::Cut,
                end: if p.through_all {
                    model::EndCondition::ThroughAll
                } else {
                    model::EndCondition::Blind { depth: p.depth }
                },
                // The model sweeps along the sketch normal; a cut goes into the face.
                reverse: !p.flip,
            }),
            FeatureKind::Fillet(p) => {
                model::FeatureKind::Fillet(model::BlendFeature { edges: p.edges.clone(), size: p.radius })
            }
            FeatureKind::Chamfer(p) => {
                model::FeatureKind::Chamfer(model::BlendFeature { edges: p.edges.clone(), size: p.distance })
            }
        }
    }

    fn from_model(kind: &model::FeatureKind) -> Self {
        match kind {
            model::FeatureKind::Sketch(s) => {
                let plane = match &s.plane {
                    model::SketchPlane::Datum { datum, .. } => {
                        SketchPlane::Reference { plane: PlaneKind::from_model(*datum) }
                    }
                    model::SketchPlane::Face { face } => SketchPlane::Face { face: face.clone() },
                };
                let mut sketch = s.sketch.clone();
                if sketch.status == sketch::SolveStatus::Empty {
                    // Solve states are not saved; a sketch read from a file gets its back.
                    sketch.normalise();
                    sketch::solve::solve(&mut sketch, None, true);
                }
                FeatureKind::Sketch(SketchFeature { plane, sketch })
            }
            model::FeatureKind::Extrude(e) => {
                let (depth, through_all) = match e.end {
                    model::EndCondition::Blind { depth } => (depth, false),
                    model::EndCondition::ThroughAll => (10.0, true),
                };
                match (e.operation, &e.profile) {
                    (model::ExtrudeOp::Cut, profile) => FeatureKind::Cut(CutParams {
                        sketch: match profile {
                            model::ProfileSource::Sketch { feature } => app_id(*feature),
                            model::ProfileSource::Shape { .. } => 0,
                        },
                        depth,
                        through_all,
                        flip: !e.reverse,
                    }),
                    (model::ExtrudeOp::Boss, model::ProfileSource::Sketch { feature }) => {
                        FeatureKind::Extrude(ExtrudeParams {
                            sketch: Some(app_id(*feature)),
                            depth,
                            flip: e.reverse,
                            ..Default::default()
                        })
                    }
                    (model::ExtrudeOp::Boss, model::ProfileSource::Shape { datum, shape }) => {
                        FeatureKind::Extrude(ExtrudeParams {
                            sketch: None,
                            profile: ProfileKind::from_model(*shape),
                            plane: PlaneKind::from_model(*datum),
                            depth,
                            flip: e.reverse,
                        })
                    }
                }
            }
            model::FeatureKind::Fillet(b) => {
                FeatureKind::Fillet(FilletParams { edges: b.edges.clone(), radius: b.size })
            }
            model::FeatureKind::Chamfer(b) => {
                FeatureKind::Chamfer(ChamferParams { edges: b.edges.clone(), distance: b.size })
            }
        }
    }
}

/// Whether a reference names a face or an edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    Face,
    Edge,
}

/// A face or edge reference, as the repair editor collects them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EntityRef {
    Face(FaceRef),
    Edge(EdgeRef),
}

impl EntityRef {
    pub fn label(&self) -> &str {
        match self {
            EntityRef::Face(f) => &f.label,
            EntityRef::Edge(e) => &e.label,
        }
    }

    pub fn kind(&self) -> RefKind {
        match self {
            EntityRef::Face(_) => RefKind::Face,
            EntityRef::Edge(_) => RefKind::Edge,
        }
    }
}

/// A reference the rebuild could not resolve.
#[derive(Clone, Debug, PartialEq)]
pub struct RepairItem {
    /// Index into the feature's references: 0 for a sketch plane, the edge's
    /// position for a fillet or chamfer.
    pub index: usize,
    pub kind: RefKind,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FeatureStatus {
    Ok,
    Error(String),
    /// Some reference no longer resolves; the feature was skipped.
    NeedsRepair(Vec<RepairItem>),
    /// Suppressed, or past the rollback bar: not built.
    Inactive,
}

/// A face of the displayed body with the name the interface shows for it.
#[derive(Clone, Debug, PartialEq)]
pub struct NamedFace {
    pub id: FaceId,
    pub feature: FeatureId,
    pub label: String,
    pub surface: SurfaceKind,
    pub area: f64,
    pub centroid: Vec3,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NamedEdge {
    pub id: EdgeId,
    pub feature: FeatureId,
    pub label: String,
    pub length: f64,
    pub midpoint: Vec3,
}

/// One row of the feature tree, mirrored from the model after every change.
#[derive(Clone, Debug)]
pub struct Feature {
    pub id: FeatureId,
    pub name: String,
    pub kind: FeatureKind,
    pub status: FeatureStatus,
    pub suppressed: bool,
    /// The body after this feature, when it was built.
    pub body: Option<BodyId>,
    /// The resolved plane of a sketch feature.
    pub plane: Option<Plane>,
    /// Who created the feature: the user through the ribbon or the assistant.
    pub via_assistant: bool,
}

impl Feature {
    pub fn summary(&self) -> String {
        match &self.kind {
            FeatureKind::Sketch(s) => {
                let n = s.sketch.drawn().count();
                match s.sketch.status {
                    sketch::SolveStatus::Empty => "empty".to_owned(),
                    _ => format!("{n} entit{}", if n == 1 { "y" } else { "ies" }),
                }
            }
            FeatureKind::Extrude(p) => format!("{} mm", fmt_mm(p.depth)),
            FeatureKind::Cut(p) => {
                if p.through_all {
                    "through all".to_owned()
                } else {
                    format!("{} mm", fmt_mm(p.depth))
                }
            }
            FeatureKind::Fillet(f) => format!("R{}", fmt_mm(f.radius)),
            FeatureKind::Chamfer(c) => format!("{} mm", fmt_mm(c.distance)),
        }
    }

    pub fn is_sketch(&self) -> bool {
        matches!(self.kind, FeatureKind::Sketch(_))
    }

    pub fn sketch(&self) -> Option<&SketchFeature> {
        match &self.kind {
            FeatureKind::Sketch(s) => Some(s),
            _ => None,
        }
    }
}

/// A corner the interface can pick. Corners are the body's vertices where three or
/// more faces meet, keyed by their feature and ordinal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VertexKey {
    pub feature: FeatureId,
    pub index: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Selection {
    Feature(FeatureId),
    Face(FaceId),
    Edge(EdgeId),
    Vertex(VertexKey),
}

/// What a pick id in the id buffer stands for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PickEntry {
    Face(FeatureId, FaceId),
    Edge(FeatureId, EdgeId),
    Vertex(VertexKey, Vec3),
}

/// Human description of a selectable entity, shown in the hover label, the
/// property panel and the status bar.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityInfo {
    pub title: String,
    pub feature: Option<FeatureId>,
    pub details: Vec<(String, String)>,
}

/// Display geometry of one body, built from the kernel mesh and topology.
/// Vertices are unshared so every one carries its face's pick id. Edges are the
/// kernel's samples of each B-rep edge's exact curve and corners its vertices, so
/// the mesh's density never changes which edges and corners exist or where.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisplayMesh {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    /// Pick id of the face each vertex belongs to.
    pub face_picks: Vec<u32>,
    /// Feature that made the face each vertex belongs to (0 when unknown).
    pub face_features: Vec<u32>,
    /// Index into `appearances` of the face each vertex belongs to.
    pub face_appearances: Vec<u32>,
    /// The appearances the body shows, as the shader reads them. Entry 0 is the part's;
    /// an empty table draws everything in [`Appearance::default_part`].
    pub appearances: Vec<Shading>,
    pub indices: Vec<u32>,
    /// Line segments (pairs of points) per B-rep edge, with the edge's pick id.
    pub edges: Vec<DisplayEdge>,
    pub corners: Vec<DisplayCorner>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DisplayEdge {
    pub pick: u32,
    pub segments: Vec<Vec3>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DisplayCorner {
    pub pick: u32,
    pub position: Vec3,
}

impl DisplayMesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut it = self.positions.iter();
        let first = *it.next()?;
        Some(it.fold((first, first), |(lo, hi), p| (lo.min(*p), hi.max(*p))))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Command {
    AddSketch {
        plane: SketchPlane,
    },
    EditSketch {
        id: FeatureId,
        sketch: Sketch,
    },
    AddExtrude {
        params: ExtrudeParams,
        via_assistant: bool,
    },
    EditExtrude {
        id: FeatureId,
        params: ExtrudeParams,
    },
    AddCut {
        params: CutParams,
    },
    EditCut {
        id: FeatureId,
        params: CutParams,
    },
    AddFillet {
        params: FilletParams,
    },
    EditFillet {
        id: FeatureId,
        params: FilletParams,
    },
    AddChamfer {
        params: ChamferParams,
    },
    EditChamfer {
        id: FeatureId,
        params: ChamferParams,
    },
    DeleteFeature(FeatureId),
    Rename {
        id: FeatureId,
        name: String,
    },
    RenamePlane {
        plane: PlaneKind,
        name: String,
    },
    Suppress {
        id: FeatureId,
        suppressed: bool,
    },
    /// Moves the feature so it sits at `index` in the list.
    Reorder {
        id: FeatureId,
        index: usize,
    },
    /// Builds only the first `count` features; `None` builds everything.
    Rollback(Option<usize>),
    /// Replaces the feature's `index`-th reference.
    Repair {
        id: FeatureId,
        index: usize,
        replacement: EntityRef,
    },
}

impl Command {
    /// Short text for the Edit menu ("Undo Add fillet").
    pub fn label(&self) -> String {
        match self {
            Command::AddSketch { .. } => "Add sketch".into(),
            Command::EditSketch { .. } => "Edit sketch".into(),
            Command::AddExtrude { .. } => "Add extrude".into(),
            Command::EditExtrude { .. } => "Edit extrude".into(),
            Command::AddCut { .. } => "Add cut".into(),
            Command::EditCut { .. } => "Edit cut".into(),
            Command::AddFillet { .. } => "Add fillet".into(),
            Command::EditFillet { .. } => "Edit fillet".into(),
            Command::AddChamfer { .. } => "Add chamfer".into(),
            Command::EditChamfer { .. } => "Edit chamfer".into(),
            Command::DeleteFeature(_) => "Delete feature".into(),
            Command::Rename { .. } => "Rename".into(),
            Command::RenamePlane { .. } => "Rename plane".into(),
            Command::Suppress { suppressed: true, .. } => "Suppress".into(),
            Command::Suppress { suppressed: false, .. } => "Unsuppress".into(),
            Command::Reorder { .. } => "Reorder".into(),
            Command::Rollback(_) => "Roll back".into(),
            Command::Repair { .. } => "Repair reference".into(),
        }
    }
}

/// A body's display mesh and picks as it was last shown.
struct Shown {
    body: BodyId,
    /// The feature the body was shown after; picks name it.
    feature: FeatureId,
    params: TessellationParams,
    mesh: Arc<DisplayMesh>,
    picks: Vec<PickEntry>,
}

/// Ghost geometry of an unapplied feature.
#[derive(Clone, Debug, Default)]
pub struct Preview {
    /// The body the feature would produce, drawn in place of the current one.
    pub result: Option<Arc<DisplayMesh>>,
    /// Material the feature adds (extrude) or removes (cut), drawn as a ghost.
    pub tool: Option<Arc<DisplayMesh>>,
    pub removes: bool,
}

pub struct Document {
    pub name: String,
    pub path: Option<std::path::PathBuf>,
    /// The feature tree as the last rebuild left it. A mirror of the model:
    /// change it through commands.
    pub features: Vec<Feature>,
    pub selection: Option<Selection>,
    /// What the display meshes are tessellated with: the shown level's parameters,
    /// from the part's viewport quality. Previews use them too.
    pub tess_params: TessellationParams,
    /// Display meshes keyed by body. Replaced when the displayed body changes.
    pub meshes: HashMap<BodyId, Arc<DisplayMesh>>,
    /// Bumped on every rebuild so the renderer knows to re-upload.
    pub mesh_version: u64,
    pub rebuild_ms: f32,
    /// Features past this index are not built. `None` builds all. Setting it
    /// and calling [`Document::rebuild`] rolls the display back without an
    /// undo entry, as the feature editors do; [`Command::Rollback`] is the
    /// undoable form.
    pub rollback: Option<usize>,
    /// Named faces and edges of the displayed body.
    pub faces: Vec<NamedFace>,
    pub edges: Vec<NamedEdge>,
    /// Changes since the last save.
    pub dirty: bool,
    /// Counts every change, so autosave can tell edits apart from redraws.
    pub edits: u64,
    /// Counts every rebuild of the tree, undo and redo included; see [`Document::revision`].
    revision: u64,
    /// Pick id (index + 1) to entity, rebuilt with the meshes.
    picks: Vec<PickEntry>,
    /// The body `meshes` and `picks` show, and the feature it was shown after.
    shown: Option<(BodyId, FeatureId)>,
    /// Bodies shown before, latest last, kept while the part keeps them.
    recent: Vec<Shown>,
    model: model::Document,
    /// Which display level is shown, and the worker that makes the others.
    quality: DisplayQuality,
    /// Marks added features as the assistant's; see [`Document::set_assistant_mark`].
    assistant_mark: bool,
}

fn kv(v: soliddraft_kernel_api::Vec3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

impl Document {
    pub fn new(name: &str, kernel: Arc<dyn Kernel>) -> Self {
        Self::from_model(model::Document::new(name, kernel))
    }

    /// A document over a part the model already holds, such as one opened from a file.
    pub fn from_model(model: model::Document) -> Self {
        let mut doc = Self {
            name: model.name().to_owned(),
            path: None,
            features: Vec::new(),
            selection: None,
            tess_params: TessellationParams::default(),
            meshes: HashMap::new(),
            mesh_version: 0,
            rebuild_ms: 0.0,
            rollback: None,
            faces: Vec::new(),
            edges: Vec::new(),
            dirty: false,
            edits: 0,
            revision: 0,
            picks: Vec::new(),
            shown: None,
            recent: Vec::new(),
            quality: DisplayQuality::new(model.kernel().clone()),
            model,
            assistant_mark: false,
        };
        doc.sync();
        doc.refresh_display();
        doc
    }

    /// Marks a change: unsaved, and one more edit for autosave.
    pub fn touch(&mut self) {
        self.dirty = true;
        self.edits += 1;
    }

    /// Changes whenever the tree is rebuilt: a command, undo, redo or a rollback. Work
    /// started at one revision, such as a drag preview, is obsolete at the next.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The part behind the interface.
    pub fn model(&self) -> &model::Document {
        &self.model
    }

    /// Runs a canonical model command and refreshes the application document.
    pub fn run_journal_command(&mut self, input: serde_json::Value) -> Result<(), String> {
        match input["op"].as_str().unwrap_or_default() {
            "app_command" => {
                self.assistant_mark = input["assistant_mark"].as_bool().unwrap_or(false);
                return self
                    .apply(serde_json::from_value(input["command"].clone()).map_err(|e| e.to_string())?)
                    .map(|_| ());
            }
            "app_undo" => return self.undo().map(|_| ()),
            "app_redo" => return self.redo().map(|_| ()),
            "app_rebuild" => {
                self.rollback = serde_json::from_value(input["rollback"].clone()).map_err(|e| e.to_string())?;
                return self.rebuild();
            }
            _ => {}
        }
        let result = self.model.run_journal_command(input);
        self.touch();
        self.after_change();
        result
    }

    pub fn kernel(&self) -> &Arc<dyn Kernel> {
        self.model.kernel()
    }

    pub fn backend_name(&self) -> String {
        self.kernel().backend().name
    }

    /// The command log: one line per executed, undone or redone command.
    pub fn command_log(&self) -> &[String] {
        self.model.log()
    }

    pub fn feature(&self, id: FeatureId) -> Option<&Feature> {
        self.features.iter().find(|f| f.id == id)
    }

    pub fn index_of(&self, id: FeatureId) -> Option<usize> {
        self.features.iter().position(|f| f.id == id)
    }

    pub fn error_count(&self) -> usize {
        self.features.iter().filter(|f| matches!(f.status, FeatureStatus::Error(_))).count()
    }

    pub fn repair_count(&self) -> usize {
        self.features.iter().filter(|f| matches!(f.status, FeatureStatus::NeedsRepair(_))).count()
    }

    pub fn triangle_count(&self) -> usize {
        self.meshes.values().map(|m| m.triangle_count()).sum()
    }

    /// The feature that consumes a sketch, if any.
    pub fn consumer_of(&self, sketch: FeatureId) -> Option<FeatureId> {
        self.features.iter().find(|f| f.kind.sketch() == Some(sketch)).map(|f| f.id)
    }

    pub fn sketch_of(&self, id: FeatureId) -> Option<&SketchFeature> {
        self.feature(id)?.sketch()
    }

    /// Index of the last feature that is built and displayed.
    pub fn display_index(&self) -> Option<usize> {
        let end = self.rollback.unwrap_or(self.features.len()).min(self.features.len());
        (0..end).rev().find(|&i| self.features[i].body.is_some())
    }

    /// The body shown in the viewport: the running body at the rollback bar.
    pub fn display_body(&self) -> Option<BodyId> {
        self.display_index().and_then(|i| self.features[i].body)
    }

    /// The running body before feature `index`.
    pub fn body_before(&self, index: usize) -> Option<BodyId> {
        (0..index.min(self.features.len())).rev().find_map(|i| self.features[i].body)
    }

    pub fn can_undo(&self) -> bool {
        self.model.undo_label().is_some()
    }

    pub fn can_redo(&self) -> bool {
        self.model.redo_label().is_some()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.model.undo_label()
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.model.redo_label()
    }

    /// How many steps the undo stack holds.
    pub fn undo_depth(&self) -> usize {
        self.model.undo_depth()
    }

    /// Joins the undo steps recorded since the stack was `depth` deep into one step
    /// called `label`, so the commands of an assistant proposal undo together.
    pub fn group_undo(&mut self, depth: usize, label: String) {
        self.model.group_undo(depth, label);
    }

    /// Puts the part back as it was when the undo stack was `depth` deep and forgets
    /// the steps since, leaving nothing to redo.
    pub fn abandon(&mut self, depth: usize) -> Result<(), String> {
        self.model.abandon(depth).map_err(|e| e.to_string())?;
        self.after_change();
        Ok(())
    }

    /// Adds a line to the command log.
    pub fn log_note(&mut self, line: impl Into<String>) {
        self.model.log_note(line);
    }

    /// While on, every feature a command adds carries the assistant's mark in the tree.
    pub fn set_assistant_mark(&mut self, on: bool) {
        self.assistant_mark = on;
    }

    pub fn undo(&mut self) -> Result<String, String> {
        let journal = self.model.journal_command(|| serde_json::json!({"op":"app_undo"}));
        journal.finish((|| {
            let label = self.model.undo().map_err(|e| e.to_string())?;
            self.touch();
            self.after_change();
            Ok(label)
        })())
    }

    pub fn redo(&mut self) -> Result<String, String> {
        let journal = self.model.journal_command(|| serde_json::json!({"op":"app_redo"}));
        journal.finish((|| {
            let label = self.model.redo().map_err(|e| e.to_string())?;
            self.touch();
            self.after_change();
            Ok(label)
        })())
    }

    /// Runs a command as one undo entry and rebuilds. Returns the id of a
    /// feature the command created. A created feature that fails to build stays
    /// in the tree with its error, and the error comes back here too.
    pub fn apply(&mut self, command: Command) -> Result<Option<FeatureId>, String> {
        let journal = self.model.journal_command(|| {
            serde_json::json!({
                "op":"app_command", "command":command, "assistant_mark":self.assistant_mark
            })
        });
        journal.finish((|| {
            let label = command.label();
            let commands = self.to_model(command)?;
            let done = self.model.execute(model::Command::Batch { label, commands }).map_err(|e| e.to_string())?;
            self.touch();
            self.after_change();
            let created = done.created.map(app_id);
            if let Some(id) = created
                && let Some(FeatureStatus::Error(msg)) = self.feature(id).map(|f| f.status.clone())
            {
                return Err(msg);
            }
            Ok(created)
        })())
    }

    /// The model commands for one panel command.
    fn to_model(&self, command: Command) -> Result<Vec<model::Command>, String> {
        let add = |kind: model::FeatureKind, via_assistant: bool| model::Command::AddFeature {
            kind,
            name: None,
            position: None,
            via_assistant: via_assistant || self.assistant_mark,
        };
        let edit =
            |id: FeatureId, kind: &FeatureKind| model::Command::EditFeature { id: model_id(id), kind: kind.to_model() };
        Ok(match command {
            Command::AddSketch { plane } => {
                let mut sketch = Sketch::new();
                sketch::solve::solve(&mut sketch, None, true);
                vec![add(FeatureKind::Sketch(SketchFeature { plane, sketch }).to_model(), false)]
            }
            Command::EditSketch { id, mut sketch } => {
                let f = self.feature(id).ok_or("no such feature")?;
                let FeatureKind::Sketch(s) = &f.kind else { return Err("not a sketch".into()) };
                sketch.normalise();
                sketch::solve::solve(&mut sketch, None, true);
                vec![edit(id, &FeatureKind::Sketch(SketchFeature { plane: s.plane.clone(), sketch }))]
            }
            Command::AddExtrude { params, via_assistant } => {
                vec![add(FeatureKind::Extrude(params).to_model(), via_assistant)]
            }
            Command::EditExtrude { id, params } => vec![edit(id, &FeatureKind::Extrude(params))],
            Command::AddCut { params } => vec![add(FeatureKind::Cut(params).to_model(), false)],
            Command::EditCut { id, params } => vec![edit(id, &FeatureKind::Cut(params))],
            Command::AddFillet { params } => vec![add(FeatureKind::Fillet(params).to_model(), false)],
            Command::EditFillet { id, params } => vec![edit(id, &FeatureKind::Fillet(params))],
            Command::AddChamfer { params } => vec![add(FeatureKind::Chamfer(params).to_model(), false)],
            Command::EditChamfer { id, params } => vec![edit(id, &FeatureKind::Chamfer(params))],
            Command::DeleteFeature(id) => {
                let index = self.index_of(id).ok_or("no such feature")?;
                // Deleting a feature also deletes the sketch it absorbed, and
                // deleting a sketch deletes the features that consume it.
                let mut doomed = vec![id];
                if let Some(s) = self.features[index].kind.sketch()
                    && self.feature(s).is_some_and(|f| f.is_sketch())
                {
                    doomed.push(s);
                }
                if self.features[index].is_sketch() {
                    doomed.extend(self.features.iter().filter(|f| f.kind.sketch() == Some(id)).map(|f| f.id));
                }
                doomed.into_iter().map(|id| model::Command::DeleteFeature { id: model_id(id) }).collect()
            }
            Command::Rename { id, name } => {
                let name = name.trim().to_owned();
                if name.is_empty() {
                    return Err("a name cannot be empty".into());
                }
                vec![model::Command::RenameFeature { id: model_id(id), name }]
            }
            Command::RenamePlane { plane, name } => {
                vec![model::Command::RenameDatumPlane { datum: plane.to_model(), name }]
            }
            Command::Suppress { id, suppressed } => {
                let f = self.feature(id).ok_or("no such feature")?;
                // A sketch's consumers go with it.
                let mut ids = vec![id];
                if f.is_sketch() {
                    ids.extend(self.features.iter().filter(|g| g.kind.sketch() == Some(id)).map(|g| g.id));
                }
                ids.into_iter().map(|id| model::Command::SetSuppressed { id: model_id(id), suppressed }).collect()
            }
            Command::Reorder { id, index } => {
                let mut order: Vec<FeatureId> = self.features.iter().map(|f| f.id).collect();
                let from = order.iter().position(|x| *x == id).ok_or("no such feature")?;
                order.remove(from);
                let index = index.min(order.len());
                order.insert(index, id);
                // Keep a sketch ahead of its consumer and the consumer after its sketch.
                if let Some(s) = self.feature(id).and_then(|f| f.kind.sketch())
                    && let Some(si) = order.iter().position(|x| *x == s)
                    && si > index
                {
                    let sk = order.remove(si);
                    order.insert(index, sk);
                }
                if self.feature(id).is_some_and(|f| f.is_sketch())
                    && let Some(c) = self.consumer_of(id)
                    && let (Some(ci), Some(si)) =
                        (order.iter().position(|x| *x == c), order.iter().position(|x| *x == id))
                    && ci < si
                {
                    let cf = order.remove(ci);
                    order.insert(si, cf);
                }
                // Placing each feature in turn keeps every step in a valid order.
                let mut moves = Vec::new();
                let mut current: Vec<FeatureId> = self.features.iter().map(|f| f.id).collect();
                for (i, want) in order.iter().enumerate() {
                    let at = current.iter().position(|x| x == want).expect("same features");
                    if at != i {
                        current.remove(at);
                        current.insert(i, *want);
                        moves.push(model::Command::MoveFeature { id: model_id(*want), position: i });
                    }
                }
                moves
            }
            Command::Rollback(count) => {
                let before = count.and_then(|c| self.features.get(c)).map(|f| model_id(f.id));
                vec![model::Command::RollTo { before }]
            }
            Command::Repair { id, index, replacement } => {
                let f = self.model.feature(model_id(id)).ok_or("no such feature")?;
                let mut kind = f.kind.clone();
                match (&mut kind, replacement) {
                    (model::FeatureKind::Sketch(s), EntityRef::Face(face)) if index == 0 => {
                        s.plane = model::SketchPlane::Face { face };
                    }
                    (model::FeatureKind::Fillet(b) | model::FeatureKind::Chamfer(b), EntityRef::Edge(edge)) => {
                        *b.edges.get_mut(index).ok_or("no such reference")? = edge;
                    }
                    _ => return Err("the replacement is the wrong kind of entity".into()),
                }
                vec![model::Command::EditFeature { id: model_id(id), kind }]
            }
        })
    }

    /// Pushes the display rollback to the model and rebuilds what it changed.
    /// The model caches every feature's body, so this rebuilds only from the
    /// first feature whose state moved.
    pub fn rebuild(&mut self) -> Result<(), String> {
        let journal = self.model.journal_command(|| serde_json::json!({"op":"app_rebuild", "rollback":self.rollback}));
        journal.finish({
            let before = self.rollback.and_then(|c| self.features.get(c)).map(|f| model_id(f.id));
            self.model.set_rollback(before);
            self.after_change();
            match self.features.iter().find_map(|f| match &f.status {
                FeatureStatus::Error(e) => Some(format!("{}: {e}", f.name)),
                _ => None,
            }) {
                Some(e) => Err(e),
                None => Ok(()),
            }
        })
    }

    /// Mirrors the model and refreshes the display, keeping the selection by name.
    fn after_change(&mut self) {
        let started = web_time::Instant::now();
        self.revision += 1;
        let previous_selection = self.selection.and_then(|s| self.describe(s)).map(|i| (i.feature, i.title));
        self.sync();
        self.refresh_display();
        self.rebuild_ms = started.elapsed().as_secs_f32() * 1000.0;
        self.restore_selection(previous_selection);
    }

    /// Copies the model's tree into the panels' shape.
    fn sync(&mut self) {
        let m = &self.model;
        let active = m.active_count();
        self.rollback = (active < m.features().len()).then_some(active);
        let names: HashMap<model::FeatureId, String> = m.features().iter().map(|f| (f.id, f.name.clone())).collect();
        self.features = m
            .features()
            .iter()
            .map(|f| {
                let status = m.status(f.id).cloned().unwrap_or(model::FeatureStatus::RolledBack);
                let inactive = matches!(status, model::FeatureStatus::Suppressed | model::FeatureStatus::RolledBack);
                Feature {
                    id: app_id(f.id),
                    name: f.name.clone(),
                    kind: FeatureKind::from_model(&f.kind),
                    status: view_status(&f.kind, status, &names),
                    suppressed: f.suppressed,
                    body: if inactive { None } else { m.body_after(f.id) },
                    plane: m.built_sketch(f.id).map(|b| b.plane),
                    via_assistant: f.via_assistant,
                }
            })
            .collect();
    }

    /// Tessellates and names the displayed body. The level the camera last asked for is
    /// tessellated at once; the worker makes the other two. A body already shown keeps
    /// its mesh: the same body only has its faces named again, and a recent body the
    /// part still keeps, such as the one undo brings back, shows its kept mesh.
    fn refresh_display(&mut self) {
        let model = &self.model;
        self.quality.retain(|b| model.keeps(b));
        self.recent.retain(|s| model.keeps(s.body));
        let next = self.display_index().and_then(|i| Some((self.features[i].body?, self.features[i].id)));
        if let Some((body, _)) = next
            && next == self.shown
            && !self.meshes.is_empty()
        {
            if let Ok(topology) = self.kernel().topology(body) {
                self.name_entities(body, &topology);
            }
            self.apply_appearances();
            self.mesh_version += 1;
            return;
        }
        if let Some((body, feature)) = self.shown.take()
            && let Some(mesh) = self.meshes.get(&body).cloned()
        {
            self.recent.retain(|s| s.body != body);
            self.recent.push(Shown {
                body,
                feature,
                params: self.tess_params,
                mesh,
                picks: std::mem::take(&mut self.picks),
            });
            if self.recent.len() > display_quality::RECENT {
                self.recent.remove(0);
            }
        }
        self.faces.clear();
        self.edges.clear();
        self.meshes = HashMap::new();
        self.picks = Vec::new();
        if let Some((body, feature)) = next
            && let Ok(topology) = self.kernel().topology(body)
        {
            self.name_entities(body, &topology);
            self.tess_params = self.quality.retarget(body, &topology, &self.model.settings().viewport);
            let kept =
                self.recent.iter().position(|s| s.body == body && s.feature == feature && s.params == self.tess_params);
            let cached = self.quality.reuse(body);
            if let (Some(i), Some(_)) = (kept, &cached) {
                let s = self.recent.remove(i);
                self.meshes = HashMap::from([(body, s.mesh)]);
                self.picks = s.picks;
                self.apply_appearances();
                self.shown = next;
            } else if let Some(mesh) = cached {
                self.show_mesh(body, &topology, &mesh);
                self.shown = next;
            } else if let Ok(mesh) = self.kernel().tessellate(body, &self.tess_params) {
                self.show_mesh(body, &topology, &mesh);
                self.quality.made(body, mesh);
                self.shown = next;
            }
        } else {
            self.quality.clear();
        }
        self.mesh_version += 1;
    }

    /// Applies an appearance to the part, a feature's faces or one face, or removes the
    /// one there with `None`, in every configuration, as one undo entry. Only the colours
    /// change: the meshes keep their geometry and the kernel is not asked for anything.
    pub fn set_appearance(
        &mut self,
        target: model::AppearanceTarget,
        appearance: Option<Appearance>,
    ) -> Result<(), String> {
        let scope = soliddraft_document::ConfigScope::All;
        self.model.execute(model::Command::SetAppearance { target, appearance, scope }).map_err(|e| e.to_string())?;
        self.touch();
        self.apply_appearances();
        self.mesh_version += 1;
        Ok(())
    }

    /// Fills each display mesh's appearance table and per-vertex index from the part's
    /// appearance assignments, resolved by the model for its default configuration.
    fn apply_appearances(&mut self) {
        let resolved = self.model.appearance_table(None);
        let table: Vec<Shading> = resolved.appearances.iter().map(Appearance::shading).collect();
        for mesh in self.meshes.values_mut() {
            let m = Arc::make_mut(mesh);
            m.face_appearances = m
                .face_picks
                .iter()
                .map(|&pick| match self.picks.get((pick as usize).wrapping_sub(1)) {
                    Some(PickEntry::Face(_, face)) => resolved.faces.get(face).copied().unwrap_or(0),
                    _ => 0,
                })
                .collect();
            m.appearances = table.clone();
        }
    }

    /// Makes `mesh` the displayed body's display mesh and its picks.
    fn show_mesh(&mut self, body: BodyId, topology: &Topology, mesh: &Mesh) {
        let mut picks = Vec::new();
        let feature = self.display_index().map(|i| self.features[i].id).unwrap_or(0);
        let makers: HashMap<FaceId, FeatureId> = self.faces.iter().map(|f| (f.id, f.feature)).collect();
        let edges = self.edge_polylines(body);
        self.meshes =
            HashMap::from([(body, Arc::new(display_mesh(feature, mesh, &edges, topology, &makers, &mut picks)))]);
        self.picks = picks;
        self.apply_appearances();
    }

    /// Moves the display level on by one frame of `dt_s` seconds under `projection`,
    /// and shows a finer or coarser mesh when the camera calls for one and it is ready.
    /// Returns true while the viewport should keep repainting: the mesh changed, or the
    /// worker still owes a level.
    pub fn update_display(&mut self, projection: &Projection, dt_s: f32) -> bool {
        let Some((level, mesh)) = self.quality.update(projection, dt_s) else {
            return self.quality.pending();
        };
        let Some(body) = self.display_body() else { return false };
        let Ok(topology) = self.kernel().topology(body) else { return false };
        self.tess_params = *self.quality.levels().params(level);
        self.show_mesh(body, &topology, &mesh);
        self.mesh_version += 1;
        true
    }

    /// The display levels: the level shown and wanted, and each level's parameters.
    pub fn display_quality(&self) -> &DisplayQuality {
        &self.quality
    }

    /// Sets the part's viewport quality, saved in its file. The part's revision does not
    /// change and nothing rebuilds: the worker makes the new meshes and the old one stays
    /// on screen until they arrive.
    pub fn set_viewport_quality(&mut self, quality: Quality) -> Result<(), String> {
        let settings = Settings { viewport: quality, ..self.model.settings().clone() };
        if settings == *self.model.settings() {
            return Ok(());
        }
        self.model.set_settings(settings)?;
        self.quality.set_quality(&quality);
        self.touch();
        Ok(())
    }

    /// Replaces the part's display and accuracy settings, saved in its file, as
    /// [`set_viewport_quality`](Self::set_viewport_quality) does for the quality.
    pub fn set_settings(&mut self, settings: Settings) -> Result<(), String> {
        if settings == *self.model.settings() {
            return Ok(());
        }
        let quality = settings.viewport;
        self.model.set_settings(settings)?;
        self.quality.set_quality(&quality);
        self.touch();
        Ok(())
    }

    /// Labels every face and edge of the displayed body by the feature that
    /// made it. Face names come from the model's persistent names (caps, sides
    /// of sketch entities, walls, blends); edges are numbered per feature.
    fn name_entities(&mut self, body: BodyId, topology: &Topology) {
        let labels: HashMap<FaceId, model::FaceLabel> =
            self.model.face_labels(body).into_iter().filter_map(|(id, l)| Some((id, l?))).collect();
        let mut unnamed = 0;
        self.faces = topology
            .faces
            .iter()
            .map(|f| {
                let (feature, label) = match labels.get(&f.id) {
                    Some(l) => (app_id(l.feature), l.part.clone()),
                    None => {
                        unnamed += 1;
                        (0, format!("Face {unnamed}"))
                    }
                };
                NamedFace { id: f.id, feature, label, surface: f.surface, area: f.area, centroid: kv(f.centroid) }
            })
            .collect();
        let makers: HashMap<EdgeId, Option<model::FeatureId>> = self.model.edge_features(body).into_iter().collect();
        let mut counters: HashMap<FeatureId, u32> = HashMap::new();
        self.edges = topology
            .edges
            .iter()
            .map(|e| {
                let feature = makers.get(&e.id).copied().flatten().map(app_id).unwrap_or(0);
                let n = counters.entry(feature).or_default();
                *n += 1;
                NamedEdge { id: e.id, feature, label: format!("Edge {n}"), length: e.length, midpoint: kv(e.midpoint) }
            })
            .collect();
    }

    /// Kernel ids change on every rebuild, so a selection is carried across a
    /// rebuild by the name it had: the same feature and the same label.
    fn restore_selection(&mut self, previous: Option<(Option<FeatureId>, String)>) {
        let Some(sel) = self.selection else { return };
        if self.describe(sel).is_some() {
            return;
        }
        let Some((Some(feature), title)) = previous else {
            self.selection = None;
            return;
        };
        let label = title.rsplit(" · ").next().unwrap_or("").to_owned();
        self.selection = match sel {
            Selection::Face(_) => {
                self.faces.iter().find(|nf| nf.feature == feature && nf.label == label).map(|nf| Selection::Face(nf.id))
            }
            Selection::Edge(_) => {
                self.edges.iter().find(|ne| ne.feature == feature && ne.label == label).map(|ne| Selection::Edge(ne.id))
            }
            Selection::Vertex(v) => {
                self.picks.iter().any(|p| matches!(p, PickEntry::Vertex(k, _) if *k == v)).then_some(sel)
            }
            Selection::Feature(id) => self.feature(id).map(|_| sel),
        };
    }

    /// Profiles of a sketch feature on its resolved plane.
    fn sketch_profiles(&self, sketch_id: FeatureId) -> Result<(Vec<Profile>, Plane), String> {
        let f = self.feature(sketch_id).ok_or("the sketch no longer exists")?;
        let s = f.sketch().ok_or("the profile feature is not a sketch")?;
        let plane = f.plane.ok_or("the sketch plane is not resolved")?;
        let profiles = sketch::profile::profiles(&s.sketch, plane).map_err(|e| e.to_string())?;
        if profiles.is_empty() {
            return Err("the sketch has no closed profile".into());
        }
        Ok((profiles, plane))
    }

    /// Extrudes every profile and unions the results into one solid, for previews.
    fn extrude_profiles(
        &self,
        profiles: &[Profile],
        direction: soliddraft_kernel_api::Vec3,
        distance: f64,
    ) -> Result<(BodyId, Vec<BodyId>), KernelError> {
        if !(distance.is_finite() && distance > 0.0) {
            return Err(KernelError::InvalidInput(format!("depth must be positive, got {distance}")));
        }
        let k = self.kernel();
        let mut made = Vec::new();
        let mut acc: Option<BodyId> = None;
        let result = (|| {
            for profile in profiles {
                let sheet = k.create_body_from_profile(&OpTag::new("preview-profile"), profile)?;
                let solid =
                    k.extrude(&OpTag::new("preview"), sheet.body, &Extrude { distance, direction: Some(direction) });
                let _ = k.release(sheet.body);
                let solid = solid?;
                made.push(solid.body);
                acc = Some(match acc {
                    None => solid.body,
                    Some(a) => {
                        let u = k.boolean(&OpTag::new("preview-join"), BooleanOp::Union, a, solid.body)?;
                        made.push(u.body);
                        u.body
                    }
                });
            }
            Ok(())
        })();
        match result {
            Ok(()) => Ok((acc.expect("at least one profile"), made)),
            Err(e) => {
                self.release_all(&made);
                Err(e)
            }
        }
    }

    fn release_all(&self, bodies: &[BodyId]) {
        for b in bodies {
            let _ = self.kernel().release(*b);
        }
    }

    /// A cut distance that passes right through the body from any face.
    fn through_all_depth(&self, body: BodyId) -> f64 {
        self.kernel().topology(body).map(|t| t.bbox.diagonal() * 2.0 + 10.0).unwrap_or(1000.0)
    }

    /// Builds the tool of an extrude without applying it: the ghost of the
    /// material it adds. Bodies are released at once; the document is untouched.
    pub fn preview_extrude(&self, params: &ExtrudeParams) -> Result<Preview, String> {
        let (profiles, plane) = match params.sketch {
            Some(s) => self.sketch_profiles(s)?,
            None => {
                let plane = params.plane.to_kernel();
                (vec![params.profile.to_kernel(plane)], plane)
            }
        };
        let dir = if params.flip { -plane.normal } else { plane.normal };
        let (tool, made) = self.extrude_profiles(&profiles, dir, params.depth).map_err(|e| e.to_string())?;
        let mesh = self.mesh_of(tool);
        self.release_all(&made);
        Ok(Preview { result: None, tool: Some(mesh?), removes: false })
    }

    /// The body after a cut and the material it removes.
    pub fn preview_cut(&self, params: &CutParams, before: Option<BodyId>) -> Result<Preview, String> {
        let base = before.ok_or("nothing to cut yet; extrude a base first")?;
        let (profiles, plane) = self.sketch_profiles(params.sketch)?;
        let dir = if params.flip { plane.normal } else { -plane.normal };
        let depth = if params.through_all { self.through_all_depth(base) } else { params.depth };
        let (tool, mut made) = self.extrude_profiles(&profiles, dir, depth).map_err(|e| e.to_string())?;
        let tool_mesh = self.mesh_of(tool);
        let result = self.kernel().boolean(&OpTag::new("preview"), BooleanOp::Subtract, base, tool);
        let result_mesh = match &result {
            Ok(r) => {
                made.push(r.body);
                self.mesh_of(r.body).ok()
            }
            Err(_) => None,
        };
        self.release_all(&made);
        let err = result.err().map(|e| e.to_string());
        let preview = Preview { result: result_mesh, tool: tool_mesh.ok(), removes: true };
        match err {
            Some(e) => Err(e),
            None => Ok(preview),
        }
    }

    /// The body after a fillet or chamfer of the given edges of `before`.
    pub fn preview_rounding(
        &self,
        refs: &[EdgeRef],
        radius: f64,
        chamfer: bool,
        before: Option<BodyId>,
    ) -> Result<Preview, String> {
        let base = before.ok_or("nothing to round yet")?;
        if refs.is_empty() {
            return Err("select at least one edge".into());
        }
        let mut edges = Vec::new();
        for r in refs {
            let e = self.model.resolve_edge(r, base).map_err(|repair| format!("{}: {}", r.label, repair.message))?;
            edges.push(e);
        }
        let k = self.kernel();
        let r = if chamfer {
            k.chamfer(&OpTag::new("preview"), base, &edges, radius)
        } else {
            k.fillet(&OpTag::new("preview"), base, &edges, radius)
        }
        .map_err(|e| e.to_string())?;
        let mesh = self.mesh_of(r.body);
        let _ = k.release(r.body);
        Ok(Preview { result: Some(mesh?), tool: None, removes: false })
    }

    fn mesh_of(&self, body: BodyId) -> Result<Arc<DisplayMesh>, String> {
        preview_mesh(self.kernel().as_ref(), body, &self.tess_params)
    }

    /// The body's edges at the shown level's parameters.
    fn edge_polylines(&self, body: BodyId) -> Vec<EdgePolyline> {
        edge_polylines(self.kernel().as_ref(), body, &self.tess_params)
    }

    /// A persistent reference to an edge of the part, for fillets and chamfers.
    pub fn edge_ref(&self, edge: EdgeId) -> Option<EdgeRef> {
        self.model.edge_ref(edge).ok()
    }

    /// A persistent reference to a planar face of the part, for sketching on it. A piece
    /// of a split face names the plane all its pieces share.
    pub fn face_ref(&self, face: FaceId) -> Option<FaceRef> {
        let nf = self.faces.iter().find(|f| f.id == face)?;
        if nf.surface != SurfaceKind::Plane {
            return None;
        }
        self.model.plane_ref(face).ok()
    }

    /// The sketch plane on a planar face of the displayed body.
    pub fn face_plane(&self, face: FaceId) -> Option<Plane> {
        let body = self.display_body()?;
        let topology = self.kernel().topology(body).ok()?;
        let info = topology.face(face)?;
        Some(model::plane_on_face(info.normal?, info.centroid))
    }

    pub fn mass_properties(&self, id: FeatureId) -> Option<soliddraft_kernel_api::MassProperties> {
        let body = self.feature(id)?.body?;
        self.kernel().mass_properties(body).ok()
    }

    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut acc: Option<(Vec3, Vec3)> = None;
        for m in self.meshes.values() {
            if let Some((lo, hi)) = m.bounds() {
                acc = Some(match acc {
                    Some((a, b)) => (a.min(lo), b.max(hi)),
                    None => (lo, hi),
                });
            }
        }
        acc
    }

    pub fn selection_feature(&self, sel: Selection) -> Option<FeatureId> {
        self.describe(sel).and_then(|i| i.feature)
    }

    /// The entity behind a pick id from the id buffer.
    pub fn pick(&self, pick: u32) -> Option<PickEntry> {
        self.picks.get(pick.checked_sub(1)? as usize).copied()
    }

    /// The pick id of a selected face, for the shader's highlight.
    pub fn pick_of_face(&self, face: FaceId) -> Option<u32> {
        self.picks.iter().position(|p| matches!(p, PickEntry::Face(_, f) if *f == face)).map(|i| i as u32 + 1)
    }

    pub fn pick_of_edge(&self, edge: EdgeId) -> Option<u32> {
        self.picks.iter().position(|p| matches!(p, PickEntry::Edge(_, e) if *e == edge)).map(|i| i as u32 + 1)
    }

    pub fn vertex_position(&self, key: VertexKey) -> Option<Vec3> {
        self.picks.iter().find_map(|p| match p {
            PickEntry::Vertex(k, pos) if *k == key => Some(*pos),
            _ => None,
        })
    }

    pub fn named_face(&self, id: FaceId) -> Option<&NamedFace> {
        self.faces.iter().find(|f| f.id == id)
    }

    pub fn named_edge(&self, id: EdgeId) -> Option<&NamedEdge> {
        self.edges.iter().find(|e| e.id == id)
    }

    pub fn describe(&self, sel: Selection) -> Option<EntityInfo> {
        match sel {
            Selection::Feature(id) => {
                let f = self.feature(id)?;
                let mut details = Vec::new();
                match &f.kind {
                    FeatureKind::Sketch(s) => {
                        details.push(("Plane".to_owned(), s.plane.label()));
                        let (dims, rels) = s.sketch.counts();
                        details.push(("Entities".to_owned(), s.sketch.drawn().count().to_string()));
                        details.push(("Relations".to_owned(), format!("{rels} relations, {dims} dimensions")));
                        details.push(("State".to_owned(), s.sketch.status.label().to_owned()));
                    }
                    FeatureKind::Extrude(p) => {
                        let profile = match p.sketch {
                            Some(s) => self.feature(s).map(|f| f.name.clone()).unwrap_or("missing sketch".into()),
                            None => format!("{} on {}", p.profile.summary(), p.plane.label()),
                        };
                        details.push(("Profile".to_owned(), profile));
                        details.push(("Depth".to_owned(), f.summary()));
                    }
                    FeatureKind::Cut(p) => {
                        details.push((
                            "Profile".to_owned(),
                            self.feature(p.sketch).map(|f| f.name.clone()).unwrap_or("missing sketch".into()),
                        ));
                        details.push(("End".to_owned(), f.summary()));
                    }
                    FeatureKind::Fillet(p) => {
                        details.push(("Edges".to_owned(), p.edges.len().to_string()));
                        details.push(("Radius".to_owned(), format!("{} mm", fmt_mm(p.radius))));
                    }
                    FeatureKind::Chamfer(p) => {
                        details.push(("Edges".to_owned(), p.edges.len().to_string()));
                        details.push(("Distance".to_owned(), format!("{} mm", fmt_mm(p.distance))));
                    }
                }
                if !f.is_sketch()
                    && let Some(mp) = self.mass_properties(id)
                {
                    details.push(("Volume".to_owned(), format!("{:.0} mm³", mp.volume)));
                    details.push(("Surface area".to_owned(), format!("{:.0} mm²", mp.area)));
                }
                let made = self.faces.iter().filter(|nf| nf.feature == id).count();
                if made > 0 {
                    details.push(("Faces made".to_owned(), made.to_string()));
                }
                match &f.status {
                    FeatureStatus::Error(e) => details.push(("Error".to_owned(), e.clone())),
                    FeatureStatus::NeedsRepair(items) => details.push((
                        "Needs repair".to_owned(),
                        items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>().join(", "),
                    )),
                    FeatureStatus::Inactive if f.suppressed => {
                        details.push(("State".to_owned(), "Suppressed".to_owned()))
                    }
                    FeatureStatus::Inactive => details.push(("State".to_owned(), "Rolled back".to_owned())),
                    FeatureStatus::Ok => {}
                }
                Some(EntityInfo { title: f.name.clone(), feature: Some(id), details })
            }
            Selection::Face(fid) => {
                let nf = self.named_face(fid)?;
                let f = self.feature(nf.feature);
                let surface = match nf.surface {
                    SurfaceKind::Plane => "Planar",
                    SurfaceKind::Cylinder => "Cylindrical",
                    SurfaceKind::Cone => "Conical",
                    SurfaceKind::Sphere => "Spherical",
                    SurfaceKind::Torus => "Toroidal",
                    SurfaceKind::Spline => "Spline",
                    SurfaceKind::Other => "Other",
                };
                let c = nf.centroid;
                let name = f.map(|f| f.name.clone()).unwrap_or("Body".into());
                let details = vec![
                    ("Feature".to_owned(), name.clone()),
                    ("Surface".to_owned(), surface.to_owned()),
                    ("Area".to_owned(), format!("{:.1} mm²", nf.area)),
                    ("Centroid".to_owned(), format!("{:.2}, {:.2}, {:.2}", c.x, c.y, c.z)),
                ];
                Some(EntityInfo { title: format!("{} · {}", name, nf.label), feature: f.map(|f| f.id), details })
            }
            Selection::Edge(eid) => {
                let ne = self.named_edge(eid)?;
                let f = self.feature(ne.feature);
                let name = f.map(|f| f.name.clone()).unwrap_or("Body".into());
                let details =
                    vec![("Feature".to_owned(), name.clone()), ("Length".to_owned(), format!("{:.2} mm", ne.length))];
                Some(EntityInfo { title: format!("{} · {}", name, ne.label), feature: f.map(|f| f.id), details })
            }
            Selection::Vertex(key) => {
                let f = self.feature(key.feature)?;
                let p = self.vertex_position(key)?;
                let details = vec![
                    ("Feature".to_owned(), f.name.clone()),
                    ("Position".to_owned(), format!("{:.2}, {:.2}, {:.2} mm", p.x, p.y, p.z)),
                ];
                Some(EntityInfo {
                    title: format!("{} · Vertex {}", f.name, key.index + 1),
                    feature: Some(f.id),
                    details,
                })
            }
        }
    }
}

/// The panels' state for a feature after a rebuild.
fn view_status(
    kind: &model::FeatureKind,
    status: model::FeatureStatus,
    names: &HashMap<model::FeatureId, String>,
) -> FeatureStatus {
    match status {
        model::FeatureStatus::Ok => FeatureStatus::Ok,
        model::FeatureStatus::Suppressed | model::FeatureStatus::RolledBack => FeatureStatus::Inactive,
        model::FeatureStatus::Failed(message) => FeatureStatus::Error(message),
        model::FeatureStatus::Blocked(input) => FeatureStatus::Error(format!(
            "{} needs repair first",
            names.get(&input).cloned().unwrap_or_else(|| input.to_string())
        )),
        model::FeatureStatus::RepairNeeded(repairs) => FeatureStatus::NeedsRepair(
            repairs
                .iter()
                .map(|r| {
                    let (index, kind, label) = match (r.slot, kind) {
                        (model::ReferenceSlot::SketchPlane, model::FeatureKind::Sketch(s)) => {
                            let label = match &s.plane {
                                model::SketchPlane::Face { face } => face.label.clone(),
                                model::SketchPlane::Datum { .. } => String::new(),
                            };
                            (0, RefKind::Face, label)
                        }
                        (
                            model::ReferenceSlot::Edge { index },
                            model::FeatureKind::Fillet(b) | model::FeatureKind::Chamfer(b),
                        ) => {
                            let label = b.edges.get(index as usize).map(|e| e.label.clone()).unwrap_or_default();
                            (index as usize, RefKind::Edge, label)
                        }
                        (model::ReferenceSlot::SketchPlane, _) => (0, RefKind::Face, String::new()),
                        (model::ReferenceSlot::Edge { index }, _) => (index as usize, RefKind::Edge, String::new()),
                    };
                    RepairItem { index, kind, label: if label.is_empty() { r.message.clone() } else { label } }
                })
                .collect(),
        ),
    }
}

/// The display mesh of a body that is only previewed: no picks, so nothing in it can be
/// selected or confused with the document's own body.
pub(crate) fn preview_mesh(
    kernel: &dyn Kernel,
    body: BodyId,
    params: &TessellationParams,
) -> Result<Arc<DisplayMesh>, String> {
    let topology = kernel.topology(body).map_err(|e| e.to_string())?;
    let mesh = kernel.tessellate(body, params).map_err(|e| e.to_string())?;
    let edges = edge_polylines(kernel, body, params);
    let mut picks = Vec::new();
    Ok(Arc::new(display_mesh(0, &mesh, &edges, &topology, &HashMap::new(), &mut picks)))
}

/// The body's edges sampled along their curves. A kernel that cannot sample them
/// leaves the body without edges rather than without faces.
fn edge_polylines(kernel: &dyn Kernel, body: BodyId, params: &TessellationParams) -> Vec<EdgePolyline> {
    kernel.tessellate_edges(body, params).unwrap_or_else(|e| {
        log::warn!("edges of body {body:?}: {e}");
        Vec::new()
    })
}

/// Builds the display mesh and registers its faces, edges and corners as picks.
/// `feature` 0 marks a preview whose picks are discarded.
fn display_mesh(
    feature: FeatureId,
    mesh: &Mesh,
    polylines: &[EdgePolyline],
    topology: &Topology,
    makers: &HashMap<FaceId, FeatureId>,
    picks: &mut Vec<PickEntry>,
) -> DisplayMesh {
    let mut out = DisplayMesh::default();
    let mut face_pick: HashMap<FaceId, u32> = HashMap::new();
    for f in &topology.faces {
        picks.push(PickEntry::Face(feature, f.id));
        face_pick.insert(f.id, picks.len() as u32);
    }
    // Unshare vertices so each carries its face.
    for (t, tri) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
        let face = mesh.triangle_faces.get(t).copied().unwrap_or(FaceId::from_raw(0));
        let pick = face_pick.get(&face).copied().unwrap_or(0);
        let maker = makers.get(&face).map(|f| *f as u32).unwrap_or(0);
        for &i in tri {
            let base = out.positions.len() as u32;
            out.positions.push(Vec3::from(mesh.positions[i as usize]));
            out.normals.push(Vec3::from(mesh.normals[i as usize]));
            out.face_picks.push(pick);
            out.face_features.push(maker);
            out.indices.push(base);
        }
    }
    // Edges in topology order, as segment pairs. A seam, where a closed face meets
    // itself, is not an edge anyone draws or picks.
    let lines: HashMap<EdgeId, &EdgePolyline> = polylines.iter().map(|l| (l.edge, l)).collect();
    for e in &topology.edges {
        let seam = e.faces.len() == 2 && e.faces[0] == e.faces[1];
        let Some(line) = lines.get(&e.id).filter(|l| l.points.len() >= 2 && !seam) else { continue };
        let segments = line.points.windows(2).flat_map(|w| [kv(w[0]), kv(w[1])]).collect();
        picks.push(PickEntry::Edge(feature, e.id));
        out.edges.push(DisplayEdge { pick: picks.len() as u32, segments });
    }
    // Corners: vertices where three or more faces meet, which leaves out the lone
    // vertex on a full circle's seam.
    let mut corners: Vec<Vec3> = topology
        .vertices
        .iter()
        .filter(|v| {
            let mut faces: Vec<FaceId> =
                v.edges.iter().filter_map(|e| topology.edge(*e)).flat_map(|e| e.faces.clone()).collect();
            faces.sort();
            faces.dedup();
            faces.len() >= 3
        })
        .map(|v| kv(v.position))
        .collect();
    corners.sort_by(|a, b| a.to_array().partial_cmp(&b.to_array()).unwrap_or(std::cmp::Ordering::Equal));
    for (i, p) in corners.into_iter().enumerate() {
        let k = VertexKey { feature, index: i as u32 };
        picks.push(PickEntry::Vertex(k, p));
        out.corners.push(DisplayCorner { pick: picks.len() as u32, position: p });
    }
    if feature == 0 {
        // A preview: no picks, and no face ids that could match the real body's.
        picks.clear();
        out.face_picks.iter_mut().for_each(|p| *p = 0);
    }
    out
}

pub fn fmt_mm(v: f64) -> String {
    if (v - v.round()).abs() < 1e-6 { format!("{}", v.round() as i64) } else { format!("{v:.2}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sketch::{ConstraintKind, EntityKind, P2, PointRef};

    fn doc() -> Document {
        Document::new("Test", Arc::new(soliddraft_kernel_catallax::CatallaxKernel::new()))
    }

    fn rect_sketch(w: f64, h: f64, at: P2) -> Sketch {
        let mut s = Sketch::new();
        let c = [at, at + P2::new(w, 0.0), at + P2::new(w, h), at + P2::new(0.0, h)];
        let ids: Vec<_> = (0..4).map(|i| s.add(EntityKind::Line { a: c[i], b: c[(i + 1) % 4] }, false)).collect();
        for i in 0..4 {
            s.constrain(ConstraintKind::Coincident {
                a: PointRef::new(ids[i], 1),
                b: PointRef::new(ids[(i + 1) % 4], 0),
            });
        }
        s
    }

    #[test]
    fn extrude_appears_in_tree_with_named_faces() {
        let mut d = doc();
        let id =
            d.apply(Command::AddExtrude { params: ExtrudeParams::default(), via_assistant: false }).unwrap().unwrap();
        assert_eq!(d.features.len(), 1);
        assert_eq!(d.features[0].name, "Boss-Extrude1");
        let labels: Vec<&str> = d.faces.iter().map(|f| f.label.as_str()).collect();
        assert!(labels.contains(&"Start cap") && labels.contains(&"End cap"), "{labels:?}");
        assert_eq!(labels.iter().filter(|l| l.starts_with("Side")).count(), 4);
        let end = d.faces.iter().find(|f| f.label == "End cap").unwrap().id;
        let info = d.describe(Selection::Face(end)).unwrap();
        assert_eq!(info.title, "Boss-Extrude1 · End cap");
        assert_eq!(info.feature, Some(id));
        let mesh = d.meshes.values().next().unwrap();
        assert_eq!(mesh.edges.len(), 12, "twelve box edges from the mesh");
        assert_eq!(mesh.corners.len(), 8, "eight corners");
        assert!(mesh.triangle_count() >= 12);
    }

    #[test]
    fn invalid_extrude_reports_error_and_keeps_feature() {
        let mut d = doc();
        let params = ExtrudeParams { depth: 0.0, ..Default::default() };
        let err = d.apply(Command::AddExtrude { params, via_assistant: false }).unwrap_err();
        assert!(err.contains("depth"), "{err}");
        assert_eq!(d.features.len(), 1);
        assert!(matches!(d.features[0].status, FeatureStatus::Error(_)));
        assert!(d.meshes.is_empty());
    }

    #[test]
    fn selection_survives_a_rebuild_by_name_and_undo_restores() {
        let mut d = doc();
        let id =
            d.apply(Command::AddExtrude { params: ExtrudeParams::default(), via_assistant: false }).unwrap().unwrap();
        let end = d.faces.iter().find(|f| f.label == "End cap").unwrap().id;
        d.selection = Some(Selection::Face(end));
        let params = ExtrudeParams { depth: 35.0, ..Default::default() };
        d.apply(Command::EditExtrude { id, params }).unwrap();
        let title = d.describe(d.selection.unwrap()).unwrap().title;
        assert_eq!(title, "Boss-Extrude1 · End cap");
        d.undo().unwrap();
        let FeatureKind::Extrude(p) = &d.features[0].kind else { panic!() };
        assert_eq!(p.depth, 20.0);
        d.redo().unwrap();
        let FeatureKind::Extrude(p) = &d.features[0].kind else { panic!() };
        assert_eq!(p.depth, 35.0);
        d.apply(Command::DeleteFeature(id)).unwrap();
        assert!(d.selection.is_none());
        assert!(d.features.is_empty());
        d.undo().unwrap();
        assert_eq!(d.features.len(), 1);
    }

    #[test]
    fn second_extrude_merges_into_one_body_and_cut_removes_material() {
        let mut d = doc();
        d.apply(Command::AddExtrude {
            params: ExtrudeParams {
                profile: ProfileKind::Rectangle { width: 100.0, height: 60.0 },
                depth: 10.0,
                ..Default::default()
            },
            via_assistant: false,
        })
        .unwrap();
        let v0 = d.mass_properties(d.features[0].id).unwrap().volume;
        // A boss sketched on the top face of the base.
        let top = d.faces.iter().find(|f| f.label == "End cap").unwrap().id;
        let face = d.face_ref(top).unwrap();
        let sk = d.apply(Command::AddSketch { plane: SketchPlane::Face { face } }).unwrap().unwrap();
        let plane = d.feature(sk).unwrap().plane.unwrap();
        assert!((plane.origin.z - 10.0).abs() < 1e-6 && (plane.normal.z - 1.0).abs() < 1e-6, "{plane:?}");
        let mut s = Sketch::new();
        s.add(EntityKind::Circle { center: P2::new(10.0, 0.0), radius: 8.0 }, false);
        d.apply(Command::EditSketch { id: sk, sketch: s }).unwrap();
        let boss = d
            .apply(Command::AddExtrude {
                params: ExtrudeParams { sketch: Some(sk), depth: 15.0, ..Default::default() },
                via_assistant: false,
            })
            .unwrap()
            .unwrap();
        assert_eq!(d.meshes.len(), 1, "one merged body");
        let v1 = d.mass_properties(boss).unwrap().volume;
        assert!((v1 - v0 - std::f64::consts::PI * 64.0 * 15.0).abs() < 1.0, "{v0} -> {v1}");
        assert_eq!(d.consumer_of(sk), Some(boss));
        // A bore through the boss from its top face.
        let boss_top = d.faces.iter().find(|f| f.feature == boss && f.label == "End cap").unwrap().id;
        let face = d.face_ref(boss_top).unwrap();
        let sk2 = d.apply(Command::AddSketch { plane: SketchPlane::Face { face } }).unwrap().unwrap();
        let mut s = Sketch::new();
        s.add(EntityKind::Circle { center: P2::new(10.0, 0.0), radius: 4.0 }, false);
        d.apply(Command::EditSketch { id: sk2, sketch: s }).unwrap();
        let cut = d.apply(Command::AddCut { params: CutParams::new(sk2) }).unwrap().unwrap();
        let v2 = d.mass_properties(cut).unwrap().volume;
        assert!(v2 < v1 - std::f64::consts::PI * 16.0 * 20.0, "{v1} -> {v2}");
        assert!(d.faces.iter().any(|f| f.feature == cut && f.surface == SurfaceKind::Cylinder));
    }

    #[test]
    fn existing_fillet_waits_for_its_stage_and_keeps_the_body_through_edits_and_reopen() {
        let mut d = doc();
        let base = d
            .apply(Command::AddExtrude {
                params: ExtrudeParams {
                    profile: ProfileKind::Rectangle { width: 100.0, height: 60.0 },
                    depth: 10.0,
                    ..Default::default()
                },
                via_assistant: false,
            })
            .unwrap()
            .unwrap();
        let flange_sketch =
            d.apply(Command::AddSketch { plane: SketchPlane::Reference { plane: PlaneKind::Front } }).unwrap().unwrap();
        d.apply(Command::EditSketch { id: flange_sketch, sketch: rect_sketch(100.0, 40.0, P2::new(-50.0, 0.0)) })
            .unwrap();
        let flange = d
            .apply(Command::AddExtrude {
                params: ExtrudeParams { sketch: Some(flange_sketch), depth: 10.0, flip: true, ..Default::default() },
                via_assistant: false,
            })
            .unwrap()
            .unwrap();
        // The inside corner: the edge the merge created on the top face, at y = 10, z = 10.
        let inside = d
            .edges
            .iter()
            .filter(|e| e.feature == flange)
            .min_by(|a, b| {
                (a.midpoint - Vec3::new(0.0, 10.0, 10.0))
                    .length()
                    .total_cmp(&(b.midpoint - Vec3::new(0.0, 10.0, 10.0)).length())
            })
            .unwrap();
        let r = d.edge_ref(inside.id).unwrap();
        assert!(r.label.contains("Boss-Extrude2"), "{}", r.label);
        let error = d.apply(Command::AddFillet { params: FilletParams { edges: vec![r], radius: 4.0 } }).unwrap_err();
        assert!(error.contains("Catallax stage 3") && error.contains("Catallax stage 4"), "{error}");
        let fillet = d.features.last().unwrap().id;
        let waiting = |d: &Document| {
            assert!(matches!(&d.feature(fillet).unwrap().status, FeatureStatus::Error(error)
                if error.contains("Catallax stage 3") && error.contains("Catallax stage 4")));
            assert_eq!(d.model.body_after(model_id(fillet)), d.model.body_before(model_id(fillet)));
        };
        waiting(&d);
        let reopened =
            Document::from_model(model::Document::from_bytes(&d.model.to_bytes().unwrap(), crate::kernel()).unwrap());
        waiting(&reopened);
        let v_before = d.mass_properties(fillet).unwrap().volume;
        // The waiting feature keeps the rebuilt body after a base edit.
        d.apply(Command::EditExtrude {
            id: base,
            params: ExtrudeParams {
                profile: ProfileKind::Rectangle { width: 100.0, height: 60.0 },
                depth: 14.0,
                ..Default::default()
            },
        })
        .unwrap();
        waiting(&d);
        let v_after = d.mass_properties(fillet).unwrap().volume;
        assert!(v_after > v_before);
        // Suppress the flange: the edge is gone and the fillet asks for repair.
        d.apply(Command::Suppress { id: flange, suppressed: true }).unwrap();
        assert!(matches!(d.feature(fillet).unwrap().status, FeatureStatus::NeedsRepair(_)));
        assert!(d.display_body().is_some(), "the body passes through a skipped feature");
        d.apply(Command::Suppress { id: flange, suppressed: false }).unwrap();
        waiting(&d);
        // Rollback hides the fillet without removing it.
        d.apply(Command::Rollback(Some(3))).unwrap();
        assert_eq!(d.feature(fillet).unwrap().status, FeatureStatus::Inactive);
        assert_eq!(d.display_index(), Some(2));
        d.apply(Command::Rollback(None)).unwrap();
        waiting(&d);
    }
}
