//! Entity and pair proofs without a mesh or a whole-body measurement.

use catallax_measure::{Accuracy, EdgePairRequest, Enclosure, distance::PreparedFace};
use catallax_numbers::Rational;
use catallax_shapes::{Curve, distance::Options};
use soliddraft_kernel_api::*;

use crate::{CatallaxKernel, Solid, numerical, query_error};

fn enclosure(e: &Enclosure) -> Measurement {
    let value = crate::convert::midpoint(e);
    if e.lo == e.hi {
        return Measurement { value, lo: value, hi: value, certainty: Certainty::Exact };
    }
    let mut lo = e.lo.to_f64_nearest();
    let mut hi = e.hi.to_f64_nearest();
    if Rational::from_f64_exact(lo).is_some_and(|r| r > e.lo) {
        lo = lo.next_down();
    }
    if Rational::from_f64_exact(hi).is_some_and(|r| r < e.hi) {
        hi = hi.next_up();
    }
    Measurement { value, lo, hi, certainty: Certainty::Bounded }
}

fn native_edge(solid: &Solid, body: BodyId, edge: EdgeId) -> KernelResult<catallax_topology::EdgeId> {
    solid
        .ids
        .edges
        .iter()
        .find_map(|(id, e)| (*e == edge).then_some(*id))
        .ok_or(KernelError::UnknownEdge { body, edge })
}

fn native_face(solid: &Solid, body: BodyId, face: FaceId) -> KernelResult<catallax_topology::FaceId> {
    solid
        .ids
        .faces
        .iter()
        .find_map(|(id, f)| (*f == face).then_some(*id))
        .ok_or(KernelError::UnknownFace { body, face })
}

/// A domain this adapter refuses before asking Catallax, with a stable code.
fn refuse(code: &str, detail: String) -> KernelError {
    KernelError::Unsupported(format!("{code}: {detail}"))
}

/// A proof with accuracy marks, or without a converged upper bound, is not a number.
fn incomplete(query: &str, detail: impl std::fmt::Debug) -> KernelError {
    refuse("measure.bounds-incomplete", format!("{query} proof is incomplete: {detail:?}"))
}

pub(super) fn edge_length(k: &CatallaxKernel, body: BodyId, edge: EdgeId) -> KernelResult<Measurement> {
    let solid = k.solid(body)?;
    let id = native_edge(&solid, body, edge)?;
    let answer = numerical("edge_length", |dec| {
        catallax_measure::edge_length(&solid.app.checkpoint.built().model, id, &Accuracy::default(), dec)
    })
    .map_err(|e| query_error("edge_length", e))?;
    if !answer.marks.is_empty() {
        return Err(KernelError::Unsupported(format!("edge_length proof has accuracy marks: {:?}", answer.marks)));
    }
    Ok(enclosure(&answer.value))
}

pub(super) fn face_area(k: &CatallaxKernel, body: BodyId, face: FaceId) -> KernelResult<Measurement> {
    let solid = k.solid(body)?;
    let id = native_face(&solid, body, face)?;
    let answer = numerical("face_area", |dec| {
        catallax_measure::face_area(&solid.app.checkpoint.built().model, id, &Accuracy::default(), dec)
    })
    .map_err(|e| query_error("face_area", e))?;
    if !answer.marks.is_empty() {
        return Err(KernelError::Unsupported(format!("face_area proof has accuracy marks: {:?}", answer.marks)));
    }
    Ok(enclosure(&answer.value))
}

/// The native arc-length midpoint of a finite straight edge. A closed curve's midpoint
/// depends on its seam, so curved edges are refused rather than given an arbitrary point.
pub(super) fn edge_midpoint(k: &CatallaxKernel, body: BodyId, edge: EdgeId) -> KernelResult<PointProof> {
    let solid = k.solid(body)?;
    let id = native_edge(&solid, body, edge)?;
    let model = &solid.app.checkpoint.built().model;
    let e = model.edge(id);
    let (Curve::Line(_), Some(start), Some(end)) = (model.geom().curve(e.curve), e.start, e.end) else {
        return Err(refuse("measure.unsupported-curve", format!("{edge} is not a finite straight edge")));
    };
    let position = |v| model.geom().position(model.vertex(v).point).as_rational();
    if start == end || position(start).is_some_and(|a| Some(a) == position(end)) {
        return Err(refuse("measure.degenerate-edge", format!("{edge} has zero length")));
    }
    let answer =
        numerical("edge_midpoint", |dec| catallax_measure::edge_midpoint(model, id, &Accuracy::default(), dec))
            .map_err(|e| query_error("edge_midpoint", e))?;
    if !answer.marks.is_empty() {
        return Err(incomplete("edge_midpoint", answer.marks));
    }
    Ok(PointProof { coordinates: answer.value.each_ref().map(enclosure) })
}

/// One scalar between two entities of `body`: straight edges through the native edge
/// pair query, parallel planar faces through the planar witness search.
pub(super) fn measure_pair(
    k: &CatallaxKernel,
    body: BodyId,
    a: PairEndpoint,
    b: PairEndpoint,
    quantity: PairQuantity,
) -> KernelResult<Measurement> {
    use PairEndpoint::{Edge, EdgeMidpoint, Face};
    let solid = k.solid(body)?;
    let edges = |x, y| Ok::<_, KernelError>((native_edge(&solid, body, x)?, native_edge(&solid, body, y)?));
    let request = match (quantity, a, b) {
        (PairQuantity::MinimumDistance, Face(x), Face(y)) => return face_gap(&solid, body, x, y),
        (PairQuantity::MinimumDistance, Edge(x), Edge(y)) => (EdgePairRequest::MinimumDistance, edges(x, y)?),
        (PairQuantity::PointDistance, EdgeMidpoint(x), EdgeMidpoint(y)) => {
            (EdgePairRequest::MidpointDistance, edges(x, y)?)
        }
        (PairQuantity::DirectionAngle, Edge(x), Edge(y)) => (EdgePairRequest::DirectionAngleDegrees, edges(x, y)?),
        _ => {
            return Err(refuse(
                "measure.unsupported-pair",
                format!("{} is not supported between {a:?} and {b:?}", quantity.name()),
            ));
        }
    };
    let (request, (x, y)) = request;
    let model = &solid.app.checkpoint.built().model;
    let answer =
        numerical("edge_pair", |dec| catallax_measure::edge_pair(model, x, y, request, &Accuracy::default(), dec))
            .map_err(|e| query_error("edge_pair", e))?;
    if !answer.marks.is_empty() {
        return Err(incomplete("edge_pair", answer.marks));
    }
    Ok(enclosure(&answer.value))
}

/// The requested face-gap width in millimetres; the shared default is coarser.
const FACE_GAP_WIDTH: (i64, i64) = (1, 100_000_000);

fn face_gap(solid: &Solid, body: BodyId, a: FaceId, b: FaceId) -> KernelResult<Measurement> {
    let (x, y) = (native_face(solid, body, a)?, native_face(solid, body, b)?);
    // The witness search classifies probes against the model, so it runs on a copy.
    let mut model = solid.app.checkpoint.built().model.clone();
    let options =
        Options { width: Rational::from_ratio(FACE_GAP_WIDTH.0, FACE_GAP_WIDTH.1).unwrap(), ..Options::default() };
    let bounds = numerical("parallel_face_to_face", |dec| {
        let p = PreparedFace::new(&model, x, dec)?;
        let q = PreparedFace::new(&model, y, dec)?;
        p.parallel_face_to_face(&q, &mut model, &options, dec).map(catallax_numbers::Answer::exact)
    })
    .map_err(|e| query_error("parallel_face_to_face", e))?
    .value;
    match bounds.upper {
        Some(hi) if bounds.converged => Ok(enclosure(&Enclosure { lo: bounds.lower, hi })),
        upper => Err(incomplete("parallel_face_to_face", (bounds.lower, upper, bounds.converged))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enclosures_round_outward() {
        let lo = Rational::from_ratio(1, 3).unwrap();
        let hi = &lo + &Rational::parse("1e-40").unwrap();
        let m = enclosure(&Enclosure { lo: lo.clone(), hi: hi.clone() });
        assert!(Rational::from_f64_exact(m.lo).unwrap() <= lo);
        assert!(Rational::from_f64_exact(m.hi).unwrap() >= hi);
        assert!(m.lo < m.hi);
        let exact = Rational::from_ratio(381, 5).unwrap();
        assert_eq!(
            enclosure(&Enclosure { lo: exact.clone(), hi: exact }),
            Measurement { value: 76.2, lo: 76.2, hi: 76.2, certainty: Certainty::Exact }
        );
    }

    /// V2 batch 2 K3's work rows: no allowance refuses with the native work code, and
    /// every pair proof on the holed block stays under the 20M application allowance.
    #[test]
    fn pair_queries_refuse_without_work_and_answer_under_the_app_allowance() {
        use PairEndpoint::{Edge, EdgeMidpoint, Face};
        let k = CatallaxKernel::new();
        let body = contract::holed_block(&k, 15.0);
        let t = k.topology(body).unwrap();
        let line = |y: f64, z: f64| {
            t.edges.iter().find(|e| e.length > 39.0 && (e.midpoint - Vec3::new(0.0, y, z)).length() < 0.01).unwrap().id
        };
        let cap = |z: f64| {
            t.faces.iter().find(|f| f.surface == SurfaceKind::Plane && (f.centroid.z - z).abs() < 1e-3).unwrap().id
        };
        let (e0, e1, f0, f1) = (line(-15.0, 0.0), line(15.0, 15.0), cap(0.0), cap(15.0));
        let requests = [
            (Edge(e0), Edge(e1), PairQuantity::MinimumDistance),
            (EdgeMidpoint(e0), EdgeMidpoint(e1), PairQuantity::PointDistance),
            (Edge(e0), Edge(e1), PairQuantity::DirectionAngle),
            (Face(f0), Face(f1), PairQuantity::MinimumDistance),
        ];
        let budget_refusal = |e: KernelError| matches!(e, KernelError::QueryFailed { message, .. } if message.contains("numbers.operation-budget"));
        for (a, b, q) in requests {
            let (refused, work) = catallax_numbers::with_operation_budget(0, || measure_pair(&k, body, a, b, q));
            assert!(work.exhausted && budget_refusal(refused.unwrap_err()), "{q:?}");
            let (answered, work) =
                catallax_numbers::with_operation_budget(20_000_000, || measure_pair(&k, body, a, b, q));
            assert!(answered.is_ok() && !work.exhausted, "{q:?}: {answered:?}");
        }
        // A rational line's midpoint is the mean of its ends: no numerical work to refuse.
        let (midpoint, work) = catallax_numbers::with_operation_budget(0, || edge_midpoint(&k, body, e0));
        assert!(midpoint.is_ok() && work.used == 0 && !work.exhausted, "{midpoint:?} {work:?}");
    }
}
