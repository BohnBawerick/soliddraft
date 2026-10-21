//! Views: projections of the model placed on a sheet.

use kurbo::BezPath;
use serde::{Deserialize, Serialize};
use soliddraft_kernel_api::drawing::{StandardView, ViewSpec};
use soliddraft_kernel_api::{Point2, Vec3};

use crate::entity::EntityRef;
use crate::ids::ViewId;

/// One view on a sheet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct View {
    /// Id, unique within the drawing.
    pub id: ViewId,
    /// How the view is derived.
    pub kind: ViewKind,
    /// Sheet position of the view's origin, in sheet millimetres from the bottom-left
    /// corner with Y up. The origin is the centre of the geometry's bounding box.
    pub position: Point2,
    /// The alignment kept with a parent view, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alignment: Option<Alignment>,
    /// Scale of the view.
    pub scale: ViewScale,
    /// Line display options.
    pub display: ViewDisplay,
    /// True when automatic updates skip this view.
    #[serde(default)]
    pub excluded_from_update: bool,
    /// The cached line work.
    pub geometry: ViewGeometry,
    /// The view's name in the drawing tree and its caption. `None` uses the default for
    /// its kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// A crop: only the line work inside this boundary, in view millimetres, is shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<Boundary>,
    /// Broken-out sections: inside each boundary the material is removed down to a
    /// depth and the cut is hatched.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub broken_out: Vec<BrokenOut>,
}

impl View {
    /// The parent view, for every kind except a base view.
    pub fn parent(&self) -> Option<ViewId> {
        match self.kind {
            ViewKind::Base { .. } => None,
            ViewKind::Projected { parent, .. }
            | ViewKind::Section { parent, .. }
            | ViewKind::Detail { parent, .. }
            | ViewKind::Auxiliary { parent, .. } => Some(parent),
        }
    }

    /// The section, detail or view letter of a derived view, if it has one.
    pub fn letter(&self) -> Option<&str> {
        match &self.kind {
            ViewKind::Section { label, .. } | ViewKind::Detail { label, .. } => Some(label),
            ViewKind::Auxiliary { label, .. } if !label.is_empty() => Some(label),
            _ => None,
        }
    }
}

/// How a view is derived from the model or from another view.
// An auxiliary view carries a full entity reference; a drawing has a handful of views,
// so the larger variant costs nothing worth a box.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ViewKind {
    /// A view of the model from a given orientation.
    Base {
        /// Where the eye looks from.
        orientation: Orientation,
    },
    /// A view folded off a parent view.
    Projected {
        /// The view it is folded from.
        parent: ViewId,
        /// Which side of the parent it sits on. The sheet's projection angle decides
        /// which face of the model that side shows.
        direction: FoldDirection,
    },
    /// A section through a parent view along a cutting line.
    ///
    /// The cutting plane holds the line and the parent's view direction. Seen on the
    /// parent, the arrows at the line's ends point the way the section looks: to the
    /// left of the line run from its leftmost (or, when vertical, lowest) point, unless
    /// `flip` turns them round. See [`section_arrow`](crate::project::section_arrow).
    Section {
        /// The view the cutting line is drawn on.
        parent: ViewId,
        /// The cutting line in the parent's view millimetres, from end to end. Only
        /// straight cuts (two points) are built.
        line: Vec<Point2>,
        /// True to look the other way along the cut.
        flip: bool,
        /// The section letter, such as `"A"` for section A-A.
        label: String,
        /// The ends of the cutting line in model space. The cut stays at this place on
        /// the part when the parent's view moves; `line` is redrawn from it at every
        /// update. Empty in drawings made before phase D3: then `line` is the reference.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        model_line: Vec<Vec3>,
        /// The hatch pattern of the cut faces, an id from the library's hatch patterns;
        /// `None` uses the first.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hatch: Option<String>,
    },
    /// A magnified region of a parent view, inside a circle or a closed profile.
    Detail {
        /// The view the boundary is drawn on.
        parent: ViewId,
        /// Centre of the circle in the parent's view millimetres. The magnified view is
        /// centred on it, also for a profile.
        center: Point2,
        /// Radius of the circle in the parent's view millimetres.
        radius: f64,
        /// Magnification relative to the parent, such as 2. With
        /// [`ViewScale::Parent`] the view's scale is the parent's times this.
        factor: f64,
        /// The detail letter, such as `"B"`.
        label: String,
        /// A closed profile in the parent's view millimetres that replaces the circle as
        /// the boundary. Empty for a circle.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        profile: Vec<Point2>,
        /// The centre in model space, so the detail stays on the same place of the part
        /// when the parent's view moves. `None` in drawings made before phase D3.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_center: Option<Vec3>,
    },
    /// A view unfolded normal to a reference edge of the parent: it looks along the
    /// parent's sheet, square to the edge, so a face the parent sees edge on along that
    /// edge shows at its true size.
    Auxiliary {
        /// The view the edge is picked in.
        parent: ViewId,
        /// The reference edge.
        edge: EntityRef,
        /// The view looks from the side of the edge away from the parent's centre;
        /// true looks from the other side.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        flip: bool,
        /// The view letter, such as `"C"` for view C, or empty for none.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        label: String,
    },
}

/// A closed region in view millimetres: a crop, a detail boundary or a broken-out
/// section.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Boundary {
    /// A circle.
    Circle {
        /// Centre.
        center: Point2,
        /// Radius, greater than zero.
        radius: f64,
    },
    /// A closed polygon; the last point joins the first.
    Polygon {
        /// The corners, at least three.
        points: Vec<Point2>,
    },
}

impl Boundary {
    /// A rectangle from two opposite corners.
    pub fn rectangle(a: Point2, b: Point2) -> Boundary {
        let (x0, x1, y0, y1) = (a.u.min(b.u), a.u.max(b.u), a.v.min(b.v), a.v.max(b.v));
        Boundary::Polygon {
            points: vec![Point2::new(x0, y0), Point2::new(x1, y0), Point2::new(x1, y1), Point2::new(x0, y1)],
        }
    }

    /// True when `p` is inside (even-odd for a polygon).
    pub fn contains(&self, p: Point2) -> bool {
        match self {
            Boundary::Circle { center, radius } => (p.u - center.u).hypot(p.v - center.v) <= *radius,
            Boundary::Polygon { points } => {
                let mut inside = false;
                let n = points.len();
                for i in 0..n {
                    let (a, b) = (points[i], points[(i + n - 1) % n]);
                    if (a.v > p.v) != (b.v > p.v) && p.u < (b.u - a.u) * (p.v - a.v) / (b.v - a.v) + a.u {
                        inside = !inside;
                    }
                }
                inside
            }
        }
    }

    /// The boundary as a closed path.
    pub fn path(&self) -> BezPath {
        match self {
            Boundary::Circle { center, radius } => {
                kurbo::Shape::to_path(&kurbo::Circle::new((center.u, center.v), *radius), 1e-3)
            }
            Boundary::Polygon { points } => {
                let mut p = BezPath::new();
                for (i, q) in points.iter().enumerate() {
                    if i == 0 {
                        p.move_to((q.u, q.v));
                    } else {
                        p.line_to((q.u, q.v));
                    }
                }
                p.close_path();
                p
            }
        }
    }

    /// Corners along the boundary, a circle at `segments` points: the polygon a hatch
    /// is clipped with.
    pub fn polygon(&self, segments: usize) -> Vec<Point2> {
        match self {
            Boundary::Circle { center, radius } => (0..segments.max(8))
                .map(|i| {
                    let t = std::f64::consts::TAU * i as f64 / segments.max(8) as f64;
                    Point2::new(center.u + radius * t.cos(), center.v + radius * t.sin())
                })
                .collect(),
            Boundary::Polygon { points } => points.clone(),
        }
    }

    /// `[min x, min y, max x, max y]`.
    pub fn bounds(&self) -> [f64; 4] {
        match self {
            Boundary::Circle { center, radius } => {
                [center.u - radius, center.v - radius, center.u + radius, center.v + radius]
            }
            Boundary::Polygon { points } => {
                points.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, p| {
                    [b[0].min(p.u), b[1].min(p.v), b[2].max(p.u), b[3].max(p.v)]
                })
            }
        }
    }

    /// The boundary moved by `(du, dv)` and scaled by `k` about the origin, in that
    /// order of application: `p -> (p + d) * k`.
    pub fn mapped(&self, du: f64, dv: f64, k: f64) -> Boundary {
        let m = |p: Point2| Point2::new((p.u + du) * k, (p.v + dv) * k);
        match self {
            Boundary::Circle { center, radius } => Boundary::Circle { center: m(*center), radius: radius * k },
            Boundary::Polygon { points } => Boundary::Polygon { points: points.iter().map(|p| m(*p)).collect() },
        }
    }
}

/// A broken-out section: inside the boundary the material in front of a plane is
/// removed and the cut is hatched, the rest of the view stays as it is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrokenOut {
    /// The region, in the view's millimetres.
    pub boundary: Boundary,
    /// How deep the cut goes, in model millimetres from the point of the part nearest
    /// the eye.
    pub depth: f64,
}

/// The orientation of a base view.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Orientation {
    /// One of the standard orientations.
    Standard {
        /// Which one.
        view: StandardView,
    },
    /// Any other orientation.
    Custom {
        /// Unit direction from the model towards the eye.
        direction: Vec3,
        /// Which way is up on the sheet.
        up: Vec3,
    },
}

impl Orientation {
    /// The kernel request for this orientation with the given display options.
    pub fn spec(&self, display: &ViewDisplay) -> ViewSpec {
        let mut spec = match *self {
            Orientation::Standard { view } => ViewSpec::standard(view),
            Orientation::Custom { direction, up } => ViewSpec::new(direction, up),
        };
        spec.smooth_edges = display.tangent != TangentEdges::Removed;
        spec.seams = display.seams;
        spec
    }
}

/// The side of its parent a projected view sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FoldDirection {
    /// Left of the parent.
    Left,
    /// Right of the parent.
    Right,
    /// Above the parent.
    Up,
    /// Below the parent.
    Down,
    /// Above and left of the parent: a pictorial view.
    UpLeft,
    /// Above and right of the parent: a pictorial view.
    UpRight,
    /// Below and left of the parent: a pictorial view.
    DownLeft,
    /// Below and right of the parent: a pictorial view.
    DownRight,
}

impl FoldDirection {
    /// True for the four diagonal directions, which give pictorial views.
    pub fn is_diagonal(self) -> bool {
        matches!(
            self,
            FoldDirection::UpLeft | FoldDirection::UpRight | FoldDirection::DownLeft | FoldDirection::DownRight
        )
    }
}

/// An alignment a child view keeps with its parent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alignment {
    /// The view it is aligned with.
    pub parent: ViewId,
    /// Which coordinate the two views share.
    pub axis: AlignAxis,
}

/// The axis of an alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlignAxis {
    /// Side by side: the views share their Y and the child slides horizontally.
    Horizontal,
    /// One above the other: the views share their X and the child slides vertically.
    Vertical,
}

/// The scale of a view.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ViewScale {
    /// The sheet's scale. The default for base views.
    Sheet,
    /// The parent's scale. The default for child views.
    Parent,
    /// A scale of its own.
    Custom {
        /// Sheet millimetres per model millimetre, such as 0.5 for 1:2.
        factor: f64,
    },
}

/// How a view draws its lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewDisplay {
    /// Hidden lines.
    pub hidden: HiddenLines,
    /// Edges between tangent faces.
    pub tangent: TangentEdges,
    /// True to draw seams.
    #[serde(default)]
    pub seams: bool,
}

impl ViewDisplay {
    /// The default for orthographic views: hidden lines and tangent edges removed.
    pub const ORTHOGRAPHIC: ViewDisplay =
        ViewDisplay { hidden: HiddenLines::Removed, tangent: TangentEdges::Removed, seams: false };

    /// The default for pictorial views: hidden lines removed, tangent edges phantom.
    pub const PICTORIAL: ViewDisplay =
        ViewDisplay { hidden: HiddenLines::Removed, tangent: TangentEdges::Phantom, seams: false };
}

impl Default for ViewDisplay {
    fn default() -> ViewDisplay {
        ViewDisplay::ORTHOGRAPHIC
    }
}

/// Whether hidden lines are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HiddenLines {
    /// Not drawn.
    Removed,
    /// Drawn dashed.
    Visible,
}

/// How edges between tangent faces are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TangentEdges {
    /// Not drawn.
    Removed,
    /// Drawn in a thin phantom line.
    Phantom,
    /// Drawn like any other edge.
    Visible,
}

/// The cached line work of a view, so a drawing opens without the kernel.
///
/// Coordinates are view millimetres: the view's scale is applied and the origin is the
/// view's [`position`](View::position), with Y up.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ViewGeometry {
    /// Digest of the model the geometry was made from. A view whose digest differs from
    /// the model's is out of date.
    pub source_digest: String,
    /// Visible and hidden curves.
    pub curves: Vec<Curve2>,
    /// Regions to hatch, for section views.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hatch: Vec<Polygon>,
    /// The boundary a detail view is drawn inside, in view millimetres.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline: Option<Boundary>,
    /// Bounds of the curves: `[min x, min y, max x, max y]`.
    pub bbox: [f64; 4],
}

/// One curve of a view.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Curve2 {
    /// The curve in view millimetres. Stored as SVG path data with absolute commands.
    #[serde(with = "svg_path")]
    pub path: BezPath,
    /// The model entity it comes from.
    pub source: EntityRef,
    /// False when the model hides it.
    pub visible: bool,
    /// What kind of curve it is.
    pub kind: CurveKind,
    /// True for an edge between tangent faces, drawn as the view's tangent edge option
    /// says.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tangent: bool,
}

/// What a curve of a view shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurveKind {
    /// An edge of the model.
    Edge,
    /// The outline of a curved face.
    Silhouette,
    /// A seam of a closed face.
    Seam,
    /// Where a section plane cuts a face.
    Section,
}

/// A closed region in view millimetres.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Polygon {
    /// The outer boundary.
    pub outer: Vec<Point2>,
    /// Boundaries of holes in the region.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holes: Vec<Vec<Point2>>,
    /// Only the part of the region inside this boundary counts, as in a detail of a
    /// section or a broken-out section.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<Boundary>,
}

/// Serialises a [`BezPath`] as SVG path data, which is shorter and easier to read than
/// kurbo's element list. Every `f64` is written with its shortest exact form, so the
/// path reads back unchanged.
mod svg_path {
    use kurbo::BezPath;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(path: &BezPath, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&path.to_svg())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<BezPath, D::Error> {
        let data = String::deserialize(deserializer)?;
        BezPath::from_svg(&data).map_err(serde::de::Error::custom)
    }
}
