//! Drawing symbols: geometric tolerance frames, datum feature symbols, surface finish
//! symbols, revision symbols, and ordinate, baseline and chain dimensions.
//!
//! The characteristic signs of ISO 1101 and the modifiers are drawn as line work, not
//! taken from a font, so every writer shows the same shapes.

use kurbo::{BezPath, Shape, Vec2};
use soliddraft_kernel_api::Point2;

use super::{
    Context, DimensionGeometry, DimensionRecord, DisplayList, Item, Owner, PathItem, Role, TextItem, VAlign, add,
    arrow, line, linear, polyline, rect, sub, text_width,
};
use crate::annotation::{
    Annotation, AnnotationKind, DimensionSet, GdtCharacteristic, LinearOrientation, MaterialCondition, SetKind,
    SurfaceProcess,
};
use crate::library::{Align, Weight};
use crate::measure::format_value;

/// A tolerance value as written in a frame: no trailing zeros, at most four decimals.
pub fn tolerance_text(value: f64) -> String {
    let s = format!("{value:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".into() } else { s.to_owned() }
}

struct Pen<'a> {
    items: &'a mut Vec<Item>,
    width: f64,
    role: Role,
    text_role: Role,
    owner: Owner,
    height: f64,
}

impl Pen<'_> {
    fn path(&mut self, path: BezPath) {
        self.items.push(Item::Path(PathItem {
            path,
            width: self.width,
            dash: None,
            role: self.role,
            owner: self.owner,
            fill: false,
            accuracy: None,
        }));
    }

    fn fill(&mut self, path: BezPath) {
        self.items.push(Item::Path(PathItem {
            path,
            width: 0.0,
            dash: None,
            role: self.role,
            owner: self.owner,
            fill: true,
            accuracy: None,
        }));
    }

    fn circle(&mut self, c: Point2, r: f64) {
        self.path(kurbo::Circle::new((c.u, c.v), r).to_path(0.01));
    }

    fn text(&mut self, pos: Point2, text: &str, halign: Align, valign: VAlign, angle: f64) {
        self.items.push(Item::Text(TextItem {
            pos,
            height: self.height,
            angle,
            text: text.to_owned(),
            halign,
            valign,
            weight: Weight::Regular,
            role: self.text_role,
            owner: self.owner,
        }));
    }

    /// A leader from `from` to `to` with an arrowhead at `to`.
    fn leader(&mut self, from: Point2, to: Point2, ctx: &Context<'_>) {
        if sub(to, from).length() < 1e-6 {
            return;
        }
        self.path(line(from, to));
        self.items.push(arrow(to, sub(to, from), ctx.style, self.role, self.owner));
    }
}

/// A GD&T frame, datum, surface finish or revision symbol as items.
pub(super) fn symbol_items(
    a: &Annotation,
    place: Point2,
    anchors: &[Point2],
    ctx: &Context<'_>,
    role: Role,
    text_role: Role,
    items: &mut Vec<Item>,
) {
    let th = ctx.style.text.height;
    let mut pen =
        Pen { items, width: ctx.format.lines.dimension, role, text_role, owner: Owner::Annotation(a.id), height: th };
    match &a.body {
        AnnotationKind::Gdt { characteristic, tolerance, diameter, material, datums } => {
            let h = 2.0 * th;
            let pad = th * 0.45;
            let value = format!(
                "{}{}",
                if *diameter { ctx.style.symbols.diameter.as_str() } else { "" },
                tolerance_text(*tolerance)
            );
            let modifier = |m: MaterialCondition| if m.is_regardless() { 0.0 } else { th * 1.3 };
            let mut cells = vec![h, text_width(&value, th) + 2.0 * pad + modifier(*material)];
            for d in datums {
                cells.push((text_width(&d.letter, th) + 2.0 * pad + modifier(d.material)).max(h));
            }
            let (x0, y0) = (place.u, place.v - h / 2.0);
            let total: f64 = cells.iter().sum();
            pen.path(rect(x0, y0, x0 + total, y0 + h));
            let mut x = x0;
            for (i, w) in cells.iter().enumerate() {
                if i > 0 {
                    pen.path(line(Point2::new(x, y0), Point2::new(x, y0 + h)));
                }
                x += w;
            }
            characteristic_sign(&mut pen, *characteristic, Point2::new(x0 + h / 2.0, place.v), h * 0.62);
            let mut x = x0 + cells[0];
            let mut write = |pen: &mut Pen, text: &str, m: MaterialCondition, w: f64| {
                let tw = text_width(text, th) + modifier(m);
                let start = x + (w - tw) / 2.0;
                pen.text(Point2::new(start, place.v), text, Align::Left, VAlign::Centre, 0.0);
                if !m.is_regardless() {
                    let c = Point2::new(start + text_width(text, th) + th * 0.75, place.v);
                    pen.circle(c, th * 0.55);
                    pen.height = th * 0.7;
                    pen.text(c, m.letter(), Align::Centre, VAlign::Centre, 0.0);
                    pen.height = th;
                }
                x += w;
            };
            write(&mut pen, &value, *material, cells[1]);
            for (d, w) in datums.iter().zip(&cells[2..]) {
                write(&mut pen, &d.letter, d.material, *w);
            }
            if let Some(to) = anchors.first() {
                let from = if to.u < x0 { Point2::new(x0, place.v) } else { Point2::new(x0 + total, place.v) };
                let from = if to.u >= x0 && to.u <= x0 + total {
                    Point2::new(to.u.clamp(x0 + h / 2.0, x0 + total - h / 2.0), if to.v < y0 { y0 } else { y0 + h })
                } else {
                    from
                };
                pen.leader(from, *to, ctx);
            }
        }
        AnnotationKind::Datum { letter } => {
            let h = 2.0 * th;
            let w = (text_width(letter, th) + th).max(h);
            let (x0, y0) = (place.u - w / 2.0, place.v - h / 2.0);
            pen.path(rect(x0, y0, x0 + w, y0 + h));
            pen.text(place, letter, Align::Centre, VAlign::Centre, 0.0);
            // A filled triangle on the feature, joined to the frame.
            let foot = anchors.first().copied().unwrap_or(Point2::new(place.u, y0 - 2.5 * h));
            let up = sub(place, foot);
            let up = if up.length() > 1e-9 { up / up.length() } else { Vec2::new(0.0, 1.0) };
            let across = Vec2::new(-up.y, up.x);
            let side = h * 0.55;
            let apex = add(foot, up * (side * 0.866));
            let mut tri = BezPath::new();
            tri.move_to((foot.u + across.x * side / 2.0, foot.v + across.y * side / 2.0));
            tri.line_to((foot.u - across.x * side / 2.0, foot.v - across.y * side / 2.0));
            tri.line_to((apex.u, apex.v));
            tri.close_path();
            pen.fill(tri);
            let edge = frame_edge(place, w, h, foot);
            pen.path(line(apex, edge));
        }
        AnnotationKind::SurfaceFinish { text, process, method, lay, allowance, all_around } => {
            let root = place;
            if let Some(to) = anchors.first() {
                pen.leader(root, *to, ctx);
            }
            let (short, long) = (1.4 * th, 3.0 * th);
            let (c60, s60) = (0.5, 0.866);
            let left = Point2::new(root.u - short * c60, root.v + short * s60);
            let top = Point2::new(root.u + long * c60, root.v + long * s60);
            pen.path(polyline(&[left, root, top]));
            match process {
                SurfaceProcess::Any => {}
                SurfaceProcess::Removal => {
                    let on_long = Point2::new(root.u + short * c60, left.v);
                    pen.path(line(left, on_long));
                }
                SurfaceProcess::NoRemoval => {
                    // The circle inscribed in the symbol's V, touching both legs.
                    let r = short * 0.32;
                    pen.circle(Point2::new(root.u, root.v + r / c60), r);
                }
            }
            let reach = text_width(text, th).max(text_width(method, th)) + th;
            let end = Point2::new(top.u + reach, top.v);
            pen.path(line(top, end));
            if *all_around {
                pen.circle(top, th * 0.35);
            }
            pen.text(Point2::new(top.u + th * 0.4, top.v - th * 0.3), text, Align::Left, VAlign::Top, 0.0);
            if !method.is_empty() {
                pen.text(Point2::new(top.u + th * 0.4, top.v + th * 0.3), method, Align::Left, VAlign::Bottom, 0.0);
            }
            if let Some(lay) = lay {
                pen.text(
                    Point2::new(root.u + th * 0.9, root.v + th * 0.2),
                    lay.symbol(),
                    Align::Left,
                    VAlign::Bottom,
                    0.0,
                );
            }
            if !allowance.is_empty() {
                pen.text(Point2::new(left.u - th * 0.3, left.v), allowance, Align::Right, VAlign::Centre, 0.0);
            }
        }
        AnnotationKind::RevisionSymbol { revision } => {
            let side = (2.6 * th).max(text_width(revision, th) * 1.9);
            let h = side * 0.866;
            let base = place.v - h / 3.0;
            let corners = [
                Point2::new(place.u - side / 2.0, base),
                Point2::new(place.u + side / 2.0, base),
                Point2::new(place.u, base + h),
                Point2::new(place.u - side / 2.0, base),
            ];
            pen.path(polyline(&corners));
            pen.text(Point2::new(place.u, base + h * 0.36), revision, Align::Centre, VAlign::Centre, 0.0);
            if let Some(to) = anchors.first() {
                let from = Point2::new(place.u, base);
                pen.leader(from, *to, ctx);
            }
        }
        _ => {}
    }
}

/// The point on a frame of size `w` by `h` centred on `c` nearest `towards`, on the
/// middle of the side facing it.
fn frame_edge(c: Point2, w: f64, h: f64, towards: Point2) -> Point2 {
    let d = sub(towards, c);
    if d.y.abs() * w >= d.x.abs() * h {
        Point2::new(c.u, c.v + h / 2.0 * d.y.signum())
    } else {
        Point2::new(c.u + w / 2.0 * d.x.signum(), c.v)
    }
}

/// A characteristic sign of ISO 1101 in a square of side `s` centred on `c`.
fn characteristic_sign(pen: &mut Pen, ch: GdtCharacteristic, c: Point2, s: f64) {
    let p = |x: f64, y: f64| Point2::new(c.u + x * s / 2.0, c.v + y * s / 2.0);
    let arc = |from: f64, sweep: f64, r: f64, at: Point2| {
        let a = kurbo::Arc::new((at.u, at.v), (r, r), from.to_radians(), sweep.to_radians(), 0.0);
        let mut path = BezPath::new();
        let start = a.center + Vec2::from_angle(a.start_angle) * r;
        path.move_to(start);
        for el in a.append_iter(0.01) {
            path.push(el);
        }
        path
    };
    match ch {
        GdtCharacteristic::Straightness => pen.path(line(p(-1.0, 0.0), p(1.0, 0.0))),
        GdtCharacteristic::Flatness => {
            pen.path(polyline(&[p(-1.0, -0.45), p(0.55, -0.45), p(1.0, 0.45), p(-0.55, 0.45), p(-1.0, -0.45)]))
        }
        GdtCharacteristic::Circularity => pen.circle(c, s * 0.42),
        GdtCharacteristic::Cylindricity => {
            pen.circle(c, s * 0.32);
            pen.path(line(p(-0.95, -0.85), p(-0.15, 0.85)));
            pen.path(line(p(0.15, -0.85), p(0.95, 0.85)));
        }
        GdtCharacteristic::ProfileOfLine => pen.path(arc(0.0, 180.0, s * 0.45, p(0.0, -0.4))),
        GdtCharacteristic::ProfileOfSurface => {
            let mut path = arc(0.0, 180.0, s * 0.45, p(0.0, -0.4));
            path.close_path();
            pen.path(path);
        }
        GdtCharacteristic::Angularity => pen.path(polyline(&[p(1.0, -0.6), p(-1.0, -0.6), p(0.6, 0.6)])),
        GdtCharacteristic::Perpendicularity => {
            pen.path(line(p(-0.9, -0.7), p(0.9, -0.7)));
            pen.path(line(p(0.0, -0.7), p(0.0, 0.9)));
        }
        GdtCharacteristic::Parallelism => {
            pen.path(line(p(-0.8, -0.8), p(-0.05, 0.8)));
            pen.path(line(p(0.05, -0.8), p(0.8, 0.8)));
        }
        GdtCharacteristic::Position => {
            pen.circle(c, s * 0.3);
            pen.path(line(p(-0.95, 0.0), p(0.95, 0.0)));
            pen.path(line(p(0.0, -0.95), p(0.0, 0.95)));
        }
        GdtCharacteristic::Concentricity => {
            pen.circle(c, s * 0.2);
            pen.circle(c, s * 0.42);
        }
        GdtCharacteristic::Symmetry => {
            pen.path(line(p(-0.95, 0.0), p(0.95, 0.0)));
            pen.path(line(p(-0.55, 0.45), p(0.55, 0.45)));
            pen.path(line(p(-0.55, -0.45), p(0.55, -0.45)));
        }
        GdtCharacteristic::CircularRunout | GdtCharacteristic::TotalRunout => {
            let (a, b) = (p(-0.4, -0.9), p(0.4, 0.75));
            pen.path(line(a, b));
            runout_head(pen, a, b, s);
            if ch == GdtCharacteristic::TotalRunout {
                let (a2, b2) = (p(0.35, -0.9), p(1.0, 0.45));
                pen.path(line(a2, b2));
                runout_head(pen, a2, b2, s);
                pen.path(line(p(-0.4, -0.9), p(0.35, -0.9)));
            }
        }
    }
}

fn runout_head(pen: &mut Pen, a: Point2, b: Point2, s: f64) {
    let d = sub(b, a);
    let d = d / d.length();
    let n = Vec2::new(-d.y, d.x);
    let len = s * 0.3;
    let base = add(b, d * -len);
    let mut tri = BezPath::new();
    tri.move_to((b.u, b.v));
    tri.line_to((base.u + n.x * len * 0.35, base.v + n.y * len * 0.35));
    tri.line_to((base.u - n.x * len * 0.35, base.v - n.y * len * 0.35));
    tri.close_path();
    pen.fill(tri);
}

/// Ordinate, baseline or chain dimensions as items, with one dimension record per
/// value.
#[allow(clippy::too_many_arguments)]
pub(super) fn set_items(
    a: &Annotation,
    set: &DimensionSet,
    place: Point2,
    anchors: &[Point2],
    ctx: &Context<'_>,
    role: Role,
    text_role: Role,
    list: &mut DisplayList,
) {
    let style = ctx.style;
    let th = style.text.height;
    let owner = Owner::Annotation(a.id);
    let width = ctx.format.lines.dimension;
    if anchors.len() < 2 {
        return;
    }
    let horizontal = set.orientation != LinearOrientation::Vertical;
    let text = |v: f64| format_value(v, set.precision);
    let push_text = |items: &mut Vec<Item>, pos: Point2, text: String, angle: f64, halign: Align, valign: VAlign| {
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
    };
    match set.kind {
        SetKind::Ordinate => {
            // Leaders run across the measuring direction from each point to the level of
            // the placement, where the values are written along them.
            for (i, p) in anchors.iter().enumerate() {
                let value = if i == 0 { 0.0 } else { set.values.get(i - 1).copied().unwrap_or(0.0) };
                let (end, sgn) = if horizontal {
                    let s = if place.v >= p.v { 1.0 } else { -1.0 };
                    (Point2::new(p.u, place.v), s)
                } else {
                    let s = if place.u >= p.u { 1.0 } else { -1.0 };
                    (Point2::new(place.u, p.v), s)
                };
                let gap = style.lines.extension_gap;
                let start =
                    if horizontal { Point2::new(p.u, p.v + gap * sgn) } else { Point2::new(p.u + gap * sgn, p.v) };
                list.items.push(Item::Path(PathItem {
                    path: line(start, end),
                    width,
                    dash: None,
                    role,
                    owner,
                    fill: false,
                    accuracy: None,
                }));
                let (pos, angle) = if horizontal {
                    (Point2::new(end.u, end.v + style.text.gap * sgn), 90.0)
                } else {
                    (Point2::new(end.u + style.text.gap * sgn, end.v), 0.0)
                };
                let halign = if sgn > 0.0 { Align::Left } else { Align::Right };
                push_text(&mut list.items, pos, text(value), angle, halign, VAlign::Centre);
                if i > 0 {
                    let w = text_width(&text(value), th);
                    let mid = if horizontal {
                        Point2::new(pos.u, pos.v + sgn * w / 2.0)
                    } else {
                        Point2::new(pos.u + sgn * w / 2.0, pos.v)
                    };
                    list.dimensions.push(DimensionRecord {
                        owner: a.id,
                        geometry: DimensionGeometry::Ordinate {
                            origin: anchors[0],
                            feature: *p,
                            leader_end: end,
                            along_x: horizontal,
                        },
                        text: text(value),
                        value,
                        text_mid: mid,
                    });
                }
            }
        }
        SetKind::Baseline | SetKind::Chain => {
            let orientation = if horizontal { LinearOrientation::Horizontal } else { LinearOrientation::Vertical };
            // Baseline dimensions stack away from the part; a chain stays on one line.
            let away = if horizontal {
                Vec2::new(0.0, if place.v >= anchors[0].v { 1.0 } else { -1.0 })
            } else {
                Vec2::new(if place.u >= anchors[0].u { 1.0 } else { -1.0 }, 0.0)
            };
            for k in 1..anchors.len() {
                let (p1, p2, level) = match set.kind {
                    SetKind::Baseline => {
                        (anchors[0], anchors[k], add(place, away * (style.lines.spacing * (k - 1) as f64)))
                    }
                    _ => (anchors[k - 1], anchors[k], place),
                };
                let value = set.values.get(k - 1).copied().unwrap_or(0.0);
                let (at, geometry) = linear(&mut list.items, p1, p2, level, orientation, style, role, owner, width);
                push_text(&mut list.items, at.0, text(value), at.1, Align::Centre, at.2);
                list.dimensions.push(DimensionRecord {
                    owner: a.id,
                    geometry,
                    text: text(value),
                    value,
                    text_mid: at.0,
                });
            }
        }
    }
}
