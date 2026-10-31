//! The part's reference geometry as the viewport shows it: three finite origin planes, the
//! origin axes and the origin. [`Visible`] says which of them the user's choices show, a
//! [`ReferenceScene`] places them for this frame, and the renderer, the plane names, the
//! axes and the hit test all read that one scene, so a click finds exactly what is drawn.
//!
//! Planes draw behind every face, whatever the depth says, so a plane through a part never
//! tints it. A reference is picked only where no model is under the pointer.

use super::camera::Camera;
use super::render::{PickRevision, PickSample};
use crate::document::{AxisKind, PlaneKind, Reference, Selection};
use crate::tree_visibility::Visible;
use egui::{Color32, Pos2, Rect};
use glam::{Mat4, Vec2, Vec3};

/// The planes are never smaller than this, half their side in millimetres.
const SMALLEST_HALF: f32 = 50.0;
/// How far past a part's furthest coordinate they reach.
const MARGIN: f32 = 1.25;
/// How near the pointer has to be, in points.
const ORIGIN_REACH: f32 = 9.0;
const AXIS_REACH: f32 = 6.0;
/// A plane turned further than this from the viewer can be neither named nor picked. The
/// cosine of the angle between its normal and the view.
const EDGE_ON: f32 = 0.05;

/// Fill and outline opacity at rest, and how much a hovered or selected plane multiplies
/// them by.
pub const FILL: f32 = 0.09;
pub const OUTLINE: f32 = 0.5;
pub const OUTLINE_POINTS: f32 = 1.25;
const HOVERED: f32 = 1.5;
const SELECTED: f32 = 2.0;

/// A plane's directions, which the shader's `vs_plane` repeats.
pub fn directions(plane: PlaneKind) -> (Vec3, Vec3) {
    match plane {
        PlaneKind::Top => (Vec3::X, Vec3::Y),
        PlaneKind::Front => (Vec3::X, Vec3::Z),
        PlaneKind::Right => (Vec3::Y, Vec3::Z),
    }
}

/// Half the side of the planes for a part with these bounds.
fn half_size(bounds: Option<(Vec3, Vec3)>) -> f32 {
    bounds.map_or(SMALLEST_HALF, |(lo, hi)| (lo.abs().max(hi.abs()).max_element() * MARGIN).max(SMALLEST_HALF))
}

/// Where a ray forward from `origin` meets the plane through `at` with directions `u` and
/// `v`: the world point and its coordinates in the plane. `None` for a ray along the plane
/// or pointing away from it.
pub fn ray_plane(origin: Vec3, along: Vec3, at: Vec3, u: Vec3, v: Vec3) -> Option<(Vec3, Vec2)> {
    let normal = u.cross(v);
    let facing = along.dot(normal);
    if facing.abs() < 1e-6 * along.length() {
        return None;
    }
    let t = (at - origin).dot(normal) / facing;
    let hit = origin + along * t;
    let local = Vec2::new((hit - at).dot(u), (hit - at).dot(v));
    (t > 0.0 && hit.is_finite() && local.is_finite()).then_some((hit, local))
}

/// Whether the id-buffer sample shows the pixel `px` of the frame now on screen: the same
/// part, seen from the same place. Until it does it cannot say that nothing is there.
pub fn sample_is_current(sample: &PickSample, revision: Option<PickRevision>, view: Mat4, px: [u32; 2]) -> bool {
    sample.revision == revision && sample.view == view && sample.texel(px[0], px[1]).is_some()
}

/// One frame's planes as the renderer draws them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaneFrame {
    pub half: f32,
    /// Colour and strength in [`PlaneKind::ALL`]'s order. Strength 0 hides a plane.
    pub planes: [(Color32, f32); 3],
}

/// What the viewport shows of the reference geometry this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReferenceScene {
    /// Half the side of each plane.
    half: f32,
    /// Which planes show, in [`PlaneKind::ALL`]'s order.
    planes: [bool; 3],
    /// Which axes show, in [`AxisKind::ALL`]'s order.
    pub axes: [bool; 3],
    /// How far the axes reach either side of the origin.
    pub reach: f32,
}

impl ReferenceScene {
    /// A sketch has its own plane, so the origin planes step aside for it.
    pub fn new(visible: &Visible, sketching: bool, bounds: Option<(Vec3, Vec3)>, camera: &Camera) -> Self {
        let reach = match bounds {
            Some((lo, hi)) => lo.abs().max(hi.abs()).max_element() * 1.15 + 5.0,
            None => camera.distance * 0.25,
        };
        let planes = visible.planes.map(|shown| shown && !sketching);
        Self { half: half_size(bounds), planes, axes: visible.axes, reach }
    }

    fn shown(&self) -> impl Iterator<Item = PlaneKind> + '_ {
        PlaneKind::ALL.into_iter().zip(self.planes).filter_map(|(plane, shown)| shown.then_some(plane))
    }

    /// For the renderer. `None` with no plane to draw.
    pub fn frame(&self, ink: Color32, hovered: Option<Reference>, selected: Option<Selection>) -> Option<PlaneFrame> {
        self.shown().next()?;
        let planes = PlaneKind::ALL.map(|plane| {
            let reference = Reference::Plane(plane);
            let strength = if !self.shown().any(|p| p == plane) {
                0.0
            } else if selected == Some(Selection::Reference(reference)) {
                SELECTED
            } else if hovered == Some(reference) {
                HOVERED
            } else {
                1.0
            };
            (ink, strength)
        });
        Some(PlaneFrame { half: self.half, planes })
    }

    /// Where each visible plane's name goes: its far corner, moved in a little.
    pub fn labels(&self, camera: &Camera, rect: Rect) -> Vec<(PlaneKind, Pos2)> {
        let toward = camera.direction();
        self.shown()
            .filter_map(|plane| {
                let (u, v) = directions(plane);
                if toward.dot(u.cross(v)).abs() < EDGE_ON {
                    return None;
                }
                let corner = (u + v) * self.half;
                let inner = corner - (u + v) * self.half * 0.04;
                Some((plane, camera.project(corner, rect)?, camera.project(inner, rect)?))
            })
            .filter(|(_, corner, _)| rect.contains(*corner))
            .map(|(plane, _, inner)| (plane, inner))
            .collect()
    }

    /// The reference under a point of the view: the origin, then an axis, then the nearest
    /// plane. Nothing is there to find when nothing shows.
    pub fn hit(&self, camera: &Camera, rect: Rect, at: Pos2) -> Option<Reference> {
        if !self.planes.contains(&true) && !self.axes.contains(&true) {
            return None;
        }
        let origin = camera.project(Vec3::ZERO, rect);
        if origin.is_some_and(|o| o.distance(at) <= ORIGIN_REACH) {
            return Some(Reference::Origin);
        }
        let near = |axis: &AxisKind| {
            [1.0, -1.0]
                .into_iter()
                .filter_map(|side| {
                    Some(segment_distance(at, origin?, camera.project(axis.direction() * self.reach * side, rect)?))
                })
                .fold(f32::MAX, f32::min)
        };
        let axes = AxisKind::ALL.iter().zip(self.axes).filter_map(|(axis, shown)| shown.then_some(axis));
        if let Some(closest) = axes.min_by(|a, b| near(a).total_cmp(&near(b)))
            && near(closest) <= AXIS_REACH
        {
            return Some(Reference::Axis(*closest));
        }
        let (from, along) = camera.ray(at, rect);
        let toward = camera.direction();
        self.shown()
            .filter_map(|plane| {
                let (u, v) = directions(plane);
                if toward.dot(u.cross(v)).abs() < EDGE_ON {
                    return None;
                }
                let (hit, local) = ray_plane(from, along, Vec3::ZERO, u, v)?;
                (local.abs().max_element() <= self.half).then_some(((hit - from).dot(along), plane))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, plane)| Reference::Plane(plane))
    }
}

fn segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let along = b - a;
    let t = ((p - a).dot(along) / along.length_sq().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + along * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECT: Rect = Rect { min: egui::pos2(0.0, 0.0), max: egui::pos2(1200.0, 800.0) };
    const NONE: [bool; 3] = [false; 3];
    const ALL: [bool; 3] = [true; 3];
    const INK: Color32 = Color32::BLACK;

    fn close(a: Vec3, b: Vec3, what: &str) {
        assert!(a.distance(b) < 1e-4, "{what}: {a} and {b}");
    }

    #[test]
    fn a_ray_meets_a_translated_plane_in_its_own_coordinates() {
        let (hit, local) =
            ray_plane(Vec3::new(10.0, -5.0, 50.0), Vec3::NEG_Z, Vec3::Z * 25.0, Vec3::X, Vec3::Y).unwrap();
        close(hit, Vec3::new(10.0, -5.0, 25.0), "z = 25");
        assert!(local.distance(Vec2::new(10.0, -5.0)) < 1e-4, "{local}");

        let (hit, local) =
            ray_plane(Vec3::new(50.0, 10.0, -5.0), Vec3::NEG_X, Vec3::X * 25.0, Vec3::Y, Vec3::Z).unwrap();
        close(hit, Vec3::new(25.0, 10.0, -5.0), "x = 25");
        assert!(local.distance(Vec2::new(10.0, -5.0)) < 1e-4, "{local}");
    }

    #[test]
    fn a_ray_pointing_away_or_along_the_plane_misses_it() {
        assert_eq!(ray_plane(Vec3::new(10.0, -5.0, 50.0), Vec3::Z, Vec3::Z * 25.0, Vec3::X, Vec3::Y), None);
        assert_eq!(ray_plane(Vec3::new(50.0, 10.0, -5.0), Vec3::X, Vec3::X * 25.0, Vec3::Y, Vec3::Z), None);
        assert_eq!(ray_plane(Vec3::new(0.0, 0.0, 50.0), Vec3::X, Vec3::Z * 25.0, Vec3::X, Vec3::Y), None);
        assert_eq!(ray_plane(Vec3::new(50.0, 0.0, 0.0), Vec3::Y, Vec3::X * 25.0, Vec3::Y, Vec3::Z), None);
    }

    #[test]
    fn a_nearly_parallel_ray_gives_a_finite_hit_or_none() {
        for tilt in [1e-3, 1e-5, 1e-7, 1e-9] {
            let along = Vec3::new(1.0, 0.0, -tilt).normalize();
            if let Some((hit, local)) = ray_plane(Vec3::new(0.0, 0.0, 1.0), along, Vec3::ZERO, Vec3::X, Vec3::Y) {
                assert!(hit.is_finite() && local.is_finite(), "{tilt}: {hit} {local}");
            }
        }
    }

    fn visible(planes: [bool; 3], axes: [bool; 3]) -> Visible {
        Visible { body: true, planes, axes, sketches: Vec::new() }
    }

    fn scene(planes: [bool; 3], axes: [bool; 3], camera: &Camera) -> ReferenceScene {
        ReferenceScene::new(&visible(planes, axes), false, None, camera)
    }

    /// The point on screen of a plane point, which must be inside the view.
    fn on_screen(camera: &Camera, world: Vec3) -> Pos2 {
        camera.project(world, RECT).expect("in front of the camera")
    }

    #[test]
    fn the_origin_beats_an_axis_and_an_axis_beats_a_plane() {
        let camera = Camera::default();
        let all = scene(ALL, ALL, &camera);
        assert_eq!(all.hit(&camera, RECT, on_screen(&camera, Vec3::ZERO)), Some(Reference::Origin));
        let on_x = on_screen(&camera, Vec3::X * 30.0);
        assert_eq!(all.hit(&camera, RECT, on_x), Some(Reference::Axis(AxisKind::X)), "an axis over a plane");
        let on_negative_y = on_screen(&camera, Vec3::Y * -30.0);
        assert_eq!(all.hit(&camera, RECT, on_negative_y), Some(Reference::Axis(AxisKind::Y)), "the fainter half");
        // With the axes hidden the same point is a plane's.
        let rest = scene(ALL, NONE, &camera);
        assert!(matches!(rest.hit(&camera, RECT, on_x), Some(Reference::Plane(_))));
        // So it is with only that axis hidden.
        let without_x = scene(ALL, [false, true, true], &camera);
        assert!(matches!(without_x.hit(&camera, RECT, on_x), Some(Reference::Plane(_))));
        assert_eq!(without_x.hit(&camera, RECT, on_negative_y), Some(Reference::Axis(AxisKind::Y)));
    }

    #[test]
    fn hit_finds_the_nearest_plane_and_stops_at_its_edge() {
        let camera = Camera::default();
        let rest = scene(ALL, NONE, &camera);
        let point = on_screen(&camera, Vec3::new(0.0, -30.0, 30.0));
        assert_eq!(rest.hit(&camera, RECT, point), Some(Reference::Plane(PlaneKind::Right)));
        assert_eq!(rest.hit(&camera, RECT, RECT.min + egui::vec2(5.0, 5.0)), None, "the view's corner is clear");
    }

    #[test]
    fn what_does_not_show_cannot_be_hit() {
        let camera = Camera::default();
        let hidden = scene(NONE, NONE, &camera);
        assert_eq!(hidden.frame(INK, None, None), None);
        for point in [Vec3::ZERO, Vec3::new(0.0, -30.0, 30.0), Vec3::X * 30.0] {
            assert_eq!(hidden.hit(&camera, RECT, on_screen(&camera, point)), None, "{point}");
        }
    }

    #[test]
    fn only_the_planes_that_show_are_drawn_named_and_hit() {
        let camera = Camera::default();
        let selected = Some(Selection::Reference(Reference::Plane(PlaneKind::Front)));
        let front = [false, true, false];
        let shown = scene(front, NONE, &camera);
        let frame = shown.frame(INK, None, selected).unwrap();
        let strengths = frame.planes.map(|(_, strength)| strength);
        assert_eq!(strengths, [0.0, SELECTED, 0.0], "only the Front plane, in Top, Front, Right order");
        let names: Vec<PlaneKind> = shown.labels(&camera, RECT).into_iter().map(|(plane, _)| plane).collect();
        assert_eq!(names, [PlaneKind::Front]);

        // Where the Right plane would answer with every plane showing.
        let point = on_screen(&camera, Vec3::new(0.0, -30.0, 30.0));
        assert_eq!(scene(ALL, NONE, &camera).hit(&camera, RECT, point), Some(Reference::Plane(PlaneKind::Right)));
        assert_ne!(shown.hit(&camera, RECT, point), Some(Reference::Plane(PlaneKind::Right)));
    }

    #[test]
    fn a_sketch_puts_the_planes_away_and_keeps_the_axes() {
        let camera = Camera::default();
        let sketching = ReferenceScene::new(&visible(ALL, NONE), true, None, &camera);
        assert_eq!(sketching.frame(INK, None, None), None);
        assert_eq!(sketching.hit(&camera, RECT, on_screen(&camera, Vec3::ZERO)), None);
        assert_eq!(ReferenceScene::new(&visible(ALL, ALL), true, None, &camera).axes, ALL);
    }

    #[test]
    fn a_plane_turned_edge_on_is_neither_named_nor_hit() {
        let mut camera = Camera::default();
        camera.set_view(super::super::camera::StandardView::Top);
        for _ in 0..600 {
            camera.tick(1.0 / 60.0);
        }
        let rest = scene(ALL, NONE, &camera);
        let names: Vec<PlaneKind> = rest.labels(&camera, RECT).into_iter().map(|(plane, _)| plane).collect();
        assert_eq!(names, [PlaneKind::Top], "from above only the Top plane faces the view");
        let point = on_screen(&camera, Vec3::new(10.0, 0.0, 0.0));
        assert_eq!(
            rest.hit(&camera, RECT, point),
            Some(Reference::Plane(PlaneKind::Top)),
            "not the Front plane on edge"
        );
    }

    #[test]
    fn the_planes_grow_past_a_large_part_and_never_shrink_below_their_size() {
        assert_eq!(half_size(None), SMALLEST_HALF);
        assert_eq!(half_size(Some((Vec3::splat(-1.0), Vec3::splat(1.0)))), SMALLEST_HALF);
        assert_eq!(half_size(Some((Vec3::new(-400.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0)))), 500.0);
    }

    fn sample(view: Mat4, revision: Option<PickRevision>) -> PickSample {
        PickSample { origin: [100, 100], width: 1, height: 1, data: vec![[0, 0]], view, revision, ..Default::default() }
    }

    #[test]
    fn a_reference_waits_for_a_sample_of_this_frame() {
        let camera = Camera::default();
        let view = camera.view();
        assert!(sample_is_current(&sample(view, None), None, view, [100, 100]));
        let moved = Mat4::from_rotation_z(0.1) * view;
        assert!(!sample_is_current(&sample(moved, None), None, view, [100, 100]), "the camera moved since");
        assert!(!sample_is_current(&sample(view, None), None, view, [200, 100]), "the pointer moved off the sample");
        let revision = Some((soliddraft_document::DocumentId::new(), 1, None));
        assert!(!sample_is_current(&sample(view, revision), None, view, [100, 100]), "the part changed since");
    }
}
