//! A complete sample drawing for building and testing UI against before projection
//! exists: the bracket on ISO A3, first angle, at 1:1.
//!
//! The line work is placeholder: hand-listed model edges projected with the kernel's
//! [`ViewFrame`], one function per view (`front_lines`, `top_lines`, `left_lines`,
//! `iso_lines`). Real hidden-line output replaces those functions in phase D1; the rest of
//! the fixture keeps working because annotations are placed through the same view
//! transform.
//!
//! The bracket, in model millimetres with Z up: a 120 x 90 x 10 plate; a 10 x 90 x 60
//! flange along X = 0; a boss of radius 12 at (70, 45) from Z 10 to 30 with a bore of
//! radius 6 through boss and plate; four holes of radius 4.5 through the plate at
//! (25, 23), (25, 67), (115, 23) and (115, 67); a slot through the flange from Y 25 to 65
//! and Z 25 to 37.

use kurbo::{BezPath, PathEl, Rect, Shape};
use soliddraft_kernel_api::drawing::{StandardView, ViewFrame, ViewSpec};
use soliddraft_kernel_api::{Point2, Vec3};

use crate::{
    AlignAxis, Alignment, Anchor, Annotation, AnnotationId, AnnotationKind, AnnotationState, Attach, ContentHash,
    Curve2, CurveKind, Dimension, DimensionKind, DocKind, DocRef, Drawing, DrawingMeta, EntityKind, EntityRef,
    FoldDirection, HiddenLines, LinearOrientation, Orientation, Scale, StyleOverrides, Template, View, ViewDisplay,
    ViewGeometry, ViewId, ViewKind, ViewScale,
};

/// The part revision the fixture's views were "made" from.
pub const REVISION: &str = "bracket-rev7";
/// The same revision as the views record it.
pub const DIGEST: &str = "fixture:bracket-rev7";

const FRONT: ViewId = ViewId(1);
const TOP: ViewId = ViewId(2);
const LEFT: ViewId = ViewId(3);
const ISO: ViewId = ViewId(4);

/// The bracket drawing: Front base view, Top and Left projected and aligned to it, an
/// Isometric at 1:2, five reference dimensions, a hole callout, a note and a centre mark.
/// The plate thickness dimension is `Changed` and the slot dimension is `Dangling`, so a
/// Needs attention list has content.
pub fn bracket_a3() -> Drawing {
    let template = Template::by_id("iso-a3").expect("built-in template");
    let model = DocRef::unbound(DocKind::Part, "bracket.json", ContentHash::foreign("fixture", REVISION));
    let mut drawing = Drawing::new(model, &template);
    drawing.meta = DrawingMeta {
        title: "Bracket".to_owned(),
        number: "SD-0001".to_owned(),
        revision: "A".to_owned(),
        designer: "P. Pérez".to_owned(),
        date: "2026-10-14".to_owned(),
        units: Default::default(),
    };

    let (front, front_xf) = view_geometry(StandardView::Front, 1.0, front_lines);
    let (top, top_xf) = view_geometry(StandardView::Top, 1.0, top_lines);
    let (left, left_xf) = view_geometry(StandardView::Left, 1.0, left_lines);
    let (iso, _) = view_geometry(StandardView::Isometric, 0.5, iso_lines);

    let ortho = ViewDisplay { hidden: HiddenLines::Visible, ..ViewDisplay::ORTHOGRAPHIC };
    let sheet = &mut drawing.sheets[0];
    sheet.scale = Scale::FULL;
    sheet.format_overrides.insert("owner".to_owned(), "SolidDraft workshop".to_owned());
    sheet.format_overrides.insert("status".to_owned(), "In work".to_owned());
    sheet.views = vec![
        View {
            id: FRONT,
            kind: ViewKind::Base { orientation: Orientation::Standard { view: StandardView::Front } },
            position: Point2::new(100.0, 190.0),
            alignment: None,
            scale: ViewScale::Sheet,
            display: ortho,
            excluded_from_update: false,
            geometry: front,
            label: None,
            crop: None,
            broken_out: Vec::new(),
        },
        View {
            id: TOP,
            kind: ViewKind::Projected { parent: FRONT, direction: FoldDirection::Down },
            position: Point2::new(100.0, 93.0),
            alignment: Some(Alignment { parent: FRONT, axis: AlignAxis::Vertical }),
            scale: ViewScale::Parent,
            display: ortho,
            excluded_from_update: false,
            geometry: top,
            label: None,
            crop: None,
            broken_out: Vec::new(),
        },
        View {
            id: LEFT,
            kind: ViewKind::Projected { parent: FRONT, direction: FoldDirection::Right },
            position: Point2::new(250.0, 190.0),
            alignment: Some(Alignment { parent: FRONT, axis: AlignAxis::Horizontal }),
            scale: ViewScale::Parent,
            display: ortho,
            excluded_from_update: false,
            geometry: left,
            label: None,
            crop: None,
            broken_out: Vec::new(),
        },
        View {
            id: ISO,
            kind: ViewKind::Base { orientation: Orientation::Standard { view: StandardView::Isometric } },
            position: Point2::new(352.0, 215.0),
            alignment: None,
            scale: ViewScale::Custom { factor: 0.5 },
            display: ViewDisplay::PICTORIAL,
            excluded_from_update: false,
            geometry: iso,
            label: None,
            crop: None,
            broken_out: Vec::new(),
        },
    ];

    let mut annotations = Vec::new();
    let mut add = |view: ViewId, body: AnnotationKind, anchors: Vec<Anchor>, placement: Point2, state| {
        let id = AnnotationId(annotations.len() as u32 + 1);
        annotations.push(Annotation { id, view, body, anchors, placement, style: StyleOverrides::default(), state });
    };
    let linear = |orientation, value| {
        AnnotationKind::Dimension(Dimension::measured(DimensionKind::Linear { orientation }, value, 0))
    };

    // Front: overall length, overall height, plate thickness (changed from 8).
    let fx = &front_xf;
    add(
        FRONT,
        linear(LinearOrientation::Horizontal, 120.0),
        vec![
            fx.anchor(
                line_ref(EntityKind::Edge, "flange", "outer", 0, v(0., 0., 0.), v(0., 0., 60.)),
                Attach::End,
                v(0., 0., 60.),
            ),
            fx.anchor(
                line_ref(EntityKind::Edge, "plate", "end", 0, v(120., 0., 0.), v(120., 0., 10.)),
                Attach::End,
                v(120., 0., 10.),
            ),
        ],
        Point2::new(0.0, 42.0),
        AnnotationState::Ok,
    );
    add(
        FRONT,
        linear(LinearOrientation::Vertical, 60.0),
        vec![
            fx.anchor(
                line_ref(EntityKind::Edge, "plate", "bottom", 0, v(0., 0., 0.), v(120., 0., 0.)),
                Attach::Start,
                v(0., 0., 0.),
            ),
            fx.anchor(
                line_ref(EntityKind::Edge, "flange", "top", 0, v(10., 0., 60.), v(0., 0., 60.)),
                Attach::End,
                v(0., 0., 60.),
            ),
        ],
        Point2::new(-72.0, 0.0),
        AnnotationState::Ok,
    );
    add(
        FRONT,
        linear(LinearOrientation::Vertical, 10.0),
        vec![
            fx.anchor(
                line_ref(EntityKind::Edge, "plate", "bottom", 0, v(0., 0., 0.), v(120., 0., 0.)),
                Attach::End,
                v(120., 0., 0.),
            ),
            fx.anchor(
                line_ref(EntityKind::Edge, "plate", "end", 0, v(120., 0., 0.), v(120., 0., 10.)),
                Attach::End,
                v(120., 0., 10.),
            ),
        ],
        Point2::new(72.0, -25.0),
        AnnotationState::Changed { previous: "8".to_owned() },
    );
    // Front: a note with a leader to the top of the boss.
    let boss_top = circle_ref("boss", "cap_end", 0, v(70., 45., 30.), 12.0);
    add(
        FRONT,
        AnnotationKind::Note { text: "FACE BOSS FLAT".to_owned(), leader: vec![Point2::new(30.0, 22.0)] },
        vec![fx.anchor(boss_top.clone(), Attach::Max, v(82., 45., 30.))],
        Point2::new(34.0, 22.0),
        AnnotationState::Ok,
    );

    // Top: boss diameter, its centre mark, and the hole callout.
    let tx = &top_xf;
    add(
        TOP,
        AnnotationKind::Dimension(Dimension::measured(DimensionKind::Diameter, 24.0, 0)),
        vec![tx.anchor(boss_top.clone(), Attach::Centre, v(70., 45., 30.))],
        tx.point(v(97., 67., 30.)),
        AnnotationState::Ok,
    );
    add(
        TOP,
        AnnotationKind::CentreMark,
        vec![tx.anchor(boss_top, Attach::Centre, v(70., 45., 30.))],
        tx.point(v(70., 45., 30.)),
        AnnotationState::Ok,
    );
    add(
        TOP,
        AnnotationKind::HoleCallout { text: "4X ⌀9 THRU".to_owned() },
        vec![tx.anchor(circle_ref("holes", "cap_end", 0, v(25., 23., 10.), 4.5), Attach::Centre, v(25., 23., 10.))],
        Point2::new(-45.0, -55.0),
        AnnotationState::Ok,
    );

    // Left: the slot height, whose slot was deleted from the part.
    let lx = &left_xf;
    add(
        LEFT,
        linear(LinearOrientation::Vertical, 12.0),
        vec![
            lx.anchor(
                line_ref(EntityKind::Edge, "slot", "floor", 0, v(0., 25., 25.), v(0., 65., 25.)),
                Attach::Mid,
                v(0., 45., 25.),
            ),
            lx.anchor(
                line_ref(EntityKind::Edge, "slot", "roof", 0, v(0., 25., 37.), v(0., 65., 37.)),
                Attach::Mid,
                v(0., 45., 37.),
            ),
        ],
        lx.point(v(0., -12., 31.)),
        AnnotationState::Dangling {
            lost: vec![0, 1],
            cause: "Both references were edges of Slot (Cut-Extrude1), which no longer exists".to_owned(),
        },
    );
    sheet.annotations = annotations;
    drawing
}

/// Front: looking along +Y. Placeholder line work.
fn front_lines(l: &mut Lines) {
    let outline = [(0., 0.), (120., 0.), (120., 10.), (10., 10.), (10., 60.), (0., 60.)];
    let names = [
        ("plate", "bottom"),
        ("plate", "end"),
        ("plate", "top"),
        ("flange", "inner"),
        ("flange", "top"),
        ("flange", "outer"),
    ];
    for (i, (feature, role)) in names.into_iter().enumerate() {
        let (a, b) = (outline[i], outline[(i + 1) % outline.len()]);
        l.edge(feature, role, 0, v(a.0, 0., a.1), v(b.0, 0., b.1), true);
    }
    l.silhouette("boss", "wall", 0, v(58., 45., 10.), v(58., 45., 30.), true);
    l.silhouette("boss", "wall", 0, v(82., 45., 10.), v(82., 45., 30.), true);
    l.edge("boss", "cap_end", 0, v(58., 45., 30.), v(82., 45., 30.), true);
    l.silhouette("bore", "wall", 0, v(64., 45., 0.), v(64., 45., 30.), false);
    l.silhouette("bore", "wall", 0, v(76., 45., 0.), v(76., 45., 30.), false);
    for (ordinal, x) in [(0, 25.), (2, 115.)] {
        l.silhouette("holes", "wall", ordinal, v(x - 4.5, 23., 0.), v(x - 4.5, 23., 10.), false);
        l.silhouette("holes", "wall", ordinal, v(x + 4.5, 23., 0.), v(x + 4.5, 23., 10.), false);
    }
    l.edge("slot", "floor", 0, v(0., 25., 25.), v(10., 25., 25.), false);
    l.edge("slot", "roof", 0, v(0., 25., 37.), v(10., 25., 37.), false);
}

/// Top: looking down -Z. Placeholder line work.
fn top_lines(l: &mut Lines) {
    l.edge("flange", "outer", 0, v(0., 0., 60.), v(0., 90., 60.), true);
    l.edge("flange", "inner", 0, v(10., 0., 60.), v(10., 90., 60.), true);
    l.edge("flange", "front", 0, v(0., 0., 60.), v(10., 0., 60.), true);
    l.edge("flange", "back", 0, v(0., 90., 60.), v(10., 90., 60.), true);
    l.edge("plate", "front", 0, v(10., 0., 10.), v(120., 0., 10.), true);
    l.edge("plate", "back", 0, v(10., 90., 10.), v(120., 90., 10.), true);
    l.edge("plate", "end", 0, v(120., 0., 10.), v(120., 90., 10.), true);
    l.circle("boss", "cap_end", 0, v(70., 45., 30.), v(12., 0., 0.), v(0., 12., 0.), 4, true);
    l.circle("bore", "top", 0, v(70., 45., 30.), v(6., 0., 0.), v(0., 6., 0.), 4, true);
    for (ordinal, (x, y)) in [(25., 23.), (25., 67.), (115., 23.), (115., 67.)].into_iter().enumerate() {
        l.circle("holes", "cap_end", ordinal as u32, v(x, y, 10.), v(4.5, 0., 0.), v(0., 4.5, 0.), 4, true);
    }
    l.edge("slot", "side", 0, v(0., 25., 25.), v(10., 25., 25.), false);
    l.edge("slot", "side", 1, v(0., 65., 25.), v(10., 65., 25.), false);
}

/// Left: looking along +X, so sheet X runs along -Y. Placeholder line work.
fn left_lines(l: &mut Lines) {
    let face = [(0., 0.), (90., 0.), (90., 60.), (0., 60.)];
    for (i, role) in ["bottom", "back", "top", "front"].into_iter().enumerate() {
        let (a, b) = (face[i], face[(i + 1) % face.len()]);
        l.edge("flange", role, 0, v(0., a.0, a.1), v(0., b.0, b.1), true);
    }
    let slot = [(25., 25.), (65., 25.), (65., 37.), (25., 37.)];
    for (i, role) in ["floor", "side", "roof", "side"].into_iter().enumerate() {
        let (a, b) = (slot[i], slot[(i + 1) % slot.len()]);
        l.edge("slot", role, (i / 2) as u32, v(0., a.0, a.1), v(0., b.0, b.1), true);
    }
    l.edge("plate", "end", 0, v(120., 0., 10.), v(120., 90., 10.), false);
    l.silhouette("boss", "wall", 0, v(70., 33., 10.), v(70., 33., 30.), false);
    l.silhouette("boss", "wall", 0, v(70., 57., 10.), v(70., 57., 30.), false);
    l.edge("boss", "cap_end", 0, v(70., 33., 30.), v(70., 57., 30.), false);
    l.silhouette("bore", "wall", 0, v(70., 39., 0.), v(70., 39., 30.), false);
    l.silhouette("bore", "wall", 0, v(70., 51., 0.), v(70., 51., 30.), false);
    for (ordinal, y) in [(0, 23.), (1, 67.)] {
        l.silhouette("holes", "wall", ordinal, v(115., y - 4.5, 0.), v(115., y - 4.5, 10.), false);
        l.silhouette("holes", "wall", ordinal, v(115., y + 4.5, 0.), v(115., y + 4.5, 10.), false);
    }
}

/// Isometric: from (+X, -Y, +Z), visible edges only. Placeholder line work.
fn iso_lines(l: &mut Lines) {
    let front = [(0., 0.), (120., 0.), (120., 10.), (10., 10.), (10., 60.), (0., 60.)];
    for i in 0..front.len() {
        let (a, b) = (front[i], front[(i + 1) % front.len()]);
        l.edge("plate", "front", i as u32, v(a.0, 0., a.1), v(b.0, 0., b.1), true);
    }
    l.edge("plate", "end", 0, v(120., 0., 0.), v(120., 90., 0.), true);
    l.edge("plate", "end", 1, v(120., 90., 0.), v(120., 90., 10.), true);
    l.edge("plate", "end", 2, v(120., 0., 10.), v(120., 90., 10.), true);
    l.edge("plate", "top", 0, v(10., 0., 10.), v(10., 90., 10.), true);
    l.edge("plate", "top", 1, v(10., 90., 10.), v(120., 90., 10.), true);
    l.edge("flange", "inner", 0, v(10., 0., 60.), v(10., 90., 60.), true);
    l.edge("flange", "inner", 1, v(10., 90., 60.), v(10., 90., 10.), true);
    l.edge("flange", "top", 0, v(0., 0., 60.), v(0., 90., 60.), true);
    l.edge("flange", "top", 1, v(0., 90., 60.), v(10., 90., 60.), true);
    let slot = [(25., 25.), (65., 25.), (65., 37.), (25., 37.)];
    for i in 0..slot.len() {
        let (a, b) = (slot[i], slot[(i + 1) % slot.len()]);
        l.edge("slot", "edge", i as u32, v(10., a.0, a.1), v(10., b.0, b.1), true);
    }
    // Boss: top circle, the front half of the bottom circle, and two silhouettes at
    // 45 and 225 degrees.
    let s = 12.0 * std::f64::consts::FRAC_1_SQRT_2;
    l.circle("boss", "cap_end", 0, v(70., 45., 30.), v(12., 0., 0.), v(0., 12., 0.), 4, true);
    l.circle("boss", "cap_start", 0, v(70., 45., 10.), v(-s, -s, 0.), v(s, -s, 0.), 2, true);
    l.silhouette("boss", "wall", 0, v(70. + s, 45. + s, 10.), v(70. + s, 45. + s, 30.), true);
    l.silhouette("boss", "wall", 0, v(70. - s, 45. - s, 10.), v(70. - s, 45. - s, 30.), true);
    l.circle("bore", "top", 0, v(70., 45., 30.), v(6., 0., 0.), v(0., 6., 0.), 4, true);
    for (ordinal, (x, y)) in [(25., 23.), (25., 67.), (115., 23.), (115., 67.)].into_iter().enumerate() {
        l.circle("holes", "cap_end", ordinal as u32, v(x, y, 10.), v(4.5, 0., 0.), v(0., 4.5, 0.), 4, true);
    }
}

/// Builds one view's geometry from its placeholder line work, centred on its bounding
/// box, and returns the transform from model space to that view's millimetres.
fn view_geometry(view: StandardView, scale: f64, lines: fn(&mut Lines)) -> (ViewGeometry, ViewTransform) {
    let frame = ViewSpec::standard(view).frame().expect("standard views have a frame");
    let mut raw = ViewTransform { frame, scale, centre: Point2::new(0.0, 0.0) };
    let mut l = Lines { xf: raw, curves: Vec::new() };
    lines(&mut l);
    let bounds = l.curves.iter().map(|c| c.path.bounding_box()).reduce(|a, b| a.union(b)).unwrap_or(Rect::ZERO);
    raw.centre = Point2::new(bounds.center().x, bounds.center().y);
    let shift = |p: kurbo::Point| kurbo::Point::new(round(p.x - raw.centre.u), round(p.y - raw.centre.v));
    let mut curves = l.curves;
    for c in &mut curves {
        c.path = c
            .path
            .elements()
            .iter()
            .map(|el| match *el {
                PathEl::MoveTo(p) => PathEl::MoveTo(shift(p)),
                PathEl::LineTo(p) => PathEl::LineTo(shift(p)),
                PathEl::QuadTo(a, b) => PathEl::QuadTo(shift(a), shift(b)),
                PathEl::CurveTo(a, b, p) => PathEl::CurveTo(shift(a), shift(b), shift(p)),
                PathEl::ClosePath => PathEl::ClosePath,
            })
            .collect();
    }
    let (min, max) = (shift(bounds.origin()), shift(kurbo::Point::new(bounds.x1, bounds.y1)));
    let geometry = ViewGeometry {
        source_digest: DIGEST.to_owned(),
        curves,
        hatch: Vec::new(),
        outline: None,
        bbox: [min.x, min.y, max.x, max.y],
    };
    (geometry, raw)
}

/// Model space to view millimetres: project, scale, then move the bounding box centre to
/// the origin.
#[derive(Clone, Copy)]
struct ViewTransform {
    frame: ViewFrame,
    scale: f64,
    centre: Point2,
}

impl ViewTransform {
    fn raw(&self, p: Vec3) -> kurbo::Point {
        let q = self.frame.project(p);
        kurbo::Point::new(q.u * self.scale, q.v * self.scale)
    }

    fn point(&self, p: Vec3) -> Point2 {
        let q = self.raw(p);
        Point2::new(round(q.x - self.centre.u), round(q.y - self.centre.v))
    }

    fn anchor(&self, entity: EntityRef, attach: Attach, at: Vec3) -> Anchor {
        Anchor { entity, attach, cached: self.point(at) }
    }
}

/// Collects placeholder curves in projected, scaled but not yet centred coordinates.
struct Lines {
    xf: ViewTransform,
    curves: Vec<Curve2>,
}

impl Lines {
    fn push(&mut self, path: BezPath, source: EntityRef, visible: bool, kind: CurveKind) {
        self.curves.push(Curve2 { path, source, visible, kind, tangent: false });
    }

    fn segment(&self, a: Vec3, b: Vec3) -> BezPath {
        let mut path = BezPath::new();
        path.move_to(self.xf.raw(a));
        path.line_to(self.xf.raw(b));
        path
    }

    fn edge(&mut self, feature: &str, role: &str, ordinal: u32, a: Vec3, b: Vec3, visible: bool) {
        let path = self.segment(a, b);
        self.push(path, line_ref(EntityKind::Edge, feature, role, ordinal, a, b), visible, CurveKind::Edge);
    }

    fn silhouette(&mut self, feature: &str, role: &str, ordinal: u32, a: Vec3, b: Vec3, visible: bool) {
        let path = self.segment(a, b);
        self.push(path, line_ref(EntityKind::Face, feature, role, ordinal, a, b), visible, CurveKind::Silhouette);
    }

    /// `quarters` quarter turns of the circle through `centre + a`, then `centre + b`, where
    /// `a` and `b` are perpendicular radius vectors. Each quarter is one cubic, so the
    /// projection of the circle is exact up to the usual cubic approximation.
    #[allow(clippy::too_many_arguments)]
    fn circle(
        &mut self,
        feature: &str,
        role: &str,
        ordinal: u32,
        centre: Vec3,
        a: Vec3,
        b: Vec3,
        quarters: usize,
        visible: bool,
    ) {
        // Control point distance for a quarter circle of radius 1.
        const KAPPA: f64 = 0.552_284_749_830_793_4;
        let radius = (a.x * a.x + a.y * a.y + a.z * a.z).sqrt();
        let (mut u, mut w) = (a, b);
        let mut path = BezPath::new();
        path.move_to(self.xf.raw(add(centre, u)));
        for _ in 0..quarters {
            let c1 = add(centre, add(u, mul(w, KAPPA)));
            let c2 = add(centre, add(w, mul(u, KAPPA)));
            path.curve_to(self.xf.raw(c1), self.xf.raw(c2), self.xf.raw(add(centre, w)));
            (u, w) = (w, mul(u, -1.0));
        }
        if quarters == 4 {
            path.close_path();
        }
        self.push(path, circle_ref(feature, role, ordinal, centre, radius), visible, CurveKind::Edge);
    }
}

fn v(x: f64, y: f64, z: f64) -> Vec3 {
    Vec3::new(x, y, z)
}

fn add(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(a.x + b.x, a.y + b.y, a.z + b.z)
}

fn mul(a: Vec3, k: f64) -> Vec3 {
    Vec3::new(a.x * k, a.y * k, a.z * k)
}

/// Rounds to 0.1 micrometre so the fixture JSON stays short and stable.
fn round(x: f64) -> f64 {
    (x * 1e4).round() / 1e4 + 0.0
}

fn line_ref(kind: EntityKind, feature: &str, role: &str, ordinal: u32, a: Vec3, b: Vec3) -> EntityRef {
    let (dx, dy, dz) = (b.x - a.x, b.y - a.y, b.z - a.z);
    let mid = v((a.x + b.x) / 2.0, (a.y + b.y) / 2.0, (a.z + b.z) / 2.0);
    EntityRef::new(kind, feature, role, ordinal, mid, round((dx * dx + dy * dy + dz * dz).sqrt()))
}

fn circle_ref(feature: &str, role: &str, ordinal: u32, centre: Vec3, radius: f64) -> EntityRef {
    EntityRef::new(EntityKind::Edge, feature, role, ordinal, centre, round(std::f64::consts::TAU * radius))
}
