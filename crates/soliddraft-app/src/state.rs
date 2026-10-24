//! Shared application state and the action queue panels write into. Panels
//! never mutate the document directly; they push an `AppAction` and the app
//! applies the queue once per frame, so every change goes through one place.

pub use crate::chat::{ChatMessage, ChatState, Role};
use crate::document::{
    ChamferParams, Command, CutParams, DisplayMesh, Document, EntityInfo, ExtrudeParams, FeatureId, FilletParams,
    Preview, Selection, SketchPlane,
};
use crate::document::{EdgeRef, EntityRef};
use crate::drag::{Dimension, DragPreview, Finish};
use crate::sketch::{ConstraintId, EntityId, P2, PointRef, Sketch};
use crate::viewport::ViewportState;
use crate::viewport::camera::StandardView;
use crate::viewport::look::Preset;
use crate::viewport::render::DisplayMode;
use glam::Vec3;
use soliddraft_kernel_api::Plane;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use web_time::Instant;

/// What to open when a sketch is left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitInto {
    Extrude,
    Cut,
}

// A drawing action can carry a placed view; actions are made one at a time by a click.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum AppAction {
    Command(Command),
    Undo,
    Redo,
    /// Starts a sketch on the selected plane or face, or asks for one.
    StartSketch,
    StartSketchOn(SketchPlane),
    /// Opens the sketch feature for editing.
    EditSketch(FeatureId),
    ExitSketch(Option<ExitInto>),
    StartExtrude,
    StartCut,
    StartFillet,
    StartChamfer,
    StartRepair(FeatureId),
    EditFeature(FeatureId),
    SketchTool(Tool),
    /// Toggles construction on the sketch selection.
    ToggleConstruction,
    /// Deletes the sketch selection, or the selected feature.
    DeleteSelection,
    RenameSelected,
    Select(Option<Selection>),
    SetView(StandardView),
    /// Look straight at the active sketch plane, or the selected planar face.
    NormalTo,
    ZoomFit,
    SetDisplay(DisplayMode),
    /// Switches the render look (Fast, Studio, Realistic).
    SetPreset(Preset),
    ToggleProjection,
    CancelEditor,
    CommitEditor,
    PreviewProposal(String),
    ApplyProposal(String),
    DiscardProposal(String),
    UndoProposal(String),
    FocusChat,
    SendChat,
    StopChat,
    /// Sends a ready-made question to the assistant.
    Ask(String),
    ToggleAutoApply,
    /// Opens the Provider dialog: which provider and model the assistant uses.
    ShowProviderSettings,
    /// Opens the File locations page, on one kind of reference file or the first.
    ShowFileLocations(Option<soliddraft_document::LocationKind>),
    NewPart,
    /// `None` asks with a file dialog.
    Open(Option<PathBuf>),
    Save,
    SaveAs(Option<PathBuf>),
    /// Exports the part's body. `None` asks with a file dialog.
    Export {
        format: soliddraft_exchange::Format,
        path: Option<PathBuf>,
    },
    ClearRecent,
    ResetLayout,
    /// Show or hide one side area.
    TogglePanel(crate::panels::Side),
    ShowShortcuts,
    ShowAbout,
    /// Opens the Recover documents window.
    ShowRecovery,
    /// Opens the document in a recovery copy, in place of the open one of its kind.
    RestoreRecovery(PathBuf),
    /// Deletes a recovery copy the user does not want.
    DiscardRecovery(PathBuf),
    Quit,
    /// Everything the drawing tab does.
    Drawing(crate::drawing::DrawingAction),
    /// A step of a drag of a feature's dimension.
    DragDimension(DragStep),
}

/// The steps of a drag of a feature's dimension, in the order a drag makes them.
#[derive(Clone, Debug, PartialEq)]
pub enum DragStep {
    Start {
        feature: FeatureId,
        dimension: Dimension,
    },
    Move(f64),
    /// The pointer let go: the final value is applied.
    Finish,
    /// Escape: nothing is applied.
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RibbonTab {
    Features,
    Sketch,
    Evaluate,
    View,
    Assistant,
}

impl RibbonTab {
    pub const ALL: [RibbonTab; 5] =
        [RibbonTab::Features, RibbonTab::Sketch, RibbonTab::Evaluate, RibbonTab::View, RibbonTab::Assistant];

    pub fn label(&self) -> &'static str {
        match self {
            RibbonTab::Features => "Features",
            RibbonTab::Sketch => "Sketch",
            RibbonTab::Evaluate => "Evaluate",
            RibbonTab::View => "View",
            RibbonTab::Assistant => "Assistant",
        }
    }
}

/// The PropertyManager while a feature is being created or edited.
pub struct FeatureEditor {
    pub target: Option<FeatureId>,
    pub params: EditorParams,
    pub preview: Preview,
    pub error: Option<String>,
    /// The document's rollback before the editor rolled the display back to the
    /// feature being edited.
    pub saved_rollback: Option<usize>,
    last: Option<EditorParams>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EditorParams {
    Extrude(ExtrudeParams),
    Cut(CutParams),
    Fillet(FilletParams),
    Chamfer(ChamferParams),
    /// Picking a plane or planar face for a new sketch.
    PickPlane,
    /// Choosing replacements for a feature's missing references.
    Repair {
        items: Vec<crate::document::RepairItem>,
        chosen: Vec<Option<EntityRef>>,
        active: usize,
    },
}

impl FeatureEditor {
    pub fn new(target: Option<FeatureId>, params: EditorParams, saved_rollback: Option<usize>) -> Self {
        Self { target, params, preview: Preview::default(), error: None, saved_rollback, last: None }
    }

    pub fn title(&self) -> String {
        let edit = if self.target.is_some() { "Edit " } else { "" };
        match &self.params {
            EditorParams::Extrude(_) => format!("{edit}Boss-Extrude"),
            EditorParams::Cut(_) => format!("{edit}Cut-Extrude"),
            EditorParams::Fillet(_) => format!("{edit}Fillet"),
            EditorParams::Chamfer(_) => format!("{edit}Chamfer"),
            EditorParams::PickPlane => "Sketch".to_owned(),
            EditorParams::Repair { .. } => "Repair references".to_owned(),
        }
    }

    /// Edge references the editor collects from viewport clicks.
    pub fn edges_mut(&mut self) -> Option<&mut Vec<EdgeRef>> {
        match &mut self.params {
            EditorParams::Fillet(f) => Some(&mut f.edges),
            EditorParams::Chamfer(c) => Some(&mut c.edges),
            _ => None,
        }
    }

    pub fn edges(&self) -> &[EdgeRef] {
        match &self.params {
            EditorParams::Fillet(f) => &f.edges,
            EditorParams::Chamfer(c) => &c.edges,
            _ => &[],
        }
    }

    /// Regenerates the preview when the parameters changed.
    pub fn refresh(&mut self, doc: &Document) {
        if self.last.as_ref() == Some(&self.params) {
            return;
        }
        self.last = Some(self.params.clone());
        let before = match self.target.and_then(|id| doc.index_of(id)) {
            Some(i) => doc.body_before(i),
            None => doc.display_body(),
        };
        let result = match &self.params {
            EditorParams::Extrude(p) => doc.preview_extrude(p),
            EditorParams::Cut(p) => doc.preview_cut(p, before),
            EditorParams::Fillet(f) => doc.preview_rounding(&f.edges, f.radius, false, before),
            EditorParams::Chamfer(c) => doc.preview_rounding(&c.edges, c.distance, true, before),
            EditorParams::PickPlane | EditorParams::Repair { .. } => Ok(Preview::default()),
        };
        match result {
            Ok(p) => {
                self.preview = p;
                self.error = None;
            }
            Err(e) => {
                self.preview = Preview::default();
                self.error = Some(e);
            }
        }
    }
}

/// The sketch tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Line,
    Rectangle,
    Circle,
    Arc,
    Slot,
    Point,
    Dimension,
}

impl Tool {
    pub fn label(&self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Line => "Line",
            Tool::Rectangle => "Rectangle",
            Tool::Circle => "Circle",
            Tool::Arc => "Arc",
            Tool::Slot => "Slot",
            Tool::Point => "Point",
            Tool::Dimension => "Smart dimension",
        }
    }
}

/// Something selected inside a sketch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SketchSel {
    Entity(EntityId),
    Point(PointRef),
    Constraint(ConstraintId),
}

/// A dimension value being typed.
#[derive(Clone, Debug, PartialEq)]
pub struct DimEdit {
    pub constraint: ConstraintId,
    pub text: String,
    pub focus: bool,
}

/// An inference the sketcher offers at the cursor.
#[derive(Clone, Debug, PartialEq)]
pub enum Snap {
    /// Snapped onto an existing point.
    Point(PointRef),
    /// Snapped onto the middle of a line.
    Midpoint(EntityId),
    /// Snapped onto an entity's curve.
    On(EntityId),
    /// Level with the previous point.
    Horizontal,
    /// Plumb with the previous point.
    Vertical,
}

/// The sketcher while a sketch is open.
pub struct SketchSession {
    pub feature: FeatureId,
    pub plane: Plane,
    pub sketch: Sketch,
    pub tool: Tool,
    /// Points placed so far by the current tool, with the snaps that landed them.
    pub pending: Vec<(P2, Vec<Snap>)>,
    /// The end of the last line drawn, so the next line chains from it.
    pub chain_from: Option<PointRef>,
    pub selection: Vec<SketchSel>,
    pub hover: Option<SketchSel>,
    pub cursor: Option<P2>,
    pub snaps: Vec<Snap>,
    pub drag: Option<SketchDrag>,
    pub dim_edit: Option<DimEdit>,
    /// Smart dimension: the first picked item, waiting for a second or a placement.
    pub dim_first: Option<SketchSel>,
    pub dim_second: Option<SketchSel>,
    pub was_orthographic: bool,
    pub message: Option<(String, Instant)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SketchDrag {
    Point { point: PointRef, moved: bool },
    Entity { entity: EntityId, last: P2, moved: bool },
    Label { constraint: ConstraintId, offset: P2 },
}

impl SketchSession {
    pub fn new(feature: FeatureId, plane: Plane, sketch: Sketch, was_orthographic: bool) -> Self {
        Self {
            feature,
            plane,
            sketch,
            tool: Tool::Select,
            pending: Vec::new(),
            chain_from: None,
            selection: Vec::new(),
            hover: None,
            cursor: None,
            snaps: Vec::new(),
            drag: None,
            dim_edit: None,
            dim_first: None,
            dim_second: None,
            was_orthographic,
            message: None,
        }
    }

    pub fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
        self.pending.clear();
        self.chain_from = None;
        self.dim_first = None;
        self.dim_second = None;
        self.dim_edit = None;
        if tool != Tool::Select {
            self.selection.clear();
        }
    }

    pub fn say(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), Instant::now()));
    }

    pub fn selected_entities(&self) -> Vec<EntityId> {
        self.selection
            .iter()
            .filter_map(|s| match s {
                SketchSel::Entity(e) => Some(*e),
                _ => None,
            })
            .collect()
    }

    pub fn selected_points(&self) -> Vec<PointRef> {
        self.selection
            .iter()
            .filter_map(|s| match s {
                SketchSel::Point(p) => Some(*p),
                _ => None,
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteKind {
    Info,
    Ok,
    Error,
}

pub struct Note {
    pub text: String,
    pub kind: NoteKind,
    pub at: Instant,
}

/// Inline rename of a tree row.
#[derive(Clone, Debug, PartialEq)]
pub struct Rename {
    pub feature: FeatureId,
    pub text: String,
    pub focus: bool,
}

pub struct AppState {
    pub doc: Document,
    pub viewport: ViewportState,
    pub editor: Option<FeatureEditor>,
    pub sketch: Option<SketchSession>,
    pub ribbon_tab: RibbonTab,
    pub chat: ChatState,
    pub note: Option<Note>,
    pub expanded: HashSet<FeatureId>,
    pub origin_open: bool,
    pub hover_info: Option<EntityInfo>,
    pub hover_point: Option<Vec3>,
    pub search: String,
    pub show_about: bool,
    pub show_shortcuts: bool,
    /// The open Provider dialog.
    pub provider_dialog: Option<crate::chat::ProviderDialog>,
    /// The open File locations page.
    pub locations_dialog: Option<crate::locations::LocationsDialog>,
    /// Bumped when File locations are saved, so cached lookups refresh.
    pub locations_epoch: u64,
    pub rename: Option<Rename>,
    /// The tree row being dragged for reordering.
    pub drag_feature: Option<FeatureId>,
    /// The feature dimension being dragged, with its previews.
    pub drag_preview: Option<DragPreview>,
    pub drag_rollback: bool,
    /// The feature whose row the tree last scrolled to.
    pub tree_followed: Option<FeatureId>,
    pub recent: Vec<PathBuf>,
    /// Smoothed frame time in milliseconds, for the status bar.
    pub frame_ms: f32,
    /// The open drawing, the New drawing PropertyManager and which document is active.
    pub drawing: crate::drawing::DrawingState,
    /// Which side areas are shown and how wide they are.
    pub layout: crate::panels::Layout,
    /// Where pinned revisions of referenced documents come from. Cache-only for now: the
    /// revisions this session opened or saved.
    pub revisions: Arc<dyn soliddraft_document::RevisionStore>,
    /// The commands run lately and the pinned ones, for the command search.
    pub commands: crate::commands::History,
    /// Autosave and the copies found after a crash. `None` until the app starts it, and
    /// in tests and automated runs that do not.
    pub recovery: Option<crate::recovery::Recovery>,
    actions: Vec<AppAction>,
}

impl AppState {
    pub fn new(doc: Document) -> Self {
        Self {
            doc,
            viewport: ViewportState::default(),
            editor: None,
            sketch: None,
            ribbon_tab: RibbonTab::Features,
            chat: ChatState::offline("The assistant is not connected.".into()),
            note: None,
            expanded: HashSet::new(),
            origin_open: false,
            hover_info: None,
            hover_point: None,
            search: String::new(),
            show_about: false,
            show_shortcuts: false,
            provider_dialog: None,
            locations_dialog: None,
            locations_epoch: 0,
            rename: None,
            drag_feature: None,
            drag_preview: None,
            drag_rollback: false,
            tree_followed: None,
            recent: crate::file::recent(),
            frame_ms: 0.0,
            drawing: Default::default(),
            layout: crate::panels::Layout::load(),
            revisions: Arc::new(soliddraft_document::CacheOnlyStore::new()),
            commands: crate::settings::command_history(),
            recovery: None,
            actions: Vec::new(),
        }
    }

    /// Hands the revision store the part as it is now, after it was opened or saved.
    pub fn remember_revision(&self) {
        let model = self.doc.model();
        if let Ok(bytes) = model.to_bytes() {
            self.revisions.record(model.id(), &model.semantic_hash(), &bytes);
        }
    }

    pub fn push(&mut self, action: AppAction) {
        self.actions.push(action);
    }

    /// Runs a command from the registry, as a ribbon button, a shortcut, a menu or the
    /// command search does, and remembers it as recent. A disabled command runs nothing
    /// and says why, when its hint has a reason.
    pub fn run_command(&mut self, command: &crate::commands::CommandSpec) {
        match command.action_now(self) {
            Some(action) => {
                self.commands.record(command.id);
                self.push(action);
            }
            None => {
                if let Some(why) = command.hint_now(self) {
                    self.note(why, NoteKind::Info);
                }
            }
        }
    }

    pub fn take_actions(&mut self) -> Vec<AppAction> {
        std::mem::take(&mut self.actions)
    }

    pub fn note(&mut self, text: impl Into<String>, kind: NoteKind) {
        self.note = Some(Note { text: text.into(), kind, at: Instant::now() });
    }

    /// Ghost geometry the viewport shows this frame: added or removed material.
    pub fn ghost(&self) -> Option<(Arc<DisplayMesh>, bool)> {
        if let Some(e) = &self.editor
            && let Some(t) = &e.preview.tool
        {
            return Some((t.clone(), e.preview.removes));
        }
        let (_, p) = self.chat.preview.as_ref()?;
        p.tool.clone().map(|t| (t, p.removes))
    }

    /// A body drawn in place of the document's while a feature is previewed.
    pub fn preview_body(&self) -> Option<Arc<DisplayMesh>> {
        if let Some(d) = &self.drag_preview {
            return d.mesh();
        }
        match &self.editor {
            Some(e) => e.preview.result.clone(),
            None => self.chat.preview.as_ref().and_then(|(_, p)| p.result.clone()),
        }
    }

    /// Runs one step of a dimension drag. Only the final value reaches the document, as
    /// one command; a drag the part changed under applies nothing.
    pub fn drag_dimension(&mut self, step: DragStep) {
        match step {
            DragStep::Start { feature, dimension } => {
                if self.editor.is_some() || self.sketch.is_some() {
                    return;
                }
                match DragPreview::start(&self.doc, feature, dimension) {
                    Ok(d) => self.drag_preview = Some(d),
                    Err(e) => self.note(e, NoteKind::Error),
                }
            }
            DragStep::Move(value) => {
                if let Some(d) = self.drag_preview.as_mut() {
                    d.request(value);
                }
            }
            DragStep::Finish => {
                let Some(d) = self.drag_preview.take() else { return };
                let what = d.dimension().label();
                match d.finish(&mut self.doc) {
                    Finish::Applied { value } => {
                        self.note(format!("{what} {} mm", crate::document::fmt_mm(value)), NoteKind::Ok)
                    }
                    Finish::Failed { value, message } | Finish::Refused { value, message } => self.note(
                        format!("{what} {} mm did not build: {message}", crate::document::fmt_mm(value)),
                        NoteKind::Error,
                    ),
                    Finish::Obsolete => self.note("The part changed during the drag; nothing applied", NoteKind::Info),
                    Finish::Unchanged => {}
                }
            }
            DragStep::Cancel => {
                if self.drag_preview.take().is_some() {
                    self.note("Drag cancelled", NoteKind::Info);
                }
            }
        }
    }

    /// Takes a dimension drag's answers, once per frame. A drag whose setup changed under
    /// it, such as by an undo, an assistant command or a new selection, ends with nothing
    /// applied. Returns true while the drag waits for an answer.
    pub fn poll_drag(&mut self) -> bool {
        let Some(d) = self.drag_preview.as_mut() else { return false };
        if !d.is_current(&self.doc) {
            self.drag_preview = None;
            self.note("The part changed during the drag; nothing applied", NoteKind::Info);
            return false;
        }
        d.poll();
        d.pending()
    }

    pub fn selected_feature(&self) -> Option<FeatureId> {
        self.doc.selection.and_then(|s| self.doc.selection_feature(s))
    }

    /// The feature a sketch-based command would use: the selected sketch, or the
    /// sketch under the selected feature.
    pub fn selected_sketch(&self) -> Option<FeatureId> {
        let id = self.selected_feature()?;
        let f = self.doc.feature(id)?;
        if f.is_sketch() { Some(id) } else { f.kind.sketch() }
    }

    pub fn in_sketch(&self) -> bool {
        self.sketch.is_some()
    }
}
