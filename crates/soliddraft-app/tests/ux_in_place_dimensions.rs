//! Double-click a solid or its feature, and edit a driving dimension where the viewport
//! shows it: the real app, the real wgpu pipeline and the real Catallax kernel, driven by
//! pointer and keyboard events only. The Merge D manifest in
//! `tests/fixtures/ux_tools/in_place_dimensions.json` holds every expected value.

use egui::{Key, Modifiers, Rect, pos2};
use egui_kittest::kittest::{By, Queryable};
use serde_json::Value;
use soliddraft_app::document::{Command, Document, ExtrudeParams, FeatureId, FeatureKind, ProfileKind};
use soliddraft_app::viewport::dimensions::DimensionTarget;

#[path = "support/ribbon.rs"]
mod ribbon;
#[path = "support/ux_tools.rs"]
mod ux_tools;

use ribbon::{App, until};
use ux_tools::{Case, Facts, Look, SMALL_DARK, STANDARD, facts, launch, rebuilt, settle, viewport};

fn manifest() -> Value {
    serde_json::from_str(include_str!("fixtures/ux_tools/in_place_dimensions.json")).expect("the manifest")
}

/// The sizes one expected part drives and measures.
struct Part {
    volume: f64,
    min: [f64; 3],
    max: [f64; 3],
    counts: (usize, usize, usize),
    width: f64,
    height: f64,
    depth: f64,
}

fn part(name: &str) -> Part {
    let e = &manifest()["expected"][name];
    let number = |key: &str| e[key].as_f64().unwrap_or_else(|| panic!("{name}.{key}"));
    let point = |key: &str| [0, 1, 2].map(|i| e[key][i].as_f64().unwrap_or_else(|| panic!("{name}.{key}")));
    let count = |key: &str| e[key].as_u64().unwrap_or_else(|| panic!("{name}.{key}")) as usize;
    Part {
        volume: number("volume"),
        min: point("min"),
        max: point("max"),
        counts: (count("faces"), count("edges"), count("vertices")),
        width: number("width"),
        height: number("height"),
        depth: number("depth"),
    }
}

fn gesture_text(key: &str) -> String {
    manifest()["gestures"][key].as_str().unwrap_or_else(|| panic!("gestures.{key}")).to_owned()
}

/// The manifest's B20 fixture.
fn b20() -> Document {
    let e = part("B20");
    let mut doc = Document::new("B20", soliddraft_app::kernel());
    doc.apply(Command::AddExtrude {
        params: ExtrudeParams {
            profile: ProfileKind::Rectangle { width: e.width, height: e.height },
            depth: e.depth,
            ..Default::default()
        },
        via_assistant: false,
    })
    .unwrap();
    doc.selection = None;
    doc
}

/// The one feature's id and its extrude values.
fn the_extrude(app: &App) -> (FeatureId, ExtrudeParams) {
    let doc = &app.state().state.doc;
    assert_eq!(
        doc.features.len(),
        1,
        "no extra feature: {:?}",
        doc.features.iter().map(|f| &f.name).collect::<Vec<_>>()
    );
    match &doc.features[0].kind {
        FeatureKind::Extrude(p) => (doc.features[0].id, p.clone()),
        kind => panic!("not an extrude: {kind:?}"),
    }
}

fn assert_part(app: &App, expected: &Part, when: &str) {
    let tolerance = &manifest()["tolerance"];
    let (volume, bounds) =
        (tolerance["volume_mm3"].as_f64().expect("a tolerance"), tolerance["bounds_mm"].as_f64().expect("a tolerance"));
    let found = facts(app);
    assert!((found.volume - expected.volume).abs() <= volume, "{when}: volume {}", found.volume);
    for i in 0..3 {
        assert!((found.min[i] - expected.min[i]).abs() <= bounds, "{when}: min {:?}", found.min);
        assert!((found.max[i] - expected.max[i]).abs() <= bounds, "{when}: max {:?}", found.max);
    }
    assert_eq!(found.topology, expected.counts, "{when}: faces, edges and vertices");
    let (_, params) = the_extrude(app);
    let (width, height) = match params.profile {
        ProfileKind::Rectangle { width, height } => (width, height),
        profile => panic!("{when}: not a rectangle: {profile:?}"),
    };
    assert_eq!((width, height, params.depth), (expected.width, expected.height, expected.depth), "{when}: the feature");
    let doc = &app.state().state.doc;
    assert_eq!(doc.display_failure(), None, "{when}: a display failure");
    assert_eq!(doc.error_count(), 0, "{when}: a feature error");
    assert!(doc.triangle_count() > 0, "{when}: an empty mesh");
    let (_, hi) = doc.bounds().expect("mesh bounds");
    assert!((hi.z as f64 - expected.max[2]).abs() <= bounds, "{when}: the mesh is a stale one, top {}", hi.z);
}

/// A dimension as the frame shows it.
#[derive(Clone, Debug)]
struct Annotation {
    rect: Rect,
    value: f64,
    witness: Option<[glam::Vec3; 2]>,
}

/// The painted dimension of that name: its control, its number on screen, and the feature
/// it belongs to.
fn annotation(app: &App, name: &str) -> Annotation {
    let (feature, _) = the_extrude(app);
    let wanted = format!("· {name} ");
    let nodes: Vec<_> = app.query_all(By::new().label_contains(&wanted)).collect();
    assert_eq!(nodes.len(), 1, "one {name} dimension in the viewport");
    let label = nodes[0].value().expect("the text of a label");
    let rect = nodes[0].rect();
    let text = label.rsplit(' ').next().expect("a value").to_owned();
    let state = &app.state().state;
    let feature_name = &state.doc.feature(feature).expect("the feature").name;
    assert!(label.starts_with(feature_name.as_str()), "{label} names the extrusion {feature_name}");
    assert!(viewport(app).expand(1.0).contains_rect(rect), "{name} is in the viewport: {rect:?}");
    let painted = ribbon::painted_in(app, rect);
    assert!(painted.iter().any(|p| p.text == text), "{text} is painted in {rect:?}: {painted:?}");
    let shown = state.viewport.dimensions.iter().find(|d| d.rect.center().distance(rect.center()) < 1.0);
    let shown = shown.unwrap_or_else(|| panic!("{name} at {rect:?}"));
    assert_eq!(shown.target, DimensionTarget::Feature(feature), "{name} belongs to the extrusion");
    Annotation { rect, value: text.parse().expect("a number"), witness: shown.witness }
}

fn assert_annotations(app: &App, expected: &Part, when: &str) {
    for (name, value) in [("Width", expected.width), ("Height", expected.height), ("Depth", expected.depth)] {
        assert_eq!(annotation(app, name).value, value, "{when}: {name}");
    }
}

fn field_label(name: &str) -> String {
    format!("{name} dimension value")
}

/// Double-clicks the value, and returns the input that opens, proved focused, reachable by
/// the pointer, and at its annotation inside the viewport.
fn open_value(app: &mut App, name: &str) -> Rect {
    settle(app);
    let shown = annotation(app, name);
    ribbon::double_click_at(app, shown.rect.center());
    let label = field_label(name);
    until(app, "the focused value", |a| a.query_by_label(&label).is_some_and(|n| n.is_focused()));
    settle(app);
    let input = app.get_by_label(&label).rect();
    let tolerance = manifest()["tolerance"]["control_bounds_points"].as_f64().expect("a tolerance") as f32;
    let view = viewport(app);
    assert!(view.expand(tolerance).contains_rect(input), "{input:?} is inside the viewport {view:?}");
    assert!(input.expand(tolerance).contains(shown.rect.center()), "{input:?} is at {:?}", shown.rect);
    let state = &app.state().state;
    assert!(state.editor.is_none() && state.sketch.is_none(), "the value opens in place, not in a side editor");
    ribbon::click_at(app, input.center());
    assert!(app.get_by_label(&label).is_focused(), "the pointer reaches the input");
    input
}

/// Replaces the focused value's text through the real widget.
fn type_over(app: &mut App, name: &str, text: &str) {
    app.key_press_modifiers(Modifiers::COMMAND, Key::A);
    app.get_by_label(&field_label(name)).type_text(text);
    app.run_steps(3);
}

fn press(app: &mut App, key: Key) {
    app.key_press(key);
    settle(app);
}

/// Enters a value that must apply, and waits out its rebuild.
fn enter_value(app: &mut App, name: &str, text: &str) {
    type_over(app, name, text);
    press(app, Key::Enter);
    rebuilt(app);
    settle(app);
    assert!(app.state().state.dimension_edit.is_none(), "Enter closes the value");
}

/// The manifest's top-face point at a depth.
fn top_face(depth: f64) -> glam::Vec3 {
    let at = &manifest()["gestures"]["top_face"];
    glam::Vec3::new(at[0].as_f64().expect("x") as f32, at[1].as_f64().expect("y") as f32, depth as f32)
}

/// The part's left column, where the tree is.
fn left_column(app: &App) -> Rect {
    let window = ribbon::window(app);
    Rect::from_min_max(window.min, pos2(viewport(app).left(), window.bottom()))
}

fn seeded(case: &Case) -> (Document, std::path::PathBuf) {
    let mut doc = b20();
    let path = case.dir.join("b20.dwprt");
    soliddraft_app::file::save(&mut doc, &path).expect("the fixture saves");
    (doc, path)
}

/// The depth's dimension line was painted, and ends at the depth reached. `with` and
/// `without` are the same view with and without the dimensions.
fn assert_witness(app: &App, shown: &Annotation, with: &image::RgbaImage, without: &image::RgbaImage, depth: f64) {
    let [base, top] = shown.witness.expect("the depth's dimension line");
    assert!((top.z as f64 - depth).abs() <= 0.001 && base.z.abs() <= 0.001, "the line runs {base} to {top}");
    let (from, to) = (ux_tools::screen(app, base), ux_tools::screen(app, top));
    let ppp = app.ctx.pixels_per_point();
    let (changed, samples) = ux_tools::changed_along(with, without, from, to, shown.rect.expand(2.0), ppp);
    assert!(samples >= 8 && changed * 10 >= samples * 8, "the line is painted: {changed} of {samples} samples");
}

/// D1 in one window: face route, Undo, Redo, reopen, then the tree route.
fn edit_in_place(case: &mut Case, look: Look) {
    let (b20_part, b30_part) = (part("B20"), part("B30"));
    let (doc, path) = seeded(case);
    let mut app = launch(doc, look);
    case.note(format!("pair {} look {look:?}", manifest()["pair"]));
    case.run(&mut app, |case, app| {
        let before = facts(app);
        let (feature, _) = the_extrude(app);
        case.note(format!("before {before:?}"));
        case.note("double-click the top face");
        assert!(app.state().state.doc.selection.is_none(), "the fixture starts with no selection");

        ux_tools::double_click_part(app, top_face(b20_part.depth));
        until(app, "the dimensions", |a| ux_tools::body_selected(a) && !a.state().state.viewport.dimensions.is_empty());
        settle(app);
        assert_annotations(app, &b20_part, "after the face double-click");
        assert_eq!(facts(app), before, "showing dimensions changes no geometry");
        case.frame(app, "01-dimensions");

        let input = open_value(app, "Depth");
        case.note(format!("double-click Depth, input at {input:?} in viewport {:?}", viewport(app)));
        case.frame(app, "02-value-open");
        case.note("type the depth, Enter");
        enter_value(app, "Depth", &gesture_text("accepted_depth"));
        let (same, _) = the_extrude(app);
        assert_eq!(same, feature, "the same feature");
        assert_part(app, &b30_part, "after Enter");
        let after = facts(app);
        assert_eq!((after.undo, after.redo), (before.undo + 1, 0), "one more undo step");
        case.note(format!("after {after:?}"));
        assert_annotations(app, &b30_part, "after Enter");

        ux_tools::pointer_to_chrome(app);
        let shown = annotation(app, "Depth");
        let with = case.frame(app, "03-depth-30");
        press(app, Key::Escape);
        assert!(app.state().state.viewport.dimensions.is_empty(), "Escape clears the selection");
        let without = case.frame(app, "04-depth-30-cleared");
        assert_witness(app, &shown, &with, &without, b30_part.depth);

        case.note("Undo, Redo, double-click the top face, File > Save");
        ux_tools::undo(app);
        assert_part(app, &b20_part, "after Undo");
        assert_eq!(facts(app).undo, before.undo, "Undo takes the step back");
        ux_tools::redo(app);
        assert_part(app, &b30_part, "after Redo");
        assert_eq!(facts(app).undo, before.undo + 1, "Redo gives the step back");
        ux_tools::double_click_part(app, top_face(b30_part.depth));
        until(app, "the dimensions", ux_tools::body_selected);
        settle(app);
        assert_annotations(app, &b30_part, "after Redo");

        ux_tools::menu_entry(app, "File", |label| label.starts_with("Save") && !label.contains(" as"));
        assert!(!app.state().state.doc.dirty, "File > Save writes the part");
    });
    drop(app);

    let mut app = launch(soliddraft_app::new_document(), look);
    case.run(&mut app, |case, app| {
        let file = path.file_name().expect("a file name").to_string_lossy().into_owned();
        case.note("fresh app, File > Open recent");
        ux_tools::open_recent(app, &file);
        until(app, "the reopened part", |a| {
            a.state().state.doc.path.as_deref().and_then(|p| p.file_name()) == path.file_name()
        });
        rebuilt(app);
        assert_part(app, &b30_part, "after reopening");
        ux_tools::double_click_part(app, top_face(b30_part.depth));
        until(app, "the dimensions", ux_tools::body_selected);
        settle(app);
        assert_annotations(app, &b30_part, "after reopening");
        case.frame(app, "05-reopened");
    });
    drop(app);

    let mut app = launch(b20(), look);
    case.run(&mut app, |case, app| {
        let before = facts(app);
        let (feature, _) = the_extrude(app);
        let name = app.state().state.doc.feature(feature).expect("the feature").name.clone();
        let row = ribbon::button_rect(app, &name, left_column(app));
        case.note(format!("fresh app, double-click the row at {row:?}"));
        settle(app);
        ribbon::double_click_at(app, row.center());
        until(app, "the dimensions", |a| !a.state().state.viewport.dimensions.is_empty());
        settle(app);
        assert_annotations(app, &b20_part, "after the row double-click");
        assert_eq!(facts(app), before, "the row double-click changes no geometry");
        let state = &app.state().state;
        assert!(state.rename.is_none(), "the row double-click does not rename");
        assert!(state.editor.is_none(), "the row double-click does not open the side editor");
        assert!(state.doc.triangle_count() > 0, "the part stays visible");
        case.frame(app, "06-row-dimensions");

        open_value(app, "Depth");
        enter_value(app, "Depth", &gesture_text("accepted_depth"));
        assert_part(app, &b30_part, "after the row route");
        assert_eq!(facts(app).undo, before.undo + 1, "one more undo step");
        assert_annotations(app, &b30_part, "after the row route");
        case.frame(app, "07-row-depth-30");
    });
}

#[test]
fn double_click_edit_rebuilds_the_existing_depth_in_place() {
    let mut case = Case::new("d1-double-click-edit");
    edit_in_place(&mut case, STANDARD);
}

#[test]
fn double_click_edit_rebuilds_the_existing_depth_in_place_small_dark() {
    let mut case = Case::new("d1-double-click-edit-small-dark");
    edit_in_place(&mut case, SMALL_DARK);
}

/// The text a refused value added to the frame, beside the input.
fn refusal(app: &App, input: Rect, before: &[String], typed: &str) -> String {
    let added: Vec<ribbon::Painted> =
        ribbon::painted(app).into_iter().filter(|p| !before.contains(&p.text) && p.text != typed).collect();
    let local = added.iter().find(|p| {
        let below = p.rect.top() >= input.top() && p.rect.top() <= input.bottom() + 24.0;
        below && p.rect.right() >= input.left() - 24.0 && p.rect.left() <= input.right() + 24.0
    });
    let local = local.unwrap_or_else(|| panic!("no refusal beside {input:?} for {typed}: {added:?}"));
    assert!(viewport(app).expand(1.0).contains_rect(local.rect), "the refusal is in the viewport: {local:?}");
    local.text.clone()
}

fn show_dimensions(app: &mut App, depth: f64) {
    ux_tools::double_click_part(app, top_face(depth));
    until(app, "the dimensions", |a| ux_tools::body_selected(a) && !a.state().state.viewport.dimensions.is_empty());
    settle(app);
}

fn assert_closed(app: &App, before: &Facts, when: &str) {
    assert_eq!(&facts(app), before, "{when}: the accepted part");
    let state = &app.state().state;
    assert!(state.dimension_edit.is_none(), "{when}: the value is closed");
    assert!(state.editor.is_none() && state.sketch.is_none(), "{when}: no tool started");
    assert!(state.preview_body().is_none(), "{when}: no preview remains");
}

#[test]
fn escape_and_invalid_text_preserve_the_accepted_part() {
    let mut case = Case::new("d2-escape-and-invalid");
    let (b20_part, b30_part, w70_part) = (part("B20"), part("B30"), part("W70"));
    let mut app = launch(b20(), STANDARD);
    case.run(&mut app, |case, app| {
        let before = facts(app);
        case.note(format!("before {before:?}"));
        show_dimensions(app, b20_part.depth);
        open_value(app, "Depth");
        type_over(app, "Depth", &gesture_text("cancelled_depth"));
        press(app, Key::Escape);
        assert_closed(app, &before, "after Escape");
        assert!(ux_tools::body_selected(app), "Escape closes the value, then the dimensions stay");
        assert_annotations(app, &b20_part, "after Escape");

        for typed in manifest()["gestures"]["refused"].as_array().expect("refused values") {
            let typed = typed.as_str().expect("a string");
            let input = open_value(app, "Depth");
            case.note(format!("type {typed}, Enter, input at {input:?}"));
            let shown: Vec<String> = ribbon::painted(app).into_iter().map(|p| p.text).collect();
            type_over(app, "Depth", typed);
            press(app, Key::Enter);
            assert_eq!(facts(app), before, "{typed} never reaches the document");
            assert!(app.state().state.dimension_edit.is_some(), "{typed} leaves the value open");
            let said = refusal(app, input, &shown, typed);
            case.note(format!("{typed} refused: {said}"));
            case.frame(app, &format!("refused-{}", typed.replace('-', "minus")));
            press(app, Key::Escape);
            assert_closed(app, &before, "after a refusal");
        }

        open_value(app, "Depth");
        type_over(app, "Depth", "abc");
        press(app, Key::Enter);
        assert_eq!(facts(app), before, "a refused value");
        let label = field_label("Depth");
        assert!(app.get_by_label(&label).is_focused(), "the refusal keeps the keyboard at the value");
        enter_value(app, "Depth", &gesture_text("accepted_depth"));
        assert_part(app, &b30_part, "after the corrected value");
        assert_eq!(facts(app).undo, before.undo + 1, "one more undo step");
    });
    drop(app);

    let mut app = launch(b20(), STANDARD);
    case.run(&mut app, |case, app| {
        let before = facts(app);
        show_dimensions(app, b20_part.depth);
        open_value(app, "Width");
        case.frame(app, "width-open");
        enter_value(app, "Width", &gesture_text("accepted_width"));
        assert_part(app, &w70_part, "after the width edit");
        assert_eq!(facts(app).undo, before.undo + 1, "one more undo step");
        assert_annotations(app, &w70_part, "after the width edit");
        case.frame(app, "width-70");
    });
}
