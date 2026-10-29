//! Rendered Shell click path. This is the analytic tray, not full P4's final intersection.

use egui::{Key, Modifiers, Pos2, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use soliddraft_app::app::{Automation, SolidDraftApp};
use soliddraft_app::document::{Command, Document, ExtrudeParams, FeatureKind, FeatureStatus, ProfileKind, Selection};
use soliddraft_app::state::{AppAction, EditorParams};

type App = Harness<'static, SolidDraftApp>;

fn button(app: &mut App, label: &str) {
    app.get_all_by_role_and_label(egui::accesskit::Role::Button, label)
        .next()
        .unwrap_or_else(|| panic!("missing button {label}"))
        .click();
    app.run_steps(4);
}

fn field(app: &mut App, value: &str) {
    app.get_by_label("Wall thickness value").click();
    app.run_steps(2);
    app.key_press_modifiers(Modifiers::COMMAND, Key::A);
    app.get_by_label("Wall thickness value").type_text(value);
    app.run_steps(3);
    app.key_press(Key::Enter);
    app.run_steps(4);
}

fn click(app: &mut App, point: Pos2) {
    app.hover_at(point);
    app.run_steps(4);
    for pressed in [true, false] {
        app.event(egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        });
        app.step();
    }
    app.run_steps(4);
}

fn check_volume(app: &App, thickness: f64) {
    let doc = &app.state().state.doc;
    let volume = doc.kernel().mass_properties(doc.display_body().unwrap()).unwrap().volume;
    let expected = 14400.0 - (40.0 - 2.0 * thickness) * (30.0 - 2.0 * thickness) * (12.0 - thickness);
    assert!((volume - expected).abs() < 1e-6, "{volume} != {expected}");
}

#[test]
fn shell_opens_picks_previews_edits_persists_selects_and_exports() {
    let instance = egui_wgpu::wgpu::Instance::default();
    assert!(
        !pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all())).is_empty(),
        "shell click acceptance requires a renderer; do not count a skipped test as acceptance"
    );
    let out = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/exit-test/l08-shell");
    std::fs::create_dir_all(&out).unwrap();
    soliddraft_app::file::set_config_dir(Some(out.join("config")));
    let mut doc = Document::new("Analytic tray", soliddraft_app::kernel());
    doc.apply(Command::AddExtrude {
        params: ExtrudeParams {
            profile: ProfileKind::Rectangle { width: 40.0, height: 30.0 },
            depth: 12.0,
            ..Default::default()
        },
        via_assistant: false,
    })
    .unwrap();
    let mut app = Harness::builder()
        .with_size(vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .with_render_every_step(true)
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_eframe(move |cc| SolidDraftApp::new(cc, doc, Automation::default()));
    app.run_steps(30);
    app.key_press(Key::Z);
    app.run_steps(30);
    let start = std::time::Instant::now();
    button(&mut app, "Shell");
    let point = {
        let state = &app.state().state;
        state.viewport.camera.project(glam::Vec3::new(0.0, 0.0, 12.0), state.viewport.rect).unwrap()
    };
    click(&mut app, point);
    let editor = app.state().state.editor.as_ref().expect("Shell editor opened");
    let EditorParams::Shell(shell) = &editor.params else { panic!("wrong editor") };
    assert_eq!(shell.open_faces.len(), 1, "viewport click selects the open top");
    assert!(editor.preview.result.is_some(), "{:?}", editor.error);
    field(&mut app, "2");
    button(&mut app, "OK (Enter)");
    app.run_steps(30);
    let build_click_ms = start.elapsed().as_secs_f64() * 1000.0;
    let shell_id = app.state().state.doc.features.iter().find(|f| matches!(f.kind, FeatureKind::Shell(_))).unwrap().id;
    assert_eq!(app.state().state.doc.feature(shell_id).unwrap().status, FeatureStatus::Ok);
    check_volume(&app, 2.0);
    let inner_reference = {
        let doc = &app.state().state.doc;
        let body = doc.display_body().unwrap();
        let topology = doc.kernel().topology(body).unwrap();
        let floor = topology
            .faces
            .iter()
            .find(|face| face.normal == Some(soliddraft_kernel_api::Vec3::Z) && (face.centroid.z - 2.0).abs() < 1e-9)
            .unwrap();
        doc.any_face_ref(floor.id).expect("inner floor has a name before edit")
    };
    app.render().unwrap().save(out.join("tray-2mm.png")).unwrap();

    button(&mut app, "Shell1");
    button(&mut app, "Edit feature");
    field(&mut app, "0");
    let editor = app.state().state.editor.as_ref().unwrap();
    assert!(editor.error.is_some(), "invalid wall must show an error");
    assert!(editor.preview.result.is_none(), "invalid preview must snap back");
    app.render().unwrap().save(out.join("invalid-wall.png")).unwrap();
    let start = std::time::Instant::now();
    field(&mut app, "1.5");
    assert!(app.state().state.editor.as_ref().unwrap().preview.result.is_some());
    button(&mut app, "OK (Enter)");
    app.run_steps(30);
    let edit_click_ms = start.elapsed().as_secs_f64() * 1000.0;
    check_volume(&app, 1.5);
    let save = out.join("tray.dwprt");
    app.state_mut().state.push(AppAction::SaveAs(Some(save.clone())));
    app.run_steps(4);
    assert!(save.is_file());
    app.state_mut().state.push(AppAction::Open(Some(save)));
    app.run_steps(30);
    check_volume(&app, 1.5);
    let doc = &app.state().state.doc;
    assert!(
        doc.model().resolve_face(&inner_reference, doc.display_body().unwrap()).is_ok(),
        "the original inner-wall reference survives the wall edit and reopen"
    );
    let point = {
        let state = &app.state().state;
        state.viewport.camera.project(glam::Vec3::new(0.0, 0.0, 1.5), state.viewport.rect).unwrap()
    };
    click(&mut app, point);
    let doc = &app.state().state.doc;
    let Some(Selection::Face(face)) = doc.selection else { panic!("inner floor must be pickable") };
    let reference = doc.any_face_ref(face).expect("inner floor has a persistent name");
    assert_eq!(reference.generator, soliddraft_model::Generator::ShellWall);
    let step = out.join("tray.step");
    app.state_mut()
        .state
        .push(AppAction::Export { format: soliddraft_exchange::Format::Step, path: Some(step.clone()) });
    app.run_steps(8);
    assert!(std::fs::read_to_string(step).unwrap().starts_with("ISO-10303-21;"));
    app.render().unwrap().save(out.join("tray-1.5mm-reopened.png")).unwrap();
    std::fs::write(
        out.join("click-times.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "scope": "rendered analytic tray click path, includes test frame settling; not full P4",
            "build_click_ms": build_click_ms, "edit_click_ms": edit_click_ms
        }))
        .unwrap(),
    )
    .unwrap();
}
