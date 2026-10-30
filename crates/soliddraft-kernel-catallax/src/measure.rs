//! Single-entity proofs without a mesh or a whole-body measurement.

use catallax_measure::{Accuracy, Enclosure};
use catallax_numbers::Rational;
use soliddraft_kernel_api::*;

use crate::{CatallaxKernel, numerical, query_error};

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

pub(super) fn edge_length(k: &CatallaxKernel, body: BodyId, edge: EdgeId) -> KernelResult<Measurement> {
    let solid = k.solid(body)?;
    let id = solid
        .ids
        .edges
        .iter()
        .find_map(|(id, e)| (*e == edge).then_some(*id))
        .ok_or(KernelError::UnknownEdge { body, edge })?;
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
    let id = solid
        .ids
        .faces
        .iter()
        .find_map(|(id, f)| (*f == face).then_some(*id))
        .ok_or(KernelError::UnknownFace { body, face })?;
    let answer = numerical("face_area", |dec| {
        catallax_measure::face_area(&solid.app.checkpoint.built().model, id, &Accuracy::default(), dec)
    })
    .map_err(|e| query_error("face_area", e))?;
    if !answer.marks.is_empty() {
        return Err(KernelError::Unsupported(format!("face_area proof has accuracy marks: {:?}", answer.marks)));
    }
    Ok(enclosure(&answer.value))
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
}
