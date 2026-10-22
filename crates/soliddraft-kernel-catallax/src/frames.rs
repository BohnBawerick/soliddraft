//! Exact rotations for the application's axis-aligned sketch frames.

use catallax_recipe::Number;
use soliddraft_kernel_api::Plane;

pub(super) fn rotation(plane: Plane) -> Option<[Number; 4]> {
    let x = [plane.x_dir.x, plane.x_dir.y, plane.x_dir.z];
    let z = [plane.normal.x, plane.normal.y, plane.normal.z];
    let axis =
        |v: [f64; 3]| v.iter().all(|c| *c == 0.0 || c.abs() == 1.0) && v.iter().map(|c| c.abs()).sum::<f64>() == 1.0;
    if !axis(x) || !axis(z) {
        return None;
    }
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
        if (0..3).all(|i| f64::from(qx[i]) == x[i] * f64::from(d) && f64::from(qz[i]) == z[i] * f64::from(d)) {
            return Some(q.map(|n| Number::parse(&n.to_string()).expect("integer quaternion")));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use soliddraft_kernel_api::Vec3;

    #[test]
    fn a_nearly_axis_aligned_frame_is_not_snapped() {
        assert!(rotation(Plane { normal: Vec3::new(1e-16, -1.0, 0.0), ..Plane::XY }).is_none());
    }
}
