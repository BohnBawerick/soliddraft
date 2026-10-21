//! Sheet templates: paper size, border, zones, title block and drafting standard.

use serde::{Deserialize, Serialize};
use soliddraft_kernel_api::Point2;

use crate::ids::TemplateId;
use crate::sheet::{Projection, Scale};

/// A sheet template.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Template {
    /// Id, such as `"iso-a3"`.
    pub id: TemplateId,
    /// Name shown in pickers.
    pub name: String,
    /// Paper size, landscape.
    pub size: PaperSize,
    /// Distance from the paper edge to the border.
    pub margins: Margins,
    /// Zone grid along the border.
    pub zones: ZoneGrid,
    /// The title block.
    pub title_block: TitleBlock,
    /// Default projection angle.
    pub projection: Projection,
    /// Dimension and text style.
    pub dimension_style: DimensionStyle,
    /// Line widths.
    pub line_widths: LineWidths,
    /// Scales offered, largest first.
    pub scales: Vec<Scale>,
}

impl Template {
    /// The four built-in templates, read from the data files in `templates/`: ISO A3
    /// and A4 (first angle) and ANSI A and B (third angle). ISO A3 comes first and is
    /// the default.
    pub fn builtin() -> Vec<Template> {
        ["iso-a3", "iso-a4", "ansi-a", "ansi-b"].into_iter().map(|id| crate::library::template(id).template()).collect()
    }

    /// The built-in template with id `id`.
    pub fn by_id(id: &str) -> Option<Template> {
        crate::library::library().templates.iter().find(|t| t.id == id).map(|t| t.template())
    }
}

/// A paper size in landscape orientation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaperSize {
    /// Name, such as "A3".
    pub name: String,
    /// Width in millimetres.
    pub width_mm: f64,
    /// Height in millimetres.
    pub height_mm: f64,
}

impl PaperSize {
    /// A paper size.
    pub fn new(name: &str, width_mm: f64, height_mm: f64) -> PaperSize {
        PaperSize { name: name.to_owned(), width_mm, height_mm }
    }

    /// ISO A3, 420 x 297 mm.
    pub fn iso_a3() -> PaperSize {
        PaperSize::new("A3", 420.0, 297.0)
    }

    /// ISO A4 landscape, 297 x 210 mm.
    pub fn iso_a4() -> PaperSize {
        PaperSize::new("A4", 297.0, 210.0)
    }

    /// ANSI A landscape, 11 x 8.5 in.
    pub fn ansi_a() -> PaperSize {
        PaperSize::new("ANSI A", 279.4, 215.9)
    }

    /// ANSI B, 17 x 11 in.
    pub fn ansi_b() -> PaperSize {
        PaperSize::new("ANSI B", 431.8, 279.4)
    }
}

/// Distances from the paper edge to the border, in millimetres.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Margins {
    /// Left, the filing margin on ISO sheets.
    pub left: f64,
    /// Right.
    pub right: f64,
    /// Top.
    pub top: f64,
    /// Bottom.
    pub bottom: f64,
}

/// The zone grid along a sheet's border: numbered columns from the left and lettered
/// rows from the top.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ZoneGrid {
    /// Number of columns.
    pub columns: u32,
    /// Number of rows.
    pub rows: u32,
}

impl ZoneGrid {
    /// A grid of `columns` by `rows`.
    pub const fn new(columns: u32, rows: u32) -> ZoneGrid {
        ZoneGrid { columns, rows }
    }
}

/// The title block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TitleBlock {
    /// Bottom-left corner in sheet millimetres.
    pub position: Point2,
    /// Width in millimetres.
    pub width: f64,
    /// Height in millimetres.
    pub height: f64,
    /// The text fields.
    pub fields: Vec<TitleField>,
}

/// A text field of the title block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TitleField {
    /// Key, used by [`Sheet::format_overrides`](crate::Sheet::format_overrides).
    pub key: String,
    /// The caption printed above the value.
    pub label: String,
    /// The value: literal text with `${property}` links to document properties, such as
    /// `${title}`, `${number}`, `${revision}`, `${designer}`, `${date}`, `${scale}`,
    /// `${sheet}` and `${sheets}`.
    pub link: String,
    /// Start of the value's baseline in sheet millimetres.
    pub position: Point2,
    /// Text height of the value in millimetres.
    pub size: f64,
}

/// Dimension and text style.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DimensionStyle {
    /// ISO 129: text above the dimension line, filled arrows.
    Iso,
    /// ASME Y14.5: text inline, breaking the dimension line.
    Asme,
}

/// Line widths in millimetres.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LineWidths {
    /// Visible edges.
    pub visible: f64,
    /// Hidden edges.
    pub hidden: f64,
    /// Centre lines and centre marks.
    pub centre: f64,
    /// Dimension, extension and leader lines.
    pub dimension: f64,
    /// Cutting lines of sections.
    pub cutting: f64,
    /// The border and the title block frame.
    pub border: f64,
}

impl LineWidths {
    /// ISO 128 widths.
    pub const ISO: LineWidths =
        LineWidths { visible: 0.5, hidden: 0.25, centre: 0.25, dimension: 0.25, cutting: 0.7, border: 0.7 };

    /// ASME Y14.2 widths.
    pub const ASME: LineWidths =
        LineWidths { visible: 0.6, hidden: 0.3, centre: 0.3, dimension: 0.3, cutting: 0.6, border: 0.6 };
}
