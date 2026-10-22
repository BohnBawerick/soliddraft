//! Exact rotations and normal sweeps for the application's sketch frames.

use catallax_numbers::Rational;
use catallax_recipe::{Number, Vector3};
use soliddraft_kernel_api::{Plane, Vec3};

pub(super) fn rotation(plane: Plane) -> Option<[Number; 4]> {
    let x = [plane.x_dir.x, plane.x_dir.y, plane.x_dir.z];
    let z = [plane.normal.x, plane.normal.y, plane.normal.z];
    let tolerance = Rational::parse("1e-9").expect("source frame tolerance");
    let within = |actual: [i32; 3], expected: [f64; 3], d: i32| {
        let mut error = Rational::zero();
        for i in 0..3 {
            let Ok(expected) = Rational::parse(&expected[i].to_string()) else { return false };
            let delta = &Rational::from_i64(i64::from(actual[i])) / &Rational::from_i64(i64::from(d)) - expected;
            error = error + &delta * &delta;
        }
        error <= &tolerance * &tolerance
    };
    // Integer quaternions cover all 24 cube rotations, without a rounded square root.
    for encoded in 0..81usize {
        let q: [i32; 4] = std::array::from_fn(|i| ((encoded / 3usize.pow(i as u32)) % 3) as i32 - 1);
        let [w, a, b, c] = q;
        let d = q.iter().map(|n| n * n).sum::<i32>();
        if d == 0 {
            continue;
        }
        let qx = [w * w + a * a - b * b - c * c, 2 * (a * b + w * c), 2 * (a * c - w * b)];
        let qz = [2 * (a * c + w * b), 2 * (b * c - w * a), w * w - a * a - b * b + c * c];
        // Use the same exact squared-axis test as catallax_app::soliddraft::plane.
        if within(qx, x, d) && within(qz, z, d) {
            return Some(q.map(|n| Number::parse(&n.to_string()).expect("integer quaternion")));
        }
    }
    None
}

/// A sweep defined by the source frame follows that frame after certified conversion.
pub(super) fn normal_sweep(plane: Plane, rotation: &[Number; 4], direction: Vec3) -> Option<Vector3> {
    let sign = if direction == plane.normal {
        1
    } else if direction == -plane.normal {
        -1
    } else {
        return None;
    };
    let r = rotation.each_ref().map(Number::value);
    let d = r.iter().map(|v| *v * *v).fold(Rational::zero(), |a, b| a + b);
    let [w, a, b, c] = r;
    let two = Rational::from_i64(2);
    let normal = [&two * &((a * c) + (w * b)), &two * &((b * c) - (w * a)), w * w - a * a - b * b + c * c];
    let scale = &Rational::from_i64(sign) / &d;
    Some(Vector3(normal.map(|v| Number::from(&v * &scale))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_frames_must_satisfy_the_source_conversion_bound() {
        let near = Plane { normal: Vec3::new(1e-16, -1.0, 0.0), x_dir: Vec3::new(1.0, 1e-16, 0.0), ..Plane::XY };
        assert!(rotation(near).is_some());
        assert!(rotation(Plane { normal: Vec3::new(3e-9, -1.0, 0.0), ..near }).is_none());
    }
}
