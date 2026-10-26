//! SolidDraft drawings: sheets of 2D views of a part, with dimensions and notes tied to
//! the part's entities.
//!
//! A [`Drawing`] is its own document. It references the part with a [`DocRef`]: the
//! part's id, its path relative to the drawing, and the part revision the views were
//! made from, which the drawing keeps showing until a Rebuild moves it. It holds one or
//! more [`Sheet`]s. Each sheet holds
//! [`View`]s, whose line work is cached as [`ViewGeometry`] so a drawing opens without
//! the kernel, and [`Annotation`]s, whose [`Anchor`]s name model entities by persistent
//! [`EntityRef`]s rather than by view geometry. Every change goes through a [`Command`].
//!
//! Units are millimetres and degrees. Sheet coordinates start at the paper's bottom-left
//! corner with Y up.
//!
//! Phase D1 makes a drawing without a window: [`make_drawing`] projects the part's views
//! through the kernel, picks the sheet scale, places the views with their alignments and
//! brings in the part's dimensions ([`model_items`]); [`update_from_model`] keeps the
//! drawing tied to the part; [`display::layout`] turns a sheet into the display list
//! every renderer draws; [`file`] reads and writes the drawing file; and [`export`]
//! writes SVG, PDF and DXF from the display list. [`fixture::bracket_a3`] is a sample
//! drawing that needs no kernel.

pub mod annotate;
mod annotation;
mod command;
pub mod derived;
pub mod display;
mod entity;
pub mod export;
pub mod file;
pub mod fixture;
mod ids;
mod layout;
pub mod library;
pub mod measure;
pub mod model_items;
mod part;
pub mod project;
mod sheet;
mod template;
pub mod text;
mod update;
mod view;

pub use annotation::{
    Anchor, Annotation, AnnotationKind, AnnotationState, Attach, DatumRef, DimType, DimValue, Dimension, DimensionKind,
    DimensionSet, GdtCharacteristic, Lay, LinearOrientation, MaterialCondition, ParameterRef, SetKind, StyleOverrides,
    SurfaceProcess, Tolerance,
};
pub use command::{Command, UndoStack, apply, apply_with, next_letter};
pub use entity::{EntityKind, EntityRef, Fingerprint, ModelEntity};
pub use ids::{AnnotationId, SHEET, TemplateId, ViewId};
pub use layout::{MadeDrawing, NewDrawing, default_views, make_drawing};
pub use model_items::{ImportReport, ModelDimension, ModelMeasure};
pub use part::{Entity, Namer, Part, PartIndex, Resolution, feature_name};
pub use sheet::{Projection, RevisionRow, Scale, Sheet, Table, TableKind};
pub use soliddraft_document::{ContentHash, DocKind, DocRef, DocumentId};
pub use template::{DimensionStyle, LineWidths, Margins, PaperSize, Template, TitleBlock, TitleField, ZoneGrid};
pub use update::{
    ChangedValue, DanglingAnnotation, DanglingView, UpdateReport, edge_near, reattach, reattach_view, regenerate_view,
    update_from_model,
};
pub use view::{
    AlignAxis, Alignment, Boundary, BrokenOut, Curve2, CurveKind, FoldDirection, HiddenLines, Orientation, Polygon,
    TangentEdges, View, ViewDisplay, ViewGeometry, ViewKind, ViewScale,
};

use serde::{Deserialize, Serialize};

/// Errors of drawing operations.
#[derive(Debug, thiserror::Error)]
pub enum DrawingError {
    /// The JSON is not a valid drawing.
    #[error("drawing JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// No view has this id.
    #[error("no {0} in the drawing")]
    UnknownView(ViewId),
    /// No annotation has this id.
    #[error("no {0} in the drawing")]
    UnknownAnnotation(AnnotationId),
    /// The operation is not built yet.
    #[error("{0} is not implemented yet")]
    NotYetImplemented(&'static str),
    /// The command reads the part; run it with [`apply_with`].
    #[error("{0} needs the part")]
    NeedsModel(&'static str),
    /// The view cannot be projected, for example because its direction is zero.
    #[error("{0} cannot be projected")]
    BadView(ViewId),
    /// The request does not fit the drawing.
    #[error("{0}")]
    BadRequest(String),
    /// A model reference a view is made from no longer resolves, such as an auxiliary
    /// view's edge.
    #[error("lost reference: {0}")]
    LostReference(String),
    /// The kernel failed.
    #[error("kernel: {0}")]
    Kernel(#[from] soliddraft_kernel_api::KernelError),
    /// Reading or writing a file failed.
    #[error("{0}")]
    Io(String),
    /// A file is not a drawing this version can read.
    #[error("{0}")]
    Format(String),
    /// The views were drawn from the kernel's unproved preview, which is for the screen
    /// only. Update them from the certified mesh before exporting.
    #[error("the views come from an unproved preview; update them from the certified mesh to export")]
    Unproved,
}

/// A drawing document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Drawing {
    /// Document properties, shown in the title block.
    pub meta: DrawingMeta,
    /// The part the drawing shows, pinned at the revision its views were made from.
    pub model: DocRef,
    /// Sheets, in order.
    pub sheets: Vec<Sheet>,
    /// The letter the next section or detail view gets, such as "A".
    pub next_view_label: String,
    /// The letter the next datum gets.
    pub next_datum_label: String,
    /// Drawing options.
    #[serde(default)]
    pub settings: DrawingSettings,
}

impl Drawing {
    /// A drawing of `model` with one empty sheet made from `template`.
    pub fn new(model: DocRef, template: &Template) -> Drawing {
        let sheet = Sheet::from_template("Sheet1", template);
        Drawing {
            meta: DrawingMeta::default(),
            model,
            sheets: vec![sheet],
            next_view_label: "A".to_owned(),
            next_datum_label: "A".to_owned(),
            settings: DrawingSettings::default(),
        }
    }

    /// Pretty-printed JSON, the content of `drawing.json`.
    pub fn to_json(&self) -> Result<String, DrawingError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Reads a drawing from JSON.
    pub fn from_json(json: &str) -> Result<Drawing, DrawingError> {
        Ok(serde_json::from_str(json)?)
    }

    /// The drawing's revision: BLAKE3 over the canonical `drawing.json`, without the
    /// views' cached line work.
    pub fn semantic_hash(&self) -> Result<ContentHash, DrawingError> {
        let mut bare = self.clone();
        for v in bare.sheets.iter_mut().flat_map(|s| s.views.iter_mut()) {
            v.geometry = ViewGeometry::default();
        }
        Ok(soliddraft_document::manifest::semantic_hash(DocKind::Drawing, &serde_json::to_value(&bare)?))
    }

    /// True when the views show the part revision the drawing is pinned at: every view
    /// that takes part in updates was made from it.
    pub fn views_match_pin(&self) -> bool {
        let pinned = self.model.pinned.as_str();
        self.sheets
            .iter()
            .flat_map(|s| &s.views)
            .filter(|v| !v.excluded_from_update)
            .all(|v| v.geometry.source_digest == pinned)
    }

    /// The view with id `id`, on any sheet.
    pub fn view(&self, id: ViewId) -> Option<&View> {
        self.sheets.iter().find_map(|s| s.view(id))
    }

    /// The view with id `id`, on any sheet, for changing.
    pub fn view_mut(&mut self, id: ViewId) -> Option<&mut View> {
        self.sheets.iter_mut().flat_map(|s| s.views.iter_mut()).find(|v| v.id == id)
    }

    /// The annotation with id `id`, on any sheet.
    pub fn annotation(&self, id: AnnotationId) -> Option<&Annotation> {
        self.sheets.iter().flat_map(|s| &s.annotations).find(|a| a.id == id)
    }

    /// The annotation with id `id`, on any sheet, for changing.
    pub fn annotation_mut(&mut self, id: AnnotationId) -> Option<&mut Annotation> {
        self.sheets.iter_mut().flat_map(|s| s.annotations.iter_mut()).find(|a| a.id == id)
    }

    /// The sheet that holds view `id`.
    pub fn sheet_of(&self, id: ViewId) -> Option<&Sheet> {
        self.sheets.iter().find(|s| s.view(id).is_some())
    }

    /// Annotations that changed or lost references in the last update, sheet by sheet:
    /// the content of the Needs attention panel.
    pub fn annotations_needing_attention(&self) -> impl Iterator<Item = &Annotation> {
        self.sheets.iter().flat_map(|s| &s.annotations).filter(|a| a.needs_attention())
    }

    /// One more than the largest view id on any sheet.
    pub fn next_view_id(&self) -> ViewId {
        self.sheets.iter().map(Sheet::next_view_id).max().unwrap_or(ViewId(1))
    }

    /// One more than the largest annotation id on any sheet.
    pub fn next_annotation_id(&self) -> AnnotationId {
        self.sheets.iter().map(Sheet::next_annotation_id).max().unwrap_or(AnnotationId(1))
    }
}

/// Document properties of a drawing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DrawingMeta {
    /// Title, such as "Bracket".
    pub title: String,
    /// Drawing number.
    pub number: String,
    /// Revision of the drawing.
    pub revision: String,
    /// Who drew it.
    pub designer: String,
    /// Date of the drawing as written in the title block, such as "2026-10-14".
    pub date: String,
    /// Units of dimensions.
    pub units: Units,
}

/// Units of dimensions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Units {
    /// Millimetres.
    #[default]
    Millimetre,
    /// Inches.
    Inch,
}

/// Drawing options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DrawingSettings {
    /// Regenerate views after every rebuild of the part while both are open.
    pub automatic_update: bool,
    /// Update when the drawing is opened and the part changed on disk. Off by default:
    /// a drawing opens showing the part revision it is pinned at, and a Rebuild moves
    /// it.
    pub update_on_open: bool,
}

impl Default for DrawingSettings {
    fn default() -> DrawingSettings {
        DrawingSettings { automatic_update: true, update_on_open: false }
    }
}
