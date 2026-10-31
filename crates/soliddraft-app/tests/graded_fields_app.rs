//! Graded fields through the window and the agent tools: the editor grades D0's beam
//! radius along z, the grade verb grades G0's level, and once a field is last every CAD
//! command is disabled with the terminal-field reason before it reads any input.
//!
//! Requires a renderer: a run without one fails.

use egui::{Key, Modifiers, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::{Value, json};
use soliddraft_app::app::{Automation, SolidDraftApp};
use soliddraft_app::app_tools::AppTools;
use soliddraft_app::commands;
use soliddraft_app::document::{Document, FeatureKind, FeatureStatus};
use soliddraft_assistant::Caller;
use soliddraft_assistant::tools::call_checked;
use soliddraft_kernel_api::{Grade, GradeAxis, LinearGrade};

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

fn tool(app: &mut App, name: &str, arguments: Value) -> Result<Value, String> {
    let caller = Caller::Mcp { client: "test".into() };
    let out = call_checked(&mut AppTools { state: &mut app.state_mut().state }, name, arguments, &caller);
    if out.is_error { Err(out.content) } else { Ok(serde_json::from_str(&out.content).unwrap()) }
}

fn renderer() -> App {
    let instance = egui_wgpu::wgpu::Instance::default();
    assert!(
        !pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all())).is_empty(),
        "graded field acceptance requires a renderer"
    );
    let temporary = std::env::temp_dir().join(format!("soliddraft-graded-{}", std::process::id()));
    std::fs::create_dir_all(&temporary).unwrap();
    soliddraft_app::file::set_config_dir(Some(temporary.join("config")));
    let mut doc = Some(Document::new("Graded", soliddraft_app::kernel()));
    Harness::builder()
        .with_size(vec2(1440.0, 900.0))
        .with_render_every_step(true)
        .wgpu()
        .build_eframe(move |cc| SolidDraftApp::new(cc, doc.take().unwrap(), Automation::default()))
}

/// The source region, made by the verbs the scenario uses: a `w` square on the top
/// plane at `z`, extruded `depth`.
fn region(app: &mut App, w: f64, z: f64, depth: f64) {
    let shape = json!({"shape":"rectangle", "center":[0,0,z], "width":w, "height":w});
    tool(app, "sketch", json!({"plane":"top", "offset":z, "shapes":[shape]})).unwrap();
    tool(app, "extrude", json!({"sketch":"Sketch1", "depth":depth, "flip":false})).unwrap();
    app.run_steps(5);
}

fn last(app: &App) -> (String, FeatureKind, FeatureStatus) {
    let f = app.state().state.doc.features.last().unwrap();
    (f.name.clone(), f.kind.clone(), f.status.clone())
}

#[test]
fn the_editor_grades_d0s_beam_radius_along_z() {
    let mut app = renderer();
    region(&mut app, 6.0, 20.0, 10.0);
    button(&mut app, "Lattice");
    field(&mut app, "Cell size value", "100");
    assert!(app.query_by_label("Beam radius (mm), controls density").is_some(), "the designed label");
    button(&mut app, "Linear along an axis");
    button(&mut app, "z");
    for (name, value) in [("from", "20"), ("to", "30"), ("start", "1"), ("end", "2")] {
        field(&mut app, &format!("Beam radius (mm), controls density {name} value"), value);
    }
    field(&mut app, "Accuracy value", "0.1");
    field(&mut app, "Minimum detail value", "0.1");
    assert!(app.query_by_label("unproved preview").is_some());
    button(&mut app, "OK (Enter)");
    app.run_steps(12);
    let (name, kind, status) = last(&app);
    assert_eq!(name, "Lattice1");
    let FeatureKind::Lattice(l) = kind else { panic!("last feature is a lattice") };
    let d0 = Grade::Linear(LinearGrade { axis: GradeAxis::Z, from: 20.0, to: 30.0, start: 1.0, end: 2.0 });
    assert_eq!((l.cell_size_mm, l.radius_mm, l.accuracy_mm, l.min_detail_mm), (Some(100.0), Some(d0), 0.1, 0.1));
    assert_eq!(status, FeatureStatus::Ok);

    // The summary states the grade as it was entered, and shows no measured property.
    let summary = tool(&mut app, "model_summary", json!({})).unwrap();
    let radius = &summary["features"][2]["radius_mm"]["linear"];
    assert_eq!(
        (radius["axis"].as_str(), radius["from"].as_f64(), radius["end"].as_f64()),
        (Some("z"), Some(20.0), Some(2.0))
    );
    assert!(summary["body"]["volume_mm3"].is_null());
    let face = tool(&mut app, "feature", json!({"name":"Lattice1"})).unwrap();
    assert!(face["faces"][0]["area_mm2"].is_null() && face["faces"][0]["centroid"].is_null(), "{face}");

    // The grade survives editing the region: the anchors stay at 20 and 30.
    tool(&mut app, "edit_extrude", json!({"feature":"Boss-Extrude1", "depth":12, "flip":false})).unwrap();
    let (_, kind, status) = last(&app);
    assert_eq!((kind, status), (FeatureKind::Lattice(l), FeatureStatus::Ok));
}

#[test]
fn cad_after_a_field_is_refused_before_its_inputs_are_read() {
    let mut app = renderer();
    region(&mut app, 2.0, -1.0, 2.0);
    tool(&mut app, "gyroid", json!({"kind":"network", "period":10, "level":0, "accuracy":0.1, "min_detail":0.1}))
        .unwrap();
    tool(&mut app, "grade_field", json!({"feature":"Gyroid1", "axis":"z", "from":-1, "to":1, "start":-0.1, "end":0.1}))
        .unwrap();
    let summary = tool(&mut app, "model_summary", json!({})).unwrap();
    assert_eq!(summary["features"][2]["level"]["linear"]["start"], json!(-0.1), "{summary}");
    assert_eq!(last(&app).2, FeatureStatus::Ok);
    let gate = "is the gyroid stage and must stay last";

    // The window: each CAD command and a second field is disabled, and says why.
    let ids = [
        "sketch",
        "extrude",
        "revolve",
        "cut",
        "hole",
        "fillet",
        "chamfer",
        "shell",
        "draft",
        "linear_pattern",
        "circular_pattern",
        "mirror",
        "lattice",
        "gyroid",
    ];
    let features = app.state().state.doc.features.len();
    for id in ids {
        let command = commands::get(id).unwrap();
        let state = &app.state().state;
        assert!(command.action_now(state).is_none(), "{id} is disabled");
        assert!(command.hint_now(state).is_some_and(|h| h.contains(gate)), "{id}: {:?}", command.hint_now(state));
    }
    for label in ["Fillet", "Shell", "Lattice"] {
        button(&mut app, label);
        assert!(app.state().state.editor.is_none(), "{label} opened no editor");
    }
    assert_eq!(app.state().state.doc.features.len(), features);

    // The agent tools: refused with the same reason before any named input resolves,
    // so a name that does not exist is never looked up.
    for (name, arguments) in [
        ("extrude", json!({"sketch":"NoSuchSketch", "depth":1, "flip":false})),
        ("fillet", json!({"edges":["NoSuchEdge"], "radius":0.1})),
        ("shell", json!({"open_faces":["NoSuchFace"], "thickness":0.1})),
        ("hole", json!({"face":"NoSuchFace", "points":[[0,0,1]], "diameter":0.2})),
        ("linear_pattern", json!({"direction":[1,0,0], "spacing":5, "count":2})),
        ("mirror", json!({"plane":"NoSuchFace", "keep_original":true})),
    ] {
        let error = tool(&mut app, name, arguments).unwrap_err();
        assert!(error.contains(gate), "{name}: {error}");
    }
    let second =
        tool(&mut app, "lattice", json!({"cell":"cubic", "cell_size":5, "radius":1, "accuracy":0.1, "min_detail":0.5}));
    assert!(second.unwrap_err().contains("must be the only one and the last feature"));
    assert_eq!(app.state().state.doc.features.len(), features);
    assert_eq!(
        app.state().state.doc.model().cad_after_field().and_then(|e| e.kernel_code()),
        Some("lattice.cad-after-lattice")
    );

    // Returning before the field brings CAD back; rolling forward rebuilds the same grade.
    tool(&mut app, "rollback", json!({"before":"Gyroid1"})).unwrap();
    assert!(commands::get("fillet").unwrap().action_now(&app.state().state).is_some());
    tool(&mut app, "rollback", json!({"before":null})).unwrap();
    let (_, kind, status) = last(&app);
    let FeatureKind::Gyroid(g) = kind else { panic!("gyroid") };
    assert_eq!(g.level, Grade::Linear(LinearGrade { axis: GradeAxis::Z, from: -1.0, to: 1.0, start: -0.1, end: 0.1 }));
    assert_eq!(status, FeatureStatus::Ok);
}
