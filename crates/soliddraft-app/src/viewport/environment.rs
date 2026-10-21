//! The studio environment the viewport lights with: a soft grey room with a large
//! overhead softbox and two strip lights, defined as a function of direction (Z up)
//! and baked at startup into a small cube map prefiltered for roughness, plus nine
//! spherical-harmonic coefficients for diffuse light.
//!
//! Metallic appearances render nearly black without an environment to reflect, so
//! every preset uses it. It is built on the CPU from the function directly, so there
//! is no image to ship and no compute pass to run (WebGL2 has none).

use glam::Vec3;
use std::f32::consts::PI;

/// Edge length of the sharpest level, in texels.
pub const SIZE: u32 = 64;
/// Mip levels: roughness 0, 0.25, 0.5, 0.75 and 1 at 64, 32, 16, 8 and 4 texels.
pub const LEVELS: u32 = 5;
/// GGX samples per texel when prefiltering a rough level.
const SAMPLES: u32 = 128;

/// The baked environment, ready to upload.
pub struct Environment {
    /// Per level, six faces of RGBA half floats in wgpu's cube face order.
    pub levels: Vec<Vec<u16>>,
    /// Irradiance as nine spherical-harmonic coefficients, already scaled so that
    /// `sum(coefficient * basis(n))` is the diffuse light for a white surface facing `n`.
    pub irradiance: [[f32; 4]; 9],
}

/// Direction of the overhead softbox and of the shadow light: above the model, tilted
/// toward the back left of the isometric view so shadows fall forward, where they show.
pub fn softbox_direction() -> Vec3 {
    Vec3::new(-0.5, 0.42, 1.0).normalize()
}

/// Linear radiance arriving from direction `d` (unit, Z up).
pub fn radiance(d: Vec3) -> Vec3 {
    let horizon = Vec3::new(0.50, 0.52, 0.55);
    let sky = Vec3::new(0.70, 0.72, 0.76);
    let floor = Vec3::new(0.16, 0.155, 0.15);
    let mut c = if d.z >= 0.0 {
        horizon.lerp(sky, smoothstep(0.0, 0.8, d.z))
    } else {
        (horizon * 0.55).lerp(floor, smoothstep(0.0, 0.35, -d.z))
    };
    // A large softbox overhead, with a soft rim so rough reflections stay smooth.
    let cos_box = d.dot(softbox_direction());
    c += Vec3::splat(1.6) * smoothstep(0.86, 0.93, cos_box);
    // Two tall strip lights left and right of the model, slightly behind it.
    for azimuth in [-2.1_f32, 1.2] {
        let a = d.y.atan2(d.x);
        let da = wrap(a - azimuth).abs();
        let width = 0.07;
        let lit = (1.0 - smoothstep(width * 0.6, width, da))
            * smoothstep(-0.05, 0.1, d.z)
            * (1.0 - smoothstep(0.55, 0.7, d.z));
        c += Vec3::splat(2.2) * lit;
    }
    c
}

/// World direction through texel coordinates `u`, `v` in [-1, 1] of cube face `face`,
/// in the WebGPU face order (+X, -X, +Y, -Y, +Z, -Z) with `v` growing downwards.
pub fn cube_direction(face: usize, u: f32, v: f32) -> Vec3 {
    match face {
        0 => Vec3::new(1.0, -v, -u),
        1 => Vec3::new(-1.0, -v, u),
        2 => Vec3::new(u, 1.0, v),
        3 => Vec3::new(u, -1.0, -v),
        4 => Vec3::new(u, -v, 1.0),
        _ => Vec3::new(-u, -v, -1.0),
    }
    .normalize()
}

pub fn build() -> Environment {
    let mut levels = Vec::with_capacity(LEVELS as usize);
    for level in 0..LEVELS {
        let size = SIZE >> level;
        let roughness = level as f32 / (LEVELS - 1) as f32;
        let mut texels = Vec::with_capacity((6 * size * size * 4) as usize);
        for face in 0..6 {
            for y in 0..size {
                for x in 0..size {
                    let u = (x as f32 + 0.5) / size as f32 * 2.0 - 1.0;
                    let v = (y as f32 + 0.5) / size as f32 * 2.0 - 1.0;
                    let c = prefiltered(cube_direction(face, u, v), roughness);
                    for k in [c.x, c.y, c.z, 1.0] {
                        texels.push(half::f16::from_f32(k).to_bits());
                    }
                }
            }
        }
        levels.push(texels);
    }
    Environment { levels, irradiance: irradiance() }
}

/// The environment seen in a mirror of roughness `roughness` along `r`, with the usual
/// split-sum assumption that the normal and the view both point along `r`.
fn prefiltered(r: Vec3, roughness: f32) -> Vec3 {
    if roughness == 0.0 {
        return radiance(r);
    }
    let a = roughness * roughness;
    let (t, b) = basis(r);
    let mut sum = Vec3::ZERO;
    let mut weight = 0.0;
    for i in 0..SAMPLES {
        let (e1, e2) = hammersley(i, SAMPLES);
        let phi = 2.0 * PI * e1;
        let cos_theta = ((1.0 - e2) / (1.0 + (a * a - 1.0) * e2)).sqrt();
        let sin_theta = (1.0 - cos_theta * cos_theta).sqrt();
        let h = t * (sin_theta * phi.cos()) + b * (sin_theta * phi.sin()) + r * cos_theta;
        let l = 2.0 * r.dot(h) * h - r;
        let n_dot_l = r.dot(l);
        if n_dot_l > 0.0 {
            sum += radiance(l) * n_dot_l;
            weight += n_dot_l;
        }
    }
    if weight > 0.0 { sum / weight } else { radiance(r) }
}

/// Projects the environment onto the first nine real spherical harmonics and folds in
/// the cosine lobe and the 1/π of a Lambertian surface (Ramamoorthi and Hanrahan, 2001).
fn irradiance() -> [[f32; 4]; 9] {
    let (n_theta, n_phi) = (64, 128);
    let mut coeffs = [Vec3::ZERO; 9];
    for i in 0..n_theta {
        let theta = (i as f32 + 0.5) / n_theta as f32 * PI;
        for j in 0..n_phi {
            let phi = (j as f32 + 0.5) / n_phi as f32 * 2.0 * PI;
            let d = Vec3::new(theta.sin() * phi.cos(), theta.sin() * phi.sin(), theta.cos());
            let solid_angle = theta.sin() * (PI / n_theta as f32) * (2.0 * PI / n_phi as f32);
            let l = radiance(d) * solid_angle;
            for (c, y) in coeffs.iter_mut().zip(sh_basis(d)) {
                *c += l * y;
            }
        }
    }
    // Cosine-lobe convolution per band, divided by π for a Lambertian surface.
    let band = [1.0, 2.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0, 0.25, 0.25, 0.25, 0.25, 0.25];
    let mut out = [[0.0; 4]; 9];
    for k in 0..9 {
        let c = coeffs[k] * band[k];
        out[k] = [c.x, c.y, c.z, 0.0];
    }
    out
}

/// The first nine real spherical harmonics at `d`. The shader evaluates the same.
pub fn sh_basis(d: Vec3) -> [f32; 9] {
    [
        0.282095,
        0.488603 * d.y,
        0.488603 * d.z,
        0.488603 * d.x,
        1.092548 * d.x * d.y,
        1.092548 * d.y * d.z,
        0.315392 * (3.0 * d.z * d.z - 1.0),
        1.092548 * d.x * d.z,
        0.546274 * (d.x * d.x - d.y * d.y),
    ]
}

fn basis(n: Vec3) -> (Vec3, Vec3) {
    let up = if n.z.abs() < 0.999 { Vec3::Z } else { Vec3::X };
    let t = up.cross(n).normalize();
    (t, n.cross(t))
}

fn hammersley(i: u32, n: u32) -> (f32, f32) {
    (i as f32 / n as f32, i.reverse_bits() as f32 * 2.328_306_4e-10)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn wrap(a: f32) -> f32 {
    let mut a = a % (2.0 * PI);
    if a > PI {
        a -= 2.0 * PI;
    } else if a < -PI {
        a += 2.0 * PI;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diffuse(env: &Environment, n: Vec3) -> Vec3 {
        sh_basis(n).iter().zip(env.irradiance).fold(Vec3::ZERO, |acc, (y, c)| acc + Vec3::new(c[0], c[1], c[2]) * *y)
    }

    #[test]
    fn faces_point_the_way_webgpu_samples_them() {
        assert!((cube_direction(0, 0.0, 0.0) - Vec3::X).length() < 1e-6);
        assert!((cube_direction(5, 0.0, 0.0) + Vec3::Z).length() < 1e-6);
        // +Z face: u to the right is +X, v down is -Y.
        assert!(cube_direction(4, 1.0, 0.0).x > 0.0);
        assert!(cube_direction(4, 0.0, 1.0).y < 0.0);
    }

    #[test]
    fn a_face_turned_up_gets_more_light_than_one_turned_down() {
        let env = build();
        let up = diffuse(&env, Vec3::Z);
        let down = diffuse(&env, -Vec3::Z);
        assert!(up.x > 2.0 * down.x, "up {up} down {down}");
        assert!(down.x > 0.0);
        // A white wall facing sideways sees about half sky and half floor.
        let side = diffuse(&env, Vec3::X);
        assert!(side.x < up.x && side.x > down.x);
    }

    #[test]
    fn prefiltering_keeps_the_energy_and_blurs_the_lights() {
        let env = build();
        assert_eq!(env.levels.len(), LEVELS as usize);
        let mean = |level: usize| {
            let t = &env.levels[level];
            let sum: f32 = t.chunks(4).map(|p| half::f16::from_bits(p[0]).to_f32()).sum();
            sum / (t.len() / 4) as f32
        };
        let max =
            |level: usize| env.levels[level].chunks(4).map(|p| half::f16::from_bits(p[0]).to_f32()).fold(0.0, f32::max);
        let (sharp, rough) = (mean(0), mean(LEVELS as usize - 1));
        assert!((sharp - rough).abs() < 0.25 * sharp, "mean {sharp} vs {rough}");
        assert!(max(LEVELS as usize - 1) < 0.6 * max(0), "the roughest level spreads the softbox");
        assert!(env.levels.iter().flatten().all(|h| half::f16::from_bits(*h).is_finite()));
    }
}
