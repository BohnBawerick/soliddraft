//! The display list: everything a sheet shows, as paths and text in sheet millimetres
//! with Y up.
//!
//! [`layout`] is the one place that decides what a sheet looks like: the border, zones
//! and title block of its template file, each view's line work with its display options,
//! and every annotation drawn to the template's dimension style. The canvas paints the
//! list and the SVG, PDF and DXF writers write it, so what you see is what you export.
//! Every item carries the role that picks its pen and layer and the owner the canvas
//! hit-tests against.

use kurbo::{Affine, BezPath, Shape, Vec2};
use soliddraft_kernel_api::Point2;

use crate::annotation::{Annotation, AnnotationKind, AnnotationState, Attach, DimensionKind, LinearOrientation};
use crate::ids::{AnnotationId, SHEET, ViewId};
use crate::library::{Align, StyleFile, TemplateFile, Weight};
use crate::measure::{CurveShape, display_text, shape_of};
use crate::project::{effective_scale, view_bounds, view_label};

mod symbols;
mod tables;
mod views;

use crate::sheet::{Projection, Sheet};
use crate::view::{HiddenLines, TangentEdges, View, ViewKind, ViewScale};
use crate::{Drawing, library};
pub(crate) use tables::keep_above_title_block;
pub use tables::{REVISION_COLUMNS, revision_table_home, revision_table_size, table_bounds};
pub use views::{LocationLabel, clip_polygon, hatch_lines, location_labels, zone_at};

/// What a path or text is for; the canvas and the writers map roles to colours, layers
/// and pens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// Visible edges and silhouettes.
    Visible,
    /// Hidden edges, dashed.
    Hidden,
    /// Tangent edges in pictorial views.
    Phantom,
    /// Centre lines and centre marks.
    Centre,
    /// Dimension, extension and leader lines and arrowheads.
    Dimension,
    /// The frame and the title block's outline.
    Border,
    /// Thin format lines: zone ticks, title block rules.
    Thin,
    /// Text: values, notes, title block entries.
    Text,
    /// Small captions: field labels, zone letters.
    Label,
    /// Section hatching.
    Hatch,
    /// A dangling annotation.
    Error,
    /// The highlight behind a changed value.
    Changed,
    /// The hatch over an out-of-date view.
    Stale,
    /// A section's cutting plane line, its arrows and letters on the parent view.
    CuttingPlane,
    /// Thin continuous boundaries: a detail's circle, a broken-out section's edge.
    Boundary,
}

/// What an item belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Owner {
    /// The sheet format.
    Sheet,
    /// A view as a whole: its caption, its out-of-date hatch.
    View(ViewId),
    /// One curve of a view, by index into its geometry.
    Curve(ViewId, usize),
    /// An annotation.
    Annotation(AnnotationId),
    /// The mark at a lost end of an annotation.
    AnnotationEnd(AnnotationId, usize),
    /// A table, by index among its sheet's tables.
    Table(usize),
}

/// A stroked or filled path.
#[derive(Clone, Debug, PartialEq)]
pub struct PathItem {
    /// The path in sheet millimetres.
    pub path: BezPath,
    /// Line width in millimetres; zero for a fill.
    pub width: f64,
    /// Dash pattern in millimetres.
    pub dash: Option<Vec<f64>>,
    /// What it is for.
    pub role: Role,
    /// What it belongs to.
    pub owner: Owner,
    /// True to fill the path instead of stroking it.
    pub fill: bool,
}

/// Which line of a text's box its position is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VAlign {
    /// The baseline.
    Bottom,
    /// Half the cap height above the baseline.
    Centre,
    /// The cap height above the baseline.
    Top,
}

/// One line of text.
#[derive(Clone, Debug, PartialEq)]
pub struct TextItem {
    /// The point `halign` and `valign` name, in sheet millimetres.
    pub pos: Point2,
    /// Cap height in millimetres (ISO 3098 lettering height).
    pub height: f64,
    /// Counter-clockwise rotation in degrees.
    pub angle: f64,
    /// The text.
    pub text: String,
    /// Which end of the line `pos` is on.
    pub halign: Align,
    /// Which line of the text box `pos` is on.
    pub valign: VAlign,
    /// Font weight.
    pub weight: Weight,
    /// What it is for.
    pub role: Role,
    /// What it belongs to.
    pub owner: Owner,
}

/// One thing the sheet shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// A path.
    Path(PathItem),
    /// A line of text.
    Text(TextItem),
}

impl Item {
    /// What the item belongs to.
    pub fn owner(&self) -> Owner {
        match self {
            Item::Path(p) => p.owner,
            Item::Text(t) => t.owner,
        }
    }

    /// What the item is for.
    pub fn role(&self) -> Role {
        match self {
            Item::Path(p) => p.role,
            Item::Text(t) => t.role,
        }
    }
}

/// A laid-out sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplayList {
    /// Items in drawing order.
    pub items: Vec<Item>,
    /// Paper width and height in millimetres.
    pub paper: (f64, f64),
    /// What each dimension measures, for writers that keep dimensions as dimensions
    /// (DXF). Its lines, arrows and text are among `items`, owned by the annotation.
    pub dimensions: Vec<DimensionRecord>,
    /// The hatched regions, for writers that keep hatches as hatches (DXF). Their lines
    /// are among `items` with [`Role::Hatch`].
    pub hatches: Vec<HatchRecord>,
}

/// One dimension as its definition points, in sheet millimetres.
#[derive(Clone, Debug, PartialEq)]
pub struct DimensionRecord {
    /// The annotation it belongs to. A dimension set gives one record per value.
    pub owner: AnnotationId,
    /// What it measures.
    pub geometry: DimensionGeometry,
    /// The text as drawn.
    pub text: String,
    /// The value in millimetres or degrees.
    pub value: f64,
    /// The middle of the text.
    pub text_mid: Point2,
}

/// The definition points of a dimension.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DimensionGeometry {
    /// A distance between two points along a direction.
    Linear {
        /// Where the first extension line starts.
        p1: Point2,
        /// Where the second extension line starts.
        p2: Point2,
        /// A point on the dimension line.
        line: Point2,
        /// The dimension line's angle in degrees.
        angle: f64,
    },
    /// A diameter: two opposite points of the circle.
    Diameter {
        /// The point the arrow touches.
        near: Point2,
        /// The opposite point.
        far: Point2,
    },
    /// A radius.
    Radius {
        /// The centre.
        centre: Point2,
        /// The point the arrow touches.
        on: Point2,
    },
    /// An ordinate: a feature's distance from an origin.
    Ordinate {
        /// The origin.
        origin: Point2,
        /// The feature.
        feature: Point2,
        /// Where the leader ends.
        leader_end: Point2,
        /// True when the distance is along the sheet's X.
        along_x: bool,
    },
}

/// A hatched region in sheet millimetres: loops under the even-odd rule.
#[derive(Clone, Debug, PartialEq)]
pub struct HatchRecord {
    /// The view it belongs to.
    pub owner: Owner,
    /// Closed loops, the outer boundary first.
    pub loops: Vec<Vec<Point2>>,
    /// Angle of the lines in degrees.
    pub angle: f64,
    /// Distance between lines in millimetres.
    pub spacing: f64,
}

impl DisplayList {
    /// The items that belong on paper: everything but the interface's own marks (the
    /// changed-value highlight and the out-of-date hatch).
    pub fn printable(&self) -> impl Iterator<Item = &Item> {
        self.items.iter().filter(|i| !matches!(i.role(), Role::Changed | Role::Stale))
    }
}

/// What the layout needs beside the drawing.
#[derive(Clone, Copy)]
pub struct Context<'a> {
    /// The sheet's template file.
    pub format: &'a TemplateFile,
    /// The dimension style.
    pub style: &'a StyleFile,
    /// Views whose geometry is older than the part.
    pub stale: &'a [ViewId],
    /// Index of the sheet laid out.
    pub sheet_index: usize,
    /// Number of sheets in the drawing.
    pub sheet_count: usize,
}

impl<'a> Context<'a> {
    /// The context of sheet `sheet` of `drawing` with its own template and style and no
    /// views marked out of date: what the exporters use.
    pub fn of(drawing: &Drawing, sheet: usize) -> Context<'static> {
        let format = library::template(drawing.sheets.get(sheet).map(|s| s.template.as_str()).unwrap_or("iso-a3"));
        Context {
            format,
            style: format.style_file(),
            stale: &[],
            sheet_index: sheet,
            sheet_count: drawing.sheets.len(),
        }
    }
}

/// Lays out sheet `sheet` with its own template and style, for export.
pub fn layout_sheet(drawing: &Drawing, sheet: usize) -> DisplayList {
    layout(drawing, sheet, &Context::of(drawing, sheet))
}

/// Cap height of Inter as a fraction of its em size; text is laid out by cap height.
pub const CAP_HEIGHT: f64 = 0.727;

/// Hidden line dash pattern in millimetres.
pub const HIDDEN_DASH: [f64; 2] = [3.0, 1.5];
/// Centre line dash pattern in millimetres.
pub const CENTRE_DASH: [f64; 4] = [8.0, 1.5, 1.5, 1.5];
/// Phantom line dash pattern in millimetres.
pub const PHANTOM_DASH: [f64; 6] = [8.0, 1.5, 1.5, 1.5, 1.5, 1.5];

fn p2(p: Point2) -> kurbo::Point {
    kurbo::Point::new(p.u, p.v)
}

fn line(a: Point2, b: Point2) -> BezPath {
    let mut p = BezPath::new();
    p.move_to(p2(a));
    p.line_to(p2(b));
    p
}

fn polyline(pts: &[Point2]) -> BezPath {
    let mut p = BezPath::new();
    for (i, q) in pts.iter().enumerate() {
        if i == 0 {
            p.move_to(p2(*q));
        } else {
            p.line_to(p2(*q));
        }
    }
    p
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
    kurbo::Rect::new(x0, y0, x1, y1).to_path(0.1)
}

fn add(a: Point2, v: Vec2) -> Point2 {
    Point2::new(a.u + v.x, a.v + v.y)
}

fn sub(a: Point2, b: Point2) -> Vec2 {
    Vec2::new(a.u - b.u, a.v - b.v)
}

/// Resolves `${property}` links in a title block value.
pub fn resolve_links(text: &str, lookup: &dyn Fn(&str) -> String) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                out.push_str(&lookup(&after[..end]));
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// The value of a title block property for a sheet.
pub fn property(drawing: &Drawing, sheet: &Sheet, sheet_index: usize, sheet_count: usize, key: &str) -> String {
    if let Some(v) = sheet.format_overrides.get(key) {
        return v.clone();
    }
    match key {
        "title" => drawing.meta.title.clone(),
        "number" => drawing.meta.number.clone(),
        "revision" => drawing
            .sheets
            .iter()
            .rev()
            .filter_map(|s| s.revision_table())
            .find_map(|t| t.revisions.last())
            .map(|r| r.revision.clone())
            .unwrap_or_else(|| drawing.meta.revision.clone()),
        "designer" => drawing.meta.designer.clone(),
        "date" => drawing.meta.date.clone(),
        "scale" => sheet.scale.to_string(),
        "projection" => match sheet.projection {
            Projection::FirstAngle => "First angle".into(),
            Projection::ThirdAngle => "Third angle".into(),
        },
        "sheet" => (sheet_index + 1).to_string(),
        "sheets" => sheet_count.to_string(),
        "document_type" => "Part drawing".into(),
        "units" => match drawing.meta.units {
            crate::Units::Millimetre => "mm".into(),
            crate::Units::Inch => "in".into(),
        },
        _ => String::new(),
    }
}

/// Lays out one sheet.
pub fn layout(drawing: &Drawing, sheet_index: usize, ctx: &Context<'_>) -> DisplayList {
    let mut list = DisplayList {
        items: Vec::new(),
        paper: (ctx.format.paper.width, ctx.format.paper.height),
        dimensions: Vec::new(),
        hatches: Vec::new(),
    };
    let Some(sheet) = drawing.sheets.get(sheet_index) else {
        return list;
    };
    format_items(drawing, sheet, ctx, &mut list.items);
    tables::table_items(drawing, sheet, ctx, &mut list.items);
    for view in &sheet.views {
        view_items(drawing, sheet, view, ctx, &mut list);
    }
    views::parent_marks(drawing, sheet_index, ctx, &mut list.items);
    for a in &sheet.annotations {
        annotation_items(sheet, a, ctx, &mut list);
    }
    list
}

/// Border, zones, title block and projection symbol.
fn format_items(drawing: &Drawing, sheet: &Sheet, ctx: &Context<'_>, items: &mut Vec<Item>) {
    let f = ctx.format;
    let lines = &f.lines;
    let [l, b, r, t] = f.frame();
    let band = f.zones.band;
    let push_path = |items: &mut Vec<Item>, path: BezPath, width: f64, role: Role| {
        items.push(Item::Path(PathItem { path, width, dash: None, role, owner: Owner::Sheet, fill: false }));
    };
    // Outer frame, and the inner zone line, which stops at the title block in the
    // bottom-right corner.
    push_path(items, rect(l, b, r, t), lines.border, Role::Border);
    let o = f.title_block_origin();
    let tb = &f.title_block;
    let under_title = |x: f64| x > o.u - 1e-9 && x < o.u + tb.width + 1e-9;
    push_path(
        items,
        polyline(&[
            Point2::new(o.u, b + band),
            Point2::new(l + band, b + band),
            Point2::new(l + band, t - band),
            Point2::new(r - band, t - band),
            Point2::new(r - band, o.v + tb.height),
        ]),
        lines.thin,
        Role::Thin,
    );
    // Zone ticks and labels: columns numbered from the left, rows lettered from the top.
    let cols = f.zones.columns.max(1);
    let rows = f.zones.rows.max(1);
    let cw = (r - l) / f64::from(cols);
    let rh = (t - b) / f64::from(rows);
    for i in 0..cols {
        let x = l + cw * f64::from(i);
        if i > 0 {
            if !under_title(x) {
                push_path(items, line(Point2::new(x, b), Point2::new(x, b + band)), lines.thin, Role::Thin);
            }
            push_path(items, line(Point2::new(x, t - band), Point2::new(x, t)), lines.thin, Role::Thin);
        }
        for y in [b + band / 2.0, t - band / 2.0] {
            if y < o.v + tb.height && under_title(x + cw / 2.0) {
                continue;
            }
            items.push(Item::Text(TextItem {
                pos: Point2::new(x + cw / 2.0, y),
                height: 2.5,
                angle: 0.0,
                text: (i + 1).to_string(),
                halign: Align::Centre,
                valign: VAlign::Centre,
                weight: Weight::Regular,
                role: Role::Label,
                owner: Owner::Sheet,
            }));
        }
    }
    for j in 0..rows {
        let y = t - rh * f64::from(j);
        if j > 0 {
            push_path(items, line(Point2::new(l, y), Point2::new(l + band, y)), lines.thin, Role::Thin);
            if y > o.v + tb.height {
                push_path(items, line(Point2::new(r - band, y), Point2::new(r, y)), lines.thin, Role::Thin);
            }
        }
        let letter = char::from(b'A' + (j % 26) as u8).to_string();
        for x in [l + band / 2.0, r - band / 2.0] {
            if x > o.u && y - rh / 2.0 < o.v + tb.height {
                continue;
            }
            items.push(Item::Text(TextItem {
                pos: Point2::new(x, y - rh / 2.0),
                height: 2.5,
                angle: 0.0,
                text: letter.clone(),
                halign: Align::Centre,
                valign: VAlign::Centre,
                weight: Weight::Regular,
                role: Role::Label,
                owner: Owner::Sheet,
            }));
        }
    }
    // Title block.
    push_path(items, rect(o.u, o.v, o.u + tb.width, o.v + tb.height), lines.border, Role::Border);
    for rule in &tb.rules {
        push_path(
            items,
            line(Point2::new(o.u + rule.from[0], o.v + rule.from[1]), Point2::new(o.u + rule.to[0], o.v + rule.to[1])),
            lines.thin,
            Role::Thin,
        );
    }
    let lookup = |key: &str| property(drawing, sheet, ctx.sheet_index, ctx.sheet_count, key);
    for field in &tb.fields {
        let [x, y, w, h] = field.cell;
        let cell = (o.u + x, o.v + y, w, h);
        items.push(Item::Text(TextItem {
            pos: Point2::new(cell.0 + 1.2, cell.1 + cell.3 - 1.0),
            height: 1.8,
            angle: 0.0,
            text: field.label.clone(),
            halign: Align::Left,
            valign: VAlign::Top,
            weight: Weight::Regular,
            role: Role::Label,
            owner: Owner::Sheet,
        }));
        if field.symbol.as_deref() == Some("projection") {
            projection_symbol(sheet.projection, Point2::new(cell.0 + cell.2 / 2.0, cell.1 + 3.2), lines.thin, items);
            continue;
        }
        let value = resolve_links(&field.value, &lookup);
        let value = sheet.format_overrides.get(&field.key).cloned().unwrap_or(value);
        if value.is_empty() {
            continue;
        }
        let (pos, halign) = match field.align {
            Align::Left => (Point2::new(cell.0 + 1.5, cell.1 + 1.5), Align::Left),
            Align::Centre => (Point2::new(cell.0 + cell.2 / 2.0, cell.1 + 1.5), Align::Centre),
            Align::Right => (Point2::new(cell.0 + cell.2 - 1.5, cell.1 + 1.5), Align::Right),
        };
        items.push(Item::Text(TextItem {
            pos,
            height: field.size.min(cell.3 - 5.0).max(1.8),
            angle: 0.0,
            text: value,
            halign,
            valign: VAlign::Bottom,
            weight: field.weight,
            role: Role::Text,
            owner: Owner::Sheet,
        }));
    }
}

/// The first or third angle projection symbol: a truncated cone seen from the side and
/// from its small end, the end view on the side the projection angle puts it.
fn projection_symbol(projection: Projection, centre: Point2, width: f64, items: &mut Vec<Item>) {
    let h = 4.5;
    let (cone_dx, circle_dx) = match projection {
        Projection::FirstAngle => (-4.5, 4.5),
        Projection::ThirdAngle => (4.5, -4.5),
    };
    let c = Point2::new(centre.u + cone_dx, centre.v);
    // Frustum: small end on the side facing the circles.
    let towards = if circle_dx > 0.0 { 1.0 } else { -1.0 };
    let x_small = c.u + towards * 3.0;
    let x_big = c.u - towards * 3.0;
    let trapezoid = polyline(&[
        Point2::new(x_small, c.v - h * 0.3),
        Point2::new(x_big, c.v - h * 0.5),
        Point2::new(x_big, c.v + h * 0.5),
        Point2::new(x_small, c.v + h * 0.3),
        Point2::new(x_small, c.v - h * 0.3),
    ]);
    items.push(Item::Path(PathItem {
        path: trapezoid,
        width,
        dash: None,
        role: Role::Thin,
        owner: Owner::Sheet,
        fill: false,
    }));
    let k = Point2::new(centre.u + circle_dx, centre.v);
    for r in [h * 0.5, h * 0.3] {
        items.push(Item::Path(PathItem {
            path: kurbo::Circle::new(p2(k), r).to_path(0.05),
            width,
            dash: None,
            role: Role::Thin,
            owner: Owner::Sheet,
            fill: false,
        }));
    }
    let ext = h * 0.75;
    for (a, b) in [
        (Point2::new(k.u - ext, k.v), Point2::new(k.u + ext, k.v)),
        (Point2::new(k.u, k.v - ext), Point2::new(k.u, k.v + ext)),
        (Point2::new(c.u - 4.0, c.v), Point2::new(c.u + 4.0, c.v)),
    ] {
        items.push(Item::Path(PathItem {
            path: line(a, b),
            width,
            dash: Some(vec![1.5, 0.5, 0.5, 0.5]),
            role: Role::Thin,
            owner: Owner::Sheet,
            fill: false,
        }));
    }
}

/// A view's line work, hatching, scale caption and, when it is out of date, its hatched
/// boundary.
fn view_items(drawing: &Drawing, sheet: &Sheet, view: &View, ctx: &Context<'_>, list: &mut DisplayList) {
    let lines = &ctx.format.lines;
    let shift = Affine::translate((view.position.u, view.position.v));
    let items = &mut list.items;
    views::hatch_items(drawing, view, ctx, items, &mut list.hatches);
    for (i, c) in view.geometry.curves.iter().enumerate() {
        let (role, width, dash) = if !c.visible {
            if view.display.hidden == HiddenLines::Removed {
                continue;
            }
            (Role::Hidden, lines.hidden, Some(HIDDEN_DASH.to_vec()))
        } else if c.tangent && view.display.tangent == TangentEdges::Phantom {
            (Role::Phantom, lines.thin, Some(PHANTOM_DASH.to_vec()))
        } else {
            (Role::Visible, lines.visible, None)
        };
        let mut path = c.path.clone();
        path.apply_affine(shift);
        items.push(Item::Path(PathItem { path, width, dash, role, owner: Owner::Curve(view.id, i), fill: false }));
    }
    views::boundary_items(view, ctx, items);
    let [x0, y0, x1, y1] = view_bounds(view);
    let sheet_index = drawing.sheets.iter().position(|s| std::ptr::eq(s, sheet)).unwrap_or(ctx.sheet_index);
    let scale = crate::project::scale_in(drawing, sheet_index, view);
    let caption = match &view.kind {
        ViewKind::Section { label, .. } => Some(format!("SECTION {label}-{label}")),
        ViewKind::Detail { label, .. } => Some(format!("DETAIL {label}")),
        ViewKind::Auxiliary { label, .. } if !label.is_empty() => Some(format!("VIEW {label}")),
        _ => None,
    };
    let scale_differs = match view.scale {
        ViewScale::Sheet => false,
        _ => (scale - sheet.scale.factor()).abs() > 1e-9,
    } || matches!(view.kind, ViewKind::Detail { .. });
    let mut lines_out = Vec::new();
    if let Some(c) = caption {
        lines_out.push(c);
    }
    if scale_differs {
        lines_out.push(format!("SCALE {}", scale_text(scale)));
    }
    if let Some(from) = views::location_from(drawing, view) {
        lines_out.push(from);
    }
    for (n, text) in lines_out.into_iter().enumerate() {
        items.push(Item::Text(TextItem {
            pos: Point2::new((x0 + x1) / 2.0, y0 - 4.0 - 4.5 * n as f64),
            height: ctx.style.text.label_height,
            angle: 0.0,
            text,
            halign: Align::Centre,
            valign: VAlign::Top,
            weight: Weight::Medium,
            role: Role::Text,
            owner: Owner::View(view.id),
        }));
    }
    if ctx.stale.contains(&view.id) {
        let m = 3.0;
        let (bx0, by0, bx1, by1) = (x0 - m, y0 - m, x1 + m, y1 + m);
        items.push(Item::Path(PathItem {
            path: rect(bx0, by0, bx1, by1),
            width: lines.thin,
            dash: Some(vec![2.0, 1.0]),
            role: Role::Stale,
            owner: Owner::View(view.id),
            fill: false,
        }));
        let mut hatch = BezPath::new();
        let step = 6.0;
        let mut d = by0 - (bx1 - bx0);
        while d < by1 {
            // The line y = x + d clipped to the box.
            let xa = (by0 - d).max(bx0);
            let xb = (by1 - d).min(bx1);
            if xa < xb {
                hatch.move_to((xa, xa + d));
                hatch.line_to((xb, xb + d));
            }
            d += step;
        }
        items.push(Item::Path(PathItem {
            path: hatch,
            width: lines.thin,
            dash: None,
            role: Role::Stale,
            owner: Owner::View(view.id),
            fill: false,
        }));
    }
}

/// "1:2" or "2:1" for a factor.
pub fn scale_text(factor: f64) -> String {
    if factor >= 1.0 {
        let n = factor.round();
        if (factor - n).abs() < 1e-9 { format!("{n}:1") } else { format!("{factor:.2}:1") }
    } else {
        let d = 1.0 / factor;
        let n = d.round();
        if (d - n).abs() < 1e-9 { format!("1:{n}") } else { format!("1:{d:.2}") }
    }
}

/// Sheet position of a view-millimetre point.
fn to_sheet(sheet: &Sheet, view: ViewId, p: Point2) -> Point2 {
    if view == SHEET {
        return p;
    }
    match sheet.view(view) {
        Some(v) => Point2::new(p.u + v.position.u, p.v + v.position.v),
        None => p,
    }
}

/// An arrowhead with its tip at `tip`, pointing along `dir`.
fn arrow(tip: Point2, dir: Vec2, style: &StyleFile, role: Role, owner: Owner) -> Item {
    let def = library::arrow(&style.arrows.kind);
    let len = style.arrows.length * def.length.max(0.1);
    let half = style.arrows.length * def.width / 2.0;
    let d = if dir.length() > 1e-9 { dir / dir.length() } else { Vec2::new(1.0, 0.0) };
    let n = Vec2::new(-d.y, d.x);
    let base = add(tip, d * -len);
    let path = polyline(&[tip, add(base, n * half), add(base, n * -half), tip]);
    Item::Path(PathItem { path, width: if def.filled { 0.0 } else { 0.18 }, dash: None, role, owner, fill: def.filled })
}

/// A path as polylines within `tolerance`, in the path's own units: what a painter
/// without curves draws.
pub fn flatten(path: &BezPath, tolerance: f64) -> Vec<Vec<kurbo::Point>> {
    let mut out: Vec<Vec<kurbo::Point>> = Vec::new();
    let mut start = kurbo::Point::ZERO;
    kurbo::flatten(path.elements().iter().copied(), tolerance, |el| match el {
        kurbo::PathEl::MoveTo(p) => {
            start = p;
            out.push(vec![p]);
        }
        kurbo::PathEl::LineTo(p) => match out.last_mut() {
            Some(l) => l.push(p),
            None => out.push(vec![p]),
        },
        kurbo::PathEl::ClosePath => {
            if let Some(l) = out.last_mut() {
                l.push(start);
            }
        }
        _ => {}
    });
    out
}

/// Width of a line of text with cap height `height`.
pub fn text_width(text: &str, height: f64) -> f64 {
    crate::text::text_width(text, height / CAP_HEIGHT)
}

/// A dimension, note, callout, symbol or centre mark as items.
fn annotation_items(sheet: &Sheet, a: &Annotation, ctx: &Context<'_>, list: &mut DisplayList) {
    let items = &mut list.items;
    let style = ctx.style;
    let lines = &ctx.format.lines;
    let owner = Owner::Annotation(a.id);
    let (role, text_role) = match a.state {
        AnnotationState::Dangling { .. } => (Role::Error, Role::Error),
        _ => (Role::Dimension, Role::Text),
    };
    let width = lines.dimension;
    let place = to_sheet(sheet, a.view, a.placement);
    let anchors: Vec<Point2> = a.anchors.iter().map(|an| to_sheet(sheet, a.view, an.cached)).collect();
    let view_scale = sheet.view(a.view).map(|v| effective_scale(sheet, v)).unwrap_or(1.0);
    let th = style.text.height;
    let mut text_at: Option<(Point2, f64, String, Align, VAlign)> = None;
    let push = |items: &mut Vec<Item>, path: BezPath, dash: Option<Vec<f64>>, r: Role| {
        items.push(Item::Path(PathItem { path, width, dash, role: r, owner, fill: false }));
    };
    match &a.body {
        AnnotationKind::Dimension(d) => {
            let text = display_text(d, &style.symbols);
            match d.kind {
                DimensionKind::Linear { orientation } if anchors.len() >= 2 => {
                    let (at, geometry) =
                        linear(items, anchors[0], anchors[1], place, orientation, style, role, owner, width);
                    list.dimensions.push(DimensionRecord {
                        owner: a.id,
                        geometry,
                        text: text.clone(),
                        value: dim_value(d),
                        text_mid: at.0,
                    });
                    text_at = Some((at.0, at.1, text, Align::Centre, at.2));
                }
                DimensionKind::Diameter | DimensionKind::Radius if !anchors.is_empty() => {
                    let c = anchors[0];
                    let value = match d.value {
                        crate::DimValue::Measured { value } => value,
                        _ => 0.0,
                    };
                    let r = if d.kind == DimensionKind::Diameter { value / 2.0 } else { value } * view_scale;
                    let u = sub(place, c);
                    let u = if u.length() > 1e-9 {
                        u / u.length()
                    } else {
                        Vec2::new(std::f64::consts::FRAC_1_SQRT_2, std::f64::consts::FRAC_1_SQRT_2)
                    };
                    let on = add(c, u * r);
                    let right = place.u >= c.u;
                    let shoulder =
                        Vec2::new(if right { style.lines.leader_shoulder } else { -style.lines.leader_shoulder }, 0.0);
                    let elbow = place;
                    let end = add(elbow, shoulder);
                    let geometry = if d.kind == DimensionKind::Radius {
                        // From the centre through the arc to the text.
                        push(items, polyline(&[c, on, elbow, end]), None, role);
                        items.push(arrow(on, u, style, role, owner));
                        DimensionGeometry::Radius { centre: c, on }
                    } else {
                        push(items, polyline(&[on, elbow, end]), None, role);
                        items.push(arrow(on, u * -1.0, style, role, owner));
                        DimensionGeometry::Diameter { near: on, far: add(c, u * -r) }
                    };
                    let halign = if right { Align::Left } else { Align::Right };
                    let pos = add(end, Vec2::new(if right { 1.0 } else { -1.0 }, 0.0));
                    let mid = add(pos, Vec2::new(if right { 1.0 } else { -1.0 } * text_width(&text, th) / 2.0, 0.0));
                    list.dimensions.push(DimensionRecord {
                        owner: a.id,
                        geometry,
                        text: text.clone(),
                        value: dim_value(d),
                        text_mid: mid,
                    });
                    text_at = Some((pos, 0.0, text, halign, VAlign::Centre));
                }
                DimensionKind::Angular if anchors.len() >= 2 => {
                    push(items, line(anchors[0], place), None, role);
                    items.push(arrow(anchors[0], sub(anchors[0], place), style, role, owner));
                    text_at = Some((place, 0.0, text, Align::Left, VAlign::Centre));
                }
                _ => {
                    text_at = Some((place, 0.0, text, Align::Left, VAlign::Centre));
                }
            }
        }
        AnnotationKind::Note { text, leader } => {
            let nh = style.text.note_height;
            if let Some(anchor) = anchors.first() {
                let mut pts = vec![*anchor];
                pts.extend(leader.iter().map(|p| to_sheet(sheet, a.view, *p)));
                let right = place.u >= anchor.u;
                let attach = Point2::new(place.u - if right { 1.0 } else { -1.0 }, place.v - nh / 2.0);
                pts.push(attach);
                push(items, polyline(&pts), None, role);
                let dir = sub(*anchor, pts[1]);
                items.push(arrow(*anchor, dir, style, role, owner));
            }
            for (i, l) in text.lines().enumerate() {
                let heavy = i == 0 && text.lines().count() > 1 && l.chars().all(|c| !c.is_lowercase());
                items.push(Item::Text(TextItem {
                    pos: Point2::new(place.u, place.v - (nh * 1.7) * i as f64),
                    height: nh,
                    angle: 0.0,
                    text: l.to_owned(),
                    halign: Align::Left,
                    valign: VAlign::Top,
                    weight: if heavy { Weight::Medium } else { Weight::Regular },
                    role: text_role,
                    owner,
                }));
            }
        }
        AnnotationKind::HoleCallout { text } => {
            if let Some(c) = anchors.first() {
                let r = circle_radius(sheet, a).unwrap_or(4.0);
                let u = sub(place, *c);
                let u = if u.length() > 1e-9 {
                    u / u.length()
                } else {
                    Vec2::new(std::f64::consts::FRAC_1_SQRT_2, std::f64::consts::FRAC_1_SQRT_2)
                };
                let on = add(*c, u * r);
                let right = place.u >= c.u;
                let shoulder =
                    Vec2::new(if right { style.lines.leader_shoulder } else { -style.lines.leader_shoulder }, 0.0);
                let end = add(place, shoulder);
                push(items, polyline(&[on, place, end]), None, role);
                items.push(arrow(on, u * -1.0, style, role, owner));
                let halign = if right { Align::Left } else { Align::Right };
                let pos = add(end, Vec2::new(if right { 1.0 } else { -1.0 }, 0.0));
                text_at = Some((pos, 0.0, text.clone(), halign, VAlign::Centre));
            } else {
                text_at = Some((place, 0.0, text.clone(), Align::Left, VAlign::Centre));
            }
        }
        AnnotationKind::CentreMark => {
            let c = anchors.first().copied().unwrap_or(place);
            let r = circle_radius(sheet, a).unwrap_or(4.0);
            let cm = &style.centre_mark;
            let s = cm.size;
            let far = r + cm.extension;
            let w = lines.centre;
            let mut cross = BezPath::new();
            cross.move_to((c.u - s, c.v));
            cross.line_to((c.u + s, c.v));
            cross.move_to((c.u, c.v - s));
            cross.line_to((c.u, c.v + s));
            items.push(Item::Path(PathItem {
                path: cross,
                width: w,
                dash: None,
                role: Role::Centre,
                owner,
                fill: false,
            }));
            if far > s + cm.gap + 1.0 {
                let mut arms = BezPath::new();
                for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                    arms.move_to((c.u + dx * (s + cm.gap), c.v + dy * (s + cm.gap)));
                    arms.line_to((c.u + dx * far, c.v + dy * far));
                }
                items.push(Item::Path(PathItem {
                    path: arms,
                    width: w,
                    dash: None,
                    role: Role::Centre,
                    owner,
                    fill: false,
                }));
            }
        }
        AnnotationKind::Centreline => {
            if anchors.len() >= 2 {
                push(items, line(anchors[0], anchors[1]), Some(CENTRE_DASH.to_vec()), Role::Centre);
            }
        }
        AnnotationKind::SurfaceFinish { .. }
        | AnnotationKind::Gdt { .. }
        | AnnotationKind::Datum { .. }
        | AnnotationKind::RevisionSymbol { .. } => {
            symbols::symbol_items(a, place, &anchors, ctx, role, text_role, items);
        }
        AnnotationKind::DimensionSet(set) => {
            symbols::set_items(a, set, place, &anchors, ctx, role, text_role, list);
        }
    }
    let items = &mut list.items;
    if let Some((pos, angle, text, halign, valign)) = text_at {
        if let AnnotationState::Changed { .. } = a.state {
            // A highlight behind the text so a reviewer sees what moved.
            let w = text_width(&text, th) + 2.0;
            let h = th + 1.6;
            let (ox, oy) = match (halign, valign) {
                (Align::Centre, VAlign::Bottom) => (-w / 2.0, -0.8),
                (Align::Centre, VAlign::Centre) => (-w / 2.0, -h / 2.0),
                (Align::Left, _) => (-1.0, -h / 2.0),
                (Align::Right, _) => (-w + 1.0, -h / 2.0),
                (Align::Centre, VAlign::Top) => (-w / 2.0, -h + 0.8),
            };
            let mut box_path = rect(ox, oy, ox + w, oy + h);
            box_path.apply_affine(Affine::translate((pos.u, pos.v)) * Affine::rotate(angle.to_radians()));
            items.push(Item::Path(PathItem {
                path: box_path,
                width: 0.0,
                dash: None,
                role: Role::Changed,
                owner,
                fill: true,
            }));
        }
        items.push(Item::Text(TextItem {
            pos,
            height: th,
            angle,
            text,
            halign,
            valign,
            weight: Weight::Regular,
            role: text_role,
            owner,
        }));
    }
    if let AnnotationState::Dangling { lost, .. } = &a.state {
        for i in lost {
            if let Some(p) = anchors.get(*i) {
                items.push(Item::Text(TextItem {
                    pos: *p,
                    height: th * 1.2,
                    angle: 0.0,
                    text: "×".into(),
                    halign: Align::Centre,
                    valign: VAlign::Centre,
                    weight: Weight::Semibold,
                    role: Role::Error,
                    owner: Owner::AnnotationEnd(a.id, *i),
                }));
            }
        }
    }
}

/// A linear dimension between `p1` and `p2` with its line through `place`: the line,
/// extension lines and arrows go into `items`. Returns the text's position, angle and
/// vertical alignment, and the dimension's definition points.
#[allow(clippy::too_many_arguments)]
pub(crate) fn linear(
    items: &mut Vec<Item>,
    p1: Point2,
    p2: Point2,
    place: Point2,
    orientation: LinearOrientation,
    style: &StyleFile,
    role: Role,
    owner: Owner,
    width: f64,
) -> ((Point2, f64, VAlign), DimensionGeometry) {
    let push = |items: &mut Vec<Item>, path: BezPath| {
        items.push(Item::Path(PathItem { path, width, dash: None, role, owner, fill: false }));
    };
    let (dir, normal, t) = match orientation {
        LinearOrientation::Horizontal => (Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0), place.v - p1.v),
        LinearOrientation::Vertical => (Vec2::new(0.0, 1.0), Vec2::new(-1.0, 0.0), -(place.u - p1.u)),
        LinearOrientation::Aligned => {
            let d = sub(p2, p1);
            let d = if d.length() > 1e-9 { d / d.length() } else { Vec2::new(1.0, 0.0) };
            let n = Vec2::new(-d.y, d.x);
            (d, n, sub(place, p1).dot(n))
        }
    };
    // Ends of the dimension line: each anchor projected onto the line.
    let line_origin = add(p1, normal * t);
    let s1 = 0.0;
    let s2 = sub(p2, p1).dot(dir);
    let (sa, sb) = if s1 <= s2 { (s1, s2) } else { (s2, s1) };
    let ea = add(line_origin, dir * sa);
    let eb = add(line_origin, dir * sb);
    push(items, line(ea, eb));
    // Extension lines from each anchor, with a gap at the outline and an overshoot.
    for p in [p1, p2] {
        let off = sub(p, p1).dot(normal);
        let sgn = if t - off >= 0.0 { 1.0 } else { -1.0 };
        let start = add(p, normal * (style.lines.extension_gap * sgn));
        let end = add(p, normal * (t - off + style.lines.extension_overshoot * sgn));
        if (t - off).abs() > style.lines.extension_gap {
            push(items, line(start, end));
        }
    }
    let span = sb - sa;
    let inside = span >= style.arrows.length * style.lines.inside_min;
    if inside {
        items.push(arrow(ea, dir * -1.0, style, role, owner));
        items.push(arrow(eb, dir, style, role, owner));
    } else {
        items.push(arrow(ea, dir, style, role, owner));
        items.push(arrow(eb, dir * -1.0, style, role, owner));
        push(items, line(add(ea, dir * -(2.0 * style.arrows.length)), ea));
        push(items, line(eb, add(eb, dir * (2.0 * style.arrows.length))));
    }
    let mid = add(line_origin, dir * ((sa + sb) / 2.0));
    let mut angle = dir.y.atan2(dir.x).to_degrees();
    let mut up = normal;
    if angle > 90.0 + 1e-9 || angle <= -90.0 - 1e-9 {
        angle += 180.0;
        up = normal * -1.0;
    }
    if orientation == LinearOrientation::Vertical {
        angle = 90.0;
        up = Vec2::new(-1.0, 0.0);
    }
    let pos = if style.text.position == "inline" { mid } else { add(mid, up * style.text.gap) };
    let valign = if style.text.position == "inline" { VAlign::Centre } else { VAlign::Bottom };
    let geometry = DimensionGeometry::Linear { p1, p2, line: eb, angle: dir.y.atan2(dir.x).to_degrees() };
    ((pos, angle, valign), geometry)
}

fn dim_value(d: &crate::Dimension) -> f64 {
    match d.value {
        crate::DimValue::Measured { value } => value,
        crate::DimValue::Driving { .. } => 0.0,
    }
}

/// The sheet radius of the circle an annotation's first anchor sits on, from the view's
/// own curves.
fn circle_radius(sheet: &Sheet, a: &Annotation) -> Option<f64> {
    let anchor = a.anchors.first()?;
    let view = sheet.view(a.view)?;
    let same = |c: &crate::Curve2| {
        (c.source.kernel_hint.is_some() && c.source.kernel_hint == anchor.entity.kernel_hint)
            || (c.source.feature == anchor.entity.feature && c.source.role == anchor.entity.role)
    };
    let mut best: Option<f64> = None;
    for c in view.geometry.curves.iter().filter(|c| same(c)) {
        if let CurveShape::Circle { centre, radius } = shape_of(&c.path)
            && (centre.u - anchor.cached.u).abs() < 1e-3
            && (centre.v - anchor.cached.v).abs() < 1e-3
            && anchor.attach == Attach::Centre
        {
            best = Some(best.map_or(radius, |b: f64| b.max(radius)));
        }
    }
    best
}

/// A short name for an annotation in lists: "⌀24", "120", "Note", "4× ⌀9 THRU".
pub fn annotation_name(a: &Annotation, style: &StyleFile) -> String {
    match &a.body {
        AnnotationKind::Dimension(d) => display_text(d, &style.symbols),
        AnnotationKind::Note { text, .. } => text.lines().next().unwrap_or("Note").to_owned(),
        AnnotationKind::HoleCallout { text } => text.clone(),
        AnnotationKind::CentreMark => "Centre mark".into(),
        AnnotationKind::Centreline => "Centreline".into(),
        AnnotationKind::SurfaceFinish { text, .. } => format!("Surface finish {text}"),
        AnnotationKind::Gdt { characteristic, tolerance, diameter, .. } => format!(
            "{} {}{}",
            characteristic.name(),
            if *diameter { style.symbols.diameter.as_str() } else { "" },
            symbols::tolerance_text(*tolerance)
        ),
        AnnotationKind::DimensionSet(set) => format!(
            "{} {}",
            match set.kind {
                crate::SetKind::Ordinate => "Ordinate",
                crate::SetKind::Baseline => "Baseline",
                crate::SetKind::Chain => "Chain",
            },
            set.values_text()
        ),
        AnnotationKind::Datum { letter } => format!("Datum {letter}"),
        AnnotationKind::RevisionSymbol { revision } => format!("Rev {revision}"),
    }
}

/// The caption of a view for lists.
pub fn view_name(sheet: &Sheet, view: &View) -> String {
    view_label(sheet, view)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::bracket_a3;

    #[test]
    fn the_fixture_lays_out_with_format_views_and_annotations() {
        let d = bracket_a3();
        let format = library::template("iso-a3");
        let ctx = Context { format, style: format.style_file(), stale: &[ViewId(3)], sheet_index: 0, sheet_count: 1 };
        let list = layout(&d, 0, &ctx);
        assert!(list.items.iter().any(|i| i.role() == Role::Border));
        assert!(list.items.iter().any(|i| matches!(i, Item::Text(t) if t.text == "Bracket")));
        assert!(list.items.iter().any(|i| matches!(i, Item::Text(t) if t.text == "1 / 1")));
        assert!(list.items.iter().any(|i| matches!(i.owner(), Owner::Curve(ViewId(1), _))));
        assert!(list.items.iter().any(|i| i.role() == Role::Hidden));
        assert!(list.items.iter().any(|i| i.role() == Role::Stale));
        assert!(list.items.iter().any(|i| matches!(i, Item::Text(t) if t.text == "Ø24")));
        assert!(list.items.iter().any(|i| matches!(i, Item::Text(t) if t.text == "SCALE 1:2")));
        assert!(list.items.iter().any(|i| matches!(i.owner(), Owner::AnnotationEnd(..))));
        assert!(list.items.iter().any(|i| i.role() == Role::Changed));
        assert!(list.items.iter().any(|i| i.role() == Role::Centre));
    }

    #[test]
    fn links_resolve() {
        let lookup = |k: &str| match k {
            "sheet" => "1".to_owned(),
            "sheets" => "2".to_owned(),
            _ => String::new(),
        };
        assert_eq!(resolve_links("${sheet} / ${sheets}", &lookup), "1 / 2");
        assert_eq!(scale_text(0.5), "1:2");
        assert_eq!(scale_text(2.0), "2:1");
    }
}
