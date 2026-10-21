//! Curves out of the kernel for 2D drawings: hidden-line views, planar sections, exact
//! edge geometry and lines-and-arcs conversion.
//!
//! Everything here is kernel-neutral and serialisable. A view is asked for with a
//! [`ViewSpec`] (the direction the eye looks from and which way is up) and comes back as
//! [`ViewCurves`]: 3D curves in model space, each marked visible or hidden and tagged with
//! the entity it came from. Projecting them onto the sheet is the drawing crate's job;
//! [`ViewFrame`] gives the projection every caller must agree on.
//!
//! [`mesh_hlr`] is a hidden-line pass over a triangle mesh that works on any backend. It
//! is approximate, the fallback for backends without exact hidden-line removal, and the
//! reference the exact backends are compared with.

pub mod mesh_hlr;
pub mod section;
pub mod visibility;

use serde::{Deserialize, Serialize};

pub use section::{CutRegion, SectionSpec, SectionView};

use crate::{EdgeId, FaceId, Point2, Vec3};

/// A curve in model space, in millimetres.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Curve3 {
    /// A straight segment from `a` to `b`.
    Line {
        /// Start point.
        a: Vec3,
        /// End point.
        b: Vec3,
    },
    /// A circular arc, counter-clockwise about `normal` from `start_deg` to `end_deg`.
    /// Angles are measured from `x_axis`, so the arc's point at angle `t` is
    /// `center + radius * (cos t * x_axis + sin t * (normal x x_axis))`.
    Arc {
        /// Centre.
        center: Vec3,
        /// Unit axis; the arc runs counter-clockwise seen from where it points.
        normal: Vec3,
        /// Unit direction of angle zero, perpendicular to `normal`.
        x_axis: Vec3,
        /// Radius, greater than zero.
        radius: f64,
        /// Start angle in degrees.
        start_deg: f64,
        /// End angle in degrees, greater than `start_deg` and at most 360 above it.
        end_deg: f64,
    },
    /// A full circle.
    Circle {
        /// Centre.
        center: Vec3,
        /// Unit axis.
        normal: Vec3,
        /// Radius, greater than zero.
        radius: f64,
    },
    /// A non-uniform rational B-spline. `knots` has `points.len() + degree + 1` entries;
    /// `weights` is empty for a non-rational curve, otherwise one per point.
    Nurbs {
        /// Polynomial degree, at least 1.
        degree: u32,
        /// Full knot vector, non-decreasing.
        knots: Vec<f64>,
        /// Control points.
        points: Vec<Vec3>,
        /// Control point weights, or empty when every weight is 1.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        weights: Vec<f64>,
    },
    /// A chain of straight segments, as approximate backends produce.
    Polyline {
        /// The vertices, in order.
        points: Vec<Vec3>,
    },
}

/// The model entity a curve in a view comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum CurveSource {
    /// All or part of an edge between two faces.
    Edge(EdgeId),
    /// The outline of a curved face seen from the view direction.
    Silhouette(FaceId),
    /// A seam: an edge whose two sides are the same face, such as the join line of a
    /// cylinder.
    Seam(FaceId),
    /// Where a section plane cuts a face.
    Section(FaceId),
    /// The backend could not tell.
    Unknown,
}

/// One curve of a view.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewCurve {
    /// The geometry, in model space.
    pub curve: Curve3,
    /// What it comes from.
    pub source: CurveSource,
    /// False when the body hides it from the eye.
    pub visible: bool,
    /// True for an edge between two tangent faces, such as where a fillet meets a wall,
    /// which drawings show thin or leave out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub smooth: bool,
}

impl ViewCurve {
    /// A curve that is not a tangent edge.
    pub fn new(curve: Curve3, source: CurveSource, visible: bool) -> ViewCurve {
        ViewCurve { curve, source, visible, smooth: false }
    }
}

/// What to look at a body from.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewSpec {
    /// Unit direction from the model towards the eye. The eye is at infinity: views are
    /// parallel projections.
    pub direction: Vec3,
    /// Which way is up on the sheet. Need not be perpendicular to `direction`, only not
    /// parallel to it; the part along `direction` is ignored.
    pub up: Vec3,
    /// Show edges between tangent faces, such as where a fillet meets a wall.
    pub smooth_edges: bool,
    /// Show seams.
    pub seams: bool,
    /// Accuracy of the curves, in millimetres. Zero lets the backend choose.
    pub tolerance: f64,
}

/// The standard orientations, for a model with Z up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StandardView {
    /// Looking along +Y: the eye is at -Y.
    Front,
    /// Looking along -Y.
    Back,
    /// Looking down -Z.
    Top,
    /// Looking up +Z.
    Bottom,
    /// Looking along -X: the eye is at +X.
    Right,
    /// Looking along +X.
    Left,
    /// From (+X, -Y, +Z), the corner between front, right and top.
    Isometric,
}

impl StandardView {
    /// Every standard view.
    pub const ALL: [StandardView; 7] = [
        StandardView::Front,
        StandardView::Back,
        StandardView::Top,
        StandardView::Bottom,
        StandardView::Right,
        StandardView::Left,
        StandardView::Isometric,
    ];

    /// Lower-case name, as on the command line.
    pub fn name(self) -> &'static str {
        match self {
            StandardView::Front => "front",
            StandardView::Back => "back",
            StandardView::Top => "top",
            StandardView::Bottom => "bottom",
            StandardView::Right => "right",
            StandardView::Left => "left",
            StandardView::Isometric => "iso",
        }
    }

    /// The view named `name`, as [`name`](Self::name) spells it ("isometric" also works).
    pub fn from_name(name: &str) -> Option<StandardView> {
        match name {
            "isometric" => Some(StandardView::Isometric),
            _ => StandardView::ALL.into_iter().find(|v| v.name() == name),
        }
    }

    /// (direction towards the eye, up).
    fn axes(self) -> (Vec3, Vec3) {
        let s = 1.0 / 3f64.sqrt();
        match self {
            StandardView::Front => (-Vec3::Y, Vec3::Z),
            StandardView::Back => (Vec3::Y, Vec3::Z),
            StandardView::Top => (Vec3::Z, Vec3::Y),
            StandardView::Bottom => (-Vec3::Z, -Vec3::Y),
            StandardView::Right => (Vec3::X, Vec3::Z),
            StandardView::Left => (-Vec3::X, Vec3::Z),
            StandardView::Isometric => (Vec3::new(s, -s, s), Vec3::Z),
        }
    }
}

impl ViewSpec {
    /// A view from `direction` with `up` up, hidden seams and smooth edges, and the
    /// backend's default tolerance.
    pub fn new(direction: Vec3, up: Vec3) -> ViewSpec {
        ViewSpec { direction, up, smooth_edges: false, seams: false, tolerance: 0.0 }
    }

    /// A standard orientation, with the defaults of [`new`](Self::new).
    pub fn standard(view: StandardView) -> ViewSpec {
        let (direction, up) = view.axes();
        ViewSpec::new(direction, up)
    }

    /// The sheet frame of this view, or `None` when the direction is zero or parallel to
    /// up.
    pub fn frame(&self) -> Option<ViewFrame> {
        let direction = self.direction.normalized()?;
        let right = self.up.cross(direction).normalized()?;
        Some(ViewFrame { right, up: direction.cross(right), direction })
    }
}

/// The orthonormal frame a view is projected with: sheet coordinates are
/// `(p . right, p . up)` and depth towards the eye is `p . direction`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewFrame {
    /// Sheet X.
    pub right: Vec3,
    /// Sheet Y.
    pub up: Vec3,
    /// Towards the eye.
    pub direction: Vec3,
}

impl ViewFrame {
    /// Sheet coordinates of a model point, in model millimetres.
    pub fn project(&self, p: Vec3) -> Point2 {
        Point2::new(p.dot(self.right), p.dot(self.up))
    }

    /// Distance of a point towards the eye.
    pub fn depth(&self, p: Vec3) -> f64 {
        p.dot(self.direction)
    }
}

/// Everything a view shows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ViewCurves {
    /// Visible and hidden curves.
    pub curves: Vec<ViewCurve>,
    /// Bounds of every curve in sheet coordinates (see [`ViewFrame::project`]):
    /// `[min x, min y, max x, max y]`. All zero when there are no curves.
    pub bbox_2d: [f64; 4],
}

impl ViewCurves {
    /// Collects curves and computes their sheet bounds in `frame`.
    pub fn new(curves: Vec<ViewCurve>, frame: &ViewFrame) -> ViewCurves {
        let bbox_2d = sheet_bounds(&curves, frame);
        ViewCurves { curves, bbox_2d }
    }

    /// Number of visible and hidden curves.
    pub fn counts(&self) -> (usize, usize) {
        let visible = self.curves.iter().filter(|c| c.visible).count();
        (visible, self.curves.len() - visible)
    }

    /// The text [`digest`](Self::digest) hashes: one line per curve with its visibility,
    /// the kind of its source, its kind, and its start, middle and end points and length
    /// rounded to a grid, sorted. It does not depend on entity ids or on curve order.
    ///
    /// The grid spacing is a power of ten about a millionth of the view's size, as for
    /// body digests: two runs that agree well within it give the same text.
    pub fn canonical_text(&self) -> String {
        let [x0, y0, x1, y1] = self.bbox_2d;
        let size = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt().max(1.0);
        let quantum = 10f64.powi(size.log10().floor() as i32 - 6);
        let q = |v: f64| {
            let r = (v / quantum).round();
            if r == 0.0 { 0 } else { r as i64 }
        };
        let qp = |p: Vec3| [q(p.x), q(p.y), q(p.z)];
        let mut lines: Vec<String> = self
            .curves
            .iter()
            .map(|c| {
                let source = match c.source {
                    CurveSource::Edge(_) => "edge",
                    CurveSource::Silhouette(_) => "silhouette",
                    CurveSource::Seam(_) => "seam",
                    CurveSource::Section(_) => "section",
                    CurveSource::Unknown => "unknown",
                };
                let kind = match c.curve {
                    Curve3::Line { .. } => "line",
                    Curve3::Arc { .. } => "arc",
                    Curve3::Circle { .. } => "circle",
                    Curve3::Nurbs { .. } => "nurbs",
                    Curve3::Polyline { .. } => "polyline",
                };
                format!(
                    "{} {source} {kind} {:?} {:?} {:?} {}",
                    if c.visible { "visible" } else { "hidden" },
                    qp(c.curve.start()),
                    qp(c.curve.point_at(0.5)),
                    qp(c.curve.end()),
                    q(c.curve.length())
                )
            })
            .collect();
        lines.sort();
        let mut out = format!("soliddraft-view-digest/1 quantum {quantum:e}\n");
        for line in lines {
            out.push_str(&line);
            out.push('\n');
        }
        out
    }

    /// A 128-bit FNV-1a hash of [`canonical_text`](Self::canonical_text), in hex: equal
    /// for two runs that produce the same curves.
    pub fn digest(&self) -> String {
        const OFFSET: u128 = 0x6c62272e07bb014262b821756295c58d;
        const PRIME: u128 = 0x0000000001000000000000000000013b;
        let hash = self.canonical_text().bytes().fold(OFFSET, |h, b| (h ^ u128::from(b)).wrapping_mul(PRIME));
        format!("{hash:032x}")
    }
}

/// The bounds of `curves` in sheet coordinates of `frame`, as [`ViewCurves::bbox_2d`]
/// holds them.
pub fn sheet_bounds(curves: &[ViewCurve], frame: &ViewFrame) -> [f64; 4] {
    if curves.is_empty() {
        return [0.0; 4];
    }
    let mut bbox = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for c in curves {
        for p in c.curve.sample(0.0) {
            let q = frame.project(p);
            bbox = [bbox[0].min(q.u), bbox[1].min(q.v), bbox[2].max(q.u), bbox[3].max(q.v)];
        }
    }
    bbox
}

/// Angle steps used when sampling arcs and splines with no chord tolerance given.
const DEFAULT_ANGLE_STEP_DEG: f64 = 5.0;

impl Curve3 {
    /// Start point.
    pub fn start(&self) -> Vec3 {
        self.point_at(0.0)
    }

    /// End point.
    pub fn end(&self) -> Vec3 {
        self.point_at(1.0)
    }

    /// True when the curve ends where it starts.
    pub fn is_closed(&self, tol: f64) -> bool {
        matches!(self, Curve3::Circle { .. }) || self.start().distance(self.end()) <= tol
    }

    /// The point at `t` in [0, 1] along the curve's parameter: by angle for arcs and
    /// circles, by knot range for splines and by vertex count for polylines.
    pub fn point_at(&self, t: f64) -> Vec3 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Curve3::Line { a, b } => *a + (*b - *a) * t,
            Curve3::Arc { center, normal, x_axis, radius, start_deg, end_deg } => {
                let angle = (start_deg + (end_deg - start_deg) * t).to_radians();
                circle_point(*center, *normal, *x_axis, *radius, angle)
            }
            Curve3::Circle { center, normal, radius } => {
                circle_point(*center, *normal, perpendicular(*normal), *radius, t * std::f64::consts::TAU)
            }
            Curve3::Nurbs { degree, knots, points, weights } => {
                let (lo, hi) = nurbs_range(*degree, knots, points.len());
                nurbs_point(*degree as usize, knots, points, weights, lo + (hi - lo) * t)
            }
            Curve3::Polyline { points } => match points.len() {
                0 => Vec3::ZERO,
                1 => points[0],
                n => {
                    let x = t * (n - 1) as f64;
                    let i = (x.floor() as usize).min(n - 2);
                    points[i] + (points[i + 1] - points[i]) * (x - i as f64)
                }
            },
        }
    }

    /// Points along the curve, ends included, close enough that the chords stay within
    /// `chord_tol` of it. With `chord_tol` zero, arcs and splines are sampled every five
    /// degrees of turning.
    pub fn sample(&self, chord_tol: f64) -> Vec<Vec3> {
        self.sample_params(chord_tol).into_iter().map(|(_, p)| p).collect()
    }

    /// Like [`sample`](Self::sample), with each point's parameter in [0, 1] as
    /// [`point_at`](Self::point_at) takes it.
    pub fn sample_params(&self, chord_tol: f64) -> Vec<(f64, Vec3)> {
        let uniform = |n: usize| (0..=n).map(|i| i as f64 / n as f64).map(|t| (t, self.point_at(t))).collect();
        match self {
            Curve3::Line { a, b } => vec![(0.0, *a), (1.0, *b)],
            Curve3::Polyline { points } => {
                let last = points.len().saturating_sub(1).max(1) as f64;
                points.iter().enumerate().map(|(i, p)| (i as f64 / last, *p)).collect()
            }
            Curve3::Arc { radius, start_deg, end_deg, .. } => {
                uniform(arc_segments(*radius, end_deg - start_deg, chord_tol))
            }
            Curve3::Circle { radius, .. } => uniform(arc_segments(*radius, 360.0, chord_tol)),
            Curve3::Nurbs { degree, knots, points, weights } => {
                let (lo, hi) = nurbs_range(*degree, knots, points.len());
                let span = if hi > lo { hi - lo } else { 1.0 };
                sample_nurbs(*degree as usize, knots, points, weights, chord_tol)
                    .into_iter()
                    .map(|(u, p)| ((u - lo) / span, p))
                    .collect()
            }
        }
    }

    /// The part of the curve between parameters `t0` and `t1` in [0, 1], `t0 < t1`, as
    /// [`point_at`](Self::point_at) measures them. Exact for every kind: a spline is cut
    /// by knot insertion.
    pub fn between(&self, t0: f64, t1: f64) -> Curve3 {
        let (t0, t1) = (t0.clamp(0.0, 1.0), t1.clamp(0.0, 1.0));
        if t0 <= 0.0 && t1 >= 1.0 {
            return self.clone();
        }
        match self {
            Curve3::Line { .. } => Curve3::Line { a: self.point_at(t0), b: self.point_at(t1) },
            Curve3::Arc { center, normal, x_axis, radius, start_deg, end_deg } => {
                let at = |t: f64| start_deg + (end_deg - start_deg) * t;
                Curve3::Arc {
                    center: *center,
                    normal: *normal,
                    x_axis: *x_axis,
                    radius: *radius,
                    start_deg: at(t0),
                    end_deg: at(t1),
                }
            }
            Curve3::Circle { center, normal, radius } => Curve3::Arc {
                center: *center,
                normal: *normal,
                x_axis: perpendicular(*normal),
                radius: *radius,
                start_deg: 360.0 * t0,
                end_deg: 360.0 * t1,
            },
            Curve3::Nurbs { degree, knots, points, weights } => {
                let (lo, hi) = nurbs_range(*degree, knots, points.len());
                let mut c = Nurbs::new(*degree as usize, knots, points, weights);
                if t1 < 1.0 {
                    c = c.split(lo + (hi - lo) * t1).0;
                }
                if t0 > 0.0 {
                    c = c.split(lo + (hi - lo) * t0).1;
                }
                c.into_curve(*degree, !weights.is_empty())
            }
            Curve3::Polyline { points } => {
                let n = points.len().saturating_sub(1).max(1) as f64;
                let (i0, i1) = ((t0 * n).floor() as usize + 1, (t1 * n).ceil() as usize);
                let mut out = vec![self.point_at(t0)];
                out.extend(points.iter().take(i1).skip(i0).copied());
                out.push(self.point_at(t1));
                Curve3::Polyline { points: out }
            }
        }
    }

    /// Arc length; exact for lines, arcs and circles, from a fine sampling otherwise.
    pub fn length(&self) -> f64 {
        match self {
            Curve3::Line { a, b } => a.distance(*b),
            Curve3::Arc { radius, start_deg, end_deg, .. } => radius * (end_deg - start_deg).to_radians(),
            Curve3::Circle { radius, .. } => radius * std::f64::consts::TAU,
            _ => polyline_length(&self.sample(1e-5 * (1.0 + self.extent()))),
        }
    }

    /// The largest distance between the curve's control or sample points: a scale for
    /// tolerances.
    fn extent(&self) -> f64 {
        match self {
            Curve3::Nurbs { points, .. } | Curve3::Polyline { points } => {
                let (lo, hi) = bounds(points);
                lo.distance(hi)
            }
            _ => self.start().distance(self.end()) + self.arc_radius().unwrap_or(0.0) * 2.0,
        }
    }

    fn arc_radius(&self) -> Option<f64> {
        match self {
            Curve3::Arc { radius, .. } | Curve3::Circle { radius, .. } => Some(*radius),
            _ => None,
        }
    }

    /// The same curve run the other way.
    pub fn reversed(&self) -> Curve3 {
        match self {
            Curve3::Line { a, b } => Curve3::Line { a: *b, b: *a },
            Curve3::Arc { center, normal, x_axis, radius, start_deg, end_deg } => Curve3::Arc {
                center: *center,
                normal: -*normal,
                x_axis: *x_axis,
                radius: *radius,
                start_deg: -end_deg,
                end_deg: -start_deg,
            },
            Curve3::Circle { center, normal, radius } => {
                Curve3::Circle { center: *center, normal: -*normal, radius: *radius }
            }
            Curve3::Nurbs { degree, knots, points, weights } => {
                let (lo, hi) = (knots.first().copied().unwrap_or(0.0), knots.last().copied().unwrap_or(0.0));
                Curve3::Nurbs {
                    degree: *degree,
                    knots: knots.iter().rev().map(|k| lo + hi - k).collect(),
                    points: points.iter().rev().copied().collect(),
                    weights: weights.iter().rev().copied().collect(),
                }
            }
            Curve3::Polyline { points } => Curve3::Polyline { points: points.iter().rev().copied().collect() },
        }
    }

    /// A spline whose interior knots all have full multiplicity (its degree) is a chain of
    /// separate Bézier pieces, as lines-and-arcs fitting produces; this returns those
    /// pieces as curves of their own. Any other curve comes back whole.
    pub fn spans(&self) -> Vec<Curve3> {
        let Curve3::Nurbs { degree, knots, points, weights } = self else {
            return vec![self.clone()];
        };
        let p = *degree as usize;
        let n = points.len();
        if p == 0 || knots.len() != n + p + 1 || (n - 1) % p != 0 {
            return vec![self.clone()];
        }
        let pieces = (n - 1) / p;
        // Clamped ends, then each interior knot repeated exactly p times.
        let full = (0..pieces).all(|i| knots[i * p + 1..=i * p + p].iter().all(|k| *k == knots[i * p + 1]))
            && knots[..=p].iter().all(|k| *k == knots[0])
            && knots[n..].iter().all(|k| *k == knots[n]);
        if pieces < 2 || !full {
            return vec![self.clone()];
        }
        (0..pieces)
            .map(|i| {
                let (a, b) = (knots[i * p + 1], knots[(i + 1) * p + 1]);
                Curve3::Nurbs {
                    degree: *degree,
                    knots: std::iter::repeat_n(a, p + 1).chain(std::iter::repeat_n(b, p + 1)).collect(),
                    points: points[i * p..=(i + 1) * p].to_vec(),
                    weights: if weights.is_empty() { Vec::new() } else { weights[i * p..=(i + 1) * p].to_vec() },
                }
            })
            .collect()
    }

    /// The same curve as a line, arc or circle when it is one to within `tol`, otherwise
    /// unchanged. Splines from a kernel's exact geometry usually are: a B-spline line or
    /// a rational quadratic arc.
    pub fn simplified(&self, tol: f64) -> Curve3 {
        let Curve3::Nurbs { degree, knots, points, weights } = self else {
            return self.clone();
        };
        // Eight points per knot span: lines and arcs fit them to round-off, and a curve
        // that is neither misses at some of them by far more than `tol`.
        let p = *degree as usize;
        let mut params = Vec::new();
        for k in p..points.len().min(knots.len().saturating_sub(p + 1)) {
            let (a, b) = (knots[k], knots[k + 1]);
            if b > a {
                params.extend((0..8).map(|i| a + (b - a) * i as f64 / 8.0));
            }
        }
        let (_, hi) = nurbs_range(*degree, knots, points.len());
        params.push(hi);
        if params.len() < 3 {
            return self.clone();
        }
        let samples: Vec<Vec3> = params.iter().map(|&u| nurbs_point(p, knots, points, weights, u)).collect();
        fit_line(&samples, tol).or_else(|| fit_arc(&samples, tol)).unwrap_or_else(|| self.clone())
    }
}

/// The largest distance from a point of `from` to the nearest of `to`: how far a chain
/// of lines and arcs strays from the curve it replaces. Measured at samples of `from`
/// within `chord_tol` against `to` sampled ten times finer, so it is accurate to about
/// `chord_tol / 10`.
pub fn deviation(from: &Curve3, to: &[Curve3], chord_tol: f64) -> f64 {
    let targets: Vec<Vec<Vec3>> = to.iter().map(|c| c.sample(chord_tol * 0.1)).collect();
    from.sample(chord_tol)
        .into_iter()
        .map(|p| {
            targets
                .iter()
                .flat_map(|t| t.windows(2))
                .map(|w| {
                    let ab = w[1] - w[0];
                    let len2 = ab.dot(ab);
                    let t = if len2 > 0.0 { ((p - w[0]).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
                    p.distance(w[0] + ab * t)
                })
                .fold(f64::INFINITY, f64::min)
        })
        .fold(0.0, f64::max)
}

fn circle_point(center: Vec3, normal: Vec3, x_axis: Vec3, radius: f64, angle: f64) -> Vec3 {
    let y_axis = normal.cross(x_axis);
    center + (x_axis * angle.cos() + y_axis * angle.sin()) * radius
}

/// A unit vector perpendicular to `n`: the angle-zero direction of a [`Curve3::Circle`].
/// It is `Z x n` normalised, or `X` when `n` is along Z.
pub fn perpendicular(n: Vec3) -> Vec3 {
    Vec3::Z.cross(n).normalized().unwrap_or(Vec3::X)
}

fn arc_segments(radius: f64, sweep_deg: f64, chord_tol: f64) -> usize {
    let sweep = sweep_deg.abs().max(1e-9);
    let step = if chord_tol > 0.0 && chord_tol < radius {
        (2.0 * (1.0 - chord_tol / radius).acos()).to_degrees().min(DEFAULT_ANGLE_STEP_DEG * 3.0)
    } else {
        DEFAULT_ANGLE_STEP_DEG
    };
    ((sweep / step).ceil() as usize).clamp(1, 100_000)
}

fn polyline_length(points: &[Vec3]) -> f64 {
    points.windows(2).map(|w| w[0].distance(w[1])).sum()
}

fn bounds(points: &[Vec3]) -> (Vec3, Vec3) {
    let mut lo = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut hi = -lo;
    for p in points {
        lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
        hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
    }
    if points.is_empty() { (Vec3::ZERO, Vec3::ZERO) } else { (lo, hi) }
}

/// The parameter range of a spline: from knot `degree` to knot `n`.
fn nurbs_range(degree: u32, knots: &[f64], n: usize) -> (f64, f64) {
    let p = degree as usize;
    if knots.len() < n + p + 1 || n == 0 {
        return (0.0, 0.0);
    }
    (knots[p], knots[n])
}

/// De Boor's algorithm on homogeneous points.
fn nurbs_point(p: usize, knots: &[f64], points: &[Vec3], weights: &[f64], u: f64) -> Vec3 {
    let n = points.len();
    if n == 0 || knots.len() < n + p + 1 {
        return Vec3::ZERO;
    }
    // The knot span k with knots[k] <= u < knots[k + 1], clamped to the valid range.
    let mut k = p;
    while k + 1 < n && knots[k + 1] <= u {
        k += 1;
    }
    let w = |i: usize| if weights.is_empty() { 1.0 } else { weights[i] };
    let mut d: Vec<[f64; 4]> = (0..=p)
        .map(|j| {
            let i = k + j - p;
            let wi = w(i);
            [points[i].x * wi, points[i].y * wi, points[i].z * wi, wi]
        })
        .collect();
    for r in 1..=p {
        for j in (r..=p).rev() {
            let i = k + j - p;
            let denom = knots[i + p + 1 - r] - knots[i];
            let a = if denom.abs() < 1e-300 { 0.0 } else { (u - knots[i]) / denom };
            let prev = d[j - 1];
            for (v, p) in d[j].iter_mut().zip(prev) {
                *v = (1.0 - a) * p + a * *v;
            }
        }
    }
    let h = d[p];
    if h[3].abs() < 1e-300 { Vec3::new(h[0], h[1], h[2]) } else { Vec3::new(h[0] / h[3], h[1] / h[3], h[2] / h[3]) }
}

/// Samples a spline span by span, bisecting each span until its chords are within
/// `chord_tol` (or turn by less than the default angle step when `chord_tol` is zero).
fn sample_nurbs(p: usize, knots: &[f64], points: &[Vec3], weights: &[f64], chord_tol: f64) -> Vec<(f64, Vec3)> {
    let n = points.len();
    if n == 0 || knots.len() < n + p + 1 {
        return Vec::new();
    }
    let eval = |u: f64| nurbs_point(p, knots, points, weights, u);
    let mut out = vec![(knots[p], eval(knots[p]))];
    let mut spans: Vec<(f64, f64)> = Vec::new();
    for k in p..n {
        if knots[k + 1] > knots[k] {
            spans.push((knots[k], knots[k + 1]));
        }
    }
    let flat = |a: Vec3, m: Vec3, b: Vec3| {
        let chord = b - a;
        let len = chord.length();
        if len < 1e-12 {
            return m.distance(a) < 1e-12;
        }
        let dev = (m - a).cross(chord).length() / len;
        if chord_tol > 0.0 {
            dev <= chord_tol
        } else {
            // The turning angle between the two half chords.
            let (u, v) = ((m - a).normalized(), (b - m).normalized());
            match (u, v) {
                (Some(u), Some(v)) => u.dot(v) >= DEFAULT_ANGLE_STEP_DEG.to_radians().cos(),
                _ => true,
            }
        }
    };
    for (lo, hi) in spans {
        // An explicit stack of intervals, emitted left to right.
        let mut stack = vec![(lo, hi, 0u32)];
        while let Some((a, b, depth)) = stack.pop() {
            let (pa, pb) = (eval(a), eval(b));
            let quarter = [eval(a + (b - a) * 0.25), eval(a + (b - a) * 0.5), eval(a + (b - a) * 0.75)];
            // Curved spans are split at least once so an S-shaped span whose quarter
            // points happen to line up is not taken for a straight one.
            let min_depth = u32::from(p > 1);
            let ok = depth >= 20
                || (depth >= min_depth
                    && flat(pa, quarter[1], pb)
                    && flat(pa, quarter[0], quarter[1])
                    && flat(quarter[1], quarter[2], pb));
            if ok {
                out.push((b, pb));
            } else {
                let m = 0.5 * (a + b);
                stack.push((m, b, depth + 1));
                stack.push((a, m, depth + 1));
            }
        }
    }
    out
}

/// A spline in homogeneous form, for knot insertion.
struct Nurbs {
    p: usize,
    knots: Vec<f64>,
    /// x w, y w, z w, w.
    points: Vec<[f64; 4]>,
}

impl Nurbs {
    fn new(p: usize, knots: &[f64], points: &[Vec3], weights: &[f64]) -> Nurbs {
        let w = |i: usize| if weights.is_empty() { 1.0 } else { weights[i] };
        let points = points.iter().enumerate().map(|(i, q)| [q.x * w(i), q.y * w(i), q.z * w(i), w(i)]).collect();
        Nurbs { p, knots: knots.to_vec(), points }
    }

    fn into_curve(self, degree: u32, rational: bool) -> Curve3 {
        let points = self.points.iter().map(|h| Vec3::new(h[0] / h[3], h[1] / h[3], h[2] / h[3])).collect();
        let weights = if rational { self.points.iter().map(|h| h[3]).collect() } else { Vec::new() };
        Curve3::Nurbs { degree, knots: self.knots, points, weights }
    }

    /// Inserts knot `u` once (Boehm's algorithm).
    fn insert(&mut self, u: f64) {
        let (p, n) = (self.p, self.points.len());
        let mut k = p;
        while k + 1 < n && self.knots[k + 1] <= u {
            k += 1;
        }
        let mut q = Vec::with_capacity(n + 1);
        for i in 0..=n {
            q.push(if i + p <= k {
                self.points[i]
            } else if i > k {
                self.points[i - 1]
            } else {
                let a = (u - self.knots[i]) / (self.knots[i + p] - self.knots[i]);
                let (lo, hi) = (self.points[i - 1], self.points[i]);
                [0, 1, 2, 3].map(|c| (1.0 - a) * lo[c] + a * hi[c])
            });
        }
        self.points = q;
        self.knots.insert(k + 1, u);
    }

    /// The curve before and after `u`, which must be inside the parameter range.
    fn split(mut self, u: f64) -> (Nurbs, Nurbs) {
        let p = self.p;
        while self.knots.iter().filter(|k| **k == u).count() < p {
            self.insert(u);
        }
        // With u of multiplicity p at knots[m..m + p], control point m - 1 is the curve
        // point at u and ends one piece and starts the other.
        let m = self.knots.iter().position(|k| *k == u).expect("inserted knot");
        let mut left_knots = self.knots[..m + p].to_vec();
        left_knots.push(u);
        let mut right_knots = vec![u];
        right_knots.extend_from_slice(&self.knots[m..]);
        let left = Nurbs { p, knots: left_knots, points: self.points[..m].to_vec() };
        let right = Nurbs { p, knots: right_knots, points: self.points[m - 1..].to_vec() };
        (left, right)
    }
}

/// A line through the samples' ends when every sample is within `tol` of it.
fn fit_line(samples: &[Vec3], tol: f64) -> Option<Curve3> {
    let (a, b) = (*samples.first()?, *samples.last()?);
    let dir = (b - a).normalized()?;
    let on_line = samples.iter().all(|p| {
        let rel = *p - a;
        let t = rel.dot(dir);
        (rel - dir * t).length() <= tol && t >= -tol && t <= a.distance(b) + tol
    });
    on_line.then_some(Curve3::Line { a, b })
}

/// The circle through three points, as (centre, unit normal, radius).
fn circle_through(a: Vec3, b: Vec3, c: Vec3) -> Option<(Vec3, Vec3, f64)> {
    let (ab, ac) = (b - a, c - a);
    let n = ab.cross(ac);
    let n2 = n.dot(n);
    if n2 < 1e-24 {
        return None;
    }
    let to_center = (n.cross(ab) * ac.dot(ac) + ac.cross(n) * ab.dot(ab)) * (1.0 / (2.0 * n2));
    let center = a + to_center;
    Some((center, n.normalized()?, to_center.length()))
}

/// An arc (or circle) through the samples when every sample is within `tol` of it.
fn fit_arc(samples: &[Vec3], tol: f64) -> Option<Curve3> {
    let n = samples.len();
    let (a, b) = (samples[0], samples[n - 1]);
    let closed = a.distance(b) <= tol;
    let (m1, m2) = if closed { (samples[n / 3], samples[2 * n / 3]) } else { (samples[n / 2], b) };
    let (center, normal, radius) = circle_through(a, m1, m2)?;
    for p in samples {
        let rel = *p - center;
        if (rel.length() - radius).abs() > tol || rel.dot(normal).abs() > tol {
            return None;
        }
    }
    // Orient the normal so the samples run counter-clockwise about it.
    let x_axis = (a - center).normalized()?;
    let y_axis = normal.cross(x_axis);
    let angle = |p: Vec3| {
        let rel = p - center;
        rel.dot(y_axis).atan2(rel.dot(x_axis))
    };
    let normal = if angle(samples[1.max(n / 8).min(n - 1)]) < 0.0 { -normal } else { normal };
    if closed {
        return Some(Curve3::Circle { center, normal, radius });
    }
    let y_axis = normal.cross(x_axis);
    let mut sweep = {
        let rel = b - center;
        rel.dot(y_axis).atan2(rel.dot(x_axis)).to_degrees()
    };
    if sweep <= 0.0 {
        sweep += 360.0;
    }
    Some(Curve3::Arc { center, normal, x_axis, radius, start_deg: 0.0, end_deg: sweep })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quarter_arc() -> Curve3 {
        // A rational quadratic quarter circle of radius 2 in the XY plane, from (2,0,0) to (0,2,0).
        let w = std::f64::consts::FRAC_1_SQRT_2;
        Curve3::Nurbs {
            degree: 2,
            knots: vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            points: vec![Vec3::new(2.0, 0.0, 0.0), Vec3::new(2.0, 2.0, 0.0), Vec3::new(0.0, 2.0, 0.0)],
            weights: vec![1.0, w, 1.0],
        }
    }

    #[test]
    fn nurbs_quarter_circle_lies_on_the_circle() {
        let c = quarter_arc();
        for p in c.sample(1e-4) {
            assert!((p.length() - 2.0).abs() < 1e-9, "{p:?}");
        }
        assert!(c.start().distance(Vec3::new(2.0, 0.0, 0.0)) < 1e-12);
        assert!(c.end().distance(Vec3::new(0.0, 2.0, 0.0)) < 1e-12);
        assert!((c.length() - std::f64::consts::PI).abs() < 1e-4);
    }

    #[test]
    fn simplifies_splines_to_lines_and_arcs() {
        match quarter_arc().simplified(1e-7) {
            Curve3::Arc { center, normal, radius, start_deg, end_deg, .. } => {
                assert!(center.length() < 1e-9 && (radius - 2.0).abs() < 1e-9);
                assert!(normal.distance(Vec3::Z) < 1e-9, "{normal:?}");
                assert!((end_deg - start_deg - 90.0).abs() < 1e-7);
            }
            other => panic!("{other:?}"),
        }
        let line = Curve3::Nurbs {
            degree: 1,
            knots: vec![0.0, 0.0, 1.0, 1.0],
            points: vec![Vec3::ZERO, Vec3::new(3.0, 4.0, 0.0)],
            weights: vec![],
        };
        assert_eq!(line.simplified(1e-9), Curve3::Line { a: Vec3::ZERO, b: Vec3::new(3.0, 4.0, 0.0) });
    }

    #[test]
    fn splits_chains_of_bezier_pieces() {
        // Two quarter circles joined at a double knot.
        let w = std::f64::consts::FRAC_1_SQRT_2;
        let half = Curve3::Nurbs {
            degree: 2,
            knots: vec![0.0, 0.0, 0.0, 0.5, 0.5, 1.0, 1.0, 1.0],
            points: vec![
                Vec3::new(2.0, 0.0, 0.0),
                Vec3::new(2.0, 2.0, 0.0),
                Vec3::new(0.0, 2.0, 0.0),
                Vec3::new(-2.0, 2.0, 0.0),
                Vec3::new(-2.0, 0.0, 0.0),
            ],
            weights: vec![1.0, w, 1.0, w, 1.0],
        };
        let spans = half.spans();
        assert_eq!(spans.len(), 2);
        assert!(spans[0].end().distance(Vec3::new(0.0, 2.0, 0.0)) < 1e-12);
        assert!(spans.iter().all(|s| matches!(s.simplified(1e-9), Curve3::Arc { .. })), "{spans:?}");
        assert_eq!(quarter_arc().spans(), vec![quarter_arc()]);
    }

    #[test]
    fn between_cuts_every_kind_exactly() {
        let cubic = Curve3::Nurbs {
            degree: 3,
            knots: vec![0.0, 0.0, 0.0, 0.0, 0.4, 1.0, 1.0, 1.0, 1.0],
            points: vec![
                Vec3::ZERO,
                Vec3::new(1.0, 2.0, 0.0),
                Vec3::new(3.0, -1.0, 1.0),
                Vec3::new(4.0, 2.0, 0.0),
                Vec3::new(6.0, 0.0, 0.0),
            ],
            weights: vec![1.0, 0.8, 1.3, 1.0, 1.0],
        };
        let arc = Curve3::Arc {
            center: Vec3::ZERO,
            normal: Vec3::Z,
            x_axis: Vec3::X,
            radius: 2.0,
            start_deg: 10.0,
            end_deg: 200.0,
        };
        let circle = Curve3::Circle { center: Vec3::ZERO, normal: Vec3::Y, radius: 1.0 };
        let line = Curve3::Line { a: Vec3::ZERO, b: Vec3::new(1.0, 1.0, 1.0) };
        let poly = Curve3::Polyline { points: vec![Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0)] };
        for c in [cubic, arc, circle, line, poly, quarter_arc()] {
            let part = c.between(0.25, 0.7);
            assert!(part.start().distance(c.point_at(0.25)) < 1e-12, "{c:?}");
            assert!(part.end().distance(c.point_at(0.7)) < 1e-12, "{c:?}");
            // Every point of the part lies on the whole curve.
            let whole = c.sample(1e-6);
            for q in part.sample(1e-3) {
                let d = whole.windows(2).map(|w| dist_to_segment(q, w[0], w[1])).fold(f64::INFINITY, f64::min);
                assert!(d < 1e-5, "{c:?}: {q:?} is {d} off");
            }
        }
    }

    fn dist_to_segment(p: Vec3, a: Vec3, b: Vec3) -> f64 {
        let ab = b - a;
        let t = if ab.dot(ab) > 0.0 { ((p - a).dot(ab) / ab.dot(ab)).clamp(0.0, 1.0) } else { 0.0 };
        p.distance(a + ab * t)
    }

    #[test]
    fn reversing_swaps_the_ends() {
        let arc = Curve3::Arc {
            center: Vec3::ZERO,
            normal: Vec3::Z,
            x_axis: Vec3::X,
            radius: 1.0,
            start_deg: 10.0,
            end_deg: 100.0,
        };
        for c in [arc, quarter_arc(), Curve3::Line { a: Vec3::ZERO, b: Vec3::X }] {
            let r = c.reversed();
            assert!(r.start().distance(c.end()) < 1e-12 && r.end().distance(c.start()) < 1e-12, "{c:?}");
            assert!(r.point_at(0.3).distance(c.point_at(0.7)) < 1e-9, "{c:?}");
        }
    }

    #[test]
    fn arcs_run_counter_clockwise_about_their_normal() {
        let arc = Curve3::Arc {
            center: Vec3::ZERO,
            normal: Vec3::Z,
            x_axis: Vec3::X,
            radius: 1.0,
            start_deg: 0.0,
            end_deg: 90.0,
        };
        assert!(arc.point_at(0.5).distance(Vec3::new(0.5f64.sqrt(), 0.5f64.sqrt(), 0.0)) < 1e-12);
        assert!(arc.end().distance(Vec3::Y) < 1e-12);
        let pts = arc.sample(1e-3);
        assert!(pts.windows(2).all(|w| w[0].cross(w[1]).z > 0.0));
    }

    #[test]
    fn standard_frames_follow_the_z_up_camera() {
        let f = ViewSpec::standard(StandardView::Front).frame().unwrap();
        assert_eq!((f.right, f.up), (Vec3::X, Vec3::Z));
        let t = ViewSpec::standard(StandardView::Top).frame().unwrap();
        assert_eq!((t.right, t.up), (Vec3::X, Vec3::Y));
        let r = ViewSpec::standard(StandardView::Right).frame().unwrap();
        assert_eq!((r.right, r.up), (Vec3::Y, Vec3::Z));
        let iso = ViewSpec::standard(StandardView::Isometric).frame().unwrap();
        assert!(iso.right.distance(Vec3::new(1.0, 1.0, 0.0) * std::f64::consts::FRAC_1_SQRT_2) < 1e-12);
        assert!(iso.up.z > 0.0);
        assert!(ViewSpec::new(Vec3::Z, Vec3::Z).frame().is_none());
        for v in StandardView::ALL {
            assert_eq!(StandardView::from_name(v.name()), Some(v));
        }
    }

    #[test]
    fn view_curves_serialise_with_tagged_sources() {
        let vc =
            ViewCurve::new(Curve3::Line { a: Vec3::ZERO, b: Vec3::X }, CurveSource::Edge(EdgeId::from_raw(7)), true);
        let json = serde_json::to_string(&vc).unwrap();
        assert_eq!(
            json,
            r#"{"curve":{"type":"line","a":[0.0,0.0,0.0],"b":[1.0,0.0,0.0]},"source":{"kind":"edge","id":7},"visible":true}"#
        );
        assert_eq!(serde_json::from_str::<ViewCurve>(&json).unwrap(), vc);
        let every = [
            Curve3::Line { a: Vec3::ZERO, b: Vec3::X },
            Curve3::Arc {
                center: Vec3::ZERO,
                normal: Vec3::Z,
                x_axis: Vec3::X,
                radius: 1.0,
                start_deg: 0.0,
                end_deg: 90.0,
            },
            Curve3::Circle { center: Vec3::ZERO, normal: Vec3::Z, radius: 1.0 },
            quarter_arc(),
            Curve3::Polyline { points: vec![Vec3::ZERO, Vec3::X, Vec3::Y] },
        ];
        for c in every {
            let json = serde_json::to_string(&c).unwrap();
            assert_eq!(serde_json::from_str::<Curve3>(&json).unwrap(), c, "{json}");
        }
        let unknown = serde_json::to_string(&CurveSource::Unknown).unwrap();
        assert_eq!(serde_json::from_str::<CurveSource>(&unknown).unwrap(), CurveSource::Unknown);
    }
}
