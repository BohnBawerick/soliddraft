//! Ribbon merge 4: every command tile has its label under its icon, and every command stays
//! within reach of the pointer when the window is too narrow to show them all.
//!
//! Real app, real kernel, real pointer and keyboard. What the ribbon does with a narrow
//! window is read from the frame: a command is either on the ribbon, or behind a button
//! named after its group that opens a row of the same tiles.

#[path = "support/ribbon.rs"]
mod ribbon;

use egui::{Color32, Key, Modifiers, Pos2, Rect, Vec2, pos2, vec2};
use egui_kittest::kittest::Queryable;
use ribbon::{App, Case, Control, Painted, TOLERANCE};
use soliddraft_app::commands::{self, CommandSpec, Tab};
use soliddraft_app::document::{Command, Document, PlaneKind, SketchPlane};
use soliddraft_app::drawing::DrawingTab;
use soliddraft_app::panels;
use soliddraft_app::settings;
use soliddraft_app::state::{EditorParams, RibbonTab};
use soliddraft_app::theme::{self, Mode};

const SCALES: [f32; 3] = [1.0, 1.5, 2.0];
const MODES: [Mode; 2] = [Mode::Light, Mode::Dark];
const NARROW: [Vec2; 2] = [vec2(1280.0, 800.0), vec2(960.0, 600.0)];

const PART_TABS: [(&str, RibbonTab); 5] = [
    ("Solid", RibbonTab::Solid),
    ("Sketch", RibbonTab::Sketch),
    ("Assembly", RibbonTab::Assembly),
    ("Inspect", RibbonTab::Inspect),
    ("View", RibbonTab::View),
];

const SOLID_CAPTIONS: [&str; 6] = ["Sketch", "Create", "Remove", "Modify", "Pattern", "Reference"];

/// WCAG's floor for text, and its floor for large or incidental text, which a label that
/// says what is off has to clear to stay readable.
const MIN_CONTRAST: f32 = 4.5;
const MIN_DISABLED_CONTRAST: f32 = 3.0;

/// The room under the ribbon a row of tiles takes while it is open.
const ROW_HEIGHT: f32 = 120.0;

type IsEditor = fn(&EditorParams) -> bool;

/// Solid's starters on a plate: where they are, and the editor each one opens.
const PLATE_STARTERS: [(&str, IsEditor); 10] = [
    ("Hole", |p| matches!(p, EditorParams::Hole(_))),
    ("Fillet", |p| matches!(p, EditorParams::Fillet(_))),
    ("Chamfer", |p| matches!(p, EditorParams::Chamfer(_))),
    ("Lattice", |p| matches!(p, EditorParams::Lattice(_))),
    ("Gyroid", |p| matches!(p, EditorParams::Gyroid(_))),
    ("Shell", |p| matches!(p, EditorParams::Shell(_))),
    ("Draft", |p| matches!(p, EditorParams::Draft(_))),
    ("Linear", |p| matches!(p, EditorParams::Pattern(_))),
    ("Circular", |p| matches!(p, EditorParams::Pattern(_))),
    ("Mirror", |p| matches!(p, EditorParams::Mirror(_))),
];

/// Solid's starters that take a selected sketch, and whether it has to sit on a body.
const SKETCH_STARTERS: [(&str, bool, IsEditor); 3] = [
    ("Extrude", false, |p| matches!(p, EditorParams::Extrude(_))),
    ("Revolve", true, |p| matches!(p, EditorParams::Revolve(_))),
    ("Cut", true, |p| matches!(p, EditorParams::Cut(_))),
];

/// What each walk may run: no file dialogs, nothing that leaves the fixture or hides a panel.
const INSPECT_RUNS: [&str; 4] = ["measure", "mass_properties", "check", "draft_check"];
const VIEW_RUNS: [&str; 9] = [
    "view_isometric",
    "zoom_fit",
    "view_front",
    "view_top",
    "view_right",
    "display_shaded_edges",
    "display_shaded",
    "display_wireframe",
    "orthographic",
];
const SKETCH_RUNS: [&str; 6] =
    ["sketch_line", "sketch_circle", "sketch_rectangle", "sketch_arc", "sketch_slot", "sketch_point"];

fn settle(app: &mut App) {
    app.run_steps(30);
}

/// The ribbon's assigned band: under the tabs, over everything else.
fn body(app: &App) -> Rect {
    let window = ribbon::window(app);
    let top = window.top() + theme::MENU_HEIGHT + theme::RIBBON_TABS_HEIGHT;
    Rect::from_min_size(pos2(window.left(), top), vec2(window.width(), theme::RIBBON_BODY_HEIGHT))
}

/// Where a group's row opens, under the ribbon.
fn row_band(app: &App) -> Rect {
    let body = body(app);
    Rect::from_min_size(body.left_bottom(), vec2(body.width(), ROW_HEIGHT))
}

fn editor(app: &App) -> Option<EditorParams> {
    app.state().state.editor.as_ref().map(|e| e.params.clone())
}

fn popup_open(app: &App) -> bool {
    egui::Popup::is_any_open(&app.ctx)
}

fn group_button(group: &str) -> String {
    format!("{group} group")
}

/// A command the registry places on a tab.
struct Entry {
    group: &'static str,
    command: &'static CommandSpec,
    label: String,
}

fn entries(app: &App, tab: Tab) -> Vec<Entry> {
    let state = &app.state().state;
    let mut found = Vec::new();
    for (group, items) in commands::ribbon(tab) {
        for (command, _) in items {
            if (command.shown)(state) {
                found.push(Entry { group, command, label: command.label_in(state) });
            }
        }
    }
    found
}

fn entry(app: &App, tab: Tab, label: &str) -> Entry {
    entries(app, tab).into_iter().find(|e| e.label == label).unwrap_or_else(|| panic!("no {label} on the tab"))
}

/// A command as the frame shows it.
struct Tile {
    control: Control,
    icon: Rect,
    label: Painted,
}

fn tile(app: &App, label: &str, within: Rect) -> Tile {
    let control = ribbon::control(app, label, within);
    let icon = ribbon::art_on(app, control.rect, label);
    assert!(icon.is_positive(), "the button {label} shows its art");
    let texts: Vec<Painted> =
        ribbon::painted(app).into_iter().filter(|p| p.text == label && control.rect.intersects(p.full)).collect();
    assert_eq!(texts.len(), 1, "the button {label} paints its label once at {:?}", control.rect);
    let label = texts.into_iter().next().unwrap();
    Tile { control, icon, label }
}

fn linear(channel: u8) -> f32 {
    let c = channel as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn luminance(c: Color32) -> f32 {
    0.2126 * linear(c.r()) + 0.7152 * linear(c.g()) + 0.0722 * linear(c.b())
}

/// The WCAG contrast of a colour, with its alpha, on an opaque ground.
fn contrast(color: Color32, ground: Color32) -> f32 {
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    let a = a as f32 / 255.0;
    let mix = |over: u8, under: u8| (over as f32 * a + under as f32 * (1.0 - a)).round() as u8;
    let shown = Color32::from_rgb(mix(r, ground.r()), mix(g, ground.g()), mix(b, ground.b()));
    let (x, y) = (luminance(shown), luminance(ground));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// The label rule: readable, below the icon, centred with it, and all of it on the button.
fn label_below(tile: &Tile, region: Rect, what: &str) {
    let Tile { control, icon, label } = tile;
    let button = control.rect;
    let name = format!("{what}, {}", label.text);
    assert!(!label.text.trim().is_empty(), "{name}: a label");
    assert!(label.rows.len() <= 2, "{name}: at most two lines: {:?}", label.rows);
    assert_eq!(label.rows.join(" "), label.text, "{name}: wraps between words");
    assert!(label.full.top() >= icon.bottom() - TOLERANCE, "{name}: label {:?} under icon {icon:?}", label.full);
    let centre = button.center().x;
    assert!((icon.center().x - centre).abs() <= TOLERANCE, "{name}: icon {icon:?} centred on {button:?}");
    assert!(
        (label.full.center().x - centre).abs() <= TOLERANCE,
        "{name}: label {:?} centred on {button:?}",
        label.full
    );
    assert!(button.expand(TOLERANCE).contains_rect(*icon), "{name}: {icon:?} is on {button:?}");
    assert!(button.expand(TOLERANCE).contains_rect(label.full), "{name}: {:?} is on {button:?}", label.full);
    assert!(region.expand(TOLERANCE).contains_rect(button), "{name}: {button:?} is inside {region:?}");
    assert!(
        (label.full.size() - label.rect.size()).length() <= TOLERANCE,
        "{name}: the whole label shows: {:?} of {:?}",
        label.rect,
        label.full
    );
    if !control.selected {
        let (seen, floor) =
            (contrast(label.color, theme::panel()), if control.enabled { MIN_CONTRAST } else { MIN_DISABLED_CONTRAST });
        assert!(seen >= floor, "{name}: contrast {seen} of {floor}");
    }
}

fn no_overlaps(buttons: &[(String, Rect)], what: &str) {
    for (i, (a, ra)) in buttons.iter().enumerate() {
        for (b, rb) in &buttons[i + 1..] {
            let both = ra.intersect(*rb);
            assert!(!both.is_positive(), "{what}: {a} {ra:?} overlaps {b} {rb:?}");
        }
    }
}

/// Everything the open tab offers is on the ribbon, under the label rule.
fn every_tile(app: &App, tab: Tab, what: &str) {
    let body = body(app);
    let entries = entries(app, tab);
    assert!(!entries.is_empty(), "{what} has commands");
    let mut expected: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
    let buttons = ribbon::buttons_in(app, body);
    let mut seen: Vec<&str> = buttons.iter().map(|(name, _)| name.as_str()).collect();
    expected.sort();
    seen.sort();
    assert_eq!(seen, expected, "{what}: the buttons on the ribbon");
    no_overlaps(&buttons, what);
    for entry in &entries {
        label_below(&tile(app, &entry.label, body), body, what);
    }
}

/// A panel's title. The View tab has a Feature tree tile, so the band above does not count.
fn panel_title(app: &App, title: &str) -> Rect {
    let bottom = body(app).bottom();
    let found = ribbon::painted(app).into_iter().find(|p| p.text == title && p.rect.top() >= bottom);
    found.unwrap_or_else(|| panic!("no panel {title} in the frame")).rect
}

/// The tree, the properties and the viewport, for comparing before and after.
fn panel_rects(app: &App, tree: &str) -> [Rect; 3] {
    [panel_title(app, tree), panel_title(app, "Properties"), app.state().state.viewport.rect]
}

fn open_sketch(app: &mut App) {
    let name = app.state().state.doc.features.iter().find(|f| f.is_sketch()).expect("a sketch").name.clone();
    let row = sketch_row(app, &name);
    ribbon::double_click_at(app, row.center());
    app.run_steps(12);
    assert!(app.state().state.sketch.is_some(), "the sketch is open");
}

fn select_sketch(app: &mut App) {
    let name = app.state().state.doc.features.iter().find(|f| f.is_sketch()).expect("a sketch").name.clone();
    let row = sketch_row(app, &name);
    ribbon::click_at(app, row.center());
    settle(app);
    assert!(app.state().state.doc.selection.is_some(), "the sketch is selected");
}

fn sketch_row(app: &App, name: &str) -> Rect {
    let view = app.state().state.viewport.rect;
    let left = Rect::from_min_max(pos2(0.0, view.top()), pos2(view.left(), view.bottom()));
    ribbon::button_rect(app, name, left)
}

fn open_drawing(app: &mut App) {
    app.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::D);
    app.run_steps(4);
    let window = ribbon::window(app);
    ribbon::click_button(app, "Create drawing (Enter)", window);
    ribbon::until(app, "the drawing", |app| app.state().state.drawing.is_active());
    settle(app);
}

/// A plate with a sketch on the Top plane above it, for what needs a body and a sketch.
fn plate_and_sketch() -> Document {
    let mut doc = ribbon::plate();
    let plane = SketchPlane::Reference { plane: PlaneKind::Top, offset: 0.0 };
    let sketch = doc.apply(Command::AddSketch { plane }).unwrap().unwrap();
    let mut drawing = soliddraft_sketch::Sketch::new();
    drawing.rectangle(soliddraft_sketch::P2::new(10.0, 10.0), 20.0, 10.0).unwrap();
    doc.apply(Command::EditSketch { id: sketch, sketch: drawing }).unwrap();
    doc
}

/// Clicks a Shell tile at a point in a fresh plate, and returns the editor it opened.
fn shell_from(case: &Case, half: &str) -> EditorParams {
    let mut plate = ribbon::app_unrendered(ribbon::plate(), ribbon::WINDOW, 1.0);
    let mut opened = None;
    case.run(&mut plate, |app| {
        settle(app);
        let start = ribbon::fingerprint(app);
        let rects = panel_rects(app, "Feature tree");
        let shell = tile(app, "Shell", body(app));
        let point = if half == "icon" { shell.icon.center() } else { shell.label.full.center() };
        let button = shell.control.rect;
        assert!(button.contains(point), "{half}: {point:?} is on the button {button:?}");
        ribbon::click_at(app, point);
        settle(app);
        let params = editor(app).unwrap_or_else(|| panic!("the {half} of Shell opens an editor"));
        assert!(matches!(params, EditorParams::Shell(_)), "the {half} of Shell opens Shell: {params:?}");
        assert_eq!(app.state().state.doc.features.len(), start.features.len(), "nothing committed");
        assert_eq!(panel_rects(app, "Feature tree"), rects, "opening a tool moves no panel");
        app.key_press(Key::Escape);
        settle(app);
        assert_eq!(ribbon::fingerprint(app).without_history(), start.clone().without_history());
        opened = Some(params);
    });
    opened.expect("the steps ran")
}

#[test]
fn every_ribbon_command_has_a_visible_label_below_its_icon() {
    let case = ribbon::case("l1");

    // Each part category, then the assembly's ribbon.
    let mut plate = ribbon::app_unrendered(ribbon::plate(), ribbon::WINDOW, 1.0);
    case.run(&mut plate, |app| {
        settle(app);
        let start = ribbon::fingerprint(app);
        for (name, tab) in PART_TABS {
            ribbon::click_tab(app, name);
            settle(app);
            assert_eq!(ribbon::fingerprint(app), start, "opening {name} changes no state");
            every_tile(app, tab.into(), name);
            case.frame(app, &format!("part-{name}"));
        }
        ribbon::menu(app, "File", "New assembly");
        settle(app);
        assert!(app.state().state.assembly.is_active(), "the assembly is the active document");
        every_tile(app, Tab::Assembly, "the active assembly");
        case.frame(app, "assembly");
    });
    drop(plate);

    // Each drawing category.
    let mut drawing = ribbon::app_unrendered(ribbon::plate(), ribbon::WINDOW, 1.0);
    case.run(&mut drawing, |app| {
        settle(app);
        open_drawing(app);
        for tab in DrawingTab::ALL {
            ribbon::click_tab(app, tab.label());
            settle(app);
            every_tile(app, tab.into(), &format!("drawing {}", tab.label()));
            case.frame(app, &format!("drawing-{}", tab.label()));
        }
    });
    drop(drawing);

    // The label is the button: both halves open the same editor.
    assert_eq!(shell_from(&case, "icon"), shell_from(&case, "label"), "both halves open the same Shell editor");

    // The same geometry in each theme and at each scale.
    for mode in MODES {
        for ppp in SCALES {
            let what = format!("{mode:?} {ppp}");
            let mut plate = ribbon::app_unrendered(ribbon::plate(), ribbon::WINDOW, ppp);
            case.run(&mut plate, |app| {
                theme::set_mode(&app.ctx.clone(), mode);
                settle(app);
                let body = body(app);
                let point = tile(app, "Point", body);
                assert!(!point.control.enabled, "reference Point is unavailable");
                label_below(&point, body, &what);
                for (name, tab) in PART_TABS {
                    ribbon::click_tab(app, name);
                    settle(app);
                    every_tile(app, tab.into(), &format!("{what} {name}"));
                }
            });
            drop(plate);
            let mut sketch = ribbon::app_unrendered(ribbon::sketch(), ribbon::WINDOW, ppp);
            case.run(&mut sketch, |app| {
                theme::set_mode(&app.ctx.clone(), mode);
                settle(app);
                open_sketch(app);
                ribbon::click_tab(app, "Sketch");
                ribbon::click_ribbon(app, "Line");
                settle(app);
                let line = tile(app, "Line", body(app));
                assert!(line.control.selected, "the Line tool is lit");
                every_tile(app, RibbonTab::Sketch.into(), &format!("{what} active sketch"));
                case.frame(app, &format!("active-sketch-{what}"));
            });
            theme::set_mode(&sketch.ctx.clone(), Mode::Light);
        }
    }
}

/// Brings a command into view by pointer, if it is not already, and holds it to the rules.
fn reveal(app: &mut App, entry: &Entry, what: &str) -> Tile {
    let name = format!("{what}, {} > {}", entry.group, entry.label);
    let direct = ribbon::buttons_in(app, body(app)).iter().any(|(button, _)| *button == entry.label);
    if !direct {
        open_group(app, entry.group, &name);
    }
    let region = if direct { body(app) } else { row_band(app) };
    let tile = tile(app, &entry.label, region);
    let window = ribbon::window(app);
    label_below(&tile, window, &name);
    assert_eq!(tile.control.enabled, entry.command.is_enabled(&app.state().state), "{name}: enabled");

    // Its group is named under it, and nothing crowds it.
    let row = Rect::from_min_size(tile.control.rect.left_bottom(), vec2(window.width(), 24.0));
    let row = Rect::from_x_y_ranges(window.x_range(), row.y_range());
    assert!(
        ribbon::painted_in(app, row).iter().any(|p| p.text == entry.group),
        "{name}: the group name is under its commands"
    );
    let mut neighbours = ribbon::buttons_in(app, region);
    neighbours.retain(|(_, r)| (r.top() - tile.control.rect.top()).abs() <= TOLERANCE);
    no_overlaps(&neighbours, &name);
    tile
}

/// Clicks a collapsed group: its row opens, and nothing runs.
fn open_group(app: &mut App, group: &str, what: &str) {
    let name = group_button(group);
    let button = ribbon::control(app, &name, body(app));
    let shown: Vec<_> = ribbon::painted_in(app, button.rect).into_iter().filter(|p| p.text == group).collect();
    assert_eq!(shown.len(), 1, "{what}: the collapsed {group} shows its name");
    let before = ribbon::fingerprint(app);
    ribbon::click_at(app, button.rect.center());
    app.run_steps(4);
    assert_eq!(ribbon::fingerprint(app), before, "{what}: opening {group} runs nothing");
    assert!(popup_open(app), "{what}: clicking {group} opens it");
}

fn dismiss(app: &mut App) {
    if popup_open(app) {
        app.key_press(Key::Escape);
        app.run_steps(4);
        assert!(!popup_open(app), "Escape closes the row");
    }
}

/// The groups the window has collapsed on the open tab.
fn collapsed(app: &App, tab: Tab) -> Vec<&'static str> {
    let buttons = ribbon::buttons_in(app, body(app));
    let names: Vec<&'static str> = commands::ribbon(tab).into_iter().map(|(group, _)| group).collect();
    names.into_iter().filter(|g| buttons.iter().any(|(name, _)| *name == group_button(g))).collect()
}

/// The ribbon stays in its band and the panels stay in the window's left column.
fn layout_holds(app: &App, tree: &str, what: &str) {
    let view = app.state().state.viewport.rect;
    let window = ribbon::window(app);
    let body = body(app);
    assert!(view.top() >= body.bottom() - TOLERANCE, "{what}: the ribbon stays above the viewport: {view:?}");
    assert!(view.right() >= window.right() - TOLERANCE, "{what}: nothing is right of the viewport: {view:?}");
    assert!(view.width() >= panels::VIEWPORT_MIN_WIDTH - TOLERANCE, "{what}: the viewport keeps its width: {view:?}");
    let tree = panel_title(app, tree);
    let properties = panel_title(app, "Properties");
    assert!(tree.right() <= view.left() + TOLERANCE, "{what}: the tree is left of the viewport");
    assert!(properties.right() <= view.left() + TOLERANCE, "{what}: so are the properties");
    assert!(properties.top() > tree.bottom(), "{what}: the properties are below the tree");
    for (name, rect) in ribbon::buttons_in(app, Rect::from_min_max(window.min, pos2(window.right(), view.top()))) {
        assert!(rect.bottom() <= body.bottom() + TOLERANCE, "{what}: {name} stays in the band: {rect:?}");
        assert!(rect.right() <= window.right() + TOLERANCE, "{what}: {name} stays in the window: {rect:?}");
    }
}

/// Reveals every command of a tab. Disabled ones are clicked and do nothing, and the ones
/// the walk may run are run and leave no row behind.
fn walk(app: &mut App, tab_name: Option<&str>, tab: Tab, runs: &[&str], tree: &str, what: &str) {
    for entry in entries(app, tab) {
        if let Some(name) = tab_name {
            ribbon::click_tab(app, name);
        }
        layout_holds(app, tree, what);
        let name = format!("{what}, {}", entry.label);
        let tile = reveal(app, &entry, what);
        let before = ribbon::fingerprint(app);
        if !tile.control.enabled {
            ribbon::click_at(app, tile.control.rect.center());
            assert_eq!(ribbon::fingerprint(app), before, "{name}: disabled, so clicking it does nothing");
        } else if runs.contains(&entry.command.id) {
            ribbon::click_at(app, tile.control.rect.center());
            settle(app);
            let recent = ribbon::fingerprint(app).recent;
            assert_eq!(recent.first().map(String::as_str), Some(entry.command.id), "{name}: it ran");
            assert!(!popup_open(app), "{name}: no row stays open over the model");
        }
        dismiss(app);
    }
}

/// A starter from a fresh fixture: the pointer opens its editor and the cancel leaves the part.
#[allow(clippy::too_many_arguments)]
fn starter(case: &Case, doc: Document, size: Vec2, ppp: f32, label: &str, select: bool, is: IsEditor, what: &str) {
    let mut app = ribbon::app_unrendered(doc, size, ppp);
    let what = format!("{what}, {label}");
    case.run(&mut app, |app| {
        settle(app);
        if select {
            select_sketch(app);
        }
        let start = ribbon::fingerprint(app);
        let rects = app.state().state.viewport.rect;
        let entry = entry(app, RibbonTab::Solid.into(), label);
        let tile = reveal(app, &entry, &what);
        assert!(tile.control.enabled, "{what} is enabled");
        ribbon::click_at(app, tile.control.rect.center());
        settle(app);
        let params = editor(app).unwrap_or_else(|| panic!("{what} opens an editor"));
        assert!(is(&params), "{what} opens its own editor: {params:?}");
        assert!(!popup_open(app), "{what}: no row stays open over the model");
        assert_eq!(app.state().state.doc.features.len(), start.features.len(), "{what}: nothing committed");
        assert_eq!(app.state().state.viewport.rect, rects, "{what}: the viewport stays put");
        assert_eq!(ribbon::fingerprint(app).recent.first().map(String::as_str), Some(entry.command.id));
        app.key_press(Key::Escape);
        settle(app);
        let after = ribbon::fingerprint(app);
        assert_eq!(after.features, start.features, "{what}: the cancel leaves the part");
        assert_eq!(after.body, start.body, "{what}: the cancel leaves the body");
        assert_eq!(after.undo_depth, start.undo_depth, "{what}: the cancel leaves the undo stack");
        assert_eq!(after.editor, None, "{what}: the editor is closed");
    });
}

/// Escape takes the open row back, not the collector under it.
fn escape_spares_the_collector(app: &mut App, what: &str) {
    let tab = Tab::from(RibbonTab::Solid);
    let Some(group) = collapsed(app, tab).into_iter().last() else { return };
    let fillet = entry(app, tab, "Fillet");
    let tile = reveal(app, &fillet, what);
    ribbon::click_at(app, tile.control.rect.center());
    settle(app);
    assert!(matches!(editor(app), Some(EditorParams::Fillet(_))), "{what}: Fillet is collecting");
    let before = ribbon::fingerprint(app);
    open_group(app, group, what);
    app.key_press(Key::Escape);
    settle(app);
    assert!(!popup_open(app), "{what}: Escape closes the row");
    assert_eq!(ribbon::fingerprint(app), before, "{what}: and leaves the collector");
    app.key_press(Key::Escape);
    settle(app);
    assert_eq!(editor(app), None, "{what}: the next Escape cancels the collector");
}

/// A group keeps its commands however the user gets back to it.
fn same_commands_after_another_tab(app: &mut App, what: &str) {
    let tab = Tab::from(RibbonTab::Solid);
    for group in collapsed(app, tab) {
        let ids = |app: &App| -> Vec<&'static str> {
            let band = row_band(app);
            let buttons = ribbon::buttons_in(app, band);
            let names = entries(app, tab);
            let found = names.iter().filter(|e| buttons.iter().any(|(name, _)| *name == e.label));
            found.map(|e| e.command.id).collect()
        };
        open_group(app, group, what);
        let first = ids(app);
        let expected: Vec<&str> = entries(app, tab).iter().filter(|e| e.group == group).map(|e| e.command.id).collect();
        assert_eq!(first, expected, "{what}: {group} shows its commands");
        dismiss(app);
        ribbon::click_tab(app, "View");
        settle(app);
        ribbon::click_tab(app, "Solid");
        settle(app);
        open_group(app, group, what);
        assert_eq!(ids(app), first, "{what}: {group} shows the same commands again");
        dismiss(app);
    }
}

/// The window goes wide with a row open and comes back: no leftovers and nothing lost.
fn resize_restores(app: &mut App, size: Vec2, what: &str) {
    let tab = Tab::from(RibbonTab::Solid);
    let start = ribbon::fingerprint(app);
    let groups = collapsed(app, tab);
    if let Some(group) = groups.last() {
        open_group(app, group, what);
    }
    app.set_size(ribbon::WINDOW);
    settle(app);
    assert_eq!(ribbon::caption_names(app), SOLID_CAPTIONS, "{what}: the six captions are back");
    every_tile(app, tab, &format!("{what} widened"));
    assert!(!popup_open(app), "{what}: no row is left open over the model");
    layout_holds(app, "Feature tree", what);
    assert_eq!(ribbon::fingerprint(app), start, "{what}: resizing runs nothing");
    app.set_size(size);
    settle(app);
    assert_eq!(collapsed(app, tab), groups, "{what}: the same groups collapse again");
    assert!(!popup_open(app), "{what}: and none opens by itself");
    assert_eq!(ribbon::fingerprint(app), start, "{what}: resizing runs nothing");
}

/// The bottom bar keeps the input and the primary status at the smallest window.
fn the_band_holds(app: &App, what: &str) {
    let band = ribbon::band(app);
    assert!(band.contains_rect(ribbon::input_rect(app)), "{what}: the command input is in the band");
    let text: Vec<String> = ribbon::painted_in(app, band).into_iter().map(|p| p.text).collect();
    for primary in ["Rebuild OK", "mm"] {
        assert!(text.iter().any(|t| t == primary), "{what}: {primary} is in the band: {text:?}");
    }
}

fn narrow_window(case: &Case, size: Vec2, ppp: f32) {
    let what = format!("{} by {} at {ppp}", size.x, size.y);

    // Each Solid starter, from a fresh fixture.
    for (label, is) in PLATE_STARTERS {
        starter(case, ribbon::plate(), size, ppp, label, false, is, &what);
    }
    for (label, on_a_body, is) in SKETCH_STARTERS {
        let doc = if on_a_body { plate_and_sketch() } else { ribbon::sketch() };
        starter(case, doc, size, ppp, label, true, is, &what);
    }
    starter(case, ribbon::plate(), size, ppp, "Sketch", false, |p| matches!(p, EditorParams::PickPlane), &what);

    // Every category on a plate, with what is safe to run.
    let mut plate = ribbon::app_unrendered(ribbon::plate(), size, ppp);
    case.run(&mut plate, |app| {
        settle(app);
        layout_holds(app, "Feature tree", &what);
        assert_eq!(ribbon::caption_names(app), SOLID_CAPTIONS, "{what}: every group keeps its name");
        case.frame(app, &format!("solid-{what}"));
        if let Some(group) = collapsed(app, RibbonTab::Solid.into()).last() {
            open_group(app, group, &what);
            case.frame(app, &format!("open-{what}"));
            dismiss(app);
        }
        if size.x <= 960.0 {
            the_band_holds(app, &what);
        }
        walk(app, Some("Solid"), RibbonTab::Solid.into(), &[], "Feature tree", &format!("{what} solid"));
        walk(app, Some("Assembly"), RibbonTab::Assembly.into(), &[], "Feature tree", &format!("{what} assembly tab"));
        walk(
            app,
            Some("Inspect"),
            RibbonTab::Inspect.into(),
            &INSPECT_RUNS,
            "Feature tree",
            &format!("{what} inspect"),
        );
    });
    drop(plate);
    let mut plate = ribbon::app_unrendered(ribbon::plate(), size, ppp);
    case.run(&mut plate, |app| {
        settle(app);
        walk(app, Some("View"), RibbonTab::View.into(), &VIEW_RUNS, "Feature tree", &format!("{what} view"));
    });
    drop(plate);

    // The row's own behaviour.
    let mut plate = ribbon::app_unrendered(ribbon::plate(), size, ppp);
    case.run(&mut plate, |app| {
        settle(app);
        same_commands_after_another_tab(app, &what);
        resize_restores(app, size, &what);
        escape_spares_the_collector(app, &what);
    });
    drop(plate);

    // An open sketch.
    let mut sketch = ribbon::app_unrendered(ribbon::sketch(), size, ppp);
    case.run(&mut sketch, |app| {
        settle(app);
        open_sketch(app);
        ribbon::click_tab(app, "Sketch");
        settle(app);
        let tree = "Feature tree";
        walk(app, Some("Sketch"), RibbonTab::Sketch.into(), &SKETCH_RUNS, tree, &format!("{what} sketch"));
    });
    drop(sketch);

    // An open assembly.
    let mut assembly = ribbon::app_unrendered(ribbon::blank(), size, ppp);
    case.run(&mut assembly, |app| {
        settle(app);
        ribbon::menu(app, "File", "New assembly");
        settle(app);
        assert!(app.state().state.assembly.is_active(), "the assembly is the active document");
        walk(app, None, Tab::Assembly, &["zoom_fit"], "Assembly tree", &format!("{what} active assembly"));
    });
}

#[test]
fn narrow_windows_keep_every_command_pointer_reachable() {
    let case = ribbon::case("l2");
    for size in NARROW {
        for ppp in SCALES {
            narrow_window(&case, size, ppp);
        }
    }
}

#[test]
fn the_drawing_ribbon_narrows_the_same_way() {
    let case = ribbon::case("l2-drawing");
    let size = vec2(960.0, 600.0);
    let mut drawing = ribbon::app_unrendered(ribbon::plate(), size, 1.0);
    case.run(&mut drawing, |app| {
        settle(app);
        open_drawing(app);
        for tab in DrawingTab::ALL {
            let what = format!("drawing {}", tab.label());
            let body = body(app);
            for entry in entries(app, tab.into()) {
                ribbon::click_tab(app, tab.label());
                reveal(app, &entry, &what);
                dismiss(app);
                for (name, rect) in ribbon::buttons_in(app, body) {
                    assert!(rect.right() <= size.x + TOLERANCE, "{what}: {name} stays in the window: {rect:?}");
                }
            }
        }
    });
}

fn drag(app: &mut App, from: Pos2, to: Pos2) {
    app.hover_at(from);
    app.run_steps(2);
    app.drag_at(from);
    app.run_steps(2);
    for step in 1..=6 {
        app.hover_at(from + (to - from) * (step as f32 / 6.0));
        app.run_steps(2);
    }
    app.drop_at(to);
    app.run_steps(4);
}

fn chip(app: &App, label: &str) -> Rect {
    app.get_by_label(&format!("{label} on the command bar")).rect()
}

fn tools(app: &App) -> Vec<String> {
    app.state().state.command_line.settings.tools.clone()
}

/// The chip opens Shell, and Escape leaves the part as it was.
fn chip_opens_shell(app: &mut App) {
    let start = ribbon::fingerprint(app).without_history();
    let rect = chip(app, "Shell");
    ribbon::click_at(app, rect.center());
    settle(app);
    assert!(matches!(editor(app), Some(EditorParams::Shell(_))), "the chip opens Shell");
    app.key_press(Key::Escape);
    settle(app);
    assert_eq!(ribbon::fingerprint(app).without_history(), start, "cancelling leaves the part");
}

#[test]
fn labels_and_art_follow_the_same_command_through_pin_and_search() {
    let case = ribbon::case("l3");
    let mut app = ribbon::app_unrendered(ribbon::plate(), ribbon::WINDOW, 1.0);
    case.run(&mut app, |app| {
        settle(app);
        let body = body(app);
        let shell = tile(app, "Shell", body);
        label_below(&shell, body, "Solid > Modify");

        // The tile becomes a chip by its stable ID.
        let last = chip(app, "Normal to");
        drag(app, shell.control.rect.center(), last.right_center() - vec2(3.0, 0.0));
        assert_eq!(tools(app).iter().filter(|t| *t == "shell").count(), 1, "{:?}", tools(app));
        assert_eq!(settings::command_line().tools, tools(app), "the chip is saved");
        chip_opens_shell(app);

        // The palette finds the same command.
        let start = ribbon::fingerprint(app).without_history();
        app.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::P);
        app.run_steps(3);
        app.event(egui::Event::Text("shell".to_owned()));
        app.run_steps(3);
        let palette = app.state().state.palette.clone().expect("the palette is open");
        let first = palette.results(&app.state().state).first().map(|m| m.command.id);
        assert_eq!(first, Some("shell"), "the search finds Shell");
        app.key_press(Key::Enter);
        settle(app);
        assert!(matches!(editor(app), Some(EditorParams::Shell(_))), "the palette opens Shell");
        app.key_press(Key::Escape);
        settle(app);
        assert_eq!(ribbon::fingerprint(app).without_history(), start, "cancelling leaves the part");
    });
    drop(app);

    // The next run starts from the saved chip.
    let mut again = ribbon::app_unrendered(ribbon::plate(), ribbon::WINDOW, 1.0);
    case.run(&mut again, |app| {
        settle(app);
        assert_eq!(tools(app).iter().filter(|t| *t == "shell").count(), 1, "{:?}", tools(app));
        chip_opens_shell(app);
    });
    drop(again);

    // A tile in an open row drags the same way, and the row goes when it lands.
    let size = vec2(960.0, 600.0);
    let mut narrow = ribbon::app_unrendered(ribbon::plate(), size, 1.0);
    case.run(&mut narrow, |app| {
        settle(app);
        let tab = Tab::from(RibbonTab::Solid);
        let linear = entry(app, tab, "Linear");
        let tile = reveal(app, &linear, "the narrow pin");
        let last = chip(app, "Shell");
        drag(app, tile.control.rect.center(), last.right_center() - vec2(3.0, 0.0));
        assert_eq!(tools(app).iter().filter(|t| *t == "linear_pattern").count(), 1, "{:?}", tools(app));
        assert!(!popup_open(app), "no row stays open after the drop");
    });
}
