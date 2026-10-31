//! The origin planes: readable in every view, never in the model's way, and picked in the
//! viewport as their tree rows are, all through the real app and the real renderer.
//!
//! The design is `sdc-d-ux-viewport`, merge C.

#[path = "support/viewport_acceptance.rs"]
mod support;

use egui::{Vec2, pos2, vec2};
use glam::Vec3;
use soliddraft_app::document::{AxisKind, PlaneKind, Reference, Selection};
use soliddraft_app::state::EditorParams;
use soliddraft_app::viewport::render::DisplayMode;
use support::*;

const SIZE: Vec2 = vec2(1440.0, 900.0);
const VIEWS: [(&str, PlaneKind); 3] =
    [("Top view (T)", PlaneKind::Top), ("Front view (F)", PlaneKind::Front), ("Right view (R)", PlaneKind::Right)];

/// The same view with the planes on and off, the planes left on.
fn plane_frames(case: &mut Case, app: &mut App, name: &str) -> (image::RgbaImage, image::RgbaImage) {
    pointer_to_chrome(app);
    let on = case.frame(app, &format!("{name}-planes-on"));
    set_display(app, |d| d.show_origin_planes = false);
    let off = case.frame(app, &format!("{name}-planes-off"));
    set_display(app, |d| d.show_origin_planes = true);
    (on, off)
}

/// Every view of the empty part, with each plane read in the view that faces it and all
/// three in the isometric view.
fn planes_read(name: &str, size: Vec2, ppp: f32) {
    let mut case = Case::gpu(name);
    let mut app = launch_for_frames(empty(), size, ppp);
    let before = model(&app);
    for plane in PlaneKind::ALL {
        assert!(plane_named(&app, plane), "{} is named at rest", plane.label());
    }
    let rest = case.frame(&mut app, "rest");
    assert_axes_hidden(&rest, &app, "with the planes showing");
    set_preference(&mut app, SHOW_GRID, false);

    let mut sizes = Vec::new();
    for (control, plane) in VIEWS {
        click_button(&mut app, control);
        let label = plane.label();
        let (on, off) = plane_frames(&mut case, &mut app, label);
        let half = assert_plane_faces_the_view(&on, &off, &app, plane, label);
        assert_plane_reads(&on, &off, &app, plane, half, label);
        case.note(format!("{label}: half {half:?}"));
        assert!(plane_named(&app, plane), "{label} is named in the view that faces it");
        assert_axes_hidden(&on, &app, label);
        sizes.push((plane, half));
    }
    click_button(&mut app, "Isometric view (I)");
    let (on, off) = plane_frames(&mut case, &mut app, "isometric");
    for (plane, half) in sizes {
        assert_plane_reads(&on, &off, &app, plane, half, &format!("{}, isometric", plane.label()));
        assert!(plane_named(&app, plane));
    }
    assert_axes_hidden(&on, &app, "isometric");
    assert_eq!(model(&app), before, "the planes do not touch the model");
}

#[test]
fn origin_planes_have_fill_outline_and_names() {
    planes_read("planes_read", SIZE, 1.0);
}

#[test]
fn origin_planes_read_at_two_pixels_per_point() {
    planes_read("planes_read_2x", SIZE, 2.0);
}

#[test]
fn origin_planes_read_in_a_small_window() {
    planes_read("planes_read_small", vec2(960.0, 640.0), 1.0);
}

/// The readout and the plane names say what the view shows. The Top plane faces every
/// view used here; the other two can be edge-on, and then have no name to show.
fn assert_shows(app: &App, grid: bool, planes: bool, when: &str) {
    assert_eq!(readout(app).is_some(), grid, "{when}: the grid readout");
    assert_eq!(plane_named(app, PlaneKind::Top), planes, "{when}: the Top plane's name");
    if !planes {
        assert!(!PlaneKind::ALL.iter().any(|p| plane_named(app, *p)), "{when}: no plane names");
    }
}

#[test]
fn plane_visibility_persists_without_hiding_the_grid() {
    let mut case = Case::gpu("plane_preference");
    let mut app = launch_for_frames(empty(), SIZE, 1.0);
    let before = model(&app);
    click_button(&mut app, "Top view (T)");
    click_menu(&mut app, "View", "Orthographic");
    let view = camera(&app);
    assert_shows(&app, true, true, "by default");
    set_preference(&mut app, SHOW_GRID, false);
    let with_planes = case.frame(&mut app, "planes-only");

    // Planes off: no fill and no outline, because both are only in the other frame.
    set_preference(&mut app, SHOW_PLANES, false);
    assert_shows(&app, false, false, "with both off");
    let bare = case.frame(&mut app, "both-off");
    let half = assert_plane_faces_the_view(&with_planes, &bare, &app, PlaneKind::Top, "planes off");
    assert_plane_reads(&with_planes, &bare, &app, PlaneKind::Top, half, "planes off");

    // The grid does not need the planes.
    set_preference(&mut app, SHOW_GRID, true);
    assert_shows(&app, true, false, "with the grid alone");
    let with_grid = case.frame(&mut app, "grid-only");
    assert_top_view_grid(&app, (&with_grid, &bare), "with the planes off");

    // The planes do not need the grid, and they do not show the axes.
    set_preference(&mut app, SHOW_GRID, false);
    set_preference(&mut app, SHOW_PLANES, true);
    assert_shows(&app, false, true, "with the planes alone");
    let again = case.frame(&mut app, "planes-only-again");
    assert_plane_reads(&again, &bare, &app, PlaneKind::Top, half, "planes back");
    assert_axes_hidden(&again, &app, "with the planes alone");
    assert_eq!((model(&app), camera(&app)), (before, view), "the choices touch neither the model nor the view");
    drop(app);

    // Each combination survives a restart.
    for (grid, planes) in [(false, true), (true, false), (false, false), (true, true)] {
        let mut app = launch_for_frames(empty(), SIZE, 1.0);
        open_grid_and_planes(&mut app);
        let saved = (checked(&app, SHOW_GRID), checked(&app, SHOW_PLANES));
        close_preferences(&mut app);
        assert_shows(&app, saved.0, saved.1, "after a restart");
        set_preference(&mut app, SHOW_GRID, grid);
        set_preference(&mut app, SHOW_PLANES, planes);
        drop(app);
        let mut app = launch_for_frames(empty(), SIZE, 1.0);
        assert_shows(&app, grid, planes, &format!("restarted with grid {grid} and planes {planes}"));
        open_grid_and_planes(&mut app);
        assert_eq!((checked(&app, SHOW_GRID), checked(&app, SHOW_PLANES)), (grid, planes), "the window agrees");
        close_preferences(&mut app);
        assert!(!app.state().state.viewport.references.always_show_origin_axes, "the third choice stays");
    }

    // With both off a sketch still draws and edits, and the grid follows its own choice.
    let mut app = launch(sketches());
    set_display(&mut app, |d| {
        d.show_grid = false;
        d.show_origin_planes = false;
    });
    settle(&mut app);
    double_click_label(&mut app, "Sketch2");
    let session = app.state().state.sketch.as_ref().expect("the sketch opened");
    assert!(!session.sketch.entities.is_empty(), "with its entities");
    assert_eq!(readout(&app), None, "no grid in the sketch with Show grid off");
    let entities = session.sketch.entities.len();
    click_button(&mut app, "Line");
    let rect = viewport(&app);
    click(&mut app, rect.center() + vec2(150.0, 120.0));
    click(&mut app, rect.center() + vec2(230.0, 170.0));
    right_click(&mut app, rect.center() + vec2(230.0, 170.0));
    let session = app.state().state.sketch.as_ref().expect("the sketch stays open");
    assert!(session.sketch.entities.len() > entities, "the tools still draw");
    set_preference(&mut app, SHOW_GRID, true);
    assert!(readout(&app).is_some(), "the sketch's grid follows Show grid");
    assert_shows(&app, true, false, "in the sketch");
}

/// The box's visible edges as world samples, from the document's own topology.
fn edge_samples(app: &App) -> Vec<Vec3> {
    let state = &app.state().state;
    let toward = state.viewport.camera.direction();
    let centre = Vec3::new(0.0, 0.0, 10.0);
    let mut samples = Vec::new();
    for mesh in state.doc.meshes.values() {
        for edge in &mesh.edges {
            for segment in edge.segments.as_chunks::<2>().0 {
                let (a, b) = (segment[0], segment[1]);
                // Away from the corners, on the side of the box that faces the viewer.
                let at = (a + b) * 0.5;
                let outward = (at - centre).dot(toward);
                if outward > 0.0 && (b - a).length() > 8.0 {
                    samples.extend((1..8).map(|k| a + (b - a) * (k as f32 / 8.0)));
                }
            }
        }
    }
    samples
}

/// The share of the view that changed strongly: a plane is a light tint, so a lot of this
/// would be a flooded view or an opaque sheet.
fn strongly_changed(on: &image::RgbaImage, off: &image::RgbaImage, app: &App) -> f32 {
    let rect = viewport(app).shrink(12.0);
    let (mut changed, mut total) = (0, 0);
    for x in (rect.left() as i32..rect.right() as i32).step_by(7) {
        for y in (rect.top() as i32..rect.bottom() as i32).step_by(7) {
            let point = pos2(x as f32, y as f32);
            let (p, q) =
                (on.get_pixel(point.x as u32, point.y as u32).0, off.get_pixel(point.x as u32, point.y as u32).0);
            changed += usize::from((0..3).any(|c| p[c].abs_diff(q[c]) > 60));
            total += 1;
        }
    }
    changed as f32 / total as f32
}

fn all_overlays(app: &mut App, on: bool) {
    set_display(app, |d| {
        d.show_grid = on;
        d.show_origin_planes = on;
    });
}

/// What the model is at each point, with the overlays as they are now.
fn picks(app: &mut App, points: &[Vec3]) -> Vec<Option<Selection>> {
    points
        .iter()
        .map(|world| {
            let point = projected(app, *world).expect("the point is in view");
            click(app, point);
            let selection = app.state().state.doc.selection;
            click_empty(app);
            selection
        })
        .collect()
}

#[test]
fn reference_overlays_leave_opaque_model_and_picks_readable() {
    let mut case = Case::gpu("overlays_and_the_model");
    let mut app = launch(box_part());
    let before = model(&app);
    pointer_to_chrome(&mut app);

    // First, that the planes are there at all: a fill beside the box.
    all_overlays(&mut app, false);
    let bare = case.frame(&mut app, "bare");
    all_overlays(&mut app, true);
    let with = case.frame(&mut app, "isometric-on");
    let beside = projected(&app, Vec3::new(45.0, 0.0, 40.0)).expect("in view");
    let (p, q) =
        (with.get_pixel(beside.x as u32, beside.y as u32).0, bare.get_pixel(beside.x as u32, beside.y as u32).0);
    assert!((0..3).any(|c| p[c].abs_diff(q[c]) >= 4), "a plane shows beside the box: {p:?} and {q:?}");

    for (control, name) in [
        ("Isometric view (I)", "isometric"),
        ("Front view (F)", "front"),
        ("Top view (T)", "top"),
        ("Right view (R)", "right"),
    ] {
        click_button(&mut app, control);
        pointer_to_chrome(&mut app);
        let on = case.frame(&mut app, &format!("{name}-on"));
        all_overlays(&mut app, false);
        let off = case.frame(&mut app, &format!("{name}-off"));
        all_overlays(&mut app, true);
        let points = opaque_points(&app);
        assert!(points.len() >= 100, "{name}: faces to sample, {}", points.len());
        let kept = untouched_share(&on, &off, &app, &points);
        case.note(format!("{name}: {kept} of {} opaque samples untouched", points.len()));
        assert!(kept >= 0.99, "{name}: {kept} of the opaque faces are as they were");
        let samples = edge_samples(&app);
        let edges = edge_kept(&on, &off, &app, &samples);
        case.note(format!("{name}: {edges} of {} edge samples kept", samples.len()));
        assert!(edges >= 0.95, "{name}: {edges} of the edges stay");
    }

    // Above the ground, through it and below it, and so edge-on to the Top plane.
    click_button(&mut app, "Isometric view (I)");
    for step in 0..6 {
        orbit(&mut app, vec2(0.0, -24.0));
        pointer_to_chrome(&mut app);
        let on = case.frame(&mut app, &format!("orbit-{step}-on"));
        all_overlays(&mut app, false);
        let off = case.frame(&mut app, &format!("orbit-{step}-off"));
        all_overlays(&mut app, true);
        let state = &app.state().state;
        let pitch = state.viewport.camera.pitch;
        let flooded = strongly_changed(&on, &off, &app);
        case.note(format!("orbit {step}, pitch {pitch}: {flooded} changed strongly"));
        assert!(flooded < 0.05, "pitch {pitch}: {flooded} of the view changed strongly");
        let centre = projected(&app, Vec3::new(0.0, 0.0, 10.0)).expect("the box is in view");
        let (p, q) =
            (on.get_pixel(centre.x as u32, centre.y as u32).0, off.get_pixel(centre.x as u32, centre.y as u32).0);
        assert!((0..3).all(|c| p[c].abs_diff(q[c]) <= 24), "pitch {pitch}: the box is still there, {p:?} and {q:?}");
    }

    // The same model under the same points, with the overlays on and off.
    click_button(&mut app, "Isometric view (I)");
    let points = [Vec3::new(0.0, 0.0, 20.0), Vec3::new(14.0, -20.0, 20.0), Vec3::new(30.0, -20.0, 20.0)];
    let with = picks(&mut app, &points);
    assert!(matches!(with[0], Some(Selection::Face(_))), "the top face, not a plane: {:?}", with[0]);
    assert!(matches!(with[1], Some(Selection::Edge(_))), "an edge: {:?}", with[1]);
    assert!(matches!(with[2], Some(Selection::Vertex(_))), "a vertex: {:?}", with[2]);
    all_overlays(&mut app, false);
    assert_eq!(picks(&mut app, &points), with, "the overlays change no pick");
    all_overlays(&mut app, true);

    // Wireframe keeps the planes outside the model and keeps edges pickable.
    let look = app.state().state.viewport.preset;
    let display = app.state().state.viewport.display;
    click_button(&mut app, "Display style");
    let mut seen = vec![app.state().state.viewport.display];
    while app.state().state.viewport.display != DisplayMode::Wireframe {
        click_button(&mut app, "Display style");
        seen.push(app.state().state.viewport.display);
        assert!(seen.len() < 5, "the control reaches Wireframe: {seen:?}");
    }
    let wire = case.frame(&mut app, "wireframe-on");
    all_overlays(&mut app, false);
    let wire_off = case.frame(&mut app, "wireframe-off");
    all_overlays(&mut app, true);
    let beside = projected(&app, Vec3::new(45.0, 0.0, 40.0)).expect("in view");
    let (p, q) =
        (wire.get_pixel(beside.x as u32, beside.y as u32).0, wire_off.get_pixel(beside.x as u32, beside.y as u32).0);
    assert!((0..3).any(|c| p[c].abs_diff(q[c]) >= 4), "a plane shows in wireframe");
    let edge = projected(&app, points[1]).expect("in view");
    click(&mut app, edge);
    assert!(matches!(app.state().state.doc.selection, Some(Selection::Edge(_))), "wireframe edges pick");
    click_empty(&mut app);

    // The overlays leave the user's own view choices alone.
    all_overlays(&mut app, false);
    while app.state().state.viewport.display != display {
        click_button(&mut app, "Display style");
    }
    assert_eq!(app.state().state.viewport.preset, look, "the look stays");
    assert_eq!(model(&app), before, "and so does the model");
}

/// The Top, Front and Right squares as seen now, from their size as measured.
fn plane_outline(app: &App, plane: PlaneKind, half: f32) -> Vec<egui::Pos2> {
    let (u, v) = plane_directions(plane);
    [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)]
        .iter()
        .map(|(s, t)| projected(app, (u * *s + v * *t) * half).expect("the corner is in view"))
        .collect()
}

fn inside(p: egui::Pos2, polygon: &[egui::Pos2]) -> bool {
    let cross = |k: usize| {
        let (a, b) = (polygon[k], polygon[(k + 1) % polygon.len()]);
        (b - a).x * (p - a).y - (b - a).y * (p - a).x
    };
    (0..polygon.len()).all(|k| cross(k) > 0.0) || (0..polygon.len()).all(|k| cross(k) < 0.0)
}

/// A world point of `plane` that no other plane covers on screen, well inside it and off
/// its axes, from the fixed list of distances. Worked out from the outlines alone.
fn lone_point(app: &App, plane: PlaneKind, half: f32) -> Vec3 {
    let (u, v) = plane_directions(plane);
    let others: Vec<Vec<egui::Pos2>> =
        PlaneKind::ALL.iter().filter(|p| **p != plane).map(|p| plane_outline(app, *p, half)).collect();
    let mine = plane_outline(app, plane, half);
    for s in [40.0, -40.0, 20.0, -20.0, 10.0, -10.0_f32] {
        for t in [40.0, -40.0, 20.0, -20.0, 10.0, -10.0_f32] {
            let world = u * s + v * t;
            let p = projected(app, world).expect("in view");
            if inside(p, &mine) && !others.iter().any(|o| inside(p, o)) && s.abs() < half * 0.9 && t.abs() < half * 0.9
            {
                return world;
            }
        }
    }
    panic!("{plane:?}: no point of the list is on this plane alone");
}

/// The side of the planes, from the frames of a view that faces one.
fn measured_half(case: &mut Case, app: &mut App) -> f32 {
    let grid = app.state().state.viewport.references.show_grid;
    set_display(app, |d| d.show_grid = false);
    click_button(app, "Top view (T)");
    let (on, off) = plane_frames(case, app, "measure");
    let (half, _) = assert_plane_faces_the_view(&on, &off, app, PlaneKind::Top, "measuring");
    click_button(app, "Isometric view (I)");
    set_display(app, |d| d.show_grid = grid);
    half
}

#[test]
fn picking_origin_plane_and_axis_shows_the_triad() {
    let mut case = Case::gpu("picking_references");
    let mut app = launch(empty());
    let half = measured_half(&mut case, &mut app);
    let before = model(&app);
    let view = camera(&app);
    pointer_to_chrome(&mut app);
    let image = case.frame(&mut app, "rest");
    assert_axes_hidden(&image, &app, "at rest");
    assert_eq!(reference(&app), None);

    // The origin.
    let origin = projected(&app, Vec3::ZERO).expect("in view");
    click(&mut app, origin);
    assert_eq!(reference(&app), Some(Reference::Origin), "a click on the origin");
    pointer_to_chrome(&mut app);
    let image = case.frame(&mut app, "origin-picked");
    assert_axes_shown(&image, &app, "with the origin picked");
    click_empty(&mut app);
    assert_eq!(reference(&app), None);

    // Each plane, away from the others.
    for plane in PlaneKind::ALL {
        let world = lone_point(&app, plane, half);
        let point = projected(&app, world).expect("in view");
        click(&mut app, point);
        assert_eq!(reference(&app), Some(Reference::Plane(plane)), "a click on {world}");
        assert!(app.state().state.sketch.is_none() && app.state().state.editor.is_none(), "no sketch and no editor");
        pointer_to_chrome(&mut app);
        let image = case.frame(&mut app, &format!("{}-picked", plane.label()));
        assert_axes_shown(&image, &app, &format!("with the {} picked", plane.label()));
        assert_eq!(camera(&app), view, "a pick does not move the camera");
        assert_eq!(model(&app), before, "and does not touch the model");
        click_empty(&mut app);
        assert_eq!(reference(&app), None);
    }

    // Each axis, shown by the Origin row.
    open_references(&mut app);
    for axis in AxisKind::ALL {
        click_label(&mut app, "Origin");
        assert_eq!(reference(&app), Some(Reference::Origin));
        let reach = app.state().state.viewport.camera.distance * 0.25;
        let on_axis = projected(&app, axis.direction() * reach * 0.8).expect("in view");
        // On the fainter half for a change, away from where the planes meet the view.
        click(&mut app, on_axis);
        assert_eq!(reference(&app), Some(Reference::Axis(axis)), "a click on the {} axis", axis.label());
        pointer_to_chrome(&mut app);
        let image = case.frame(&mut app, &format!("{}-picked", axis.label()));
        assert_axes_shown(&image, &app, "with an axis picked");
        escape(&mut app);
        assert_eq!(reference(&app), None, "Escape clears it");
        let image = case.frame(&mut app, &format!("{}-cleared", axis.label()));
        assert_axes_hidden(&image, &app, "once it clears");
    }

    // Hover alone latches nothing, and the grid is not a thing to select.
    let lone = projected(&app, lone_point(&app, PlaneKind::Top, half)).expect("in view");
    app.hover_at(lone);
    app.run_steps(8);
    let hovered = case.frame(&mut app, "hovering");
    assert_axes_hidden(&hovered, &app, "hovering a plane");
    assert_eq!(reference(&app), None);
    pointer_to_chrome(&mut app);
    let image = case.frame(&mut app, "after-hover");
    assert_axes_hidden(&image, &app, "after the pointer leaves");
    click_empty(&mut app);
    assert_eq!(app.state().state.doc.selection, None, "a click on the grid selects nothing");
    assert_eq!(camera(&app), view);
    assert_eq!(model(&app), before);
}

#[test]
fn plane_pick_starts_a_sketch_only_when_the_command_requests_one() {
    let mut case = Case::gpu("plane_pick_and_sketch");
    let mut app = launch(empty());
    let half = measured_half(&mut case, &mut app);
    let world = lone_point(&app, PlaneKind::Front, half);
    let point = projected(&app, world).expect("in view");
    let (on, off) = plane_frames(&mut case, &mut app, "isometric");
    assert_plane_reads(&on, &off, &app, PlaneKind::Front, (half, half), "the plane to pick");

    // Sketch, then the plane: one sketch on that plane, with its grid.
    let features = app.state().state.doc.features.len();
    click_button(&mut app, "Sketch");
    assert!(matches!(app.state().state.editor.as_ref().map(|e| &e.params), Some(EditorParams::PickPlane)));
    click(&mut app, point);
    assert_eq!(app.state().state.doc.features.len(), features + 1, "exactly one sketch");
    let session = app.state().state.sketch.as_ref().expect("the sketch opened");
    let normal = session.plane.normal;
    assert!(normal.y.abs() > 0.999, "on the Front plane: {normal:?}");
    assert!(readout(&app).is_some(), "with its grid");
    click_button(&mut app, "Exit");
    assert!(app.state().state.sketch.is_none());
    click_empty(&mut app);
    click_button(&mut app, "Isometric view (I)");

    // At rest the same click only selects.
    let view = camera(&app);
    let features = app.state().state.doc.features.len();
    let point = projected(&app, world).expect("in view");
    click(&mut app, point);
    assert_eq!(reference(&app), Some(Reference::Plane(PlaneKind::Front)));
    assert_eq!(app.state().state.doc.features.len(), features, "no sketch at rest");
    assert!(app.state().state.editor.is_none() && app.state().state.sketch.is_none(), "no editor");
    pointer_to_chrome(&mut app);
    let image = case.frame(&mut app, "selected-at-rest");
    assert_axes_shown(&image, &app, "with the plane selected");
    assert_eq!(camera(&app), view);
    click_empty(&mut app);

    // The tree row's double click still sketches.
    open_references(&mut app);
    double_click_label(&mut app, "Top plane");
    assert_eq!(app.state().state.doc.features.len(), features + 1, "a double click sketches");
    click_button(&mut app, "Exit");
    click_empty(&mut app);
    click_button(&mut app, "Isometric view (I)");
    drop(app);

    // A pattern asks for an axis: the axes show, and a pick sets its direction.
    let mut app = launch(box_part());
    click_button(&mut app, "Linear");
    let wanted = matches!(app.state().state.editor.as_ref().map(|e| &e.params), Some(EditorParams::Pattern(_)));
    assert!(wanted, "the pattern editor is open");
    pointer_to_chrome(&mut app);
    let image = case.frame(&mut app, "pattern-open");
    assert_axes_shown(&image, &app, "while the pattern asks for an axis");
    let reach = 30.0 * 1.15 + 5.0;
    let on_axis = projected(&app, Vec3::Y * reach * -1.0).expect("in view");
    click(&mut app, on_axis);
    let editor = app.state().state.editor.as_ref().expect("the editor stays open");
    let EditorParams::Pattern(pattern) = &editor.params else { panic!("the pattern editor") };
    let along = format!("{:?}", pattern.layout);
    assert!(along.contains("y: 1.0"), "the pattern runs along Y: {along}");
    escape(&mut app);
    assert!(app.state().state.editor.is_none(), "Escape cancels the editor");
    click_empty(&mut app);
    pointer_to_chrome(&mut app);
    let image = case.frame(&mut app, "pattern-closed");
    assert_axes_hidden(&image, &app, "once the editor closes and the selection clears");
}

/// The tree's eyes reach the scene the view draws, names and hits. A choice wins over the
/// selection and an open tool, and an item never chosen keeps its defaults.
#[test]
fn a_tree_choice_wins_over_the_selection_and_the_tools() {
    let mut case = Case::gpu("tree_choices");
    let mut app = launch(empty());
    let half = measured_half(&mut case, &mut app);
    open_references(&mut app);
    let front = PlaneKind::Front;
    let point = projected(&app, lone_point(&app, front, half)).expect("in view");

    // A hidden plane loses its name and its hit target, and selecting it cannot bring it back.
    click_label(&mut app, "Hide Front plane");
    assert!(!plane_named(&app, front), "the eye takes its name away");
    assert!(plane_named(&app, PlaneKind::Top), "and leaves the others");
    click(&mut app, point);
    assert_eq!(reference(&app), None, "nothing is there to find");
    click_label(&mut app, "Front plane");
    assert_eq!(reference(&app), Some(Reference::Plane(front)), "the row still selects");
    assert!(!plane_named(&app, front), "a selected plane stays hidden");
    click_empty(&mut app);
    click_label(&mut app, "Show Front plane");
    assert!(plane_named(&app, front), "the eye brings it back");
    click(&mut app, point);
    assert_eq!(reference(&app), Some(Reference::Plane(front)), "with its hit target");
    click_empty(&mut app);

    // With the planes off in Preferences, a plane never chosen shows while it is selected.
    set_preference(&mut app, SHOW_PLANES, false);
    assert!(!plane_named(&app, PlaneKind::Top), "the preference hides it");
    assert!(plane_named(&app, front), "and not the plane the eye showed");
    click_label(&mut app, "Top plane");
    assert!(plane_named(&app, PlaneKind::Top), "the selected plane shows");
    assert!(!plane_named(&app, PlaneKind::Right), "by itself");
    click_empty(&mut app);
    assert!(!plane_named(&app, PlaneKind::Top), "and goes with the selection");
    drop(app);

    // A hidden axis stays hidden while a tool asks for the axes.
    let mut app = launch(box_part());
    let shown = |app: &App| soliddraft_app::tree_visibility::visible(&app.state().state).axes;
    open_references(&mut app);
    assert_eq!(shown(&app), [false; 3], "the axes are off at rest");
    click_label(&mut app, &format!("Show {}", AxisKind::X.label()));
    click_label(&mut app, &format!("Hide {}", AxisKind::X.label()));
    assert_eq!(shown(&app), [false; 3], "chosen off now, not by default");
    click_button(&mut app, "Linear");
    assert_eq!(shown(&app), [false, true, true], "the tool brings back the others");
    escape(&mut app);
    click_empty(&mut app);
    assert_eq!(shown(&app), [false; 3], "until it closes");
}
