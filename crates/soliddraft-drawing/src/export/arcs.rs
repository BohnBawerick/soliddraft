//! Paths as lines, circular arcs and circles, for formats and shops that want those and
//! not splines.
//!
//! A cubic that is a circular arc within the tolerance becomes that arc; consecutive arcs
//! on the same circle merge, and one that closes on itself becomes a circle. Any other
//! cubic (an ellipse from a circle seen at an angle, a spline) is split until each piece
//! is an arc within the tolerance, so the result never needs a spline.

use kurbo::{BezPath, CubicBez, ParamCurve, PathSeg, Point, Vec2};

/// One piece of a path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Piece {
    /// A straight segment.
    Line(Point, Point),
    /// A circular arc running counter-clockwise from `start_deg` to `end_deg`, with
    /// `end_deg > start_deg`.
    Arc {
        /// Centre.
        center: Point,
        /// Radius.
        radius: f64,
        /// Start angle in degrees.
        start_deg: f64,
        /// End angle in degrees.
        end_deg: f64,
    },
    /// A full circle.
    Circle {
        /// Centre.
        center: Point,
        /// Radius.
        radius: f64,
    },
}

/// Splitting depth limit: 2^12 pieces per cubic at most.
const MAX_DEPTH: u32 = 12;

/// `path` as lines, arcs and circles within `tol` of it.
pub fn lines_and_arcs(path: &BezPath, tol: f64) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    let mut run: Vec<RawArc> = Vec::new();
    let flush = |run: &mut Vec<RawArc>, out: &mut Vec<Piece>| {
        if !run.is_empty() {
            out.extend(merge(run, tol));
            run.clear();
        }
    };
    for seg in path.segments() {
        match seg {
            PathSeg::Line(l) => {
                flush(&mut run, &mut out);
                if l.p0.distance(l.p1) > 0.0 {
                    out.push(Piece::Line(l.p0, l.p1));
                }
            }
            PathSeg::Quad(q) => cubic(q.raise(), tol, 0, &mut run, &mut out),
            PathSeg::Cubic(c) => cubic(c, tol, 0, &mut run, &mut out),
        }
    }
    flush(&mut run, &mut out);
    out
}

/// An arc piece before merging: centre, radius, start and end points, and whether it
/// turns counter-clockwise.
#[derive(Clone, Copy, Debug)]
struct RawArc {
    center: Point,
    radius: f64,
    from: Point,
    to: Point,
    ccw: bool,
    sweep: f64,
}

fn cubic(c: CubicBez, tol: f64, depth: u32, run: &mut Vec<RawArc>, out: &mut Vec<Piece>) {
    // Straight within the tolerance: a line.
    let chord = c.p3 - c.p0;
    let len = chord.hypot();
    let off = |p: Point| if len > 0.0 { (p - c.p0).cross(chord).abs() / len } else { p.distance(c.p0) };
    if off(c.p1) <= tol && off(c.p2) <= tol && len > 0.0 {
        if !run.is_empty() {
            out.extend(merge(run, tol));
            run.clear();
        }
        out.push(Piece::Line(c.p0, c.p3));
        return;
    }
    if let Some(a) = fit_arc(&c, tol) {
        run.push(a);
        return;
    }
    if depth >= MAX_DEPTH {
        if !run.is_empty() {
            out.extend(merge(run, tol));
            run.clear();
        }
        out.push(Piece::Line(c.p0, c.p3));
        return;
    }
    let (a, b) = c.subdivide();
    cubic(a, tol, depth + 1, run, out);
    cubic(b, tol, depth + 1, run, out);
}

/// The circle through the cubic's ends and middle, when the cubic stays within `tol` of
/// it and turns less than half a circle.
fn fit_arc(c: &CubicBez, tol: f64) -> Option<RawArc> {
    let (a, m, b) = (c.p0, c.eval(0.5), c.p3);
    let center = circumcentre(a, m, b)?;
    let radius = center.distance(a);
    for i in 1..8 {
        let p = c.eval(f64::from(i) / 8.0);
        if (p.distance(center) - radius).abs() > tol {
            return None;
        }
    }
    let ccw = (m - a).cross(b - m) > 0.0;
    let angle = |p: Point| (p.y - center.y).atan2(p.x - center.x);
    let mut sweep = angle(b) - angle(a);
    if ccw && sweep <= 0.0 {
        sweep += std::f64::consts::TAU;
    }
    if !ccw && sweep >= 0.0 {
        sweep -= std::f64::consts::TAU;
    }
    if sweep.abs() > std::f64::consts::PI + 1e-9 {
        return None;
    }
    Some(RawArc { center, radius, from: a, to: b, ccw, sweep })
}

fn circumcentre(a: Point, b: Point, c: Point) -> Option<Point> {
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    let scale = (b - a).hypot().max((c - a).hypot()).max(1e-12);
    if d.abs() < 1e-12 * scale * scale {
        return None;
    }
    let (a2, b2, c2) = (a.to_vec2().hypot2(), b.to_vec2().hypot2(), c.to_vec2().hypot2());
    let x = (a2 * (b.y - c.y) + b2 * (c.y - a.y) + c2 * (a.y - b.y)) / d;
    let y = (a2 * (c.x - b.x) + b2 * (a.x - c.x) + c2 * (b.x - a.x)) / d;
    Some(Point::new(x, y))
}

/// Joins consecutive arcs on one circle, turning the same way, into one arc, and a run
/// that closes a full turn into a circle.
fn merge(run: &[RawArc], tol: f64) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < run.len() {
        let first = run[i];
        let mut last = first;
        let mut sweep = first.sweep;
        let mut j = i + 1;
        while j < run.len() {
            let n = run[j];
            if n.ccw == first.ccw
                && n.center.distance(first.center) <= tol
                && (n.radius - first.radius).abs() <= tol
                && n.from.distance(last.to) <= tol
                && (sweep + n.sweep).abs() <= std::f64::consts::TAU + 1e-3
            {
                sweep += n.sweep;
                last = n;
                j += 1;
            } else {
                break;
            }
        }
        // Back at its start after more than half a turn: a full circle, whatever the
        // round-off in the summed sweeps.
        let closes = last.to.distance(first.from) <= tol && sweep.abs() > std::f64::consts::PI;
        if closes {
            out.push(Piece::Circle { center: first.center, radius: first.radius });
        } else {
            let deg = |p: Point| (p.y - first.center.y).atan2(p.x - first.center.x).to_degrees();
            // Counter-clockwise from the start; a clockwise arc runs backwards.
            let (s, e) = if first.ccw { (deg(first.from), deg(last.to)) } else { (deg(last.to), deg(first.from)) };
            let mut end = e;
            while end <= s {
                end += 360.0;
            }
            out.push(Piece::Arc { center: first.center, radius: first.radius, start_deg: s, end_deg: end });
        }
        i = j;
    }
    out
}

/// The point of an arc at an angle in degrees.
pub fn arc_point(center: Point, radius: f64, deg: f64) -> Point {
    let t = deg.to_radians();
    center + Vec2::new(t.cos(), t.sin()) * radius
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{Circle, Ellipse, Shape};

    #[test]
    fn circles_come_back_as_circles() {
        let path = Circle::new((10.0, 20.0), 5.0).to_path(1e-4);
        let pieces = lines_and_arcs(&path, 1e-3);
        assert_eq!(pieces.len(), 1, "{pieces:?}");
        let Piece::Circle { center, radius } = pieces[0] else { panic!("{pieces:?}") };
        assert!(center.distance(Point::new(10.0, 20.0)) < 1e-3 && (radius - 5.0).abs() < 1e-3);
    }

    #[test]
    fn quarter_arcs_merge_into_one_arc_and_lines_stay_lines() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 0.0));
        let arc = kurbo::Arc::new((10.0, 5.0), (5.0, 5.0), -std::f64::consts::FRAC_PI_2, std::f64::consts::PI, 0.0);
        arc.to_cubic_beziers(1e-4, |a, b, c| p.curve_to(a, b, c));
        let pieces = lines_and_arcs(&p, 1e-3);
        assert_eq!(pieces.len(), 2, "{pieces:?}");
        assert!(matches!(pieces[0], Piece::Line(..)));
        let Piece::Arc { center, radius, start_deg, end_deg } = pieces[1] else { panic!("{pieces:?}") };
        assert!(center.distance(Point::new(10.0, 5.0)) < 1e-3 && (radius - 5.0).abs() < 1e-3);
        assert!((start_deg + 90.0).abs() < 0.1 && (end_deg - 90.0).abs() < 0.1, "{start_deg} {end_deg}");
    }

    #[test]
    fn ellipses_become_arcs_within_tolerance() {
        let path = Ellipse::new((0.0, 0.0), (20.0, 8.0), 0.3).to_path(1e-5);
        let tol = 0.01;
        let pieces = lines_and_arcs(&path, tol);
        assert!(pieces.len() > 4 && pieces.iter().all(|p| matches!(p, Piece::Arc { .. })), "{pieces:?}");
        // Every arc's middle lies on the ellipse within the tolerance.
        let (sin, cos) = 0.3f64.sin_cos();
        for p in pieces {
            if let Piece::Arc { center, radius, start_deg, end_deg } = p {
                let m = arc_point(center, radius, (start_deg + end_deg) / 2.0);
                let (x, y) = (m.x * cos + m.y * sin, -m.x * sin + m.y * cos);
                let k = ((x / 20.0).powi(2) + (y / 8.0).powi(2)).sqrt();
                assert!((k - 1.0).abs() * 8.0 < tol * 2.0, "{m:?}");
            }
        }
    }
}
