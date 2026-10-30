//! Merge 1 of the ribbon layout: the command line and the status share one bottom row.
//! Acceptance cases B1 and B2 of the ribbon test design, on the real app, the real wgpu
//! pipeline and the real kernel, with the gestures a user makes.
//!
//! Requires a renderer: a run without one fails.
//!
//! Run them one at a time, away from the other GPU targets:
//! `cargo test -p soliddraft-app --test ribbon_bottom -- --test-threads=1`.

#[path = "support/ribbon.rs"]
mod ribbon;

use egui::{Key, Modifiers, Rect, pos2};
use ribbon::{App, TOLERANCE};
use soliddraft_app::state::EditorParams;
use soliddraft_app::viewport::camera::StandardView;
use soliddraft_drawing::Projection;

/// The tallest the combined row may be.
const BAND_HEIGHT: f32 = 32.0;
/// How far the viewport reached down on main before the merge, at 1440 by 900. Measured
/// on the baseline, frozen here so the recovered room is not read from the code under test.
const MAIN_VIEWPORT_BOTTOM: f32 = 844.0;
/// What the merge gives back to the viewport.
const RECOVERED: f32 = 22.0;

fn viewport(app: &App) -> Rect {
    app.state().state.viewport.rect
}

fn same_rect(a: Rect, b: Rect) -> bool {
    (a.min - b.min).length() <= TOLERANCE && (a.max - b.max).length() <= TOLERANCE
}

fn settle(app: &mut App) {
    app.run_steps(30);
}

/// The one painted piece of this text in the band, which must sit beside the input.
fn once_beside_the_input(app: &App, text: &str) {
    let band = ribbon::band(app);
    let input = ribbon::input_rect(app);
    let found: Vec<_> = ribbon::painted(app).into_iter().filter(|p| p.text == text).collect();
    assert_eq!(found.len(), 1, "one visible {text}: {found:?}");
    let shown = found[0].rect;
    assert!(band.expand(TOLERANCE).contains_rect(shown), "{text} {shown:?} is in the band {band:?}");
    assert!(!shown.intersects(input), "{text} {shown:?} does not cover the input {input:?}");
    assert!(
        shown.center().y >= input.top() - TOLERANCE && shown.center().y <= input.bottom() + TOLERANCE,
        "{text} {shown:?} is on the input's row {input:?}"
    );
}

/// Nothing is painted under the viewport outside the band: one row, not two.
fn only_one_row(app: &App) {
    let band = ribbon::band(app);
    let (view, window) = (viewport(app), ribbon::window(app));
    let below = Rect::from_min_max(pos2(window.left(), view.bottom()), window.max);
    for text in ribbon::painted_in(app, below) {
        assert!(band.expand(TOLERANCE).contains_rect(text.rect), "{text:?} is outside the band {band:?}");
    }
}

fn band_is_one_low_row(app: &App) {
    let band = ribbon::band(app);
    let window = ribbon::window(app);
    assert!(band.height() <= BAND_HEIGHT, "the band is at most {BAND_HEIGHT} tall: {band:?}");
    assert!((band.bottom() - window.bottom()).abs() <= TOLERANCE, "the band {band:?} touches the bottom of {window:?}");
    only_one_row(app);
}

/// The tree's and the properties' titles stay in the left column, in that order.
fn properties_under_the_tree(app: &App) {
    let view = viewport(app);
    let tree = ribbon::title_rect(app, "Feature tree");
    let properties = ribbon::title_rect(app, "Properties");
    assert!(tree.right() <= view.left() + TOLERANCE, "the tree is left of the viewport: {tree:?} {view:?}");
    assert!(properties.right() <= view.left() + TOLERANCE, "so are the properties: {properties:?} {view:?}");
    assert!(properties.top() > tree.bottom(), "the properties are below the tree: {properties:?} {tree:?}");
    let right = Rect::from_min_max(pos2(view.right() + TOLERANCE, 0.0), ribbon::window(app).max);
    assert!(ribbon::painted_in(app, right).is_empty(), "nothing is right of the viewport");
}

/// A button of an open editor, in the left column.
fn in_the_left_column(app: &App, label: &str) -> Rect {
    let view = viewport(app);
    let left = Rect::from_min_max(pos2(0.0, view.top()), pos2(view.left(), view.bottom()));
    ribbon::button_rect(app, label, left)
}

fn cancel_the_editor(app: &mut App) {
    let cancel = in_the_left_column(app, "Cancel (Esc)");
    ribbon::click_at(app, cancel.center());
    settle(app);
    assert!(app.state().state.editor.is_none(), "Cancel closes the editor");
}

#[test]
fn command_and_status_share_one_bottom_band() {
    let case = ribbon::case("b1");
    let mut app = ribbon::app(ribbon::blank());
    case.run(&mut app, |app| {
        settle(app);
        let start = ribbon::fingerprint(app).without_history();

        // Status and input in one row.
        once_beside_the_input(app, "Rebuild OK");
        once_beside_the_input(app, "mm");
        for text in ["0 features", "0 triangles", "Editing part"] {
            assert!(ribbon::status_lines(app).iter().any(|l| l == text), "the status says {text}");
        }
        settle(app);
        band_is_one_low_row(app);

        // The room comes back to the viewport.
        let view = viewport(app);
        assert!(
            view.bottom() >= MAIN_VIEWPORT_BOTTOM + RECOVERED,
            "the viewport gained {RECOVERED} on {MAIN_VIEWPORT_BOTTOM}: {view:?}"
        );

        // A typed command opens its editor, which stays in the left column.
        ribbon::type_command(app, "extrude");
        settle(app);
        let state = &app.state().state;
        assert!(matches!(state.editor.as_ref().map(|e| &e.params), Some(EditorParams::Extrude(_))));
        assert!(state.doc.features.is_empty(), "nothing is committed yet");
        let tree = ribbon::title_rect(app, "Feature tree");
        let cancel = in_the_left_column(app, "Cancel (Esc)");
        assert!(cancel.top() > tree.bottom(), "the editor's controls are below the tree: {cancel:?} {tree:?}");
        assert!(ribbon::status_lines(app).iter().any(|l| l == "Editing Extrude"));
        cancel_the_editor(app);
        assert_eq!(ribbon::fingerprint(app).without_history(), start, "cancelling leaves the part as it was");

        // A refused command says why and changes nothing.
        ribbon::type_command(app, "chamfer");
        assert!(app.state().state.editor.is_none());
        assert_eq!(ribbon::fingerprint(app).without_history(), start, "a refused command leaves the part as it was");
        let said = "Chamfer: Chamfer needs a body";
        assert!(ribbon::status_lines(app).iter().any(|l| l == said), "the status says {said}");

        // The compressed input still takes history and canonical ids.
        let input = ribbon::input_rect(app);
        ribbon::click_at(app, input.center());
        app.key_press(Key::ArrowUp);
        app.run_steps(3);
        assert_eq!(app.state().state.command_line.input, "chamfer", "Up recalls the line");
        app.key_press(Key::Escape);
        app.run_steps(3);
        assert_eq!(app.state().state.command_line.input, "", "Escape clears the line");
        assert!(app.state().state.editor.is_none(), "without running it");
        ribbon::type_command(app, "view_top");
        settle(app);
        let state = &app.state().state;
        let (yaw, pitch) = StandardView::Top.angles();
        assert!((state.viewport.camera.yaw - yaw.to_radians()).abs() < 1e-3, "{}", state.viewport.camera.yaw);
        assert!((state.viewport.camera.pitch - pitch.to_radians()).abs() < 1e-3, "{}", state.viewport.camera.pitch);
        assert_eq!(state.command_line.history.last().map(String::as_str), Some("view_top"));

        // The session opens above the same row and gives its room back.
        let collapsed = viewport(app);
        let band = ribbon::band(app);
        app.key_press_modifiers(Modifiers::SHIFT, Key::F9);
        settle(app);
        assert!(app.state().state.command_line.session_open, "Shift+F9 opens the session");
        let open = viewport(app);
        assert!(open.bottom() < collapsed.bottom() - 100.0, "the session sits above the row: {open:?}");
        assert!(open.right() >= ribbon::window(app).right() - TOLERANCE, "no assistant on the right: {open:?}");
        let session = ribbon::painted(app);
        assert!(session.iter().any(|p| p.text == "Rebuild OK" && band.expand(TOLERANCE).contains_rect(p.rect)));
        app.key_press_modifiers(Modifiers::SHIFT, Key::F9);
        settle(app);
        assert!(!app.state().state.command_line.session_open, "Shift+F9 closes it");
        assert!(same_rect(viewport(app), collapsed), "the viewport comes back: {:?}", viewport(app));
        band_is_one_low_row(app);
    });
}

#[test]
fn compact_status_tracks_the_active_document_without_moving_properties() {
    let case = ribbon::case("b2");
    let mut app = ribbon::app(ribbon::plate());
    case.run(&mut app, |app| {
        settle(app);

        // The part.
        band_is_one_low_row(app);
        let triangles = format!("{} triangles", app.state().state.doc.triangle_count());
        let lines = ribbon::status_lines(app);
        for text in ["Rebuild OK", "1 feature", "Editing part", triangles.as_str()] {
            assert!(lines.iter().any(|l| l == text), "the part says {text}: {lines:?}");
        }
        settle(app);
        band_is_one_low_row(app);

        // Its drawing, through the existing flow and the default template.
        ribbon::menu(app, "File", "New drawing from part…");
        settle(app);
        let create = in_the_left_column(app, "Create drawing (Enter)");
        ribbon::click_at(app, create.center());
        settle(app);
        assert!(app.state().state.drawing.is_active(), "the drawing is the active document");
        let (sheet, format, link) = {
            let s = app.state().state.drawing.session.as_ref().expect("a drawing session");
            let sheet = s.sheet();
            let projection = match sheet.projection {
                Projection::FirstAngle => "first angle",
                Projection::ThirdAngle => "third angle",
            };
            (
                format!("Sheet {} of {}", s.sheet + 1, s.doc.drawing.sheets.len()),
                format!("{} · {} · {projection}", sheet.size.name, sheet.scale),
                format!("Up to date with {} rev {}", s.doc.drawing.meta.title, s.doc.drawing.model.pinned.short()),
            )
        };
        let lines = ribbon::status_lines(app);
        for text in [&sheet, &format, &link] {
            assert!(lines.contains(text), "the drawing says {text}: {lines:?}");
        }
        assert!(!lines.iter().any(|l| l == "Editing part"), "the part's context is gone: {lines:?}");
        settle(app);
        band_is_one_low_row(app);

        // An empty assembly.
        ribbon::menu(app, "File", "New assembly");
        settle(app);
        assert!(app.state().state.assembly.is_active(), "the assembly is the active document");
        let lines = ribbon::status_lines(app);
        for text in ["Assembly", "0 components", "0 parts placed", "Editing assembly"] {
            assert!(lines.iter().any(|l| l == text), "the assembly says {text}: {lines:?}");
        }
        assert!(!lines.iter().any(|l| l == "Rebuild OK"), "the part's rebuild is not the assembly's: {lines:?}");
        settle(app);
        band_is_one_low_row(app);

        // Back to the part, whose context returns.
        ribbon::document_tab(app, "Ribbon acceptance");
        settle(app);
        assert!(!app.state().state.assembly.is_active() && !app.state().state.drawing.is_active());
        let lines = ribbon::status_lines(app);
        for text in ["Rebuild OK", "1 feature", "Editing part"] {
            assert!(lines.iter().any(|l| l == text), "the part says {text} again: {lines:?}");
        }
        settle(app);
        band_is_one_low_row(app);

        // An editor changes nothing in the arrangement.
        let before = viewport(app);
        properties_under_the_tree(app);
        ribbon::type_command(app, "extrude");
        settle(app);
        assert!(matches!(app.state().state.editor.as_ref().map(|e| &e.params), Some(EditorParams::Extrude(_))));
        assert!(same_rect(viewport(app), before), "open: {:?} from {before:?}", viewport(app));
        properties_under_the_tree(app);
        cancel_the_editor(app);
        assert!(same_rect(viewport(app), before), "cancelled: {:?} from {before:?}", viewport(app));
        properties_under_the_tree(app);
    });
}
