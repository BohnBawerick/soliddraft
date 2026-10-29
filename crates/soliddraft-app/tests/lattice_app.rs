//! The human lattice route uses the same saved feature as the agent verbs.

use egui::{Key, Modifiers, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use soliddraft_app::app::{Automation, SolidDraftApp};
use soliddraft_app::document::{Command, Document, ExtrudeParams, FeatureKind, ProfileKind};

type App = Harness<'static, SolidDraftApp>;

fn button(app: &mut App, label: &str) {
    app.get_all_by_role_and_label(egui::accesskit::Role::Button, label)
        .next()
        .unwrap_or_else(|| panic!("missing button {label}"))
        .click();
    app.run_steps(4);
}

fn field(app: &mut App, label: &str, text: &str) {
    app.get_by_label(label).click();
    app.run_steps(2);
    app.key_press_modifiers(Modifiers::COMMAND, Key::A);
    app.get_by_label(label).type_text(text);
    app.key_press(Key::Enter);
    app.run_steps(4);
}

#[test]
fn ribbon_and_editor_create_a_terminal_unproved_lattice() {
    let instance = egui_wgpu::wgpu::Instance::default();
    assert!(
        !pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all())).is_empty(),
        "lattice click acceptance requires a renderer"
    );
    let temporary = std::env::temp_dir().join(format!("soliddraft-lattice-{}", std::process::id()));
    std::fs::create_dir_all(&temporary).unwrap();
    soliddraft_app::file::set_config_dir(Some(temporary.join("config")));
    let mut doc = Document::new("Lattice UI", soliddraft_app::kernel());
    doc.apply(Command::AddExtrude {
        params: ExtrudeParams {
            profile: ProfileKind::Rectangle { width: 25.0, height: 15.0 },
            depth: 10.0,
            ..Default::default()
        },
        via_assistant: false,
    })
    .unwrap();
    let mut doc = Some(doc);
    let mut app = Harness::builder()
        .with_size(vec2(1440.0, 900.0))
        .with_render_every_step(true)
        .wgpu()
        .build_eframe(move |cc| SolidDraftApp::new(cc, doc.take().unwrap(), Automation::default()));
    app.run_steps(5);
    button(&mut app, "Lattice");
    assert!(app.state().state.editor.is_some());
    // Seed other values so typing proves each control writes its parameter.
    let soliddraft_app::state::EditorParams::Lattice(l) = &mut app.state_mut().state.editor.as_mut().unwrap().params
    else {
        panic!("lattice editor")
    };
    l.cell_size_mm = Some(6.0);
    l.radius_mm = Some(2.0);
    app.run_steps(2);
    field(&mut app, "Cell size value", "5");
    field(&mut app, "Density (beam radius) value", "1");
    assert!(app.query_by_label("unproved preview").is_some());
    button(&mut app, "OK (Enter)");
    app.run_steps(12);
    let doc = &app.state().state.doc;
    let last = doc.features.last().unwrap();
    assert_eq!(last.name, "Lattice1");
    let FeatureKind::Lattice(l) = last.kind else { panic!("last feature is lattice") };
    assert_eq!(l.cell_size_mm, Some(5.0));
    assert_eq!(l.radius_mm, Some(1.0));
    assert_eq!(last.status, soliddraft_app::document::FeatureStatus::Ok);
    assert!(last.summary().contains("unproved preview"));
    let image = std::path::PathBuf::from(
        std::env::var_os("LATTICE_UI_EVIDENCE").unwrap_or_else(|| temporary.join("lattice.png").into_os_string()),
    );
    app.render().unwrap().save(image).unwrap();
}
