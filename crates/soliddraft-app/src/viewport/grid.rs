//! The modelling grid: a lattice on the ground, or on the open sketch's plane, whose
//! spacing follows the zoom. The renderer draws a [`GridFrame`] and the viewport's pill
//! names the same [`Spacing`], so the two cannot disagree.

use super::camera::Camera;
use glam::{DVec3, Vec3, dvec3};
use soliddraft_kernel_api::Plane;

/// Minor lines stay at least this far apart, in points. With steps of 1, 2 and 5 they are
/// never more than 2.5 times as far.
const MIN_POINTS: f64 = 16.0;
/// Every fifth line is a major line, so 1, 2 and 5 give majors of 5, 10 and 25.
const MAJOR_EVERY: u32 = 5;
/// Line opacity and width in points.
pub const MINOR_LINE: (f32, f32) = (0.16, 1.0);
pub const MAJOR_LINE: (f32, f32) = (0.36, 1.5);

/// The lattice's spacing in millimetres. A function of the view's scale alone, so the same
/// zoom always gives the same grid and a resting camera cannot make it flicker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spacing {
    digit: u32,
    exponent: i32,
}

impl Spacing {
    /// The finest step that keeps minor lines apart at `mm_per_point`. `None` for a scale
    /// that is not a positive number.
    pub fn for_scale(mm_per_point: f32) -> Option<Spacing> {
        if !mm_per_point.is_finite() || mm_per_point <= 0.0 {
            return None;
        }
        let wanted = f64::from(mm_per_point) * MIN_POINTS;
        let exponent = wanted.log10().floor() as i32;
        Some(match [1, 2, 5].into_iter().find(|digit| f64::from(*digit) * 10f64.powi(exponent) >= wanted) {
            Some(digit) => Spacing { digit, exponent },
            None => Spacing { digit: 1, exponent: exponent + 1 },
        })
    }

    pub fn minor(&self) -> f64 {
        f64::from(self.digit) * 10f64.powi(self.exponent)
    }

    pub fn major(&self) -> f64 {
        self.minor() * f64::from(MAJOR_EVERY)
    }

    fn mm(&self, value: f64) -> String {
        let text = format!("{:.*}", (-self.exponent).max(0) as usize, value);
        if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_owned() } else { text }
    }

    /// What the viewport's pill says.
    pub fn label(&self) -> String {
        format!("Grid {} mm / {} mm", self.mm(self.minor()), self.mm(self.major()))
    }

    /// The same spacing in words, for screen readers and the pill's tooltip.
    pub fn description(&self) -> String {
        format!("Grid spacing: minor {} mm, major {} mm", self.mm(self.minor()), self.mm(self.major()))
    }
}

/// The plane a grid lies in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridPlane {
    origin: DVec3,
    u: DVec3,
    v: DVec3,
    /// How far the grid is moved toward the viewer, in the edges' pull. The ground goes
    /// behind a face lying on it; a sketch comes in front of the face it is on.
    pull: f32,
}

impl GridPlane {
    pub const GROUND: GridPlane = GridPlane { origin: DVec3::ZERO, u: DVec3::X, v: DVec3::Y, pull: -0.5 };

    pub fn of_sketch(plane: &Plane) -> GridPlane {
        let dvec = |v: soliddraft_kernel_api::Vec3| dvec3(v.x, v.y, v.z);
        GridPlane { origin: dvec(plane.origin), u: dvec(plane.x_dir), v: dvec(plane.y_dir()), pull: 0.5 }
    }
}

/// One frame's grid as the renderer draws it, in view space: all the shader's arithmetic
/// is relative to the eye and to a lattice point near the view, so it keeps its precision
/// wherever the camera is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridFrame {
    /// A major intersection of the lattice near the view's target.
    pub anchor: Vec3,
    pub u: Vec3,
    pub v: Vec3,
    pub minor: f32,
    pub major: f32,
    pub pull: f32,
    pub ink: egui::Color32,
}

impl GridFrame {
    /// `None` when the camera is not at a finite place.
    pub fn new(plane: &GridPlane, spacing: Spacing, camera: &Camera, ink: egui::Color32) -> Option<GridFrame> {
        let major = spacing.major();
        let from = camera.target.as_dvec3() - plane.origin;
        let snap = |axis: DVec3| (from.dot(axis) / major).round() * major;
        let anchor = plane.origin + plane.u * snap(plane.u) + plane.v * snap(plane.v);
        let (right, up, back) = (camera.right().as_dvec3(), camera.up().as_dvec3(), camera.direction().as_dvec3());
        let in_view = |d: DVec3| dvec3(d.dot(right), d.dot(up), d.dot(back)).as_vec3();
        let frame = GridFrame {
            anchor: in_view(anchor - camera.eye().as_dvec3()),
            u: in_view(plane.u),
            v: in_view(plane.v),
            minor: spacing.minor() as f32,
            major: major as f32,
            pull: plane.pull,
            ink,
        };
        (frame.anchor.is_finite() && frame.u.is_finite() && frame.v.is_finite()).then_some(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The world point a view-space point of `frame` stands for.
    fn world(camera: &Camera, p: Vec3) -> Vec3 {
        camera.eye() + camera.right() * p.x + camera.up() * p.y + camera.direction() * p.z
    }

    #[test]
    fn spacing_stays_in_range_over_every_zoom() {
        // The camera's whole range, from 0.5 mm away on a tall view to 100 m on a tiny one.
        let scales = [0.5 * 0.7 / 2000.0, 1e-4, 0.03, 0.2, 1.0, 37.0, 100_000.0 * 0.7 / 10.0];
        for scale in scales {
            let s = Spacing::for_scale(scale).unwrap();
            let points = s.minor() / f64::from(scale);
            assert!((16.0..=40.0).contains(&points), "{scale} mm per point: {points} points");
            assert!(s.minor().is_finite() && s.minor() > 0.0);
            assert_eq!(s.major() / s.minor(), 5.0);
        }
    }

    #[test]
    fn spacing_on_both_sides_of_a_step_stays_in_range() {
        // 5 mm minors are exactly 16 points apart at this scale.
        let at = 5.0 / 16.0_f32;
        for scale in [at * 0.999, at, at * 1.001] {
            let s = Spacing::for_scale(scale).unwrap();
            let points = s.minor() / f64::from(scale);
            assert!((15.9..=40.1).contains(&points), "{scale}: {points}");
        }
        assert_ne!(Spacing::for_scale(at * 0.99), Spacing::for_scale(at * 1.01));
        assert_eq!(Spacing::for_scale(at * 0.99), Spacing::for_scale(at * 0.99), "the same scale, the same grid");
    }

    #[test]
    fn a_scale_that_is_not_a_positive_number_gives_no_grid() {
        for scale in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(Spacing::for_scale(scale), None, "{scale}");
        }
    }

    #[test]
    fn the_readout_names_both_families_without_float_noise() {
        let s = |digit, exponent| Spacing { digit, exponent };
        assert_eq!(s(5, 0).label(), "Grid 5 mm / 25 mm");
        assert_eq!(s(2, 1).label(), "Grid 20 mm / 100 mm");
        assert_eq!(s(1, -1).label(), "Grid 0.1 mm / 0.5 mm");
        assert_eq!(s(2, -3).label(), "Grid 0.002 mm / 0.01 mm");
        assert_eq!(s(5, 2).description(), "Grid spacing: minor 500 mm, major 2500 mm");
    }

    #[test]
    fn the_anchor_is_on_the_lattice_whatever_the_sign_of_the_coordinates() {
        let spacing = Spacing { digit: 5, exponent: 0 };
        for target in [
            Vec3::ZERO,
            Vec3::new(1.0, 2.0, 0.0),
            Vec3::new(-1.0, -2.0, 0.0),
            Vec3::new(-73.0, 41.0, 9.0),
            Vec3::new(-1e6, 1e6, 0.0),
        ] {
            let mut camera = Camera::default();
            camera.target = target;
            let frame = GridFrame::new(&GridPlane::GROUND, spacing, &camera, egui::Color32::BLACK).unwrap();
            let anchor = world(&camera, frame.anchor);
            let tolerance = 1e-5 * target.length().max(10.0);
            assert!(anchor.z.abs() < tolerance, "{target}: {anchor}");
            for along in [anchor.x, anchor.y] {
                let off = along.rem_euclid(25.0);
                assert!(off.min(25.0 - off) < tolerance, "{target}: {anchor}");
            }
        }
    }

    #[test]
    fn a_sketch_grid_keeps_the_plane_frame_and_origin() {
        let plane = Plane {
            origin: soliddraft_kernel_api::Vec3::new(25.0, 0.0, 0.0),
            normal: soliddraft_kernel_api::Vec3::X,
            x_dir: soliddraft_kernel_api::Vec3::Y,
        };
        let spacing = Spacing { digit: 2, exponent: 0 };
        let camera = Camera::default();
        let frame = GridFrame::new(&GridPlane::of_sketch(&plane), spacing, &camera, egui::Color32::BLACK).unwrap();
        let direction = |v: Vec3| camera.right() * v.x + camera.up() * v.y + camera.direction() * v.z;
        assert!(direction(frame.u).distance(Vec3::Y) < 1e-6);
        assert!(direction(frame.v).distance(Vec3::Z) < 1e-6);
        assert!((world(&camera, frame.anchor).x - 25.0).abs() < 1e-3, "the anchor is in the plane");
    }

    #[test]
    fn a_camera_that_is_not_finite_gives_no_grid() {
        let mut camera = Camera::default();
        camera.target = Vec3::NAN;
        let spacing = Spacing { digit: 1, exponent: 0 };
        assert_eq!(GridFrame::new(&GridPlane::GROUND, spacing, &camera, egui::Color32::BLACK), None);
    }
}
