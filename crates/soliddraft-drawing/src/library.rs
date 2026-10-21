//! Sheet templates and drafting styles, read from the data files a drawing office would
//! keep: `templates/*.toml` (paper, margins, zones, line widths, title block rules and
//! fields, default notes) and `styles/*.toml` (dimension styles, hatch patterns,
//! arrowheads, fonts). The files are compiled in, so a drawing opens without a data
//! directory; they are parsed once and cached. [`TemplateFile::template`] turns one into
//! the [`Template`] that [`Drawing::new`](crate::Drawing::new) takes.
//!
//! An office adds its own templates and styles through File locations: the app hands
//! the listed files to [`use_files`] at startup, and they join the built-in ones.

use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Deserialize;
use soliddraft_kernel_api::Point2;

use crate::sheet::{Projection, Scale};
use crate::template::{DimensionStyle, LineWidths, Margins, PaperSize, Template, TitleBlock, TitleField, ZoneGrid};

/// One sheet template file.
#[derive(Clone, Debug, Deserialize)]
pub struct TemplateFile {
    /// Id, such as "iso-a3".
    pub id: String,
    /// Name shown in pickers.
    pub name: String,
    /// Drafting standard: "iso" or "asme".
    pub standard: String,
    /// Default projection angle: "first_angle" or "third_angle".
    pub projection: String,
    /// Units of dimensions, such as "mm".
    #[serde(default = "default_units")]
    pub units: String,
    /// Id of the dimension style in `styles/`.
    #[serde(default = "default_style")]
    pub style: String,
    /// Paper size.
    pub paper: Paper,
    /// Distances from the paper edge to the frame.
    pub margins: MarginsDef,
    /// Zone grid along the frame.
    pub zones: ZonesDef,
    /// Line widths.
    pub lines: LinesDef,
    /// Scales offered, largest first, such as "1:2".
    #[serde(default)]
    pub scales: Vec<String>,
    /// The title block.
    pub title_block: TitleBlockDef,
    /// The general notes block, if the template has one.
    #[serde(default)]
    pub notes: Option<NotesDef>,
}

fn default_units() -> String {
    "mm".into()
}

fn default_style() -> String {
    "iso".into()
}

/// A paper size.
#[derive(Clone, Debug, Deserialize)]
pub struct Paper {
    /// Name, such as "A3".
    pub name: String,
    /// Width in millimetres.
    pub width: f64,
    /// Height in millimetres.
    pub height: f64,
}

/// Distances from the paper edge to the frame, in millimetres.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct MarginsDef {
    /// Left, the filing margin on ISO sheets.
    pub left: f64,
    /// Right.
    pub right: f64,
    /// Top.
    pub top: f64,
    /// Bottom.
    pub bottom: f64,
}

/// The zone grid.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct ZonesDef {
    /// Numbered columns from the left.
    pub columns: u32,
    /// Lettered rows from the top.
    pub rows: u32,
    /// Width of the band inside the frame that carries the zone labels.
    #[serde(default = "default_band")]
    pub band: f64,
}

fn default_band() -> f64 {
    5.0
}

/// Line widths in millimetres, after ISO 128.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct LinesDef {
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
    /// The frame and the title block outline.
    pub border: f64,
    /// Zone ticks and title block rules.
    #[serde(default = "default_thin")]
    pub thin: f64,
}

fn default_thin() -> f64 {
    0.18
}

/// The title block.
#[derive(Clone, Debug, Deserialize)]
pub struct TitleBlockDef {
    /// Width in millimetres.
    pub width: f64,
    /// Height in millimetres.
    pub height: f64,
    /// Lines inside the block, from its bottom-left corner.
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// The fields.
    #[serde(default, rename = "field")]
    pub fields: Vec<FieldDef>,
}

/// A line inside the title block.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Rule {
    /// Start, from the block's bottom-left corner.
    pub from: [f64; 2],
    /// End.
    pub to: [f64; 2],
}

/// A field of the title block.
#[derive(Clone, Debug, Deserialize)]
pub struct FieldDef {
    /// Key, used by [`Sheet::format_overrides`](crate::Sheet::format_overrides).
    pub key: String,
    /// The caption in the top-left corner of the cell.
    pub label: String,
    /// Literal text with `${property}` links.
    #[serde(default)]
    pub value: String,
    /// `[x, y, width, height]` from the block's bottom-left corner.
    pub cell: [f64; 4],
    /// Cap height of the value in millimetres.
    #[serde(default = "default_field_size")]
    pub size: f64,
    /// Weight of the value.
    #[serde(default)]
    pub weight: Weight,
    /// Alignment of the value in the cell.
    #[serde(default)]
    pub align: Align,
    /// `"projection"` draws the projection symbol in the cell instead of text.
    #[serde(default)]
    pub symbol: Option<String>,
}

fn default_field_size() -> f64 {
    3.5
}

/// Font weight.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Weight {
    /// Regular.
    #[default]
    Regular,
    /// Medium.
    Medium,
    /// Semibold.
    Semibold,
}

/// Horizontal alignment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    /// Left, or the start of the line.
    #[default]
    Left,
    /// Centred.
    Centre,
    /// Right, or the end of the line.
    Right,
}

/// The general notes block.
#[derive(Clone, Debug, Deserialize)]
pub struct NotesDef {
    /// From the drawing area's bottom-left corner: right, then up.
    pub offset: [f64; 2],
    /// The heading, such as "NOTES".
    #[serde(default = "default_notes_heading")]
    pub heading: String,
    /// Cap height of the lines.
    #[serde(default = "default_notes_height")]
    pub height: f64,
    /// The notes.
    #[serde(default)]
    pub lines: Vec<String>,
}

fn default_notes_heading() -> String {
    "NOTES".into()
}

fn default_notes_height() -> f64 {
    2.5
}

/// A dimension style file.
#[derive(Clone, Debug, Deserialize)]
pub struct StyleFile {
    /// Id, such as "iso".
    pub id: String,
    /// Name shown in pickers.
    pub name: String,
    /// Lettering.
    pub text: TextStyle,
    /// Arrowheads.
    pub arrows: ArrowStyle,
    /// Dimension line spacing.
    pub lines: DimLines,
    /// Symbols in dimension text.
    pub symbols: Symbols,
    /// Centre marks.
    pub centre_mark: CentreMarkStyle,
}

/// Lettering of a dimension style.
#[derive(Clone, Debug, Deserialize)]
pub struct TextStyle {
    /// A font id from `fonts.toml`.
    pub font: String,
    /// Cap height of dimension values.
    pub height: f64,
    /// Cap height of notes and callouts.
    pub note_height: f64,
    /// Cap height of view captions.
    pub label_height: f64,
    /// "above" the dimension line (ISO) or "inline", breaking it (ASME).
    pub position: String,
    /// Clearance between text and its line.
    pub gap: f64,
    /// Decimal separator.
    #[serde(default = "default_decimal")]
    pub decimal_separator: String,
}

fn default_decimal() -> String {
    ".".into()
}

/// Arrowheads of a dimension style.
#[derive(Clone, Debug, Deserialize)]
pub struct ArrowStyle {
    /// An arrow id from `arrows.toml`.
    pub kind: String,
    /// Arrow length in millimetres.
    pub length: f64,
}

/// Spacing of dimension lines, in millimetres.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct DimLines {
    /// From the outline to the start of an extension line.
    pub extension_gap: f64,
    /// How far an extension line runs past the dimension line.
    pub extension_overshoot: f64,
    /// First dimension line from the outline.
    pub offset: f64,
    /// Between parallel dimension lines.
    pub spacing: f64,
    /// The horizontal tail of a leader before its text.
    pub leader_shoulder: f64,
    /// Arrows go outside when the span is shorter than this many arrow lengths.
    #[serde(default = "default_inside_min")]
    pub inside_min: f64,
}

fn default_inside_min() -> f64 {
    2.0
}

/// Symbols in dimension text.
#[derive(Clone, Debug, Deserialize)]
pub struct Symbols {
    /// Before a diameter.
    pub diameter: String,
    /// Before a radius.
    pub radius: String,
    /// After an angle.
    pub degree: String,
    /// Before a hole depth.
    pub depth: String,
    /// Counterbore.
    pub counterbore: String,
    /// Countersink.
    pub countersink: String,
    /// Between a count and what it counts, as in "4× Ø9".
    pub times: String,
    /// After a through hole.
    pub through: String,
    /// Write reference dimensions in parentheses.
    #[serde(default)]
    pub reference_parentheses: bool,
}

/// Centre mark sizes in millimetres.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct CentreMarkStyle {
    /// Half-length of the central cross.
    pub size: f64,
    /// Gap between the cross and the extensions.
    pub gap: f64,
    /// How far the centre lines reach past the circle.
    pub extension: f64,
}

#[derive(Clone, Debug, Deserialize)]
struct HatchFile {
    #[serde(rename = "pattern")]
    patterns: Vec<HatchPattern>,
}

/// A section hatch pattern.
#[derive(Clone, Debug, Deserialize)]
pub struct HatchPattern {
    /// Id.
    pub id: String,
    /// Name shown in pickers.
    pub name: String,
    /// Angle of the lines in degrees.
    pub angle: f64,
    /// Distance between lines.
    pub spacing: f64,
    /// Line width.
    pub width: f64,
    /// Lines come in pairs.
    #[serde(default)]
    pub double: bool,
    /// Gap within a pair.
    #[serde(default)]
    pub double_gap: f64,
    /// A second set of lines at right angles.
    #[serde(default)]
    pub cross: bool,
    /// Dash pattern, empty for continuous lines.
    #[serde(default)]
    pub dash: Vec<f64>,
}

#[derive(Clone, Debug, Deserialize)]
struct ArrowsFile {
    #[serde(rename = "arrow")]
    arrows: Vec<ArrowDef>,
}

/// An arrowhead definition, sized as multiples of the style's arrow length.
#[derive(Clone, Debug, Deserialize)]
pub struct ArrowDef {
    /// Id.
    pub id: String,
    /// Name shown in pickers.
    pub name: String,
    /// Length as a multiple of the style's arrow length.
    pub length: f64,
    /// Width as a multiple of the style's arrow length.
    pub width: f64,
    /// Filled.
    #[serde(default)]
    pub filled: bool,
    /// Closed outline.
    #[serde(default)]
    pub closed: bool,
    /// An oblique stroke instead of an arrow.
    #[serde(default)]
    pub stroke: bool,
    /// A dot instead of an arrow.
    #[serde(default)]
    pub dot: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct FontsFile {
    #[serde(rename = "font")]
    fonts: Vec<FontDef>,
}

/// A sheet font.
#[derive(Clone, Debug, Deserialize)]
pub struct FontDef {
    /// Id.
    pub id: String,
    /// Family name.
    pub family: String,
    /// Font files.
    #[serde(default)]
    pub files: Vec<String>,
    /// PDF base font to fall back on.
    #[serde(default = "default_pdf_base")]
    pub pdf_base: String,
    /// DXF text style name.
    #[serde(default = "default_dxf_style")]
    pub dxf_style: String,
    /// Lettering heights offered.
    #[serde(default)]
    pub heights: Vec<f64>,
}

fn default_pdf_base() -> String {
    "Helvetica".into()
}

fn default_dxf_style() -> String {
    "STANDARD".into()
}

/// Everything the data files define.
pub struct Library {
    /// Sheet templates.
    pub templates: Vec<TemplateFile>,
    /// Dimension styles.
    pub styles: Vec<StyleFile>,
    /// Hatch patterns.
    pub hatch: Vec<HatchPattern>,
    /// Arrowheads.
    pub arrows: Vec<ArrowDef>,
    /// Fonts.
    pub fonts: Vec<FontDef>,
    /// Template and style files from File locations that could not be used, with why.
    pub problems: Vec<String>,
}

/// Template and style files from File locations, read when the library is first built.
static USER_FILES: OnceLock<(Vec<PathBuf>, Vec<PathBuf>)> = OnceLock::new();

/// Adds sheet templates and drafting styles from files, such as the ones File locations
/// lists. They are read when the library is first used, after the built-in ones, so
/// call this at startup. A file that does not parse, or whose id is already taken, is
/// skipped and listed in [`Library::problems`]: built-in templates cannot be replaced.
/// Returns false when the files were already set or the library is already built.
pub fn use_files(templates: Vec<PathBuf>, styles: Vec<PathBuf>) -> bool {
    USER_FILES.set((templates, styles)).is_ok() && LIBRARY.get().is_none()
}

/// Reads `files` into `into`, skipping ids already present.
fn add_user<T: serde::de::DeserializeOwned>(
    files: &[PathBuf],
    into: &mut Vec<T>,
    id: impl Fn(&T) -> &str,
    problems: &mut Vec<String>,
) {
    for path in files {
        let parsed = std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|text| toml::from_str::<T>(&text).map_err(|e| e.to_string()));
        match parsed {
            Ok(item) if into.iter().any(|t| id(t) == id(&item)) => {
                problems.push(format!("{}: the id {} is already taken", path.display(), id(&item)));
            }
            Ok(item) => into.push(item),
            Err(e) => problems.push(format!("{}: {e}", path.display())),
        }
    }
}

static LIBRARY: OnceLock<Library> = OnceLock::new();

const TEMPLATE_SOURCES: [(&str, &str); 4] = [
    ("iso-a3", include_str!("../templates/iso-a3.toml")),
    ("iso-a4", include_str!("../templates/iso-a4.toml")),
    ("ansi-b", include_str!("../templates/ansi-b.toml")),
    ("ansi-a", include_str!("../templates/ansi-a.toml")),
];
const STYLE_SOURCES: [(&str, &str); 2] =
    [("iso", include_str!("../styles/iso.toml")), ("asme", include_str!("../styles/asme.toml"))];
const HATCH_SOURCE: &str = include_str!("../styles/hatch.toml");
const ARROWS_SOURCE: &str = include_str!("../styles/arrows.toml");
const FONTS_SOURCE: &str = include_str!("../styles/fonts.toml");

fn parse<T: serde::de::DeserializeOwned>(name: &str, text: &str) -> T {
    toml::from_str(text).unwrap_or_else(|e| panic!("drawing data file {name}.toml: {e}"))
}

/// The parsed data files. Parsed on first use.
pub fn library() -> &'static Library {
    LIBRARY.get_or_init(|| {
        let mut lib = Library {
            templates: TEMPLATE_SOURCES.iter().map(|(n, t)| parse(n, t)).collect(),
            styles: STYLE_SOURCES.iter().map(|(n, t)| parse(n, t)).collect(),
            hatch: parse::<HatchFile>("hatch", HATCH_SOURCE).patterns,
            arrows: parse::<ArrowsFile>("arrows", ARROWS_SOURCE).arrows,
            fonts: parse::<FontsFile>("fonts", FONTS_SOURCE).fonts,
            problems: Vec::new(),
        };
        if let Some((templates, styles)) = USER_FILES.get() {
            add_user(styles, &mut lib.styles, |s: &StyleFile| &s.id, &mut lib.problems);
            add_user(templates, &mut lib.templates, |t: &TemplateFile| &t.id, &mut lib.problems);
        }
        lib
    })
}

/// The template with id `id`, or the default (ISO A3) when there is none.
pub fn template(id: &str) -> &'static TemplateFile {
    let lib = library();
    lib.templates.iter().find(|t| t.id == id).unwrap_or(&lib.templates[0])
}

/// The dimension style with id `id`, or ISO when there is none.
pub fn style(id: &str) -> &'static StyleFile {
    let lib = library();
    lib.styles.iter().find(|s| s.id == id).unwrap_or(&lib.styles[0])
}

/// The arrowhead with id `id`, or the filled one.
pub fn arrow(id: &str) -> &'static ArrowDef {
    let lib = library();
    lib.arrows.iter().find(|a| a.id == id).unwrap_or(&lib.arrows[0])
}

/// The font with id `id`, or the first one.
pub fn font(id: &str) -> &'static FontDef {
    let lib = library();
    lib.fonts.iter().find(|f| f.id == id).unwrap_or(&lib.fonts[0])
}

/// Parses "1:2" into a scale.
pub fn parse_scale(text: &str) -> Option<Scale> {
    let (n, d) = text.split_once(':')?;
    Some(Scale::new(n.trim().parse().ok()?, d.trim().parse().ok()?))
}

impl TemplateFile {
    /// Where the frame (the border line) runs: `[left, bottom, right, top]`.
    pub fn frame(&self) -> [f64; 4] {
        [
            self.margins.left,
            self.margins.bottom,
            self.paper.width - self.margins.right,
            self.paper.height - self.margins.top,
        ]
    }

    /// Bottom-left corner of the title block.
    pub fn title_block_origin(&self) -> Point2 {
        let [_, bottom, right, _] = self.frame();
        Point2::new(right - self.title_block.width, bottom)
    }

    /// The area views may use: inside the frame and its zone band, above the title
    /// block: `[left, bottom, right, top]`.
    pub fn drawing_area(&self) -> [f64; 4] {
        let [l, b, r, t] = self.frame();
        let band = self.zones.band;
        [l + band, b + band + self.title_block.height, r - band, t - band]
    }

    /// The default projection angle.
    pub fn projection(&self) -> Projection {
        if self.projection == "third_angle" { Projection::ThirdAngle } else { Projection::FirstAngle }
    }

    /// The drafting standard's dimension style.
    pub fn dimension_style(&self) -> DimensionStyle {
        if self.standard == "asme" { DimensionStyle::Asme } else { DimensionStyle::Iso }
    }

    /// The style file this template uses.
    pub fn style_file(&self) -> &'static StyleFile {
        style(&self.style)
    }

    /// The scales offered, largest first; full size when the file lists none.
    pub fn scales(&self) -> Vec<Scale> {
        let parsed: Vec<Scale> = self.scales.iter().filter_map(|s| parse_scale(s)).collect();
        if parsed.is_empty() { vec![Scale::FULL] } else { parsed }
    }

    /// The model's template: what `Drawing::new` needs.
    pub fn template(&self) -> Template {
        let origin = self.title_block_origin();
        let fields = self
            .title_block
            .fields
            .iter()
            .filter(|f| f.symbol.is_none())
            .map(|f| TitleField {
                key: f.key.clone(),
                label: f.label.clone(),
                link: f.value.clone(),
                position: Point2::new(origin.u + f.cell[0] + 1.5, origin.v + f.cell[1] + 1.5),
                size: f.size,
            })
            .collect();
        Template {
            id: self.id.as_str().into(),
            name: self.name.clone(),
            size: PaperSize::new(&self.paper.name, self.paper.width, self.paper.height),
            margins: Margins {
                left: self.margins.left,
                right: self.margins.right,
                top: self.margins.top,
                bottom: self.margins.bottom,
            },
            zones: ZoneGrid::new(self.zones.columns, self.zones.rows),
            title_block: TitleBlock {
                position: origin,
                width: self.title_block.width,
                height: self.title_block.height,
                fields,
            },
            projection: self.projection(),
            dimension_style: self.dimension_style(),
            line_widths: LineWidths {
                visible: self.lines.visible,
                hidden: self.lines.hidden,
                centre: self.lines.centre,
                dimension: self.lines.dimension,
                cutting: self.lines.cutting,
                border: self.lines.border,
            },
            scales: self.scales(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_files_parse_and_match_the_model_templates() {
        let lib = library();
        let ids: Vec<&str> = lib.templates.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["iso-a3", "iso-a4", "ansi-b", "ansi-a"]);
        for t in &lib.templates {
            let model = t.template();
            let builtin = Template::by_id(&t.id).expect("the model knows this template");
            assert_eq!(model.size, builtin.size, "{}", t.id);
            assert_eq!(model.projection, builtin.projection, "{}", t.id);
            assert!(model.title_block.fields.iter().any(|f| f.link == "${title}"), "{}", t.id);
            let [l, b, r, top] = t.drawing_area();
            assert!(l < r && b < top, "{}", t.id);
            assert!(t.title_block.fields.iter().any(|f| f.symbol.as_deref() == Some("projection")));
        }
        assert_eq!(lib.styles.len(), 2);
        assert_eq!(style("iso").text.position, "above");
        assert_eq!(style("asme").text.position, "inline");
        assert!(lib.hatch.iter().any(|h| h.id == "iso-general"));
        assert!(arrow("filled").filled);
        assert_eq!(font("inter").family, "Inter");
        assert_eq!(parse_scale("1:2"), Some(Scale::new(1, 2)));
        assert_eq!(template("nope").id, "iso-a3");
    }
}
