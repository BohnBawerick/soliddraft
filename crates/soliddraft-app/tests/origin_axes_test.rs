//! The origin axes, driven through egui_kittest against the real wgpu pipeline: an
//! existing part file opens unchanged, lists the X, Y and Z axes in the tree beside the
//! planes and shows them in the viewport once one is selected, and clicking an axis row
//! sets the direction of the pattern being edited. Frames are written to
//! `target/origin-axes-test/` for review.
//!
//! Skips itself when no GPU adapter is available (plain CI runners).

use egui::{Color32, Pos2, Rect, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use glam::Vec3;
use soliddraft_app::app::{Automation, SolidDraftApp};
use soliddraft_app::document::Selection;
use soliddraft_app::state::{AppAction, EditorParams};
use soliddraft_kernel_api::Vec3 as KVec3;
use soliddraft_model::PatternSpec;
use std::path::PathBuf;

fn gpu_available() -> bool {
    let _ = env_logger::try_init();
    let instance = egui_wgpu::wgpu::Instance::default();
    !pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all())).is_empty()
}

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/origin-axes-test");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

type App<'a> = Harness<'a, SolidDraftApp>;

fn harness() -> App<'static> {
    Harness::builder()
        .with_size(vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .with_render_every_step(true)
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_eframe(|cc| SolidDraftApp::new(cc, soliddraft_app::new_document(), Automation::default()))
}

fn push(harness: &mut App<'_>, action: AppAction) {
    harness.state_mut().state.push(action);
    harness.run_steps(3);
}

fn render(harness: &mut App<'_>, name: &str) -> image::RgbaImage {
    let image = harness.render().expect("render");
    let path = out_dir().join(format!("{name}.png"));
    image.save(&path).expect("save png");
    eprintln!("wrote {}", path.display());
    image
}

/// How many steps from the projected origin toward `axis` land on a pixel `is_axis`
/// accepts.
fn hits_along(image: &image::RgbaImage, harness: &App<'_>, axis: Vec3, is_axis: impl Fn(Color32) -> bool) -> usize {
    let viewport = &harness.state().state.viewport;
    let rect: Rect = viewport.rect;
    let o = viewport.camera.project(Vec3::ZERO, rect).expect("origin on screen");
    let a = viewport.camera.project(axis * 10.0, rect).expect("axis on screen");
    let dir = (a - o).normalized();
    (2..40)
        .filter(|i| {
            let p: Pos2 = o + dir * (*i as f32 * 4.0);
            (-1..=1).any(|dx| {
                (-1..=1).any(|dy| {
                    let (x, y) = ((p.x as i32 + dx) as u32, (p.y as i32 + dy) as u32);
                    let px = image.get_pixel(x, y).0;
                    is_axis(Color32::from_rgb(px[0], px[1], px[2]))
                })
            })
        })
        .count()
}

#[test]
fn origin_axes_show_in_every_part_and_set_directions() {
    if !gpu_available() {
        eprintln!("no GPU adapter; skipping");
        return;
    }
    // A part saved before the axes existed opens unchanged.
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/near-coincident-flange.dwprt");
    let file = std::env::temp_dir().join(format!("soliddraft-origin-axes-{}.dwprt", std::process::id()));
    std::fs::copy(&fixture, &file).unwrap();
    let before = std::fs::read(&file).unwrap();

    let mut harness = harness();
    push(&mut harness, AppAction::Open(Some(file.clone())));
    harness.run_steps(24);
    assert!(!harness.state().state.doc.features.is_empty(), "the part opened");
    assert_eq!(std::fs::read(&file).unwrap(), before, "opening leaves the file as it was");

    harness.get_by_label("Origin, planes and axes").click();
    harness.run_steps(3);
    for name in ["Front plane", "Top plane", "Right plane", "X axis", "Y axis", "Z axis"] {
        harness.get_by_label(name);
    }
    let hits = |image: &image::RgbaImage, harness: &App<'_>| {
        [
            hits_along(image, harness, Vec3::X, |c| {
                c.r() > c.g().saturating_add(60) && c.r() > c.b().saturating_add(60)
            }),
            hits_along(image, harness, Vec3::Y, |c| {
                c.g() > c.r().saturating_add(40) && c.g() > c.b().saturating_add(20)
            }),
            hits_along(image, harness, Vec3::Z, |c| {
                c.b() > c.r().saturating_add(60) && c.b() > c.g().saturating_add(30)
            }),
        ]
    };
    // Hidden at rest, on show once a reference is selected.
    let image = render(&mut harness, "part-at-rest");
    let [red, green, blue] = hits(&image, &harness);
    assert!(red < 3 && green < 3 && blue < 3, "no axis lines at rest: red {red}, green {green}, blue {blue}");

    harness.get_by_label("Origin").click();
    harness.run_steps(3);
    let image = render(&mut harness, "part-with-axes");
    let [red, green, blue] = hits(&image, &harness);
    assert!(
        red >= 10 && green >= 10 && blue >= 10,
        "axis lines in the viewport: red {red}, green {green}, blue {blue}"
    );

    // A linear pattern starts along X; the Y axis row turns it.
    push(&mut harness, AppAction::StartPattern { circular: false });
    harness.get_by_label("Y axis").click();
    harness.run_steps(3);
    match &harness.state().state.editor.as_ref().expect("pattern editor").params {
        EditorParams::Pattern(p) => match p.layout {
            PatternSpec::Linear { direction, .. } => assert_eq!(direction, KVec3::Y),
            ref other => panic!("linear pattern expected, got {other:?}"),
        },
        _ => panic!("pattern editor expected"),
    }
    render(&mut harness, "pattern-along-y-axis");

    // Cancelled, and with the reference cleared, the axes go away again.
    push(&mut harness, AppAction::CancelEditor);
    push(&mut harness, AppAction::Select(None));
    assert!(!matches!(harness.state().state.doc.selection, Some(Selection::Reference(_))));
    let image = render(&mut harness, "part-after-pattern");
    let [red, green, blue] = hits(&image, &harness);
    assert!(red < 3 && green < 3 && blue < 3, "no axis lines after clearing: red {red}, green {green}, blue {blue}");

    let _ = std::fs::remove_file(&file);
}
