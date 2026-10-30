//! Z-up orbit camera with smooth transitions to the standard views.

use egui::{Pos2, Rect};
use glam::{Mat4, Vec3, Vec4};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandardView {
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
    Isometric,
}

impl StandardView {
    pub fn label(&self) -> &'static str {
        match self {
            StandardView::Front => "Front",
            StandardView::Back => "Back",
            StandardView::Left => "Left",
            StandardView::Right => "Right",
            StandardView::Top => "Top",
            StandardView::Bottom => "Bottom",
            StandardView::Isometric => "Isometric",
        }
    }

    /// Yaw and pitch in degrees. Yaw is measured from +X toward +Y; the
    /// viewer stands at `target + distance * dir(yaw, pitch)`.
    pub fn angles(&self) -> (f32, f32) {
        match self {
            StandardView::Front => (-90.0, 0.0),
            StandardView::Back => (90.0, 0.0),
            StandardView::Right => (0.0, 0.0),
            StandardView::Left => (180.0, 0.0),
            StandardView::Top => (-90.0, 90.0),
            StandardView::Bottom => (-90.0, -90.0),
            StandardView::Isometric => (-45.0, 35.264),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Goal {
    yaw: f32,
    pitch: f32,
    target: Vec3,
    distance: f32,
}

#[derive(Clone, Debug)]
pub struct Camera {
    pub target: Vec3,
    pub distance: f32,
    /// Radians, around +Z, measured from +X toward +Y.
    pub yaw: f32,
    /// Radians above the XY plane.
    pub pitch: f32,
    pub fov_y: f32,
    pub orthographic: bool,
    goal: Option<Goal>,
    pub view_name: &'static str,
}

impl Default for Camera {
    fn default() -> Self {
        let (yaw, pitch) = StandardView::Isometric.angles();
        Self {
            target: Vec3::ZERO,
            distance: 260.0,
            yaw: yaw.to_radians(),
            pitch: pitch.to_radians(),
            fov_y: 38.0_f32.to_radians(),
            orthographic: false,
            goal: None,
            view_name: "Isometric",
        }
    }
}

impl Camera {
    pub fn direction(&self) -> Vec3 {
        Vec3::new(self.pitch.cos() * self.yaw.cos(), self.pitch.cos() * self.yaw.sin(), self.pitch.sin())
    }

    /// Up vector that keeps the horizon level and stays well defined at the poles.
    pub fn up(&self) -> Vec3 {
        Vec3::new(-self.pitch.sin() * self.yaw.cos(), -self.pitch.sin() * self.yaw.sin(), self.pitch.cos()).normalize()
    }

    pub fn right(&self) -> Vec3 {
        self.forward().cross(self.up()).normalize()
    }

    pub fn forward(&self) -> Vec3 {
        -self.direction()
    }

    pub fn eye(&self) -> Vec3 {
        self.target + self.direction() * self.distance
    }

    pub fn view(&self) -> Mat4 {
        glam::camera::rh::view::look_at_mat4(self.eye(), self.target, self.up())
    }

    pub fn near_far(&self) -> (f32, f32) {
        (self.distance * 0.02, self.distance * 50.0)
    }

    /// Projection with reversed depth: the near plane maps to 1 and far (infinity for
    /// perspective) to 0, which spreads a float depth buffer's precision evenly.
    pub fn projection(&self, aspect: f32) -> Mat4 {
        let (near, far) = self.near_far();
        if self.orthographic {
            let half_h = self.distance * (self.fov_y / 2.0).tan();
            let half_w = half_h * aspect;
            glam::camera::rh::proj::directx::orthographic(-half_w, half_w, -half_h, half_h, far, -far)
        } else {
            glam::camera::rh::proj::directx::perspective_infinite_reverse(self.fov_y, aspect, near)
        }
    }

    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        self.projection(aspect) * self.view()
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.goal = None;
        self.view_name = "Custom";
        self.yaw -= dx * 0.008;
        self.pitch = (self.pitch + dy * 0.008).clamp(-1.553, 1.553);
    }

    pub fn pan(&mut self, dx: f32, dy: f32, viewport_height: f32) {
        self.goal = None;
        let world_per_px = 2.0 * self.distance * (self.fov_y / 2.0).tan() / viewport_height.max(1.0);
        self.target -= self.right() * dx * world_per_px;
        self.target += self.up() * dy * world_per_px;
    }

    /// Zoom by `factor` (> 1 zooms in) keeping the point under `ndc` fixed.
    pub fn zoom(&mut self, factor: f32, ndc: Option<(f32, f32)>, aspect: f32) {
        self.goal = None;
        let old = self.distance;
        let new = (old / factor).clamp(0.5, 100_000.0);
        if let Some((x, y)) = ndc {
            let t = (self.fov_y / 2.0).tan();
            let shift = (self.right() * x * aspect + self.up() * y) * (old - new) * t;
            self.target += shift;
        }
        self.distance = new;
    }

    pub fn set_view(&mut self, view: StandardView) {
        let (yaw, pitch) = view.angles();
        self.turn_to(view.label(), yaw, pitch);
    }

    /// Turns, animated, to look from `yaw` and `pitch` degrees, and names the view.
    pub fn turn_to(&mut self, name: &'static str, yaw: f32, pitch: f32) {
        self.view_name = name;
        self.goal = Some(Goal {
            yaw: yaw.to_radians(),
            pitch: pitch.to_radians(),
            target: self.target,
            distance: self.distance,
        });
    }

    /// Where the view looks from once any transition ends: [`Camera::direction`] at the
    /// goal.
    pub fn heading(&self) -> Vec3 {
        let (yaw, pitch) = self.goal.map_or((self.yaw, self.pitch), |g| (g.yaw, g.pitch));
        Vec3::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), pitch.sin())
    }

    /// Looks straight at a plane from the side its normal points to, animated.
    pub fn look_along(&mut self, normal: Vec3) {
        let n = normal.normalize_or_zero();
        if n == Vec3::ZERO {
            return;
        }
        let pitch = n.z.clamp(-1.0, 1.0).asin();
        let yaw = if n.x.abs() < 1e-6 && n.y.abs() < 1e-6 { -std::f32::consts::FRAC_PI_2 } else { n.y.atan2(n.x) };
        self.view_name = "Normal to";
        self.goal = Some(Goal { yaw, pitch: pitch.clamp(-1.553, 1.553), target: self.target, distance: self.distance });
    }

    pub fn fit(&mut self, bounds: Option<(Vec3, Vec3)>) {
        let (center, radius) = match bounds {
            Some((lo, hi)) => ((lo + hi) / 2.0, ((hi - lo).length() / 2.0).max(1.0)),
            None => (Vec3::ZERO, 60.0),
        };
        let distance = radius / (self.fov_y / 2.0).sin() * 1.12;
        let (yaw, pitch) = self.goal.map(|g| (g.yaw, g.pitch)).unwrap_or((self.yaw, self.pitch));
        self.goal = Some(Goal { yaw, pitch, target: center, distance });
    }

    /// Advances any view transition. Returns true while still animating.
    pub fn tick(&mut self, dt: f32) -> bool {
        let Some(goal) = self.goal else { return false };
        let k = 1.0 - (-dt * 14.0).exp();
        let dyaw = wrap_angle(goal.yaw - self.yaw);
        self.yaw += dyaw * k;
        self.pitch += (goal.pitch - self.pitch) * k;
        self.target += (goal.target - self.target) * k;
        self.distance += (goal.distance - self.distance) * k;
        let done = dyaw.abs() < 0.0005
            && (goal.pitch - self.pitch).abs() < 0.0005
            && (goal.target - self.target).length() < 0.01 * self.distance
            && (goal.distance - self.distance).abs() < 0.001 * self.distance;
        if done {
            self.yaw = wrap_angle(goal.yaw);
            self.pitch = goal.pitch;
            self.target = goal.target;
            self.distance = goal.distance;
            self.goal = None;
        }
        true
    }

    /// World-space ray through a viewport pixel.
    pub fn ray(&self, pos: Pos2, rect: Rect) -> (Vec3, Vec3) {
        let ndc_x = (pos.x - rect.left()) / rect.width() * 2.0 - 1.0;
        let ndc_y = 1.0 - (pos.y - rect.top()) / rect.height() * 2.0;
        let aspect = rect.width() / rect.height().max(1.0);
        let t = (self.fov_y / 2.0).tan();
        let (right, up, fwd) = (self.right(), self.up(), self.forward());
        if self.orthographic {
            let origin = self.eye() + right * ndc_x * t * self.distance * aspect + up * ndc_y * t * self.distance;
            (origin - fwd * self.distance * 10.0, fwd)
        } else {
            let dir = (fwd + right * ndc_x * t * aspect + up * ndc_y * t).normalize();
            (self.eye(), dir)
        }
    }

    /// Projects a world point to viewport pixels. `None` when behind the eye.
    pub fn project(&self, p: Vec3, rect: Rect) -> Option<Pos2> {
        let aspect = rect.width() / rect.height().max(1.0);
        let clip = self.view_projection(aspect) * Vec4::new(p.x, p.y, p.z, 1.0);
        if clip.w <= 1e-6 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        Some(Pos2::new(
            rect.left() + (ndc.x + 1.0) / 2.0 * rect.width(),
            rect.top() + (1.0 - ndc.y) / 2.0 * rect.height(),
        ))
    }
}

fn wrap_angle(a: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let mut a = a % tau;
    if a > std::f32::consts::PI {
        a -= tau;
    } else if a < -std::f32::consts::PI {
        a += tau;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_view_looks_along_plus_y_with_z_up() {
        let mut c = Camera::default();
        c.set_view(StandardView::Front);
        for _ in 0..200 {
            c.tick(0.016);
        }
        assert!((c.forward() - Vec3::Y).length() < 1e-3);
        assert!((c.up() - Vec3::Z).length() < 1e-3);
    }

    #[test]
    fn top_view_keeps_y_up_on_screen() {
        let mut c = Camera::default();
        c.set_view(StandardView::Top);
        for _ in 0..200 {
            c.tick(0.016);
        }
        assert!((c.forward() + Vec3::Z).length() < 1e-3);
        assert!((c.up() - Vec3::Y).length() < 1e-3);
    }

    #[test]
    fn depth_is_reversed_in_both_projections() {
        let mut c = Camera::default();
        for ortho in [false, true] {
            c.orthographic = ortho;
            let m = c.view_projection(1.5);
            let depth = |p: Vec3| {
                let v = m * Vec4::new(p.x, p.y, p.z, 1.0);
                v.z / v.w
            };
            let near = depth(c.eye() + c.forward() * c.near_far().0 * 1.01);
            let target = depth(c.target);
            let far = depth(c.target + c.forward() * c.distance * 10.0);
            assert!(near > target && target > far && far > 0.0 && near <= 1.0, "{ortho}: {near} {target} {far}");
        }
    }

    #[test]
    fn ray_through_centre_hits_target() {
        let c = Camera::default();
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let (o, d) = c.ray(Pos2::new(400.0, 300.0), rect);
        let closest = o + d * (c.target - o).dot(d);
        assert!((closest - c.target).length() < 1e-3);
    }
}
