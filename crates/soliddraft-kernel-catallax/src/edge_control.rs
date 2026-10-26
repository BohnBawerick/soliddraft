//! The maximum edge length control: cache identity and float conversion accounting.
//!
//! Catallax bounds every triangle edge and boundary polyline segment in binary64.
//! The trait returns f32 positions, and rounding each endpoint can lengthen a segment.
//! The native request therefore asks for the limit less that growth, and the returned
//! f32 positions are checked against the requested limit before they are handed out.

use catallax_mesh::{Mesh, MeshOptions};
use catallax_numbers::Rational;
use soliddraft_kernel_api::{KernelError, KernelResult};

fn refusal() -> KernelError {
    KernelError::Unsupported("requested maximum edge length cannot survive f32 display rounding".into())
}

/// Native options for a body whose coordinates have magnitude at most `extent`. A
/// requested edge length is reduced by the f32 rounding growth; the cache stays keyed
/// by the request, so the reduced limit never stands in for another request.
pub(crate) fn native_options(options: &MeshOptions, extent: f64) -> KernelResult<MeshOptions> {
    let mut native = options.clone();
    if let Some(edge) = &options.max_edge_length {
        let reduced = edge - &rounding_growth(extent)?;
        if reduced <= Rational::zero() {
            return Err(refusal());
        }
        native.max_edge_length = Some(reduced);
    }
    Ok(native)
}

/// Checks the f32 positions of every triangle edge and edge polyline segment against
/// the requested limit. The native bound covers binary64 positions only.
pub(crate) fn check(options: &MeshOptions, mesh: &Mesh) -> KernelResult<()> {
    let triangles = mesh.triangles.iter().flat_map(|t| {
        let [a, b, c] = t.vertices;
        [(a, b), (b, c), (c, a)]
    });
    let polylines = mesh.edges.iter().flat_map(|e| e.vertices.windows(2).map(|w| (w[0], w[1])));
    check_segments(options.max_edge_length.as_ref(), &mesh.positions, triangles.chain(polylines))
}

fn check_segments(
    limit: Option<&Rational>,
    positions: &[[f64; 3]],
    segments: impl Iterator<Item = (usize, usize)>,
) -> KernelResult<()> {
    let Some(limit) = limit else { return Ok(()) };
    // A coordinate beyond the f32 range rounds to infinity and cannot be displayed.
    let rounded = |i: usize| positions[i].map(|v| f64::from(v as f32));
    for (a, b) in segments {
        let (a, b) = (rounded(a), rounded(b));
        if !a.iter().chain(&b).all(|v| v.is_finite()) || exceeds(a, b, limit) {
            return Err(refusal());
        }
    }
    Ok(())
}

/// An upper bound on how much f32 rounding can lengthen a segment between points of
/// magnitude at most `extent`. Each coordinate moves by at most `2^-24` of its size,
/// so an endpoint moves by at most `sqrt(3) * extent * 2^-24` and a segment by twice
/// that, which `extent * 2^-22` covers.
fn rounding_growth(extent: f64) -> KernelResult<Rational> {
    Rational::from_f64_exact(extent.abs() * 2f64.powi(-22))
        .ok_or_else(|| KernelError::Unsupported("display coordinates are not finite".into()))
}

/// Whether the segment is longer than `limit`. A binary64 screen settles clear cases;
/// the exact squared length decides the rest.
fn exceeds(a: [f64; 3], b: [f64; 3], limit: &Rational) -> bool {
    let squared: f64 = (0..3).map(|i| (a[i] - b[i]).powi(2)).sum();
    let bound = limit.to_f64_nearest().powi(2);
    if squared < bound * (1.0 - 1e-9) {
        return false;
    }
    if squared > bound * (1.0 + 1e-9) {
        return true;
    }
    // The caller has checked finiteness; an unconvertible value still counts as too long.
    let exact = |v: f64| Rational::from_f64_exact(v);
    let mut squared = Rational::zero();
    for i in 0..3 {
        let (Some(x), Some(y)) = (exact(a[i]), exact(b[i])) else { return true };
        let d = &x - &y;
        squared = &squared + &(&d * &d);
    }
    squared > limit * limit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rational(text: &str) -> Rational {
        Rational::parse(text).unwrap()
    }

    fn requested(edge: Option<&str>) -> MeshOptions {
        MeshOptions { max_edge_length: edge.map(rational), ..MeshOptions::default() }
    }

    #[test]
    fn native_limit_is_the_request_less_rounding_growth() {
        let native = native_options(&requested(Some("2")), 100.0).unwrap().max_edge_length.unwrap();
        assert!(native < rational("2"));
        assert!(rational("1.99") < native);
        assert_eq!(native_options(&requested(None), 100.0).unwrap().max_edge_length, None);
    }

    #[test]
    fn other_controls_pass_through_unchanged() {
        let options = MeshOptions { angular_deviation: Some(rational("0.2")), ..requested(Some("2")) };
        let native = native_options(&options, 10.0).unwrap();
        assert_eq!(native.angular_deviation, options.angular_deviation);
        assert_eq!(native.chord_tolerance, options.chord_tolerance);
    }

    #[test]
    fn a_limit_within_rounding_growth_is_refused() {
        let error = native_options(&requested(Some("0.00001")), 1000.0).unwrap_err();
        assert!(matches!(error, KernelError::Unsupported(_)), "{error:?}");
        assert!(native_options(&requested(Some("0.00001")), 1.0).is_ok());
    }

    #[test]
    fn rounded_segments_are_compared_exactly_at_the_limit() {
        let limit = rational("3");
        assert!(!exceeds([0.0; 3], [3.0, 0.0, 0.0], &limit));
        assert!(!exceeds([0.0; 3], [1.0, 2.0, 2.0], &limit));
        assert!(exceeds([0.0; 3], [3.0 + 1e-12, 0.0, 0.0], &limit));
        assert!(exceeds([0.0; 3], [2.0, 2.0, 2.0], &limit));
    }

    #[test]
    fn check_measures_f32_positions_not_native_ones() {
        // The native length 1 + 1e-9 rounds to exactly 1 in f32.
        let positions = [[0.0, 0.0, 0.0], [1.0 + 1e-9, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let sides = || [(0, 1), (1, 2), (2, 0)].into_iter();
        let one = rational("1");
        assert!(check_segments(Some(&one), &positions, [(0, 1), (0, 2)].into_iter()).is_ok());
        assert!(check_segments(Some(&one), &positions, sides()).is_err(), "the hypotenuse is about 1.414");
        assert!(check_segments(Some(&rational("1.5")), &positions, sides()).is_ok());
        assert!(check_segments(None, &positions, sides()).is_ok());
    }

    #[test]
    fn positions_beyond_f32_range_are_refused_not_measured() {
        // Both x values are finite in f64 but round to f32 infinity.
        let big = 2f64.powi(130);
        let positions = [[big, 0.0, 0.0], [big, 1.0, 0.0]];
        let limit = Rational::from_f64_exact(2f64.powi(110)).unwrap();
        let error = check_segments(Some(&limit), &positions, [(0, 1)].into_iter()).unwrap_err();
        assert!(matches!(error, KernelError::Unsupported(_)), "{error:?}");
    }
}
