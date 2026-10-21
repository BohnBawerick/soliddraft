//! Tables on a sheet. The revision table lists the drawing's revisions, oldest first,
//! with the zones their revision symbols are in.

use soliddraft_kernel_api::Point2;

use super::{Context, Item, Owner, PathItem, Role, TextItem, VAlign, line, rect, to_sheet, zone_at};
use crate::Drawing;
use crate::annotation::AnnotationKind;
use crate::library::{self, Align, Weight};
use crate::sheet::{Sheet, Table, TableKind};

/// The revision table's columns: heading and width in millimetres.
pub const REVISION_COLUMNS: [(&str, f64); 5] =
    [("ZONE", 16.0), ("REV", 12.0), ("DESCRIPTION", 70.0), ("DATE", 24.0), ("APPROVED", 24.0)];

/// Height of each row of a table, in millimetres.
const ROW: f64 = 7.0;

/// Width and height of a revision table with `rows` revisions: a title row, a heading
/// row and one row per revision.
pub fn revision_table_size(rows: usize) -> (f64, f64) {
    (REVISION_COLUMNS.iter().map(|(_, w)| w).sum(), ROW * (rows as f64 + 2.0))
}

/// Where a sheet's revision table goes when it is made: its top-left corner in the
/// top-right corner of the drawing area, or, when views are there, just above the
/// title block, or in the top-left corner; the first that covers no view.
pub fn revision_table_home(sheet: &Sheet) -> Point2 {
    let f = library::template(sheet.template.as_str());
    let [l, _, r, t] = f.frame();
    let band = f.zones.band;
    let (w, h) = revision_table_size(0);
    let o = f.title_block_origin();
    let candidates = [
        Point2::new(r - band - w, t - band),
        Point2::new(r - band - w, o.v + f.title_block.height + GAP + h),
        Point2::new(l + band, t - band),
    ];
    let clear = |p: &Point2| {
        let rect = [p.u, p.v - h, p.u + w, p.v];
        sheet.views.iter().all(|v| {
            let [a, b, c, d] = crate::project::view_bounds(v);
            rect[2] <= a - GAP || c + GAP <= rect[0] || rect[3] <= b - GAP || d + GAP <= rect[1]
        })
    };
    candidates.iter().copied().find(clear).unwrap_or(candidates[0])
}

/// Room kept between a table and the views or title block around it.
const GAP: f64 = 3.0;

/// Moves a revision table up when its rows would reach into the title block below it.
pub(crate) fn keep_above_title_block(sheet: &mut Sheet) {
    let f = library::template(sheet.template.as_str());
    let o = f.title_block_origin();
    let top = o.v + f.title_block.height + GAP;
    for table in sheet.tables.iter_mut().filter(|t| t.kind == TableKind::Revision) {
        let (w, h) = revision_table_size(table.revisions.len());
        let over = table.position.u < o.u + f.title_block.width && table.position.u + w > o.u;
        if over && table.position.v - h < top && table.position.v > o.v {
            table.position.v = top + h;
        }
    }
}

/// `[left, bottom, right, top]` of every table on a sheet, in sheet millimetres.
pub fn table_bounds(sheet: &Sheet) -> Vec<[f64; 4]> {
    sheet
        .tables
        .iter()
        .map(|t| {
            let (w, h) = revision_table_size(t.revisions.len());
            [t.position.u, t.position.v - h, t.position.u + w, t.position.v]
        })
        .collect()
}

/// The zones of the revision symbols that carry `revision`, joined.
fn symbol_zones(sheet: &Sheet, revision: &str, ctx: &Context<'_>) -> String {
    let mut zones: Vec<String> = sheet
        .annotations
        .iter()
        .filter(|a| matches!(&a.body, AnnotationKind::RevisionSymbol { revision: r } if r == revision))
        .map(|a| zone_at(ctx.format, to_sheet(sheet, a.view, a.placement)))
        .collect();
    zones.sort();
    zones.dedup();
    zones.join(", ")
}

/// Every table of a sheet as items.
pub(super) fn table_items(_drawing: &Drawing, sheet: &Sheet, ctx: &Context<'_>, items: &mut Vec<Item>) {
    for (i, table) in sheet.tables.iter().enumerate() {
        if table.kind == TableKind::Revision {
            revision_items(sheet, table, Owner::Table(i), ctx, items);
        }
    }
}

fn revision_items(sheet: &Sheet, table: &Table, owner: Owner, ctx: &Context<'_>, items: &mut Vec<Item>) {
    let lines = &ctx.format.lines;
    let (w, h) = revision_table_size(table.revisions.len());
    let (x0, top) = (table.position.u, table.position.v);
    let mut path = |p: kurbo::BezPath, width: f64, role: Role| {
        items.push(Item::Path(PathItem { path: p, width, dash: None, role, owner, fill: false }));
    };
    path(rect(x0, top - h, x0 + w, top), lines.border, Role::Border);
    for r in 1..(table.revisions.len() + 2) {
        let y = top - ROW * r as f64;
        path(line(Point2::new(x0, y), Point2::new(x0 + w, y)), lines.thin, Role::Thin);
    }
    let mut x = x0;
    for (i, (_, cw)) in REVISION_COLUMNS.iter().enumerate() {
        if i > 0 {
            path(line(Point2::new(x, top - ROW), Point2::new(x, top - h)), lines.thin, Role::Thin);
        }
        x += cw;
    }
    let mut text = |pos: Point2, t: String, height: f64, align: Align, weight: Weight, role: Role| {
        items.push(Item::Text(TextItem {
            pos,
            height,
            angle: 0.0,
            text: t,
            halign: align,
            valign: VAlign::Centre,
            weight,
            role,
            owner,
        }));
    };
    text(
        Point2::new(x0 + w / 2.0, top - ROW / 2.0),
        "REVISIONS".into(),
        2.5,
        Align::Centre,
        Weight::Medium,
        Role::Text,
    );
    let mut x = x0;
    for (heading, cw) in REVISION_COLUMNS {
        text(
            Point2::new(x + cw / 2.0, top - 1.5 * ROW),
            heading.into(),
            1.8,
            Align::Centre,
            Weight::Regular,
            Role::Label,
        );
        x += cw;
    }
    for (r, row) in table.revisions.iter().enumerate() {
        let y = top - ROW * (r as f64 + 2.5);
        let zone = if row.zone.is_empty() { symbol_zones(sheet, &row.revision, ctx) } else { row.zone.clone() };
        let cells = [zone, row.revision.clone(), row.description.clone(), row.date.clone(), row.approved.clone()];
        let mut x = x0;
        for (i, (cell, (_, cw))) in cells.into_iter().zip(REVISION_COLUMNS).enumerate() {
            let (pos, align) = if i == 2 {
                (Point2::new(x + 1.5, y), Align::Left)
            } else {
                (Point2::new(x + cw / 2.0, y), Align::Centre)
            };
            if !cell.is_empty() {
                text(pos, cell, 2.5, align, Weight::Regular, Role::Text);
            }
            x += cw;
        }
    }
}
