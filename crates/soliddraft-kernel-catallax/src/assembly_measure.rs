//! Exact support planes read from Catallax's native plane definitions.

use catallax_numbers::{Rational, Sign};
use catallax_shapes::Surface;
use catallax_topology::Sense;
use soliddraft_kernel_api::*;

use crate::CatallaxKernel;

fn refused(what: String) -> KernelError {
    KernelError::Unsupported(format!("proven support plane: {what}"))
}

fn exact(value: &Rational) -> ExactRational {
    ExactRational { numerator: value.numerator().to_string(), denominator: value.denominator().to_string() }
}

/// The one nonzero component of a native normal, proven with exact arithmetic.
fn axis_of(normal: [&Rational; 3]) -> Option<(Axis, &Rational)> {
    let mut nonzero = [Axis::X, Axis::Y, Axis::Z].into_iter().zip(normal).filter(|(_, c)| !c.is_zero());
    let found = nonzero.next()?;
    nonzero.next().is_none().then_some(found)
}

pub(super) fn face_support_plane(k: &CatallaxKernel, body: BodyId, face: FaceId) -> KernelResult<ExactAxisPlane> {
    let solid = k.solid(body)?;
    let id = solid
        .ids
        .faces
        .iter()
        .find_map(|(id, f)| (*f == face).then_some(*id))
        .ok_or(KernelError::UnknownFace { body, face })?;
    let model = &solid.app.checkpoint.built().model;
    let native = model.face(id);
    let Surface::Plane(plane) = model.geom().surface(native.surface) else {
        return Err(refused(format!("{face} is not planar")));
    };
    let rational = |r: &'_ catallax_numbers::Real| r.as_rational();
    let (Some(nx), Some(ny), Some(nz), Some(ox), Some(oy), Some(oz)) = (
        rational(&plane.normal.x),
        rational(&plane.normal.y),
        rational(&plane.normal.z),
        rational(&plane.origin.x),
        rational(&plane.origin.y),
        rational(&plane.origin.z),
    ) else {
        return Err(refused(format!("the plane of {face} is not structurally rational")));
    };
    let (axis, raw) = axis_of([&nx, &ny, &nz]).ok_or_else(|| refused(format!("{face} is not axis-aligned")))?;
    // For a raw plane r · x = c and face sense s, the outward normal is s r / |r_j|.
    let positive = (raw.sign() == Sign::Positive) == (native.sense == Sense::Same);
    let origin = [&ox, &oy, &oz][axis.index()];
    let offset = if positive { origin.clone() } else { &Rational::zero() - origin };
    Ok(ExactAxisPlane { axis, positive, offset: exact(&offset) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::Diagnostics;

    /// Under a budget of nothing the query still answers, so it cannot be spending any.
    #[test]
    fn a_zero_work_budget_is_enough() {
        let mut k = CatallaxKernel::new();
        let profile = Profile::rectangle(Plane::XY, Point2::new(0.0, 0.0), 40.0, 30.0);
        let base = k.create_body_from_profile(&OpTag::new("p"), &profile).unwrap();
        let body = k.extrude(&OpTag::new("b"), base.body, &Extrude { distance: 10.0, direction: None }).unwrap().body;
        let face = k.topology(body).unwrap().faces.iter().find(|f| f.normal == Some(Vec3::Z)).unwrap().id;
        k.diagnostics = Some(Diagnostics::new(Some(0)));
        let plane = k.face_support_plane(body, face).unwrap();
        assert_eq!(plane.normal(), [0, 0, 1]);
    }
}
