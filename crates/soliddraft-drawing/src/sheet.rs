//! Sheets: one page of a drawing with its views, annotations and tables.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use soliddraft_kernel_api::Point2;

use crate::annotation::Annotation;
use crate::ids::{AnnotationId, TemplateId, ViewId};
use crate::template::{PaperSize, ZoneGrid};
use crate::view::View;

/// One sheet of a drawing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sheet {
    /// Name, such as "Sheet1".
    pub name: String,
    /// The template its format comes from.
    pub template: TemplateId,
    /// Paper size.
    pub size: PaperSize,
    /// The scale views follow by default.
    pub scale: Scale,
    /// Projection angle.
    pub projection: Projection,
    /// Zone grid along the border.
    pub zones: ZoneGrid,
    /// Title block values that replace the linked document properties, by field key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub format_overrides: BTreeMap<String, String>,
    /// Views, parents before their children.
    #[serde(default)]
    pub views: Vec<View>,
    /// Annotations.
    #[serde(default)]
    pub annotations: Vec<Annotation>,
    /// Tables.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tables: Vec<Table>,
}

impl Sheet {
    /// An empty sheet named `name` made from the template with id `template`: its
    /// paper, projection angle and zones, at full scale.
    pub fn from_template(name: &str, template: &crate::Template) -> Sheet {
        Sheet {
            name: name.to_owned(),
            template: template.id.clone(),
            size: template.size.clone(),
            scale: Scale::FULL,
            projection: template.projection,
            zones: template.zones,
            format_overrides: BTreeMap::new(),
            views: Vec::new(),
            annotations: Vec::new(),
            tables: Vec::new(),
        }
    }

    /// The sheet's revision table, if it has one.
    pub fn revision_table(&self) -> Option<&Table> {
        self.tables.iter().find(|t| t.kind == TableKind::Revision)
    }

    /// One more than the largest view id on this sheet. With several sheets use
    /// [`Drawing::next_view_id`](crate::Drawing::next_view_id), which looks at all of them.
    pub fn next_view_id(&self) -> ViewId {
        ViewId(self.views.iter().map(|v| v.id.0 + 1).max().unwrap_or(1))
    }

    /// One more than the largest annotation id on this sheet.
    pub fn next_annotation_id(&self) -> AnnotationId {
        AnnotationId(self.annotations.iter().map(|a| a.id.0 + 1).max().unwrap_or(1))
    }

    /// The view with id `id`.
    pub fn view(&self, id: ViewId) -> Option<&View> {
        self.views.iter().find(|v| v.id == id)
    }

    /// The annotations of view `id`.
    pub fn annotations_of(&self, id: ViewId) -> impl Iterator<Item = &Annotation> {
        self.annotations.iter().filter(move |a| a.view == id)
    }
}

/// A drawing scale as a ratio, such as 1:2.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Scale {
    /// Sheet length.
    pub numerator: u32,
    /// Model length.
    pub denominator: u32,
}

impl Scale {
    /// Full size.
    pub const FULL: Scale = Scale::new(1, 1);

    /// The scale `numerator:denominator`.
    pub const fn new(numerator: u32, denominator: u32) -> Scale {
        Scale { numerator, denominator }
    }

    /// Sheet millimetres per model millimetre.
    pub fn factor(self) -> f64 {
        f64::from(self.numerator) / f64::from(self.denominator)
    }

    /// The ratio for a factor: 0.5 is 1:2, 2.5 is 5:2. Factors that are not a ratio of
    /// small whole numbers are rounded to the nearest one with a denominator up to 1000.
    pub fn from_factor(factor: f64) -> Scale {
        let f = factor.abs().max(1e-6);
        let mut best = (f64::INFINITY, Scale::FULL);
        for d in 1..=1000u32 {
            let n = (f * f64::from(d)).round().max(1.0);
            let err = (n / f64::from(d) - f).abs() / f;
            if err < best.0 - 1e-12 {
                best = (err, Scale::new(n as u32, d));
            }
            if err < 1e-9 {
                break;
            }
        }
        let s = best.1;
        let g = gcd(s.numerator, s.denominator);
        Scale::new(s.numerator / g, s.denominator / g)
    }
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a.max(1) } else { gcd(b, a % b) }
}

impl fmt::Display for Scale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.numerator, self.denominator)
    }
}

/// The projection angle, which decides where folded views go.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Projection {
    /// ISO: a view is placed on the side opposite the face it shows (the left view goes
    /// right of the front view, the top view below it).
    FirstAngle,
    /// ASME: a view is placed on the side of the face it shows.
    ThirdAngle,
}

/// A table on a sheet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Table {
    /// What the table lists.
    pub kind: TableKind,
    /// Top-left corner in sheet millimetres.
    pub position: Point2,
    /// The rows of a revision table, oldest first. The last one is the drawing's
    /// current revision.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub revisions: Vec<RevisionRow>,
}

impl Table {
    /// An empty revision table with its top-left corner at `position`.
    pub fn revision(position: Point2) -> Table {
        Table { kind: TableKind::Revision, position, revisions: Vec::new() }
    }
}

/// One row of a revision table.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionRow {
    /// The revision, such as "B".
    pub revision: String,
    /// What changed.
    pub description: String,
    /// When, as written, such as "2026-10-14".
    #[serde(default)]
    pub date: String,
    /// Who approved it.
    #[serde(default)]
    pub approved: String,
    /// The sheet zones the change is in, such as "C3". Empty to list the zones of the
    /// revision symbols that carry this revision.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub zone: String,
}

/// What a table lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TableKind {
    /// Revisions of the drawing.
    Revision,
    /// Holes with their positions and sizes.
    Hole,
    /// Bill of materials.
    Bom,
}
