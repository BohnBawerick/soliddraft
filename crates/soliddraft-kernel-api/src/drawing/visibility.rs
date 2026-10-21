//! Re-deciding visibility where curves cross in a view.
//!
//! An exact hidden-line pass splits each curve where it passes behind the outline and
//! decides each piece from one test point. When it misses a crossing, a whole piece takes
//! the visibility of its test point. [`refine_visibility`] finds every place where one
//! curve's projection crosses another's, cuts the curves there, asks the backend about
//! the cut pieces, and joins neighbouring pieces that agree again. The cuts are exact:
//! crossings are polished with Newton steps on the exact curves.

use crate::KernelResult;
use crate::drawing::{ViewCurve, ViewFrame};
use crate::{Point2, Vec3};

/// Newton steps used to polish each crossing.
const NEWTON_STEPS: usize = 8;

/// A curve sampled in sheet coordinates.
struct Flat {
    /// (parameter, sheet point).
    samples: Vec<(f64, Point2)>,
    lo: Point2,
    hi: Point2,
}

fn flat(c: &ViewCurve, frame: &ViewFrame, tol: f64) -> Flat {
    let samples: Vec<(f64, Point2)> =
        c.curve.sample_params(tol).into_iter().map(|(t, p)| (t, frame.project(p))).collect();
    let mut lo = Point2::new(f64::INFINITY, f64::INFINITY);
    let mut hi = Point2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for (_, q) in &samples {
        lo = Point2::new(lo.u.min(q.u), lo.v.min(q.v));
        hi = Point2::new(hi.u.max(q.u), hi.v.max(q.v));
    }
    Flat { samples, lo, hi }
}

fn cross(a: Point2, b: Point2) -> f64 {
    a.u * b.v - a.v * b.u
}

fn sub(a: Point2, b: Point2) -> Point2 {
    Point2::new(a.u - b.u, a.v - b.v)
}

/// Where segment a0-a1 crosses segment b0-b1, as fractions along each, or `None` for
/// parallel or disjoint segments. Both ends of each segment count, so a crossing at a
/// shared sample is found from either side.
fn segment_crossing(a0: Point2, a1: Point2, b0: Point2, b1: Point2) -> Option<(f64, f64)> {
    let (da, db) = (sub(a1, a0), sub(b1, b0));
    let denom = cross(da, db);
    let scale = (da.u.hypot(da.v)) * (db.u.hypot(db.v));
    if denom.abs() <= 1e-12 * scale || scale == 0.0 {
        return None;
    }
    let w = sub(b0, a0);
    let (s, r) = (cross(w, db) / denom, cross(w, da) / denom);
    let eps = 1e-9;
    ((-eps..=1.0 + eps).contains(&s) && (-eps..=1.0 + eps).contains(&r)).then_some((s, r))
}

/// Polishes a crossing of the projections of `a` and `b` near parameters (ta, tb) with
/// Newton steps on the exact curves. `None` when it does not converge, as for curves
/// that only touch.
fn polish(a: &ViewCurve, b: &ViewCurve, frame: &ViewFrame, mut ta: f64, mut tb: f64, tol: f64) -> Option<(f64, f64)> {
    let pa = |t: f64| frame.project(a.curve.point_at(t));
    let pb = |t: f64| frame.project(b.curve.point_at(t));
    let h = 1e-7;
    for _ in 0..NEWTON_STEPS {
        let f = sub(pa(ta), pb(tb));
        if f.u.hypot(f.v) <= 1e-12 * (1.0 + tol) {
            break;
        }
        // One-sided differences that stay inside [0, 1].
        let step = |t: f64| if t + h <= 1.0 { h } else { -h };
        let (ha, hb) = (step(ta), step(tb));
        let ja = sub(pa(ta + ha), pa(ta));
        let jb = sub(pb(tb + hb), pb(tb));
        let (ja, jb) = (Point2::new(ja.u / ha, ja.v / ha), Point2::new(-jb.u / hb, -jb.v / hb));
        let det = cross(ja, jb);
        if det.abs() < 1e-300 {
            return None;
        }
        // Solve [ja jb] [dta dtb]^T = -f.
        let dta = -cross(f, jb) / det;
        let dtb = -cross(ja, f) / det;
        ta = (ta + dta).clamp(0.0, 1.0);
        tb = (tb + dtb).clamp(0.0, 1.0);
    }
    let f = sub(pa(ta), pb(tb));
    (f.u.hypot(f.v) <= tol * 1e-3).then_some((ta, tb))
}

/// For each curve, the parameters strictly inside it where its projection crosses the
/// projection of another curve, sorted and without near duplicates. `tol` is the sheet
/// accuracy of the search, in millimetres.
pub fn projected_crossings(curves: &[ViewCurve], frame: &ViewFrame, tol: f64) -> Vec<Vec<f64>> {
    let flats: Vec<Flat> = curves.iter().map(|c| flat(c, frame, tol)).collect();
    let mut cuts: Vec<Vec<f64>> = vec![Vec::new(); curves.len()];
    for i in 0..curves.len() {
        for j in i + 1..curves.len() {
            let (a, b) = (&flats[i], &flats[j]);
            if a.lo.u > b.hi.u + tol || b.lo.u > a.hi.u + tol || a.lo.v > b.hi.v + tol || b.lo.v > a.hi.v + tol {
                continue;
            }
            for sa in a.samples.windows(2) {
                for sb in b.samples.windows(2) {
                    let Some((s, r)) = segment_crossing(sa[0].1, sa[1].1, sb[0].1, sb[1].1) else { continue };
                    let ta = sa[0].0 + (sa[1].0 - sa[0].0) * s;
                    let tb = sb[0].0 + (sb[1].0 - sb[0].0) * r;
                    if let Some((ta, tb)) = polish(&curves[i], &curves[j], frame, ta, tb, tol) {
                        cuts[i].push(ta);
                        cuts[j].push(tb);
                    }
                }
            }
        }
    }
    for (c, list) in curves.iter().zip(&mut cuts) {
        // A cut within round-off of an end, or of another cut, cuts nothing.
        let (start, end) = (c.curve.start(), c.curve.end());
        let near = |t: f64, p: Vec3| c.curve.point_at(t).distance(p) <= 1e-9 * (1.0 + tol);
        list.retain(|&t| t > 1e-9 && t < 1.0 - 1e-9 && !near(t, start) && !near(t, end));
        list.sort_by(f64::total_cmp);
        list.dedup_by(|x, y| (*x - *y).abs() <= 1e-9 || c.curve.point_at(*x).distance(c.curve.point_at(*y)) <= 1e-9);
    }
    cuts
}

/// Cuts `curves` wherever their projections cross, asks `hidden` about the middle of
/// every piece of every cut curve (it gets the points and answers, in order, whether the
/// body hides each), and joins neighbouring pieces of one curve that agree. Uncut curves
/// keep the visibility they came with.
pub fn refine_visibility(
    curves: Vec<ViewCurve>,
    frame: &ViewFrame,
    tol: f64,
    hidden: impl FnOnce(&[Vec3]) -> KernelResult<Vec<bool>>,
) -> KernelResult<Vec<ViewCurve>> {
    let cuts = projected_crossings(&curves, frame, tol);
    let mut probes = Vec::new();
    let mut breaks: Vec<Vec<f64>> = Vec::with_capacity(curves.len());
    for (c, list) in curves.iter().zip(&cuts) {
        let mut b = vec![0.0];
        b.extend_from_slice(list);
        b.push(1.0);
        if b.len() > 2 {
            probes.extend(b.windows(2).map(|w| c.curve.point_at(0.5 * (w[0] + w[1]))));
        }
        breaks.push(b);
    }
    if probes.is_empty() {
        return Ok(curves);
    }
    let answers = hidden(&probes)?;
    if answers.len() != probes.len() {
        return Err(crate::KernelError::QueryFailed {
            query: "hidden points".into(),
            code: -1,
            message: format!("{} answers for {} points", answers.len(), probes.len()),
        });
    }
    let mut answers = answers.into_iter();
    let mut out = Vec::with_capacity(curves.len());
    for (c, b) in curves.into_iter().zip(breaks) {
        if b.len() == 2 {
            out.push(c);
            continue;
        }
        // Runs of pieces with the same visibility, as (start, end, visible).
        let mut runs: Vec<(f64, f64, bool)> = Vec::new();
        for w in b.windows(2) {
            let visible = !answers.next().unwrap_or(false);
            match runs.last_mut() {
                Some(last) if last.2 == visible => last.1 = w[1],
                _ => runs.push((w[0], w[1], visible)),
            }
        }
        for (t0, t1, visible) in runs {
            out.push(ViewCurve { curve: c.curve.between(t0, t1), source: c.source, visible, smooth: c.smooth });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawing::{Curve3, CurveSource, StandardView, ViewSpec};

    fn line(a: [f64; 3], b: [f64; 3], visible: bool) -> ViewCurve {
        ViewCurve::new(Curve3::Line { a: a.into(), b: b.into() }, CurveSource::Unknown, visible)
    }

    #[test]
    fn finds_crossings_in_projection_only() {
        let top = ViewSpec::standard(StandardView::Top).frame().unwrap();
        // Two lines that cross seen from the top but are 5 apart in Z, and a third that
        // only touches the first at its end.
        let curves = vec![
            line([0.0, 0.0, 0.0], [4.0, 0.0, 0.0], true),
            line([1.0, -1.0, 5.0], [1.0, 1.0, 5.0], true),
            line([4.0, 0.0, 1.0], [4.0, 3.0, 1.0], true),
        ];
        let cuts = projected_crossings(&curves, &top, 1e-3);
        assert_eq!(cuts[0].len(), 1);
        assert!((cuts[0][0] - 0.25).abs() < 1e-12);
        assert_eq!(cuts[1], vec![0.5]);
        assert!(cuts[2].is_empty(), "a touch at an end cuts nothing");
    }

    #[test]
    fn re_decides_and_joins_pieces() {
        let top = ViewSpec::standard(StandardView::Top).frame().unwrap();
        let curves = vec![
            line([0.0, 0.0, 0.0], [4.0, 0.0, 0.0], true),
            line([1.0, -1.0, 5.0], [1.0, 1.0, 5.0], true),
            line([3.0, -1.0, 5.0], [3.0, 1.0, 5.0], true),
        ];
        // Pretend the body hides the first line between x = 1 and x = 3.
        let refined = refine_visibility(curves, &top, 1e-3, |points| {
            Ok(points.iter().map(|p| p.z < 1.0 && p.x > 1.0 && p.x < 3.0).collect())
        })
        .unwrap();
        let first: Vec<(bool, f64)> =
            refined.iter().filter(|c| c.curve.start().z == 0.0).map(|c| (c.visible, c.curve.length())).collect();
        assert_eq!(first, vec![(true, 1.0), (false, 2.0), (true, 1.0)]);
        // The two crossing lines are cut in the middle but both halves are visible, so
        // they are joined again.
        assert_eq!(refined.len(), 5);
    }
}
