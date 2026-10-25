//! The maximum edge length control: cache identity and float conversion accounting.
//!
//! Catallax bounds every triangle edge and boundary polyline segment in binary64.
//! The trait returns f32 positions, and rounding each endpoint can lengthen a segment.
//! The native request therefore asks for the limit less that growth, and the returned
//! f32 positions are checked against the requested limit before they are handed out.

use catallax_mesh::{Mesh, MeshOptions};
use catallax_numbers::Rational;
use soliddraft_kernel_api::{KernelError, KernelResult, TessellationParams};

/// The controls that select one cached display mesh: the native chord bound and the
/// edge length as requested, before any rounding allowance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Controls {
    pub chord: Rational,
    pub edge: Option<Rational>,
}

impl Controls {
    /// Reads the edge length of a request. Zero disables it, as in the trait.
    pub fn new(chord: Rational, params: &TessellationParams) -> KernelResult<Self> {
        let edge = if params.max_edge_length > 0.0 {
            Some(
                Rational::parse(&params.max_edge_length.to_string())
                    .map_err(|e| KernelError::InvalidInput(e.to_string()))?,
            )
        } else {
            None
        };
        Ok(Self { chord, edge })
    }

    /// The request the certified native display mesh answers.
    pub fn native_default() -> Self {
        Self { chord: MeshOptions::default().chord_tolerance, edge: None }
    }

    /// The certified native display mesh answers only a request with no other control.
    pub fn is_native_default(&self) -> bool {
        *self == Self::native_default()
    }

    /// Native options for a body whose coordinates have magnitude at most `extent`.
    pub fn native_options(&self, extent: f64) -> KernelResult<MeshOptions> {
        let mut options = MeshOptions { chord_tolerance: self.chord.clone(), ..MeshOptions::default() };
        if let Some(edge) = &self.edge {
            let margin = rounding_growth(extent)?;
            let native = edge - &margin;
            if native <= Rational::zero() {
                return Err(KernelError::Unsupported(
                    "requested maximum edge length cannot survive f32 display rounding".into(),
                ));
            }
            options.max_edge_length = Some(native);
        }
        Ok(options)
    }

    /// Checks the f32 positions of every triangle edge and edge polyline segment.
    /// The native bound covers binary64 positions only, so this is the final gate.
    pub fn check(&self, mesh: &Mesh) -> KernelResult<()> {
        let triangles = mesh.triangles.iter().flat_map(|t| {
            let [a, b, c] = t.vertices;
            [(a, b), (b, c), (c, a)]
        });
        let polylines = mesh.edges.iter().flat_map(|e| e.vertices.windows(2).map(|w| (w[0], w[1])));
        self.check_segments(&mesh.positions, triangles.chain(polylines))
    }

    fn check_segments(
        &self,
        positions: &[[f64; 3]],
        segments: impl Iterator<Item = (usize, usize)>,
    ) -> KernelResult<()> {
        let Some(limit) = &self.edge else { return Ok(()) };
        let rounded = |i: usize| positions[i].map(|v| f64::from(v as f32));
        for (a, b) in segments {
            if exceeds(rounded(a), rounded(b), limit) {
                return Err(KernelError::Unsupported(
                    "requested maximum edge length cannot survive f32 display rounding".into(),
                ));
            }
        }
        Ok(())
    }
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
    let exact = |v: f64| Rational::from_f64_exact(v).expect("finite rounded coordinate");
    let squared = (0..3).fold(Rational::zero(), |sum, i| {
        let d = &exact(a[i]) - &exact(b[i]);
        &sum + &(&d * &d)
    });
    squared > limit * limit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rational(text: &str) -> Rational {
        Rational::parse(text).unwrap()
    }

    fn controls(chord: &str, edge: f64) -> Controls {
        let params = TessellationParams { max_edge_length: edge, ..Default::default() };
        Controls::new(rational(chord), &params).unwrap()
    }

    #[test]
    fn edge_length_separates_cache_identity_at_equal_chord() {
        let none = controls("0.01", 0.0);
        let coarse = controls("0.01", 2.0);
        let fine = controls("0.01", 1.5);
        assert_ne!(none, coarse);
        assert_ne!(coarse, fine);
        assert_eq!(coarse, controls("0.01", 2.0));
        assert_ne!(coarse, controls("0.02", 2.0));
    }

    #[test]
    fn native_default_shortcut_excludes_any_edge_length() {
        assert!(controls("0.01", 0.0).is_native_default());
        assert!(!controls("0.01", 5.0).is_native_default());
        assert!(!controls("0.02", 0.0).is_native_default());
    }

    #[test]
    fn native_limit_is_the_request_less_rounding_growth() {
        let options = controls("0.01", 2.0).native_options(100.0).unwrap();
        let native = options.max_edge_length.unwrap();
        assert!(native < rational("2"));
        assert!(rational("1.99") < native);
        assert_eq!(controls("0.01", 0.0).native_options(100.0).unwrap().max_edge_length, None);
    }

    #[test]
    fn a_limit_within_rounding_growth_is_refused() {
        let error = controls("0.01", 1e-5).native_options(1000.0).unwrap_err();
        assert!(matches!(error, KernelError::Unsupported(_)), "{error:?}");
        assert!(controls("0.01", 1e-5).native_options(1.0).is_ok());
    }

    #[test]
    fn requests_that_are_not_positive_numbers_disable_the_control() {
        assert_eq!(controls("0.01", 0.0).edge, None);
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
        let mut c = controls("0.01", 1.0);
        assert!(c.check_segments(&positions, [(0, 1), (0, 2)].into_iter()).is_ok());
        assert!(c.check_segments(&positions, sides()).is_err(), "the hypotenuse is about 1.414");
        c.edge = Some(rational("1.5"));
        assert!(c.check_segments(&positions, sides()).is_ok());
        c.edge = None;
        assert!(c.check_segments(&positions, sides()).is_ok());
    }
}
