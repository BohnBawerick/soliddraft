//! Origin axes on demand: hidden at rest in every part, shown while a reference is
//! selected, through the real tree and the real renderer.
//!
//! The design is `sdc-d-ux-viewport`, merge A.

#[path = "support/viewport_acceptance.rs"]
mod support;

use egui::{pos2, vec2};
use egui_kittest::kittest::Queryable;
use soliddraft_app::document::{AxisKind, PlaneKind, Reference, Selection};
use soliddraft_app::state::AppAction;
use support::*;

/// The view cube still turns the view: its top face takes the camera to Top, and the
/// heads-up toolbar brings it back.
fn assert_cube_works(app: &mut App) {
    let rect = viewport(app);
    let cube = pos2(rect.right() - 60.0, rect.top() + 62.0);
    click(app, cube + vec2(0.0, -10.0));
    assert_eq!(app.state().state.viewport.camera.view_name, "Top", "the view cube turned the view");
    click_button(app, "Isometric view (I)");
    assert_eq!(app.state().state.viewport.camera.view_name, "Isometric");
}

fn assert_hidden_at_rest(case: &mut Case, app: &mut App, name: &str) {
    assert_eq!(reference(app), None, "{name}: nothing selected");
    assert!(!app.state().state.viewport.references.always_show_origin_axes, "{name}: the default is off");
    pointer_to_chrome(app);
    let image = case.frame(app, &format!("{name}-rest"));
    case.note(format!("{name} rest: rays {:?}, corner {}", rays(&image, app), corner_triad_pixels(&image, app)));
    assert_axes_hidden(&image, app, name);
    assert_cube_works(app);
}

#[test]
fn origin_axes_are_hidden_in_a_fresh_and_old_part() {
    {
        let mut case = Case::gpu("origin_axes_are_hidden_fresh");
        let mut app = launch(empty());
        assert_hidden_at_rest(&mut case, &mut app, "fresh");

        // The assembly viewport has the same rest.
        click_menu(&mut app, "File", "New assembly");
        assert!(app.state().state.assembly.is_active(), "File > New assembly opened");
        pointer_to_chrome(&mut app);
        let image = case.frame(&mut app, "assembly-rest");
        let corner = corner_triad_pixels(&image, &app);
        case.note(format!("assembly rest: corner {corner}"));
        assert_eq!(corner, 0, "no resting triad in the assembly viewport");

        // Back on the part tab, whose title sits at the left of the bar over the viewport.
        let rect = viewport(&app);
        click(&mut app, pos2(rect.left() + 24.0, rect.top() - 14.0));
        assert!(!app.state().state.assembly.is_active(), "the part tab is active again");

        // The disclosure control is not a selection.
        click_label(&mut app, "Origin, planes and axes");
        assert!(app.query_by_label("Origin").is_some(), "the group opened");
        click_label(&mut app, "Origin, planes and axes");
        assert!(app.query_by_label("Origin").is_none(), "the group closed");
        pointer_to_chrome(&mut app);
        let image = case.frame(&mut app, "fresh-after-group");
        assert_axes_hidden(&image, &app, "after the group opened and closed");
    }
    {
        let mut case = Case::gpu("origin_axes_are_hidden_old");
        let (file, bytes) = old_part(&case);
        let mut app = launch(empty());
        app.state_mut().state.push(AppAction::Open(Some(file.clone())));
        while app.state().state.doc.features.is_empty() {
            app.step();
        }
        settle(&mut app);
        assert_hidden_at_rest(&mut case, &mut app, "old");
        click_label(&mut app, "Origin, planes and axes");
        click_label(&mut app, "Origin, planes and axes");
        pointer_to_chrome(&mut app);
        let image = case.frame(&mut app, "old-after-group");
        assert_axes_hidden(&image, &app, "after the group opened and closed");
        assert_eq!(std::fs::read(&file).unwrap(), bytes, "opening leaves the file as it was");
    }
}

#[test]
fn origin_leaf_selection_shows_axes_and_clearing_hides_them() {
    let mut case = Case::gpu("origin_leaf_selection");
    let mut app = launch(empty());
    let before = model(&app);
    let view = camera(&app);
    click_label(&mut app, "Origin, planes and axes");
    assert_eq!(reference(&app), None, "opening the group selects nothing");

    click_label(&mut app, "Origin");
    assert_eq!(reference(&app), Some(Reference::Origin), "the Origin row selects the origin");
    assert!(row_selected(&app, "Origin"), "the row shows selected");
    assert!(app.state().state.editor.is_none() && app.state().state.sketch.is_none(), "no editor and no sketch");
    pointer_to_chrome(&mut app);
    let image = case.frame(&mut app, "origin-selected");
    case.note(format!("origin selected: rays {:?}", rays(&image, &app)));
    assert_axes_shown(&image, &app, "with the origin selected");
    assert_eq!(camera(&app), view, "selecting does not move the camera");
    assert_eq!(model(&app), before, "selecting does not touch the model");

    click_empty(&mut app);
    assert_eq!(reference(&app), None, "an empty click clears the origin");
    pointer_to_chrome(&mut app);
    let image = case.frame(&mut app, "origin-cleared-by-click");
    assert_axes_hidden(&image, &app, "after an empty click");

    click_label(&mut app, "Origin");
    assert_eq!(reference(&app), Some(Reference::Origin));
    pointer_to_chrome(&mut app);
    let image = case.frame(&mut app, "origin-selected-again");
    assert_axes_shown(&image, &app, "with the origin selected again");
    escape(&mut app);
    assert_eq!(reference(&app), None, "Escape clears the origin");
    let image = case.frame(&mut app, "origin-cleared-by-escape");
    assert_axes_hidden(&image, &app, "after Escape");
    assert_eq!(camera(&app), view);
    assert_eq!(model(&app), before);
}

#[test]
fn plane_and_axis_tree_selection_controls_the_triad() {
    let mut case = Case::gpu("plane_and_axis_tree_selection");
    let mut app = launch(empty());
    let before = model(&app);
    let view = camera(&app);
    click_label(&mut app, "Origin, planes and axes");

    let rows = [
        ("Front plane", Reference::Plane(PlaneKind::Front)),
        ("Top plane", Reference::Plane(PlaneKind::Top)),
        ("Right plane", Reference::Plane(PlaneKind::Right)),
        ("X axis", Reference::Axis(AxisKind::X)),
        ("Y axis", Reference::Axis(AxisKind::Y)),
        ("Z axis", Reference::Axis(AxisKind::Z)),
    ];
    for (label, expected) in rows {
        // Hovering a row alone latches nothing.
        app.get_by_label(label).hover();
        app.run_steps(8);
        let image = case.frame(&mut app, &format!("{label}-hover"));
        assert_axes_hidden(&image, &app, &format!("hovering {label}"));
        assert_eq!(reference(&app), None);

        click_label(&mut app, label);
        assert_eq!(reference(&app), Some(expected), "{label} is the selected reference");
        assert!(row_selected(&app, label), "{label} shows selected");
        for (other, _) in rows.iter().filter(|(other, _)| *other != label) {
            assert!(!row_selected(&app, other), "only {label} shows selected, not {other}");
        }
        let state = &app.state().state;
        assert!(state.editor.is_none() && state.sketch.is_none(), "{label}: no editor and no sketch");
        pointer_to_chrome(&mut app);
        let image = case.frame(&mut app, &format!("{label}-selected"));
        case.note(format!("{label} selected: rays {:?}", rays(&image, &app)));
        assert_axes_shown(&image, &app, &format!("with {label} selected"));
        assert_eq!(camera(&app), view, "{label}: the camera stays");
        assert_eq!(model(&app), before, "{label}: the model stays");

        click_empty(&mut app);
        assert_eq!(reference(&app), None, "an empty click clears {label}");
        pointer_to_chrome(&mut app);
        let image = case.frame(&mut app, &format!("{label}-cleared"));
        assert_axes_hidden(&image, &app, &format!("after clearing {label}"));
    }
    drop(app);

    // A model selection takes over from a reference.
    let mut app = launch(box_part());
    for row in ["Solid body", "Boss-Extrude1"] {
        open_references(&mut app);
        click_label(&mut app, "Origin");
        assert_eq!(reference(&app), Some(Reference::Origin));
        pointer_to_chrome(&mut app);
        let image = case.frame(&mut app, &format!("box-origin-before-{row}"));
        assert_axes_shown(&image, &app, "with the origin selected on the box");

        // The open group pushes the feature rows out of the tree's view. Closing it is a
        // disclosure, so the origin stays selected.
        click_label(&mut app, "Origin, planes and axes");
        assert_eq!(reference(&app), Some(Reference::Origin), "closing the group keeps the selection");
        click_label(&mut app, row);
        assert_eq!(reference(&app), None, "selecting {row} clears the reference");
        let selection = app.state().state.doc.selection;
        assert!(matches!(selection, Some(Selection::Body | Selection::Feature(_))), "{row} is selected: {selection:?}");
        pointer_to_chrome(&mut app);
        let image = case.frame(&mut app, &format!("box-{row}-selected"));
        assert_axes_hidden(&image, &app, &format!("with {row} selected"));
        click_empty(&mut app);
    }
}

#[test]
fn a_saved_always_on_choice_shows_the_axes_with_nothing_selected() {
    let mut case = Case::gpu("saved_always_on");
    case.seed_settings(r#"{"view":{"always_show_origin_axes":true}}"#);
    let mut app = launch(empty());
    assert_eq!(reference(&app), None);
    assert!(app.state().state.viewport.references.always_show_origin_axes);
    pointer_to_chrome(&mut app);
    let image = case.frame(&mut app, "always-on");
    case.note(format!("always on: rays {:?}", rays(&image, &app)));
    assert_axes_shown(&image, &app, "with the axes kept on");
}
