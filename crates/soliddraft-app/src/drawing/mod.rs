//! Drawings: 2D sheets of the part with views, dimensions and notes, as a document that
//! opens in a tab beside the part. The sheet canvas, the drawing ribbon tabs, the drawing
//! tree, the PropertyManagers and the automatic update live here; the model types come
//! from `soliddraft-drawing`.
//!
//! Hooks into the shell are small: `Tab::Drawing` in `app.rs`, one arm in `ribbon.rs`,
//! one `AppAction::Drawing` variant and one `DrawingState` field in `state.rs`.

pub mod canvas;
pub mod document;
pub mod part;
pub mod properties_d3;
pub mod properties_drawing;
pub mod reference;
pub mod ribbon_drawing;
pub mod sheets;
pub mod tools;
pub mod tree_drawing;

pub use document::DrawingDoc;
use part::PartContext;
pub use reference::{LinkState, ModelLink, RepairDialog};
pub use soliddraft_drawing::SHEET;
use soliddraft_drawing::display::{self, Owner};
use soliddraft_drawing::export;
use soliddraft_drawing::library;
use soliddraft_drawing::measure::{Pick, Proposal};
use soliddraft_drawing::project::{self, Projected};

use crate::app::Tab;
use crate::state::{AppState, NoteKind};
use egui_dock::DockState;
use soliddraft_document::DocumentId;
use soliddraft_drawing::{
    Anchor, Annotation, AnnotationId, AnnotationKind, AnnotationState, Command, DatumRef, DocKind, DocRef, Drawing,
    DrawingMeta, FoldDirection, GdtCharacteristic, Lay, LinearOrientation, MaterialCondition, Projection, Scale,
    SetKind, StyleOverrides, SurfaceProcess, View, ViewId,
};
use soliddraft_kernel_api::Point2;
use soliddraft_kernel_api::drawing::StandardView;
use std::collections::HashMap;
use std::path::PathBuf;
use web_time::Instant;

/// The drawing's ribbon tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawingTab {
    Views,
    Annotate,
    Sheet,
    View,
    Assistant,
}

impl DrawingTab {
    pub const ALL: [DrawingTab; 5] =
        [DrawingTab::Views, DrawingTab::Annotate, DrawingTab::Sheet, DrawingTab::View, DrawingTab::Assistant];

    pub fn label(&self) -> &'static str {
        match self {
            DrawingTab::Views => "Views",
            DrawingTab::Annotate => "Annotate",
            DrawingTab::Sheet => "Sheet",
            DrawingTab::View => "View",
            DrawingTab::Assistant => "Assistant",
        }
    }
}

/// What a new view is placed as.
#[derive(Clone, Debug, PartialEq)]
pub enum PlaceKind {
    /// A base view of a standard orientation.
    Standard(StandardView),
    /// A view folded off a parent; the direction follows the pointer.
    Projected { parent: ViewId },
}

/// The sheet canvas tools, small state machines driven by clicks.
#[derive(Clone, Debug, PartialEq)]
pub enum DrawingTool {
    Select,
    PlaceView(PlaceKind),
    SmartDimension,
    Note,
    HoleCallout,
    CentreMark,
    /// Picking a new entity for a lost end of an annotation.
    Reattach {
        annotation: AnnotationId,
        end: usize,
    },
    /// A section view: two clicks draw the cutting line on the parent, a third places
    /// the view. With `replace`, the clicks redraw an existing section's line.
    Section {
        replace: Option<ViewId>,
    },
    /// A detail view: a click for the centre, one for the radius (or several for a
    /// profile), one to place.
    Detail,
    /// An auxiliary view: click the reference edge, then place.
    Auxiliary,
    /// Crop a view to a rectangle: two corners.
    Crop,
    /// A broken-out section: a click for the centre and one for the radius.
    BrokenOut,
    /// Ordinate, baseline or chain dimensions: pick the points, click to place.
    DimensionSet(SetKind),
    /// A feature control frame: click an edge, then where the frame goes.
    Gdt,
    /// A datum feature symbol: click an edge, then where the letter goes.
    Datum,
    /// A surface finish symbol: click a surface's edge, then where the symbol goes.
    SurfaceFinish,
    /// A revision symbol: click an edge or a place, then where the triangle goes.
    RevisionSymbol,
}

impl DrawingTool {
    pub fn label(&self) -> &'static str {
        match self {
            DrawingTool::Select => "Select",
            DrawingTool::PlaceView(PlaceKind::Standard(_)) => "Model view",
            DrawingTool::PlaceView(PlaceKind::Projected { .. }) => "Projected view",
            DrawingTool::SmartDimension => "Smart dimension",
            DrawingTool::Note => "Note",
            DrawingTool::HoleCallout => "Hole callout",
            DrawingTool::CentreMark => "Centre mark",
            DrawingTool::Reattach { .. } => "Reattach",
            DrawingTool::Section { replace: None } => "Section view",
            DrawingTool::Section { replace: Some(_) } => "Redraw cutting line",
            DrawingTool::Detail => "Detail view",
            DrawingTool::Auxiliary => "Auxiliary view",
            DrawingTool::Crop => "Crop view",
            DrawingTool::BrokenOut => "Broken-out section",
            DrawingTool::DimensionSet(SetKind::Ordinate) => "Ordinate dimensions",
            DrawingTool::DimensionSet(SetKind::Baseline) => "Baseline dimensions",
            DrawingTool::DimensionSet(SetKind::Chain) => "Chain dimensions",
            DrawingTool::Gdt => "Geometric tolerance",
            DrawingTool::Datum => "Datum feature",
            DrawingTool::SurfaceFinish => "Surface finish",
            DrawingTool::RevisionSymbol => "Revision symbol",
        }
    }

    /// True for the tools that draw or place something view by view: their prompt
    /// comes from the draft's stage rather than from the pick count.
    pub fn is_derived(&self) -> bool {
        matches!(
            self,
            DrawingTool::Section { .. }
                | DrawingTool::Detail
                | DrawingTool::Auxiliary
                | DrawingTool::Crop
                | DrawingTool::BrokenOut
        )
    }

    /// True for the tools that place a symbol with an anchor and a placement click.
    pub fn is_symbol(&self) -> bool {
        matches!(self, DrawingTool::Gdt | DrawingTool::Datum | DrawingTool::SurfaceFinish | DrawingTool::RevisionSymbol)
    }

    /// What the status pill tells the user to do next. `picks` is the number of picks
    /// or clicks made so far; `pending` is true while a derived view waits for its place.
    pub fn prompt(&self, picks: usize) -> String {
        self.prompt_at(picks, false)
    }

    /// See [`prompt`](Self::prompt).
    pub fn prompt_at(&self, picks: usize, pending: bool) -> String {
        match self {
            DrawingTool::Select => String::new(),
            DrawingTool::PlaceView(PlaceKind::Standard(v)) => {
                format!("{}: click to place. Esc cancels.", project::standard_label(*v))
            }
            DrawingTool::PlaceView(PlaceKind::Projected { .. }) => {
                "Click to place. Esc cancels. The view folds off its parent; hold Ctrl to break alignment.".into()
            }
            DrawingTool::SmartDimension => match picks {
                0 => "Smart dimension · pick an edge, circle, arc, two edges or two ends".into(),
                _ => "Smart dimension · pick a second item, or click to place".into(),
            },
            DrawingTool::Note => "Note · click an edge for a leader, or anywhere for plain text".into(),
            DrawingTool::HoleCallout => "Hole callout · click a hole circle seen face-on".into(),
            DrawingTool::CentreMark => "Centre mark · click a circle seen face-on".into(),
            DrawingTool::Reattach { .. } => "Reattach · click a new edge or point for the lost end marked ×".into(),
            DrawingTool::Section { .. } if pending => {
                "Section · click to place along the alignment. F flips the direction, Ctrl frees the view.".into()
            }
            DrawingTool::Section { .. } => match picks {
                0 => "Section · click where the cutting line starts; circle centres snap".into(),
                _ => "Section · move for a horizontal or vertical line, click to finish it".into(),
            },
            DrawingTool::Detail if pending => "Detail · click to place the magnified view".into(),
            DrawingTool::Detail => match picks {
                0 => "Detail · click the centre of the region on a view".into(),
                1 => "Detail · move for the radius, click to finish the circle".into(),
                _ => "Detail · click more corners; click the first corner or press Enter to close".into(),
            },
            DrawingTool::Auxiliary if pending => {
                "Auxiliary · click to place. F looks from the other side of the edge.".into()
            }
            DrawingTool::Auxiliary => "Auxiliary · click the reference edge; the view unfolds square to it".into(),
            DrawingTool::Crop => match picks {
                0 => "Crop · click the first corner of the region to keep".into(),
                _ => "Crop · click the opposite corner".into(),
            },
            DrawingTool::BrokenOut => match picks {
                0 => "Broken-out · click the centre of the region to cut away".into(),
                _ => "Broken-out · move for the radius, click to cut".into(),
            },
            DrawingTool::DimensionSet(_) => match picks {
                0 => "Pick the origin: an edge end, a circle centre or an edge".into(),
                1 => "Pick the points to dimension; click away from the edges to place".into(),
                _ => "Pick more points, or click away from the edges to place".into(),
            },
            DrawingTool::Gdt => match picks {
                0 => "Geometric tolerance · click the toleranced edge, or anywhere for a free frame".into(),
                _ => "Geometric tolerance · click where the frame goes".into(),
            },
            DrawingTool::Datum => match picks {
                0 => "Datum · click the edge of the datum feature".into(),
                _ => "Datum · click where the letter goes".into(),
            },
            DrawingTool::SurfaceFinish => match picks {
                0 => "Surface finish · click the surface's edge".into(),
                _ => "Surface finish · click where the symbol goes".into(),
            },
            DrawingTool::RevisionSymbol => match picks {
                0 => "Revision symbol · click the changed edge, or anywhere for a free symbol".into(),
                _ => "Revision symbol · click where the triangle goes".into(),
            },
        }
    }
}

/// What is selected on the sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawingSel {
    View(ViewId),
    Annotation(AnnotationId),
    /// The sheet format: border and title block.
    Format,
    /// The current sheet's revision table, made on the first revision.
    Revisions,
    /// The model reference: the part the drawing shows.
    Model,
}

/// A feature control frame being built or edited.
#[derive(Clone, Debug, PartialEq)]
pub struct GdtDraft {
    pub characteristic: GdtCharacteristic,
    pub tolerance: f64,
    pub diameter: bool,
    pub material: MaterialCondition,
    pub datums: Vec<DatumRef>,
}

impl Default for GdtDraft {
    fn default() -> Self {
        GdtDraft {
            characteristic: GdtCharacteristic::Position,
            tolerance: 0.1,
            diameter: true,
            material: MaterialCondition::Regardless,
            datums: vec![DatumRef::new("A")],
        }
    }
}

impl GdtDraft {
    pub fn body(&self) -> AnnotationKind {
        let datums = if self.characteristic.takes_datums() == Some(false) { Vec::new() } else { self.datums.clone() };
        AnnotationKind::Gdt {
            characteristic: self.characteristic,
            tolerance: self.tolerance,
            diameter: self.diameter,
            material: self.material,
            datums,
        }
    }

    pub fn from_body(body: &AnnotationKind) -> Option<GdtDraft> {
        match body {
            AnnotationKind::Gdt { characteristic, tolerance, diameter, material, datums } => Some(GdtDraft {
                characteristic: *characteristic,
                tolerance: *tolerance,
                diameter: *diameter,
                material: *material,
                datums: datums.clone(),
            }),
            _ => None,
        }
    }
}

/// A surface finish symbol being built or edited.
#[derive(Clone, Debug, PartialEq)]
pub struct FinishDraft {
    pub text: String,
    pub process: SurfaceProcess,
    pub method: String,
    pub lay: Option<Lay>,
    pub allowance: String,
    pub all_around: bool,
}

impl Default for FinishDraft {
    fn default() -> Self {
        FinishDraft {
            text: "Ra 1.6".into(),
            process: SurfaceProcess::Removal,
            method: String::new(),
            lay: None,
            allowance: String::new(),
            all_around: false,
        }
    }
}

impl FinishDraft {
    pub fn body(&self) -> AnnotationKind {
        AnnotationKind::SurfaceFinish {
            text: self.text.clone(),
            process: self.process,
            method: self.method.clone(),
            lay: self.lay,
            allowance: self.allowance.clone(),
            all_around: self.all_around,
        }
    }

    pub fn from_body(body: &AnnotationKind) -> Option<FinishDraft> {
        match body {
            AnnotationKind::SurfaceFinish { text, process, method, lay, allowance, all_around } => Some(FinishDraft {
                text: text.clone(),
                process: *process,
                method: method.clone(),
                lay: *lay,
                allowance: allowance.clone(),
                all_around: *all_around,
            }),
            _ => None,
        }
    }
}

/// A revision row being written.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RevisionDraft {
    pub revision: String,
    pub description: String,
    pub date: String,
    pub approved: String,
}

/// A derived view computed and waiting for its place on the sheet.
#[derive(Clone, Debug)]
pub struct PendingDerived {
    pub view: View,
    pub projected: Projected,
    pub sheet: usize,
}

/// What the derived-view and symbol tools hold between clicks, and the settings they
/// keep from one use to the next.
#[derive(Clone, Debug)]
pub struct ToolDraft {
    /// The view the tool works in, chosen by the first click.
    pub view: Option<ViewId>,
    /// Points clicked so far, in that view's millimetres: a cutting line's start, a
    /// circle's centre, a profile's corners, a crop's first corner.
    pub points: Vec<Point2>,
    /// Section and auxiliary: look the other way.
    pub flip: bool,
    /// Section: the cutting line runs horizontally; false for vertically. Follows the
    /// pointer until the line is finished.
    pub horizontal: bool,
    /// Section: the hatch pattern the cut faces get; `None` for the library's first.
    pub hatch: Option<String>,
    /// Detail: magnification relative to the parent.
    pub factor: f64,
    /// Detail: a drawn profile instead of a circle.
    pub profile: bool,
    /// Detail: the sheet the view goes to; `None` for the current one.
    pub target_sheet: Option<usize>,
    /// Broken-out: how deep the cut goes, in model millimetres.
    pub depth: f64,
    /// Dimension sets: the direction measured in.
    pub orientation: LinearOrientation,
    /// Picks so far: dimension set points, or a symbol's anchor.
    pub anchors: Vec<Anchor>,
    /// The feature control frame being built.
    pub gdt: GdtDraft,
    /// The next datum's letter; empty for the drawing's next one.
    pub datum: String,
    /// The surface finish being built.
    pub finish: FinishDraft,
    /// The revision a symbol marks; empty for the drawing's current one.
    pub revision: String,
    /// The revision row being written.
    pub revision_row: RevisionDraft,
    /// A derived view computed and waiting for its place.
    pub pending: Option<PendingDerived>,
}

impl Default for ToolDraft {
    fn default() -> Self {
        ToolDraft {
            view: None,
            points: Vec::new(),
            flip: false,
            horizontal: false,
            hatch: None,
            factor: 2.0,
            profile: false,
            target_sheet: None,
            depth: 10.0,
            orientation: LinearOrientation::Horizontal,
            anchors: Vec::new(),
            gdt: GdtDraft::default(),
            datum: String::new(),
            finish: FinishDraft::default(),
            revision: String::new(),
            revision_row: RevisionDraft::default(),
            pending: None,
        }
    }
}

impl ToolDraft {
    /// Forgets the clicks of the current use, keeping the settings.
    pub fn reset(&mut self) {
        self.view = None;
        self.points.clear();
        self.anchors.clear();
        self.pending = None;
        self.flip = false;
        self.target_sheet = None;
    }

    /// How many clicks or picks the current use has made.
    pub fn stage(&self) -> usize {
        self.points.len().max(self.anchors.len())
    }
}

/// The New drawing PropertyManager, open on the part.
#[derive(Clone, Debug, PartialEq)]
pub struct NewDrawing {
    pub template: String,
    /// `None` fits the part to the sheet.
    pub scale: Option<Scale>,
    pub projection: Projection,
    pub views: Vec<(StandardView, bool)>,
    pub error: Option<String>,
}

impl NewDrawing {
    pub fn new() -> NewDrawing {
        let t = library::template("iso-a3");
        NewDrawing {
            template: t.id.clone(),
            scale: None,
            projection: t.projection(),
            views: vec![
                (StandardView::Front, true),
                (StandardView::Top, true),
                (StandardView::Left, true),
                (StandardView::Isometric, true),
                (StandardView::Right, false),
                (StandardView::Back, false),
                (StandardView::Bottom, false),
            ],
            error: None,
        }
    }

    pub fn set_template(&mut self, id: &str) {
        let t = library::template(id);
        self.template = t.id.clone();
        self.projection = t.projection();
    }
}

impl Default for NewDrawing {
    fn default() -> Self {
        NewDrawing::new()
    }
}

/// A view being placed: its projection at the pointer.
#[derive(Clone, Debug)]
pub struct PendingView {
    pub view: View,
    pub projected: Projected,
    pub direction: Option<FoldDirection>,
    pub aligned: bool,
}

/// A drag of a view or an annotation on the sheet.
#[derive(Clone, Debug)]
pub struct Drag {
    pub target: DrawingSel,
    pub origin: Point2,
    pub start_pointer: Point2,
    pub moved: bool,
}

/// Inline editing of a note's text.
#[derive(Clone, Debug, PartialEq)]
pub struct NoteEdit {
    pub annotation: AnnotationId,
    pub text: String,
    pub focus: bool,
}

/// The 2D camera of the sheet canvas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera2 {
    /// Sheet point at the centre of the canvas.
    pub centre: Point2,
    /// Pixels per sheet millimetre.
    pub ppm: f32,
}

impl Default for Camera2 {
    fn default() -> Self {
        Camera2 { centre: Point2::new(210.0, 148.5), ppm: 2.0 }
    }
}

impl Camera2 {
    pub fn to_screen(&self, rect: egui::Rect, p: Point2) -> egui::Pos2 {
        let c = rect.center();
        egui::pos2(c.x + (p.u - self.centre.u) as f32 * self.ppm, c.y - (p.v - self.centre.v) as f32 * self.ppm)
    }

    pub fn to_sheet(&self, rect: egui::Rect, pos: egui::Pos2) -> Point2 {
        let c = rect.center();
        Point2::new(
            self.centre.u + f64::from((pos.x - c.x) / self.ppm),
            self.centre.v - f64::from((pos.y - c.y) / self.ppm),
        )
    }

    /// Fits the paper into the rect with a margin.
    pub fn fit(&mut self, rect: egui::Rect, paper: (f64, f64)) {
        let pad = 28.0;
        let sx = (rect.width() - 2.0 * pad) / paper.0 as f32;
        let sy = (rect.height() - 2.0 * pad) / paper.1 as f32;
        self.ppm = sx.min(sy).max(0.05);
        self.centre = Point2::new(paper.0 / 2.0, paper.1 / 2.0);
    }

    /// Zooms about a screen point.
    pub fn zoom_at(&mut self, rect: egui::Rect, pos: egui::Pos2, factor: f32) {
        let before = self.to_sheet(rect, pos);
        self.ppm = (self.ppm * factor).clamp(0.05, 200.0);
        let after = self.to_sheet(rect, pos);
        self.centre = Point2::new(self.centre.u + before.u - after.u, self.centre.v + before.v - after.v);
    }
}

/// The open drawing with everything the interface keeps beside the document.
pub struct DrawingSession {
    pub doc: DrawingDoc,
    pub tab: DrawingTab,
    pub tool: DrawingTool,
    pub selection: Option<DrawingSel>,
    pub hover: Option<Owner>,
    pub hover_label: Option<String>,
    pub camera: Camera2,
    pub canvas_rect: egui::Rect,
    pub sheet: usize,
    pub picks: Vec<Pick>,
    pub proposal: Option<Proposal>,
    pub pending: Option<PendingView>,
    /// Projections per fold direction while placing a projected view.
    pub fold_cache: HashMap<(ViewId, FoldDirection), (View, Projected)>,
    pub drag: Option<Drag>,
    pub note_edit: Option<NoteEdit>,
    pub cursor: Option<Point2>,
    pub show_grid: bool,
    pub snap: bool,
    pub needs_fit: bool,
    pub message: Option<(String, Instant)>,
    /// Where exports go without asking; set by automation.
    pub export_dir: Option<PathBuf>,
    pub last_export: Option<PathBuf>,
    /// The part's mesh version the drawing last looked at.
    pub seen_version: u64,
    /// Rows expanded in the drawing tree.
    pub expanded: std::collections::HashSet<ViewId>,
    pub annotations_open: bool,
    /// The annotation selected in the Needs attention list.
    pub attention_focus: Option<AnnotationId>,
    /// The derived-view and symbol tools' state.
    pub draft: ToolDraft,
    /// The sheet name being edited in the sheet strip or panel.
    pub sheet_rename: Option<(usize, String)>,
    /// Where the model is and whether it moved on, as last resolved.
    pub link: Option<ModelLink>,
    /// The part document the session has synced with. A part that becomes the model
    /// after the drawing opened (opened from its file) is first contact: the drawing
    /// keeps its pinned views and offers a Rebuild instead of updating by itself.
    pub part_seen: Option<DocumentId>,
    /// The reference bar over the sheet was dismissed for the current state.
    pub marker_dismissed: bool,
}

impl DrawingSession {
    pub fn new(doc: DrawingDoc) -> DrawingSession {
        DrawingSession {
            doc,
            tab: DrawingTab::Views,
            tool: DrawingTool::Select,
            selection: None,
            hover: None,
            hover_label: None,
            camera: Camera2::default(),
            canvas_rect: egui::Rect::NOTHING,
            sheet: 0,
            picks: Vec::new(),
            proposal: None,
            pending: None,
            fold_cache: HashMap::new(),
            drag: None,
            note_edit: None,
            cursor: None,
            show_grid: false,
            snap: true,
            needs_fit: true,
            message: None,
            export_dir: None,
            last_export: None,
            seen_version: 0,
            expanded: std::collections::HashSet::new(),
            annotations_open: true,
            attention_focus: None,
            draft: ToolDraft::default(),
            sheet_rename: None,
            link: None,
            part_seen: None,
            marker_dismissed: false,
        }
    }

    /// "Bracket · Drawing1".
    pub fn title(&self) -> String {
        let name = self.doc.drawing.meta.title.clone();
        let file = self
            .doc
            .path
            .as_ref()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "Drawing1".into());
        format!("{name} · {file}")
    }

    pub fn say(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), Instant::now()));
    }

    pub fn set_tool(&mut self, tool: DrawingTool) {
        self.tool = tool;
        self.picks.clear();
        self.proposal = None;
        self.pending = None;
        self.fold_cache.clear();
        self.draft.reset();
    }

    /// The prompt of the current tool at its current stage.
    pub fn tool_prompt(&self) -> String {
        if self.tool.is_derived() || self.tool.is_symbol() || matches!(self.tool, DrawingTool::DimensionSet(_)) {
            self.tool.prompt_at(self.draft.stage(), self.draft.pending.is_some())
        } else {
            self.tool.prompt(self.picks.len())
        }
    }

    /// The current sheet's revision table, if it has one, as its index among the
    /// sheet's tables.
    pub fn revision_table_index(&self) -> Option<usize> {
        self.sheet().tables.iter().position(|t| t.kind == soliddraft_drawing::TableKind::Revision)
    }

    pub fn sheet(&self) -> &soliddraft_drawing::Sheet {
        &self.doc.drawing.sheets[self.sheet.min(self.doc.drawing.sheets.len() - 1)]
    }

    pub fn selected_annotation(&self) -> Option<&Annotation> {
        match self.selection {
            Some(DrawingSel::Annotation(id)) => self.doc.drawing.annotation(id),
            _ => None,
        }
    }

    pub fn selected_view(&self) -> Option<&View> {
        match self.selection {
            Some(DrawingSel::View(id)) => self.doc.drawing.view(id),
            _ => None,
        }
    }

    /// Sheet position of a model point in a view, for automation and the hover label.
    pub fn sheet_point(&self, view: ViewId, model: soliddraft_kernel_api::Vec3) -> Option<Point2> {
        let xf = self.doc.transforms.get(&view)?;
        let v = self.doc.drawing.view(view)?;
        let p = xf.point(model);
        Some(Point2::new(p.u + v.position.u, p.v + v.position.v))
    }

    /// Screen position of a sheet point, for automation.
    pub fn screen_of(&self, sheet: Point2) -> egui::Pos2 {
        self.camera.to_screen(self.canvas_rect, sheet)
    }

    /// Screen position of a model point seen in a view.
    pub fn screen_of_model(&self, view: ViewId, model: soliddraft_kernel_api::Vec3) -> Option<egui::Pos2> {
        self.sheet_point(view, model).map(|p| self.screen_of(p))
    }

    /// True while the drawing has stale views or annotations needing attention: the
    /// dot on the document tab.
    pub fn has_issues(&self) -> bool {
        self.doc.is_stale() || self.doc.attention_count() > 0 || self.link_state() == LinkState::Missing
    }

    pub fn is_up_to_date(&self) -> bool {
        !self.doc.is_stale()
    }
}

/// The drawing side of the application state.
#[derive(Default)]
pub struct DrawingState {
    pub session: Option<DrawingSession>,
    pub new: Option<NewDrawing>,
    /// The open repair (or Replace model) dialog.
    pub repair: Option<RepairDialog>,
    /// True while the drawing tab is the active document: the ribbon, tree, property
    /// panel and status bar show the drawing.
    pub active: bool,
}

impl DrawingState {
    pub fn is_active(&self) -> bool {
        self.active && self.session.is_some()
    }
}

/// Actions the drawing panels push.
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum DrawingAction {
    /// Open the New drawing PropertyManager on the part.
    MakeDrawing,
    CancelNewDrawing,
    CreateDrawing,
    CloseDrawing,
    Command(Command),
    Undo,
    Redo,
    SetTab(DrawingTab),
    SetTool(DrawingTool),
    Select(Option<DrawingSel>),
    ZoomFit,
    ZoomIn,
    ZoomOut,
    ToggleGrid,
    ToggleSnap,
    /// Rebuild: bring the drawing to the current revision of its part. Opens the
    /// part from its file first when it is not the open part.
    Rebuild,
    UndoUpdate,
    DismissBanner,
    /// Review the update: open the Needs attention list.
    Review,
    Export {
        format: export::Format,
        path: Option<PathBuf>,
    },
    Save(Option<PathBuf>),
    ModelItems,
    AutoCentreMarks,
    StartReattach {
        annotation: AnnotationId,
        end: usize,
    },
    DeleteSelection,
    ToggleAutomaticUpdate,
    AcceptChange(AnnotationId),
    SetSheet(usize),
    /// Add a sheet after the last one, made from the first sheet's template, and show it.
    AddSheet,
    /// Delete a sheet; the first remaining one is shown.
    DeleteSheet(usize),
    /// Move a sheet to another place in the order and follow it.
    MoveSheet {
        sheet: usize,
        to: usize,
    },
    /// Flip the direction of the section or auxiliary view being placed.
    FlipPending,
    /// Close the profile of the detail being drawn and compute the view.
    CloseProfile,
    /// Move the selected view to another sheet, at a free place.
    MoveViewToSheet {
        view: ViewId,
        sheet: usize,
    },
    /// Bring the part tab forward.
    ShowPart,
    /// Bring the drawing tab forward.
    ShowDrawing,
    /// Show or hide one side area.
    TogglePanel(crate::panels::Side),
    /// Open a drawing file in the drawing tab, and its part in the part tab when the
    /// part is found and the part tab has nothing unsaved.
    OpenFile(PathBuf),
    /// Open the drawing's part, found by the resolver, in the part tab.
    OpenModel,
    /// Open the repair dialog; with `replace`, the Replace model dialog.
    ShowRepair {
        replace: bool,
    },
    CloseRepair,
    /// Point the model reference at `path`; `replace` accepts a different document.
    RepairModel {
        path: PathBuf,
        replace: bool,
    },
    /// Open the File locations page on the search folders.
    ShowLocations,
    /// Hide the reference bar until the reference's state changes.
    DismissMarker,
}

/// The part context for the current part, if it has a body.
pub fn part_context(state: &AppState) -> Option<PartContext<'_>> {
    PartContext::new(&state.doc)
}

/// Today's date as the title block writes it.
pub fn today() -> String {
    let secs = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = (secs / 86_400) as i64;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Makes the drawing the New drawing PropertyManager describes.
pub fn create_drawing(state: &AppState, new: &NewDrawing) -> Result<DrawingSession, String> {
    let ctx = part_context(state).ok_or("The part has no body yet; extrude something first")?;
    let template = library::template(&new.template);
    let model_template = template.template();
    // The drawing is not saved yet, so the path is the part's file name; saving the
    // drawing makes it relative to where the drawing goes.
    let path = state
        .doc
        .path
        .as_ref()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| format!("{}.{}", state.doc.name, crate::file::EXTENSION));
    let model = DocRef::new(ctx.id, DocKind::Part, path, ctx.digest.clone());
    let mut drawing = Drawing::new(model, &model_template);
    let mut title: Vec<char> = state.doc.name.chars().collect();
    if let Some(c) = title.first_mut() {
        *c = c.to_ascii_uppercase();
    }
    drawing.meta = DrawingMeta {
        title: title.into_iter().collect(),
        number: "SD-0001".into(),
        revision: "A".into(),
        designer: std::env::var("USER").map(|u| u.chars().take(24).collect()).unwrap_or_default(),
        date: today(),
        units: Default::default(),
    };
    drawing.sheets[0].projection = new.projection;
    drawing.sheets[0].format_overrides.insert("status".into(), "In work".into());
    drawing.sheets[0].format_overrides.insert("owner".into(), "SolidDraft workshop".into());

    // Project the ticked views, choose the scale and place them, with centre marks on
    // the orthographic views.
    let wanted: Vec<StandardView> = new.views.iter().filter(|(_, on)| *on).map(|(v, _)| *v).collect();
    if wanted.is_empty() {
        return Err("Tick at least one view to place".into());
    }
    let mut model_template = model_template;
    model_template.projection = new.projection;
    let request = soliddraft_drawing::NewDrawing {
        template: model_template,
        views: wanted,
        model: drawing.model.clone(),
        meta: drawing.meta.clone(),
        hidden: soliddraft_drawing::HiddenLines::Visible,
        model_items: false,
        centre_marks: true,
        scale: new.scale,
    };
    let part = ctx.part();
    let made = soliddraft_drawing::make_drawing(&part, &request).map_err(|e| e.to_string())?;
    let overrides = std::mem::take(&mut drawing.sheets[0].format_overrides);
    drawing = made.drawing;
    drawing.sheets[0].format_overrides = overrides;
    let mut annotation_id = drawing.next_annotation_id().0;
    let mut doc = DrawingDoc::new(drawing);
    for (id, p) in &made.projections {
        doc.remember(*id, p);
    }
    // The template's general notes, editable like any note, at the bottom-left of the
    // drawing area, clear of the views.
    if let Some(notes) = &template.notes
        && !notes.lines.is_empty()
    {
        let [l, b, _, _] = template.drawing_area();
        let lines = notes.lines.len() as f64 + 1.0;
        let t = b + notes.offset[1] + lines * notes.height * 1.7;
        let text =
            std::iter::once(notes.heading.clone()).chain(notes.lines.iter().cloned()).collect::<Vec<_>>().join("\n");
        doc.drawing.sheets[0].annotations.push(Annotation {
            id: AnnotationId({
                annotation_id += 1;
                annotation_id - 1
            }),
            view: SHEET,
            body: AnnotationKind::Note { text, leader: Vec::new() },
            anchors: Vec::new(),
            placement: Point2::new(l + notes.offset[0], t - notes.offset[1]),
            style: StyleOverrides { text_height: Some(notes.height), ..Default::default() },
            state: AnnotationState::Ok,
        });
    }
    doc.dirty = true;
    let mut session = DrawingSession::new(doc);
    session.seen_version = state.doc.mesh_version;
    session.part_seen = Some(state.doc.model().id());
    session.doc.seen_digest = ctx.digest.clone();
    session.doc.log_mark = state.doc.command_log().len();
    Ok(session)
}

/// Opens the drawing tab beside the part and makes it the active document.
fn show_drawing_tab(dock: &mut DockState<Tab>) {
    if let Some(path) = dock.find_tab(&Tab::Drawing) {
        let _ = dock.set_active_tab(path);
        dock.set_focused_node_and_surface(egui_dock::NodePath { surface: path.surface, node: path.node });
        return;
    }
    if let Some(path) = dock.find_tab(&Tab::Viewport) {
        dock.set_focused_node_and_surface(egui_dock::NodePath { surface: path.surface, node: path.node });
    }
    dock.push_to_focused_leaf(Tab::Drawing);
}

fn show_part_tab(dock: &mut DockState<Tab>) {
    if let Some(path) = dock.find_tab(&Tab::Viewport) {
        let _ = dock.set_active_tab(path);
        dock.set_focused_node_and_surface(egui_dock::NodePath { surface: path.surface, node: path.node });
    }
}

/// Runs once per frame before the panels: tracks which document is active, keeps the
/// model reference resolved, and when the part changed, re-resolves the drawing
/// (automatic update) or marks it stale. A part that becomes the model after the
/// drawing opened, such as the part loaded from the drawing's reference, is first
/// contact: the drawing keeps its pinned views and offers a Rebuild.
pub fn frame(state: &mut AppState, dock: &mut DockState<Tab>) {
    if let Some((_, tab)) = dock.find_active_focused() {
        match tab {
            Tab::Viewport => state.drawing.active = false,
            Tab::Drawing => state.drawing.active = true,
            _ => {}
        }
    }
    if dock.find_tab(&Tab::Drawing).is_none() && state.drawing.session.is_some() && state.drawing.active {
        state.drawing.active = false;
    }
    reference::refresh(state);
    let Some(session) = state.drawing.session.as_mut() else { return };
    if session.seen_version == state.doc.mesh_version {
        return;
    }
    // A feature being edited rolls the part back; wait for the editor to close.
    if state.editor.is_some() || state.doc.rollback.is_some() {
        return;
    }
    session.seen_version = state.doc.mesh_version;
    // Only the part the drawing references moves it.
    let part = state.doc.model().id();
    if !session.model_is(part) {
        return;
    }
    let first_contact = session.part_seen != Some(part);
    session.part_seen = Some(part);
    let digest = part::digest(&state.doc);
    session.doc.seen_digest = digest.clone();
    if digest == session.doc.drawing.model.pinned {
        return;
    }
    if first_contact {
        session.say("Newer version of the part: Rebuild (Ctrl+B) when ready");
        return;
    }
    if session.doc.drawing.settings.automatic_update {
        rebuild(state);
    } else {
        session.say("The part changed. Rebuild when ready.");
    }
}

/// True when the open part is the one the drawing references.
fn part_is_model(state: &AppState) -> bool {
    state.drawing.session.as_ref().is_some_and(|s| s.model_is(state.doc.model().id()))
}

/// Where the drawing's model is and whether it moved on from the revision the drawing
/// is pinned at: the open part first, then the drawing's stored path, then the
/// drawing's folder and the File locations search folders. `None` without a drawing.
/// The "newer version available" marker and the missing-reference repair read this.
pub fn resolve_model(state: &AppState) -> Option<soliddraft_document::Resolution> {
    let session = state.drawing.session.as_ref()?;
    let part = soliddraft_document::OpenDocument {
        id: state.doc.model().id(),
        kind: DocKind::Part,
        path: state.doc.path.clone(),
        revision: part::digest(&state.doc),
    };
    let resolver = soliddraft_document::Resolver::new(session.doc.path.as_deref())
        .with_open([part])
        .with_search_folders(crate::settings::file_locations().search_folders());
    Some(resolver.resolve(&session.doc.drawing.model))
}

/// Rebuild: brings the drawing to the current revision of its part, the open part's
/// unsaved edits included, as one undo step, and binds the reference to where the
/// part was found. When the part is not open but its file is, the file is opened in
/// the part tab first and the rebuild follows; a missing part asks for a repair.
fn rebuild(state: &mut AppState) {
    if !part_is_model(state) {
        if let Some(path) = state.drawing.session.as_ref().and_then(DrawingSession::model_file) {
            if state.doc.dirty {
                let name = state.doc.name.clone();
                if let Some(s) = state.drawing.session.as_mut() {
                    s.say(format!("Save or close {name} first: rebuilding opens the drawing's part in the part tab"));
                }
                return;
            }
            if let Some(s) = state.drawing.session.as_mut() {
                s.say(format!("Opening {}", path.display()));
            }
            state.push(crate::state::AppAction::Open(Some(path)));
            state.push(crate::state::AppAction::Drawing(DrawingAction::Rebuild));
            return;
        }
        let missing = state.drawing.session.as_ref().is_some_and(|s| s.link_state() == LinkState::Missing);
        if missing && state.drawing.repair.is_none() {
            state.drawing.repair = RepairDialog::new(state, false);
        }
        if let Some(s) = state.drawing.session.as_mut() {
            s.say("The part could not be found; repair the reference first");
        }
        return;
    }
    let Some(ctx) = PartContext::new(&state.doc) else {
        if let Some(s) = state.drawing.session.as_mut() {
            s.say("The part has no body; the drawing keeps its last views");
        }
        return;
    };
    let Some(session) = state.drawing.session.as_mut() else { return };
    match session.doc.apply(Command::UpdateFromModel, Some(&ctx)) {
        Ok(()) => {
            if let Some(t) = session.model_target().cloned() {
                soliddraft_document::resolve::rebind(&mut session.doc.drawing.model, &t);
            }
            let report = session.doc.last_update.clone();
            if let Some(r) = report {
                let text = format!(
                    "Rebuilt to {} rev {} · {} view{} · {} changed · {} dangling",
                    ctx.name,
                    r.to_revision.short(),
                    r.views,
                    if r.views == 1 { "" } else { "s" },
                    r.changed.len(),
                    r.dangling.len()
                );
                session.say(text);
                if !r.dangling.is_empty() {
                    session.attention_focus = r.dangling.first().copied();
                }
            }
            session.fold_cache.clear();
            session.pending = None;
        }
        Err(e) => session.say(format!("Rebuild failed: {e}")),
    }
}

/// Opens a drawing file in the drawing tab. The drawing shows the part revision it is
/// pinned at. Its part is opened in the part tab when the reference resolves to a
/// file and the part tab holds nothing unsaved; a missing part opens the repair dialog.
fn open_file(state: &mut AppState, dock: &mut DockState<Tab>, path: PathBuf) {
    let doc = match DrawingDoc::open(&path) {
        Ok(doc) => doc,
        Err(e) => {
            state.note(e, NoteKind::Error);
            return;
        }
    };
    let mut session = DrawingSession::new(doc);
    // Let the frame hook see the open part as first contact.
    session.seen_version = u64::MAX;
    let views: usize = session.doc.drawing.sheets.iter().map(|s| s.views.len()).sum();
    let title = session.title();
    state.drawing.session = Some(session);
    state.drawing.repair = None;
    state.drawing.active = true;
    show_drawing_tab(dock);
    crate::file::remember(&path);
    state.recent = crate::file::recent();
    reference::refresh(state);
    let Some(session) = state.drawing.session.as_ref() else { return };
    let part_open = session.model_is(state.doc.model().id());
    let link = session.link_state();
    let words = match &link {
        LinkState::Missing => "the part was not found",
        LinkState::Newer { .. } => "a newer version of the part is available",
        LinkState::Current => "up to date with the part",
        LinkState::Unverified => "the source is not a SolidDraft document",
        LinkState::Unknown => "the part is not resolved",
    };
    state.note(format!("Opened {title} · {views} view{} · {words}", if views == 1 { "" } else { "s" }), NoteKind::Ok);
    if link == LinkState::Missing {
        state.drawing.repair = RepairDialog::new(state, false);
    } else if !part_open {
        open_model(state);
    }
}

/// Opens the drawing's part, found by the resolver, in the part tab when the part tab
/// holds nothing unsaved.
fn open_model(state: &mut AppState) {
    let Some(session) = state.drawing.session.as_ref() else { return };
    let Some(file) = session.model_file() else { return };
    if session.model_is(state.doc.model().id()) {
        return;
    }
    if state.doc.dirty {
        let name = state.doc.name.clone();
        if let Some(s) = state.drawing.session.as_mut() {
            s.say(format!("{name} has unsaved changes; the drawing's part was not opened"));
        }
        return;
    }
    state.push(crate::state::AppAction::Open(Some(file)));
}

/// Points the model reference at `path`, as the repair dialog asked.
fn repair_model(state: &mut AppState, path: PathBuf, replace: bool) {
    let Some(session) = state.drawing.session.as_mut() else { return };
    let Some(drawing_path) = session.doc.path.clone() else {
        if let Some(d) = state.drawing.repair.as_mut() {
            d.error = Some("Save the drawing first, so the reference has a folder to be relative to".into());
        }
        return;
    };
    let was = session.doc.drawing.model.id;
    match soliddraft_document::resolve::repair(&mut session.doc.drawing.model, &drawing_path, &path, replace) {
        Ok(identity) => {
            session.doc.dirty = true;
            session.link = None;
            session.marker_dismissed = false;
            let what = if was.is_some_and(|w| w != identity.id) { "Model replaced" } else { "Reference repaired" };
            let shown = session.doc.drawing.model.path.clone();
            session.say(format!("{what}: {shown}"));
            state.drawing.repair = None;
            state.note(format!("{what}: {shown}"), NoteKind::Ok);
            reference::refresh(state);
            open_model(state);
        }
        Err(e) => {
            if let Some(d) = state.drawing.repair.as_mut() {
                d.error = Some(e.to_string());
            }
        }
    }
}

/// Applies one drawing action. Called from the app's action loop.
pub fn apply(state: &mut AppState, dock: &mut DockState<Tab>, ctx: &egui::Context, action: DrawingAction) {
    let _ = ctx;
    match action {
        DrawingAction::MakeDrawing => {
            if state.doc.display_body().is_none() {
                state.note("Make drawing needs a body; extrude something first", NoteKind::Info);
                return;
            }
            state.editor = None;
            state.drawing.new = Some(NewDrawing::new());
            state.note("Choose the template and the views, then Create drawing", NoteKind::Info);
        }
        DrawingAction::CancelNewDrawing => state.drawing.new = None,
        DrawingAction::CreateDrawing => {
            let Some(new) = state.drawing.new.clone() else { return };
            match create_drawing(state, &new) {
                Ok(session) => {
                    let n = session.doc.drawing.sheets[0].views.len();
                    state.drawing.session = Some(session);
                    state.drawing.new = None;
                    state.drawing.active = true;
                    show_drawing_tab(dock);
                    state.note(
                        format!("Drawing created · {n} view{} on {}", if n == 1 { "" } else { "s" }, new.template),
                        NoteKind::Ok,
                    );
                }
                Err(e) => {
                    if let Some(n) = state.drawing.new.as_mut() {
                        n.error = Some(e.clone());
                    }
                    state.note(e, NoteKind::Error);
                }
            }
        }
        DrawingAction::CloseDrawing => {
            state.drawing.session = None;
            state.drawing.active = false;
            if let Some(path) = dock.find_tab(&Tab::Drawing) {
                dock.remove_tab(path);
            }
            show_part_tab(dock);
        }
        DrawingAction::ShowPart => {
            show_part_tab(dock);
            state.drawing.active = false;
        }
        DrawingAction::TogglePanel(side) => state.layout.toggle(side),
        DrawingAction::ShowDrawing => {
            if state.drawing.session.is_some() {
                show_drawing_tab(dock);
                state.drawing.active = true;
            }
        }
        DrawingAction::Command(cmd) => {
            let needs_part = matches!(
                cmd,
                Command::UpdateFromModel
                    | Command::Reattach { .. }
                    | Command::EditView { .. }
                    | Command::SetViewCrop { .. }
                    | Command::AddBrokenOut { .. }
                    | Command::RemoveBrokenOut { .. }
                    | Command::ReattachView { .. }
                    | Command::SetViewDisplay { .. }
            );
            if needs_part && !part_is_model(state) {
                if let Some(s) = state.drawing.session.as_mut() {
                    s.say("The open part is not the one this drawing shows");
                }
                return;
            }
            let part = if needs_part { PartContext::new(&state.doc) } else { None };
            let Some(session) = state.drawing.session.as_mut() else { return };
            let label = cmd.name();
            let result = session.doc.apply(cmd, part.as_ref());
            match result {
                Ok(()) => {
                    if label == "Update from model" {
                        session.fold_cache.clear();
                    }
                }
                Err(e) => session.say(format!("{label}: {e}")),
            }
        }
        DrawingAction::Undo => {
            if let Some(s) = state.drawing.session.as_mut() {
                match s.doc.undo() {
                    Some(l) => s.say(format!("Undid {l}")),
                    None => s.say("Nothing to undo"),
                }
                s.selection = None;
                s.fold_cache.clear();
            }
        }
        DrawingAction::Redo => {
            if let Some(s) = state.drawing.session.as_mut() {
                match s.doc.redo() {
                    Some(l) => s.say(format!("Redid {l}")),
                    None => s.say("Nothing to redo"),
                }
                s.selection = None;
            }
        }
        DrawingAction::SetTab(t) => {
            if let Some(s) = state.drawing.session.as_mut() {
                s.tab = t;
            }
        }
        DrawingAction::SetTool(t) => {
            if let Some(s) = state.drawing.session.as_mut() {
                if let DrawingTool::PlaceView(PlaceKind::Projected { parent }) = &t
                    && s.doc.drawing.view(*parent).is_none()
                {
                    s.say("Select the view to fold the new view off first");
                    return;
                }
                let prompt = t.prompt(0);
                s.set_tool(t);
                if !prompt.is_empty() {
                    s.say(prompt);
                }
            }
        }
        DrawingAction::Select(sel) => {
            if let Some(s) = state.drawing.session.as_mut() {
                s.selection = sel;
                s.note_edit = None;
                if let Some(DrawingSel::Annotation(id)) = sel
                    && let Some(AnnotationKind::Note { text, .. }) = s.doc.drawing.annotation(id).map(|a| &a.body)
                {
                    s.note_edit = Some(NoteEdit { annotation: id, text: text.clone(), focus: false });
                }
            }
        }
        DrawingAction::ZoomFit => {
            if let Some(s) = state.drawing.session.as_mut() {
                s.needs_fit = true;
            }
        }
        DrawingAction::ZoomIn | DrawingAction::ZoomOut => {
            if let Some(s) = state.drawing.session.as_mut() {
                let f = if action == DrawingAction::ZoomIn { 1.25 } else { 0.8 };
                let c = s.canvas_rect.center();
                s.camera.zoom_at(s.canvas_rect, c, f);
            }
        }
        DrawingAction::ToggleGrid => {
            if let Some(s) = state.drawing.session.as_mut() {
                s.show_grid = !s.show_grid;
            }
        }
        DrawingAction::ToggleSnap => {
            if let Some(s) = state.drawing.session.as_mut() {
                s.snap = !s.snap;
            }
        }
        DrawingAction::Rebuild => rebuild(state),
        DrawingAction::OpenFile(path) => open_file(state, dock, path),
        DrawingAction::OpenModel => open_model(state),
        DrawingAction::ShowRepair { replace } => {
            state.drawing.repair = RepairDialog::new(state, replace);
        }
        DrawingAction::CloseRepair => state.drawing.repair = None,
        DrawingAction::RepairModel { path, replace } => repair_model(state, path, replace),
        DrawingAction::ShowLocations => {
            state.locations_dialog = Some(crate::locations::LocationsDialog::new(Some(
                soliddraft_document::LocationKind::ReferencedDocuments,
            )));
        }
        DrawingAction::DismissMarker => {
            if let Some(s) = state.drawing.session.as_mut() {
                s.marker_dismissed = true;
            }
        }
        DrawingAction::UndoUpdate => {
            if let Some(s) = state.drawing.session.as_mut() {
                if s.doc.undo_label() == Some("Update from model") {
                    s.doc.undo();
                    s.doc.last_update = None;
                    s.say("Rebuild undone; the views are older than the part again");
                } else {
                    s.say("The last change was not an update");
                }
            }
        }
        DrawingAction::DismissBanner => {
            if let Some(s) = state.drawing.session.as_mut()
                && let Some(r) = s.doc.last_update.as_mut()
            {
                r.dismissed = true;
            }
        }
        DrawingAction::Review => {
            if let Some(s) = state.drawing.session.as_mut() {
                let first = s.doc.drawing.annotations_needing_attention().next().map(|a| a.id);
                s.attention_focus = first;
                s.selection = first.map(DrawingSel::Annotation);
                if let Some(r) = s.doc.last_update.as_mut() {
                    r.dismissed = true;
                }
            }
        }
        DrawingAction::Export { format, path } => export_sheet(state, format, path),
        DrawingAction::Save(path) => save_drawing(state, path),
        DrawingAction::ModelItems => {
            let Some(ctx) = PartContext::new(&state.doc) else { return };
            let Some(s) = state.drawing.session.as_mut() else { return };
            let items = match model_items(&ctx, &s.doc.drawing, s.sheet) {
                Ok(items) => items,
                Err(e) => {
                    s.say(format!("Model items: {e}"));
                    return;
                }
            };
            let n = items.len();
            for a in items {
                let _ = s.doc.apply(Command::AddAnnotation { annotation: a }, None);
            }
            s.say(if n == 0 {
                "Model items: every model dimension is already on the sheet".to_owned()
            } else {
                format!("Model items: {n} dimension{} imported", if n == 1 { "" } else { "s" })
            });
        }
        DrawingAction::AutoCentreMarks => {
            let Some(s) = state.drawing.session.as_mut() else { return };
            let views: Vec<ViewId> = match s.selection {
                Some(DrawingSel::View(id)) => vec![id],
                _ => s.sheet().views.iter().map(|v| v.id).collect(),
            };
            let mut added = 0;
            for id in views {
                let Some(p) = s.doc.projected_of(id) else { continue };
                let marks = project::centre_marks(&p, id, s.doc.next_annotation_id().0);
                for m in marks {
                    let dup = s.sheet().annotations.iter().any(|a| {
                        a.view == id
                            && matches!(a.body, AnnotationKind::CentreMark)
                            && (a.placement.u - m.placement.u).abs() < 1e-3
                            && (a.placement.v - m.placement.v).abs() < 1e-3
                    });
                    if !dup {
                        let _ = s.doc.apply(Command::AddAnnotation { annotation: m }, None);
                        added += 1;
                    }
                }
            }
            s.say(format!("{added} centre mark{} added", if added == 1 { "" } else { "s" }));
        }
        DrawingAction::StartReattach { annotation, end } => {
            if let Some(s) = state.drawing.session.as_mut() {
                s.selection = Some(DrawingSel::Annotation(annotation));
                s.attention_focus = Some(annotation);
                let prompt = DrawingTool::Reattach { annotation, end }.prompt(0);
                s.set_tool(DrawingTool::Reattach { annotation, end });
                s.say(prompt);
            }
        }
        DrawingAction::DeleteSelection => {
            let Some(s) = state.drawing.session.as_mut() else { return };
            match s.selection {
                Some(DrawingSel::Annotation(id)) => {
                    let _ = s.doc.apply(Command::DeleteAnnotation { annotation: id }, None);
                    s.selection = None;
                }
                Some(DrawingSel::View(id)) => match s.doc.apply(Command::DeleteView { view: id }, None) {
                    Ok(()) => s.selection = None,
                    Err(e) => s.say(e),
                },
                _ => {}
            }
        }
        DrawingAction::ToggleAutomaticUpdate => {
            let Some(s) = state.drawing.session.as_mut() else { return };
            let on = !s.doc.drawing.settings.automatic_update;
            let _ = s.doc.apply(Command::SetAutomaticUpdate { on }, None);
            if on && s.doc.is_stale() {
                rebuild(state);
            }
        }
        DrawingAction::AcceptChange(id) => {
            if let Some(s) = state.drawing.session.as_mut() {
                let _ = s.doc.apply(Command::AcceptChange { annotation: id }, None);
            }
        }
        DrawingAction::SetSheet(i) => {
            if let Some(s) = state.drawing.session.as_mut()
                && i < s.doc.drawing.sheets.len()
                && i != s.sheet
            {
                s.sheet = i;
                s.needs_fit = true;
                s.selection = None;
                s.note_edit = None;
                s.sheet_rename = None;
            }
        }
        DrawingAction::AddSheet => {
            let Some(s) = state.drawing.session.as_mut() else { return };
            let n = s.doc.drawing.sheets.len();
            let template = s.doc.drawing.sheets[0].template.clone();
            let name = sheets::fresh_name(&s.doc.drawing);
            match s.doc.apply(Command::AddSheet { name: name.clone(), template }, None) {
                Ok(()) => {
                    s.sheet = n;
                    s.needs_fit = true;
                    s.selection = None;
                    s.say(format!("{name} added"));
                }
                Err(e) => s.say(e),
            }
        }
        DrawingAction::DeleteSheet(i) => {
            let Some(s) = state.drawing.session.as_mut() else { return };
            let name = s.doc.drawing.sheets.get(i).map(|sh| sh.name.clone()).unwrap_or_default();
            match s.doc.apply(Command::DeleteSheet { sheet: i }, None) {
                Ok(()) => {
                    s.sheet = s.sheet.min(s.doc.drawing.sheets.len() - 1);
                    if s.sheet >= i && s.sheet > 0 {
                        s.sheet -= 1;
                    }
                    s.needs_fit = true;
                    s.selection = None;
                    s.say(format!("{name} deleted"));
                }
                Err(e) => s.say(e),
            }
        }
        DrawingAction::MoveSheet { sheet, to } => {
            let Some(s) = state.drawing.session.as_mut() else { return };
            match s.doc.apply(Command::MoveSheet { sheet, to }, None) {
                Ok(()) => {
                    if s.sheet == sheet {
                        s.sheet = to;
                    } else if sheet < s.sheet && to >= s.sheet {
                        s.sheet -= 1;
                    } else if sheet > s.sheet && to <= s.sheet {
                        s.sheet += 1;
                    }
                }
                Err(e) => s.say(e),
            }
        }
        DrawingAction::FlipPending => tools::flip_pending(state),
        DrawingAction::CloseProfile => tools::close_profile(state),
        DrawingAction::MoveViewToSheet { view, sheet } => {
            let Some(s) = state.drawing.session.as_mut() else { return };
            let Some(v) = s.doc.drawing.view(view).cloned() else { return };
            if sheet >= s.doc.drawing.sheets.len() {
                return;
            }
            let format = library::template(s.doc.drawing.sheets[sheet].template.as_str());
            let [l, b, r, t] = format.drawing_area();
            let centre = Point2::new((l + r) / 2.0, (b + t) / 2.0);
            let to = soliddraft_drawing::derived::free_spot(&s.doc.drawing, sheet, &v, centre, None).unwrap_or(centre);
            match s.doc.apply(Command::MoveViewToSheet { view, sheet, to }, None) {
                Ok(()) => {
                    let name = s.doc.drawing.sheets[sheet].name.clone();
                    s.sheet = sheet;
                    s.needs_fit = true;
                    s.selection = Some(DrawingSel::View(view));
                    s.say(format!("Moved to {name}; location labels point both ways"));
                }
                Err(e) => s.say(e),
            }
        }
    }
}

/// Writes the current sheet as a file, asking for a path unless automation set one.
fn export_sheet(state: &mut AppState, format: export::Format, path: Option<PathBuf>) {
    let Some(s) = state.drawing.session.as_mut() else { return };
    // The PDF carries every sheet as a page; DXF and SVG are one sheet each.
    let name = match format {
        export::Format::Pdf => format!("{}.pdf", s.doc.drawing.meta.title.to_lowercase()),
        _ => format!(
            "{}-{}.{}",
            s.doc.drawing.meta.title.to_lowercase(),
            s.sheet().name.to_lowercase(),
            format.extension()
        ),
    };
    let path = path.or_else(|| s.export_dir.as_ref().map(|d| d.join(&name))).or_else(|| {
        let title = format!("Export sheet as {}", format.label());
        crate::platform::save_path(&title, format.label(), format.extension(), &name)
    });
    let Some(path) = path else { return };
    let path = if path.extension().is_none() { path.with_extension(format.extension()) } else { path };
    let stale = s.doc.stale_views();
    let count = s.doc.drawing.sheets.len();
    let list_of = |i: usize| {
        let format_file = library::template(s.doc.drawing.sheets[i].template.as_str());
        let ctx = display::Context {
            format: format_file,
            style: format_file.style_file(),
            stale: &stale,
            sheet_index: i,
            sheet_count: count,
        };
        display::layout(&s.doc.drawing, i, &ctx)
    };
    let bytes = match format {
        export::Format::Pdf => {
            let lists: Vec<_> = (0..count).map(list_of).collect();
            let info = export::PdfInfo {
                title: s.doc.drawing.meta.title.clone(),
                author: s.doc.drawing.meta.designer.clone(),
            };
            export::pdf(&lists, &info)
        }
        _ => export::bytes(&list_of(s.sheet), format, &s.doc.drawing.meta.title),
    };
    let written = bytes.map_err(|e| e.to_string()).and_then(|bytes| crate::platform::write_file(&path, &bytes));
    match written {
        Ok(()) => {
            s.last_export = Some(path.clone());
            s.say(format!("Exported {}", path.display()));
            state.note(format!("Exported {}", path.display()), NoteKind::Ok);
        }
        Err(e) => {
            s.say(e.clone());
            state.note(e, NoteKind::Error);
        }
    }
}

fn save_drawing(state: &mut AppState, path: Option<PathBuf>) {
    let Some(s) = state.drawing.session.as_mut() else { return };
    let path = path.or_else(|| s.doc.path.clone()).or_else(|| {
        let extension = soliddraft_drawing::file::EXTENSION;
        let name = format!("{}.{extension}", s.doc.drawing.meta.title.to_lowercase());
        crate::platform::save_path("Save drawing", "SolidDraft drawing", extension, &name)
    });
    let Some(path) = path else { return };
    let path = soliddraft_document::kind::saved_path(&path, DocKind::Drawing);
    // The stored path to the part follows where both files are now, and an unbound
    // reference takes the id of the document it resolved to.
    let target = s.model_target().map(|t| (t.path().map(std::path::Path::to_path_buf), t.id()));
    if let Some((file, id)) = target {
        if let Some(file) = file {
            s.doc.drawing.model.set_path(&path, &file);
        }
        if s.doc.drawing.model.id.is_none() {
            s.doc.drawing.model.id = id;
        }
    }
    let kernel = state.doc.kernel().backend();
    match s.doc.save(&path, &format!("{} {}", kernel.name, kernel.version)) {
        Ok(()) => {
            s.say(format!("Saved {}", path.display()));
            state.note(format!("Saved {}", path.display()), NoteKind::Ok);
        }
        Err(e) => state.note(e, NoteKind::Error),
    }
}

/// The status bar while the drawing is active: sheet, size, scale, projection, view
/// count and whether the sheet is up to date with the part.
pub fn status_bar(ui: &mut egui::Ui, state: &mut AppState) {
    use crate::theme;
    let Some(s) = state.drawing.session.as_ref() else { return };
    let sheet = s.sheet();
    let muted = |t: String| egui::RichText::new(t).size(11.5).color(theme::MUTED);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 18.0;
        ui.add_space(4.0);
        ui.label(muted(format!("Sheet {} of {}", s.sheet + 1, s.doc.drawing.sheets.len())));
        let projection = match sheet.projection {
            Projection::FirstAngle => "first angle",
            Projection::ThirdAngle => "third angle",
        };
        ui.label(muted(format!("{} · {} · {projection}", sheet.size.name, sheet.scale)));
        let n = sheet.views.len();
        ui.label(muted(format!("{n} view{}", if n == 1 { "" } else { "s" })));
        let part = &s.doc.drawing.meta.title;
        let rev = s.doc.drawing.model.pinned.short();
        let dangling = s.doc.dangling_count();
        let changed = s.doc.attention_count() - dangling;
        if s.link_state() == LinkState::Missing {
            ui.label(
                egui::RichText::new(format!("{part} not found · repair the reference"))
                    .family(theme::semibold())
                    .size(11.5)
                    .color(theme::ERROR),
            );
        } else if s.doc.is_stale() {
            ui.label(
                egui::RichText::new(format!("Newer version of {part} available · showing rev {rev} (Ctrl+B rebuilds)"))
                    .family(theme::semibold())
                    .size(11.5)
                    .color(theme::WARN),
            );
        } else if dangling > 0 || changed > 0 {
            let mut parts = Vec::new();
            if dangling > 0 {
                parts.push(format!("{dangling} dangling"));
            }
            if changed > 0 {
                parts.push(format!("{changed} changed"));
            }
            ui.label(
                egui::RichText::new(format!("Updated to {part} rev {rev} · {}", parts.join(" · ")))
                    .family(theme::semibold())
                    .size(11.5)
                    .color(if dangling > 0 { theme::ERROR } else { theme::WARN }),
            );
        } else {
            ui.label(
                egui::RichText::new(format!("Up to date with {part} rev {rev}"))
                    .family(theme::semibold())
                    .size(11.5)
                    .color(theme::OK),
            );
        }
        if s.tool != DrawingTool::Select {
            ui.label(muted(s.tool.label().to_owned()));
        }
        if let Some((text, at)) = &s.message
            && at.elapsed() < std::time::Duration::from_secs(8)
        {
            ui.label(muted(text.clone()));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(10.0);
            let coords = match s.cursor {
                Some(c) => format!("x {:.1}   y {:.1} mm", c.u, c.v),
                None => "mm".to_owned(),
            };
            ui.label(muted(coords));
            ui.label(muted(format!("{:.1} ms", state.frame_ms))).on_hover_text("Frame time, smoothed");
        });
    });
}

/// The part's dimensions brought into the views of sheet `sheet`, once: those the sheet
/// already shows are left out. The annotations are returned for the caller to add as
/// commands.
fn model_items(ctx: &PartContext<'_>, drawing: &Drawing, sheet: usize) -> Result<Vec<Annotation>, String> {
    let mut copy = drawing.clone();
    let before = copy.sheets[sheet].annotations.len();
    soliddraft_drawing::model_items::import_part(&mut copy, sheet, &ctx.part()).map_err(|e| e.to_string())?;
    let existing = &drawing.sheets[sheet].annotations;
    let same = |a: &Annotation, b: &Annotation| match (&a.body, &b.body) {
        (AnnotationKind::Dimension(x), AnnotationKind::Dimension(y)) => {
            x.kind == y.kind
                && a.anchors.len() == b.anchors.len()
                && a.anchors
                    .iter()
                    .zip(&b.anchors)
                    .all(|(p, q)| p.entity.model == q.entity.model && p.entity.role == q.entity.role)
        }
        _ => false,
    };
    Ok(copy.sheets[sheet]
        .annotations
        .split_off(before)
        .into_iter()
        .filter(|a| !existing.iter().any(|e| same(a, e)))
        .collect())
}
