//! Merge 3 of the ribbon layout: the original operation icons. Acceptance cases I1 and I2
//! of the ribbon test design. The references under `tests/fixtures/ribbon_icons` are the
//! prototype's drawings rendered by rsvg-convert, so nothing here compares the app's
//! renderer with itself.
//!
//! Requires a renderer: a run without one fails.
//!
//! Run them one at a time, away from the other GPU targets:
//! `cargo test -p soliddraft-app --test ribbon_icons -- --test-threads=1`.

#[path = "support/ribbon.rs"]
mod ribbon;

use egui::{Color32, Key, Rect, pos2, vec2};
use egui_kittest::Harness;
use image::{Rgba, RgbaImage, imageops};
use resvg::usvg;
use ribbon::App;
use soliddraft_app::commands;
use soliddraft_app::icons::{self, Icon, Treatment};
use soliddraft_app::state::EditorParams;
use soliddraft_app::theme::{self, Mode};
use std::fmt::Write as _;
use std::path::PathBuf;

const SCALES: [f32; 3] = [1.0, 1.5, 2.0];
const MODES: [Mode; 2] = [Mode::Light, Mode::Dark];

/// How far a frame may stray from a reference, as the mean of every channel of every pixel,
/// out of 255. Two renderers antialias differently; a different drawing is far past this.
const MAX_DIFFERENCE: f32 = 12.0;
/// How far apart two different symbols must be, on the same measure.
const MIN_DISTINCT: f32 = 4.0;
/// The grid every comparison happens on.
const GRID: u32 = 20;
/// How much of an icon square an accent must cover: a marker pixel is not the port.
const MIN_ACCENT: f32 = 0.025;
/// What a disabled drawing does to the reference: grey, then faded.
const DISABLED_GREY: f32 = 0.85;
const DISABLED_ALPHA: f32 = 0.55;

/// What a colour says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    Body,
    Added,
    Removed,
    Profile,
    Reference,
    Surface,
}
use Family::{Added, Body, Profile, Reference, Removed, Surface};

fn family(px: Rgba<u8>) -> Option<Family> {
    let [r, g, b] = [px[0], px[1], px[2]].map(|c| f32::from(c) / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let chroma = max - min;
    let saturation = if max > 0.0 { chroma / max } else { 0.0 };
    if saturation < 0.16 {
        return (0.45..0.98).contains(&max).then_some(Body);
    }
    let hue = if max == r {
        60.0 * ((g - b) / chroma).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / chroma + 2.0)
    } else {
        60.0 * ((r - g) / chroma + 4.0)
    };
    match hue {
        h if !(26.0..340.0).contains(&h) => Some(Removed),
        h if (26.0..60.0).contains(&h) => Some(Profile),
        h if (150.0..200.0).contains(&h) => Some(Reference),
        h if (200.0..245.0).contains(&h) => Some(Added),
        h if (245.0..290.0).contains(&h) => Some(Surface),
        _ => None,
    }
}

/// The source key each operation must show, and the colours that say what it does.
const SOLID: [(&str, &str, &[Family]); 19] = [
    ("Sketch", "sketch", &[Profile]),
    ("Extrude", "extrude", &[Profile, Added]),
    ("Revolve", "revolve", &[Added]),
    ("Sweep", "sweep", &[Added, Profile]),
    ("Loft", "loft", &[Added, Profile]),
    ("Cut", "cut", &[Body, Removed]),
    ("Hole", "hole", &[Body, Removed]),
    ("Slot", "slot", &[Body, Removed]),
    ("Fillet", "fillet", &[Body, Added]),
    ("Chamfer", "chamfer", &[Body, Added]),
    ("Shell", "shell", &[Body, Added]),
    ("Draft", "draft", &[Added]),
    ("Lattice", "lattice", &[Added]),
    ("Linear", "linear", &[Body, Added]),
    ("Circular", "circular", &[Body, Added]),
    ("Mirror", "mirror", &[Body, Reference, Added]),
    ("Plane", "plane", &[Reference]),
    ("Axis", "axis", &[Reference]),
    ("Point", "refpoint", &[Reference]),
];

const SKETCH: [(&str, &str, &[Family]); 15] = [
    ("Exit", "exit_sketch", &[Profile]),
    ("Line", "line", &[Profile]),
    ("Circle", "circle", &[Profile]),
    ("Rectangle", "rect", &[Profile]),
    ("Arc", "arc", &[Profile]),
    ("Slot", "slot_sketch", &[Profile]),
    ("Point", "sk_point", &[Profile]),
    ("Smart dim", "smart_dim", &[Added]),
    ("Construction", "construction", &[Profile]),
    ("Relations", "relations", &[Profile, Added]),
    ("Extrude", "extrude", &[Profile, Added]),
    ("Revolve", "revolve", &[Added]),
    ("Cut", "cut", &[Body, Removed]),
    ("Normal to", "normal_to", &[Reference]),
    ("Sketch", "sketch", &[Profile]),
];

const INSPECT: [(&str, &str, &[Family]); 5] = [
    ("Measure", "measure", &[]),
    ("Mass props", "mass", &[Body]),
    ("Check", "check", &[Body]),
    ("Section", "section", &[Body, Reference]),
    ("Draft check", "draft_check", &[]),
];

const ASSEMBLY: [(&str, &str, &[Family]); 1] = [("Insert component", "insert_comp", &[Body, Added])];

/// The chrome glyphs that follow the text colour.
const ASSEMBLY_GLYPHS: [(&str, &str); 4] =
    [("Save assembly", "save"), ("Save assembly as", "save"), ("Undo", "undo"), ("Redo", "redo")];

/// Pairs a user must be able to tell apart.
const CORE_PAIRS: [(&str, &str); 7] = [
    ("fillet", "chamfer"),
    ("extrude", "cut"),
    ("shell", "part"),
    ("linear", "circular"),
    ("slot_sketch", "slot"),
    ("sk_point", "refpoint"),
    ("lattice", "linear"),
];

/// What each source key's drawing must carry, beyond the operations on the ribbon above.
const SURFACE_AND_CLOUD: [&str; 18] = [
    "srf_extrude",
    "srf_revolve",
    "srf_sweep",
    "srf_loft",
    "srf_boundary",
    "srf_fill",
    "srf_offset",
    "srf_trim",
    "srf_extend",
    "srf_knit",
    "srf_thicken",
    "pc_import",
    "pc_clean",
    "pc_decimate",
    "pc_register",
    "pc_mesh",
    "pc_fit",
    "pc_deviation",
];

fn fixture(path: &str) -> RgbaImage {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ribbon_icons").join(path);
    image::open(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display())).to_rgba8()
}

fn reference(key: &str, dark: bool) -> RgbaImage {
    fixture(&format!("{}/cad/{key}.png", if dark { "dark" } else { "light" }))
}

fn premultiplied(image: &RgbaImage) -> RgbaImage {
    let mut out = image.clone();
    for px in out.pixels_mut() {
        let a = f32::from(px[3]) / 255.0;
        for channel in &mut px.0[..3] {
            *channel = (f32::from(*channel) * a).round() as u8;
        }
    }
    out
}

/// A premultiplied reference, scaled to a crop and laid over a background.
fn over(
    reference: &RgbaImage,
    size: (u32, u32),
    background: Rgba<u8>,
    treat: impl Fn(Rgba<u8>) -> Rgba<u8>,
) -> RgbaImage {
    let scaled = imageops::resize(&premultiplied(reference), size.0, size.1, imageops::FilterType::Triangle);
    let mut out = RgbaImage::from_pixel(size.0, size.1, background);
    for (to, from) in out.pixels_mut().zip(scaled.pixels()) {
        let from = treat(*from);
        let rest = 1.0 - f32::from(from[3]) / 255.0;
        for i in 0..3 {
            to[i] = (f32::from(from[i]) + f32::from(to[i]) * rest).round() as u8;
        }
    }
    out
}

fn normal(px: Rgba<u8>) -> Rgba<u8> {
    px
}

/// A drawing is nearer its own reference than the one it could be mistaken for.
fn is_its_own_drawing(actual: &RgbaImage, key: &str, other: &str, ground: Rgba<u8>, dark: bool) {
    let against = |k: &str| difference(actual, &over(&reference(k, dark), actual.dimensions(), ground, normal));
    let (own, theirs) = (against(key), against(other));
    assert!(own < theirs, "{key} is nearer {other}: {own} against {theirs}");
}

/// The documented disabled treatment, on a premultiplied pixel.
fn disabled(px: Rgba<u8>) -> Rgba<u8> {
    let grey = 0.3 * f32::from(px[0]) + 0.59 * f32::from(px[1]) + 0.11 * f32::from(px[2]);
    let fade = |c: u8| ((f32::from(c) + (grey - f32::from(c)) * DISABLED_GREY) * DISABLED_ALPHA) as u8;
    Rgba([fade(px[0]), fade(px[1]), fade(px[2]), (f32::from(px[3]) * DISABLED_ALPHA) as u8])
}

/// A line glyph takes its alpha from the reference and its colour from the text.
fn tinted(color: Color32) -> impl Fn(Rgba<u8>) -> Rgba<u8> {
    let [r, g, b, a] = color.to_array();
    move |px| {
        let share = f32::from(px[3]) / 255.0;
        let scale = |c: u8| (f32::from(c) * share).round() as u8;
        Rgba([scale(r), scale(g), scale(b), scale(a)])
    }
}

/// How far apart two crops are, on a common coarse grid: two renderers place a thin stroke
/// a fraction of a pixel apart, and a different drawing differs at any scale.
fn difference(a: &RgbaImage, b: &RgbaImage) -> f32 {
    let coarse = |i: &RgbaImage| imageops::resize(i, GRID, GRID, imageops::FilterType::Triangle);
    let (a, b) = (coarse(a), coarse(b));
    let sum: u64 =
        a.pixels().zip(b.pixels()).map(|(p, q)| (0..3).map(|i| u64::from(p[i].abs_diff(q[i]))).sum::<u64>()).sum();
    sum as f32 / (3 * a.width() * a.height()) as f32
}

fn crop(frame: &RgbaImage, rect: Rect, ppp: f32) -> RgbaImage {
    let x = (rect.left() * ppp).round() as u32;
    let y = (rect.top() * ppp).round() as u32;
    let side = (rect.width() * ppp).round() as u32;
    imageops::crop_imm(frame, x, y, side, side).to_image()
}

/// The icon's own square from the frame: the image the production ribbon drew on the button.
fn icon_square(app: &App, label: &str, button: Rect) -> Rect {
    let images = ribbon::images_in(app, button);
    assert_eq!(images.len(), 1, "the button {label} shows one drawing: {images:?}");
    images[0]
}

fn share(crop: &RgbaImage, wanted: Family) -> f32 {
    crop.pixels().filter(|p| family(**p) == Some(wanted)).count() as f32 / crop.pixels().len() as f32
}

/// Nothing solid lands in the ring of pixels around the square: the art stays in its bounds.
fn inside_its_square(frame: &RgbaImage, square: Rect, ppp: f32, background: Rgba<u8>, what: &str) {
    let (x0, y0) = ((square.left() * ppp).round() as i64, (square.top() * ppp).round() as i64);
    let side = (square.width() * ppp).round() as i64;
    let solid = |x: i64, y: i64| {
        let p = frame.get_pixel(x as u32, y as u32);
        (0..3).any(|i| p[i].abs_diff(background[i]) > 150)
    };
    let ring = (-2..=side + 1)
        .flat_map(|i| [(x0 + i, y0 - 2), (x0 + i, y0 + side + 1), (x0 - 2, y0 + i), (x0 + side + 1, y0 + i)]);
    let spilled = ring.filter(|(x, y)| solid(*x, *y)).count();
    assert_eq!(spilled, 0, "{what} spills past its {side} pixel square");
}

/// The drawing's strokes fit its own square, so no size can clip it.
fn fits_its_view(key: &str, svg: &str) {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).unwrap_or_else(|e| panic!("{key}: {e}"));
    let bounds = tree.root().abs_stroke_bounding_box();
    let size = tree.size();
    let inside = bounds.left() >= -0.05
        && bounds.top() >= -0.05
        && bounds.right() <= size.width() + 0.05
        && bounds.bottom() <= size.height() + 0.05;
    assert!(inside, "{key} reaches past its view: {bounds:?} in {size:?}");
}

#[derive(Default)]
struct Log(String);

impl Log {
    fn row(&mut self, what: &str, key: &str, state: &str, logical: f32, physical: u32, off: f32) {
        writeln!(self.0, "{what}\t{key}\t{state}\t{logical}\t{physical}\t{off:.2}").unwrap();
    }
}

/// One tab as the app drew it, against the references. Commands the state switches off are
/// held to the disabled treatment.
fn compare_tab(app: &mut App, rows: &[(&str, &str, &[Family])], dark: bool, ppp: f32, log: &mut Log, seen: &str) {
    let frame = app.render().expect("the frame renders");
    let panel = theme::panel().to_array();
    let background = Rgba(panel);
    for (label, key, families) in rows {
        let button = ribbon::ribbon_button(app, label);
        let square = icon_square(app, label, button.rect);
        let actual = crop(&frame, square, ppp);
        let what = format!("{seen} {label}");
        let wanted = reference(key, dark);
        let expected = if button.enabled {
            over(&wanted, actual.dimensions(), background, normal)
        } else {
            over(&wanted, actual.dimensions(), background, disabled)
        };
        let off = difference(&actual, &expected);
        let state = if button.enabled { "enabled" } else { "disabled" };
        log.row(&what, key, state, square.width(), actual.width(), off);
        assert!(off <= MAX_DIFFERENCE, "{what} is {off} from the {key} reference");
        inside_its_square(&frame, square, ppp, background, &what);
        if button.enabled {
            for accent in *families {
                let covered = share(&actual, *accent);
                assert!(covered >= MIN_ACCENT, "{what} shows {accent:?} on {covered} of its square");
            }
        }
        if ["hole", "shell"].contains(key) {
            // The opening stays open: its middle matches the drawing on its own.
            let side = actual.width();
            let middle = |i: &RgbaImage| imageops::crop_imm(i, side / 4, side / 5, side / 2, side / 2).to_image();
            let off = difference(&middle(&actual), &middle(&expected));
            assert!(off <= MAX_DIFFERENCE * 1.5, "{what} fills its opening: {off}");
        }
    }
}

fn compare_glyphs(app: &mut App, rows: &[(&str, &str)], ppp: f32, log: &mut Log, seen: &str) {
    let frame = app.render().expect("the frame renders");
    let background = Rgba(theme::panel().to_array());
    for (label, key) in rows {
        let button = ribbon::ribbon_button(app, label);
        let square = icon_square(app, label, button.rect);
        let actual = crop(&frame, square, ppp);
        let color = if button.enabled { theme::ink() } else { theme::muted().gamma_multiply(0.55) };
        let wanted = fixture(&format!("line/{key}.png"));
        let off = difference(&actual, &over(&wanted, actual.dimensions(), background, tinted(color)));
        let what = format!("{seen} {label}");
        log.row(&what, key, "glyph", square.width(), actual.width(), off);
        assert!(off <= MAX_DIFFERENCE, "{what} is {off} from the {key} glyph");
    }
}

/// Fillet: the hover and the active treatments, and the same command behind them.
fn fillet_stays_a_fillet(app: &mut App, ppp: f32) {
    let camera = |a: &App| a.state().state.viewport.camera.distance;
    let mut last = camera(app);
    loop {
        app.run_steps(4);
        if camera(app) == last {
            break;
        }
        last = camera(app);
    }
    let start = ribbon::fingerprint(app).without_history();
    let button = ribbon::ribbon_button(app, "Fillet");
    let square = icon_square(app, "Fillet", button.rect);
    let resting = crop(&app.render().expect("the frame renders"), button.rect, ppp);
    app.hover_at(button.rect.center());
    app.run_steps(3);
    let hovered = crop(&app.render().expect("the frame renders"), button.rect, ppp);
    assert!(difference(&resting, &hovered) > 0.2, "hovering Fillet shows");
    ribbon::click_ribbon(app, "Fillet");
    let rest = ribbon::window(app).center();
    app.hover_at(rest);
    app.run_steps(4);
    assert!(
        matches!(app.state().state.editor.as_ref().map(|e| &e.params), Some(EditorParams::Fillet(_))),
        "the button opens the Fillet editor"
    );
    assert!(app.state().state.doc.undo_depth() == start.undo_depth, "opening it changes nothing");
    let active = ribbon::ribbon_button(app, "Fillet");
    assert!(active.selected, "Fillet reads as active");
    assert!(ribbon::painted_in(app, active.rect).iter().any(|p| p.text == "Fillet"), "its label stays");
    let frame = app.render().expect("the frame renders");
    assert!(difference(&resting, &crop(&frame, button.rect, ppp)) > 0.5, "the active treatment shows");
    let shown = crop(&frame, square, ppp);
    assert!(share(&shown, Added) >= MIN_ACCENT, "the drawing stays on the active button");
    app.key_press(Key::Escape);
    app.run_steps(6);
    assert_eq!(ribbon::fingerprint(app).without_history(), start, "cancelling leaves the part");
}

fn one_pass(case: &ribbon::Case, mode: Mode, ppp: f32, log: &mut Log) {
    let dark = mode == Mode::Dark;
    let seen = format!("{mode:?} {ppp}");

    // Disabled art on a blank part.
    let mut blank = ribbon::app_at(ribbon::blank(), ribbon::WINDOW, ppp);
    case.run(&mut blank, |app| {
        theme::set_mode(&app.ctx.clone(), mode);
        app.run_steps(6);
        let button = ribbon::ribbon_button(app, "Fillet");
        assert!(!button.enabled);
        compare_tab(app, &SOLID[8..10], dark, ppp, log, &format!("{seen} blank"));
        let said = ribbon::tooltip(app, button.rect);
        assert!(said.contains("Fillet needs a body"), "{said:?}");
    });

    // The plate on every tab, with each first paint held to the references.
    let mut plate = ribbon::app_at(ribbon::plate(), ribbon::WINDOW, ppp);
    case.run(&mut plate, |app| {
        app.run_steps(6);
        if dark {
            app.run_steps(2);
            let view = ribbon::button_rect(app, "View", ribbon::window(app));
            ribbon::click_at(app, view.center());
            let entry = ribbon::menu_entry(app, "View", "Dark theme");
            ribbon::click_for_first_paint(app, entry.center());
            // The menu closes on the next frame; the drawings are already the dark ones.
            app.run_steps(2);
            assert!(theme::mode().is_dark(), "the menu switched the theme");
        } else {
            theme::set_mode(&app.ctx.clone(), mode);
            app.run_steps(2);
        }
        compare_tab(app, &SOLID, dark, ppp, log, &format!("{seen} solid"));
        fillet_stays_a_fillet(app, ppp);

        // Lattice and Linear are different drawings on the real ribbon.
        let frame = app.render().expect("the frame renders");
        let [lattice, linear] = ["Lattice", "Linear"].map(|label| {
            let square = icon_square(app, label, ribbon::ribbon_button(app, label).rect);
            crop(&frame, square, ppp)
        });
        assert!(difference(&lattice, &linear) >= MIN_DISTINCT, "Lattice is not the Linear symbol");
        let ground = Rgba(theme::panel().to_array());
        is_its_own_drawing(&lattice, "lattice", "linear", ground, dark);

        for (name, rows) in [("Sketch", &SKETCH[..]), ("Inspect", &INSPECT[..])] {
            let tab = ribbon::tab(app, name).rect;
            ribbon::click_for_first_paint(app, tab.center());
            compare_tab(app, rows, dark, ppp, log, &format!("{seen} {name}"));
        }
        case.frame(app, &format!("inspect-{seen}"));
        let tab = ribbon::tab(app, "Assembly").rect;
        ribbon::click_for_first_paint(app, tab.center());
        compare_tab(app, &ASSEMBLY, dark, ppp, log, &format!("{seen} assembly tab"));

        ribbon::menu(app, "File", "New assembly");
        app.run_steps(6);
        assert!(app.state().state.assembly.is_active(), "the assembly is the active document");
        compare_tab(app, &ASSEMBLY, dark, ppp, log, &format!("{seen} assembly"));
        compare_glyphs(app, &ASSEMBLY_GLYPHS, ppp, log, &format!("{seen} assembly"));
    });

    // The sketch tab with its tools enabled, in a real sketch.
    let mut sketch = ribbon::app_at(ribbon::sketch(), ribbon::WINDOW, ppp);
    case.run(&mut sketch, |app| {
        theme::set_mode(&app.ctx.clone(), mode);
        app.run_steps(6);
        let name = app.state().state.doc.features[0].name.clone();
        let view = app.state().state.viewport.rect;
        let left = Rect::from_min_max(pos2(0.0, view.top()), pos2(view.left(), view.bottom()));
        let row = ribbon::button_rect(app, &name, left);
        ribbon::double_click_at(app, row.center());
        app.run_steps(12);
        assert!(app.state().state.sketch.is_some(), "the sketch is open");
        // The open sketch lights its own button; the active ground is held by the Fillet case.
        compare_tab(app, &SKETCH[..14], dark, ppp, log, &format!("{seen} in a sketch"));
    });
    theme::set_mode(&plate.ctx.clone(), Mode::Light);
}

#[test]
fn the_real_ribbon_uses_original_operation_art_in_each_theme_and_scale() {
    let case = ribbon::case("i1");
    let mut log = Log::default();
    for mode in MODES {
        for ppp in SCALES {
            one_pass(&case, mode, ppp, &mut log);
        }
    }
    let table = "shown\tkey\tstate\tlogical\tphysical\tdifference\n".to_owned() + &log.0;
    std::fs::write(case.path("captures.tsv"), table).expect("the capture table is written");
}

/// One sheet of drawings at one size, drawn by the production renderer with nothing else on
/// it. Each row of `cells` is a drawing, a treatment and the ground under it.
struct Cell {
    icon: Icon,
    treatment: Treatment,
    ground: Color32,
    color: Color32,
}

fn sheet(cells: Vec<Cell>, side: f32, ppp: f32, mode: Mode) -> (RgbaImage, Vec<Rect>) {
    const COLUMNS: usize = 11;
    let pitch = side + 8.0;
    let rows = cells.len().div_ceil(COLUMNS);
    let place = |i: usize| {
        let at = pos2((i % COLUMNS) as f32 * pitch + 4.0, (i / COLUMNS) as f32 * pitch + 4.0);
        Rect::from_min_size(at, vec2(side, side))
    };
    let squares: Vec<Rect> = (0..cells.len()).map(place).collect();
    let drawn = squares.clone();
    let mut harness = Harness::builder()
        .with_size(vec2(COLUMNS as f32 * pitch, rows as f32 * pitch))
        .with_pixels_per_point(ppp)
        .wgpu()
        .build_ui(move |ui| {
            let painter = ui.painter();
            painter.rect_filled(ui.ctx().content_rect(), 0.0, theme::panel());
            for (cell, square) in cells.iter().zip(&drawn) {
                painter.rect_filled(square.expand(3.0), 0.0, cell.ground);
                icons::paint_as(painter, cell.icon, *square, cell.color, cell.treatment);
            }
        });
    theme::set_mode(&harness.ctx.clone(), mode);
    harness.run_steps(3);
    let frame = harness.render().expect("the sheet renders");
    let shown = ribbon::images(&harness.output().shapes);
    assert_eq!(shown.len(), squares.len(), "every drawing is on the sheet");
    (frame, shown)
}

fn glyph_keys() -> Vec<&'static str> {
    icons::GLYPHS.iter().map(|(key, _)| *key).collect()
}

fn cad_keys() -> Vec<&'static str> {
    icons::CAD.iter().map(|(key, _)| *key).collect()
}

#[test]
fn the_complete_icon_catalogue_survives_real_size_rendering() {
    let case = ribbon::case("i2");

    // The catalogue is the prototype's, whole, and every registry row points into it.
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(env!("CARGO_MANIFEST_DIR").to_owned() + "/tests/fixtures/ribbon_icons/manifest.json").unwrap(),
    )
    .unwrap();
    assert_eq!(
        manifest["source_sha256"], "49d0d17d10720a30b8fe7e1c1f84a808f3841906115a11ba4f2e65ea498dca69",
        "the references come from the prototype's source"
    );
    assert_eq!(icons::CAD.len(), 65);
    assert_eq!(icons::GLYPHS.len(), 36);
    for (key, svg) in icons::CAD {
        assert!(manifest["cad"].get(key).is_some(), "{key} is not in the source");
        // Zebra's stripes are cut by its clip path, which bounding boxes do not read.
        if key != "zebra" {
            fits_its_view(key, svg);
        }
    }
    for c in commands::all() {
        match c.icon {
            Icon::Cad(key) => assert!(cad_keys().contains(&key), "{} wants {key}", c.id),
            Icon::Glyph(key) => assert!(glyph_keys().contains(&key), "{} wants {key}", c.id),
            _ => {}
        }
    }
    for (id, key) in [
        ("linear_pattern", "linear"),
        ("circular_pattern", "circular"),
        ("reference_point", "refpoint"),
        ("sketch_point", "sk_point"),
        ("slot", "slot"),
        ("sketch_slot", "slot_sketch"),
        ("make_drawing", "drawing"),
        ("show_drawing", "open_drawings"),
        ("insert_component", "insert_comp"),
        ("mass_properties", "mass"),
        ("smart_dimension", "smart_dim"),
        ("normal_to", "normal_to"),
        ("lattice", "lattice"),
    ] {
        assert_eq!(commands::get(id).unwrap().icon, Icon::Cad(key), "{id}");
    }
    for (id, key) in [("zoom_fit", "fit"), ("view_isometric", "iso"), ("undo", "undo"), ("redo", "redo")] {
        assert_eq!(commands::get(id).unwrap().icon, Icon::Glyph(key), "{id}");
    }

    let mut report = String::new();
    for mode in MODES {
        let dark = mode == Mode::Dark;
        for ppp in SCALES {
            // The sizes the ribbon ships.
            for side in [26.0, 22.0] {
                let panel = Color32::TRANSPARENT;
                let ink = {
                    let probe = Harness::builder().with_size(vec2(8.0, 8.0)).build_ui(|_| {});
                    theme::set_mode(&probe.ctx.clone(), mode);
                    theme::ink()
                };
                let mut cells: Vec<Cell> = cad_keys()
                    .into_iter()
                    .map(|key| Cell { icon: Icon::Cad(key), treatment: Treatment::Normal, ground: panel, color: ink })
                    .collect();
                cells.extend(glyph_keys().into_iter().map(|key| Cell {
                    icon: Icon::Glyph(key),
                    treatment: Treatment::Normal,
                    ground: panel,
                    color: ink,
                }));
                let (frame, squares) = sheet(cells, side, ppp, mode);
                let name = format!("sheet-{}-{}-{side}.png", mode.label(), ppp * 100.0);
                frame.save(case.path(&name)).expect("the sheet is written");
                let ground = Rgba(theme::panel().to_array());
                let (cad, glyphs) = squares.split_at(65);
                let mut worst = (0.0_f32, "");
                let mut rasters = std::collections::HashMap::new();
                for (key, square) in cad_keys().into_iter().zip(cad) {
                    let actual = crop(&frame, *square, ppp);
                    assert!(actual.pixels().any(|p| *p != ground), "{key} is empty at {side}");
                    inside_its_square(&frame, *square, ppp, ground, key);
                    let expected = over(&reference(key, dark), actual.dimensions(), ground, normal);
                    let off = difference(&actual, &expected);
                    if off > worst.0 {
                        worst = (off, key);
                    }
                    assert!(off <= MAX_DIFFERENCE, "{key} at {side} {mode:?} {ppp} is {off} from its reference");
                    rasters.insert(key, actual);
                }
                for (key, square) in glyph_keys().into_iter().zip(glyphs) {
                    let actual = crop(&frame, *square, ppp);
                    assert!(actual.pixels().any(|p| *p != ground), "{key} is empty at {side}");
                    let wanted = fixture(&format!("line/{key}.png"));
                    let off = difference(&actual, &over(&wanted, actual.dimensions(), ground, tinted(theme::ink())));
                    if off > worst.0 {
                        worst = (off, key);
                    }
                    assert!(
                        off <= MAX_DIFFERENCE,
                        "the {key} glyph at {side} {mode:?} {ppp} is {off} from its reference"
                    );
                }
                let mut closest = (f32::MAX, "");
                for (a, b) in CORE_PAIRS {
                    let off = difference(&rasters[a], &rasters[b]);
                    if off < closest.0 {
                        closest = (off, a);
                    }
                    assert!(off >= MIN_DISTINCT, "{a} and {b} look alike at {side}: {off}");
                    is_its_own_drawing(&rasters[a], a, b, ground, dark);
                    is_its_own_drawing(&rasters[b], b, a, ground, dark);
                }
                for (_, key, families) in SOLID.iter().chain(&SKETCH).chain(&INSPECT).chain(&ASSEMBLY) {
                    for accent in *families {
                        assert!(share(&rasters[key], *accent) >= MIN_ACCENT, "{key} lost {accent:?} at {side}");
                    }
                }
                for key in SURFACE_AND_CLOUD.iter().filter(|k| k.starts_with("srf_")) {
                    let sheet_or_body = share(&rasters[key], Surface).max(share(&rasters[key], Added));
                    assert!(sheet_or_body >= MIN_ACCENT, "{key} lost its sheet at {side}");
                }
                let (worst_off, worst_key) = worst;
                let (closest_off, closest_key) = closest;
                writeln!(
                    report,
                    "{}\t{}\t{side}\t{worst_off:.2}\t{worst_key}\t{closest_off:.2}\t{closest_key}",
                    mode.label(),
                    ppp * 100.0
                )
                .unwrap();

                // The states of the core drawings.
                let core = ["extrude", "cut", "fillet", "chamfer", "shell", "hole", "linear", "circular", "lattice"];
                let grounds = [
                    (theme::panel(), Treatment::Normal),
                    (theme::hover(), Treatment::Normal),
                    (theme::selection_bg(), Treatment::Normal),
                    (theme::panel(), Treatment::Disabled),
                ];
                let cells = core
                    .iter()
                    .flat_map(|key| grounds.map(|(ground, treatment)| (key, ground, treatment)))
                    .map(|(key, ground, treatment)| Cell { icon: Icon::Cad(key), treatment, ground, color: ink })
                    .collect();
                let (frame, squares) = sheet(cells, side, ppp, mode);
                let mut squares = squares.into_iter();
                for key in core {
                    for (_, treatment) in grounds {
                        let square = squares.next().unwrap();
                        let actual = crop(&frame, square, ppp);
                        let x = (square.left() * ppp).round() as u32 - 2;
                        let ground = *frame.get_pixel(x, (square.top() * ppp).round() as u32);
                        let treat = if treatment == Treatment::Disabled { disabled } else { normal };
                        let off = difference(&actual, &over(&reference(key, dark), actual.dimensions(), ground, treat));
                        assert!(off <= MAX_DIFFERENCE, "{key} {treatment:?} at {side} is {off} from its reference");
                    }
                }
            }
        }
    }
    std::fs::write(case.path("worst.tsv"), "mode\tscale\tside\tworst\tof\tclosest pair\tof\n".to_owned() + &report)
        .unwrap();
    let probe = Harness::builder().with_size(vec2(8.0, 8.0)).build_ui(|_| {});
    theme::set_mode(&probe.ctx.clone(), Mode::Light);
}
