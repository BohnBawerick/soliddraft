//! Intersect's persistent command and real editor contract; not full U8 acceptance.
use serde_json::{Value, json};
use soliddraft_app::app_tools::AppTools;
use soliddraft_app::document::{Command, Document, ExtrudeParams, FeatureKind, ProfileKind, Selection};
use soliddraft_app::state::AppState;
use soliddraft_assistant::{Caller, tools::call_checked};

fn call(state: &mut AppState, name: &str, args: Value) {
    let out = call_checked(&mut AppTools { state }, name, args, &Caller::Mcp { client: "intersect-test".into() });
    assert!(!out.is_error, "{name}: {}", out.content);
}
fn base() -> AppState {
    let mut doc = Document::new("intersection", soliddraft_app::kernel());
    doc.apply(Command::AddExtrude {
        params: ExtrudeParams {
            profile: ProfileKind::Rectangle { width: 10.0, height: 10.0 },
            depth: 10.0,
            ..Default::default()
        },
        via_assistant: false,
    })
    .unwrap();
    AppState::new(doc)
}
fn profile(state: &mut AppState, center: f64) {
    call(
        state,
        "sketch",
        json!({"plane":"top","shapes":[{"shape":"rectangle","center":[center,0,0],"width":8,"height":8}]}),
    );
}
fn volume(doc: &Document, expected: f64) {
    let actual = doc.kernel().mass_properties(doc.display_body().unwrap()).unwrap().volume;
    assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
}

#[test]
fn add_cut_and_intersect_have_distinct_material_and_persistent_operations() {
    for (verb, expected, flip) in [("extrude", 1096.0, false), ("cut", 712.0, true), ("intersect", 288.0, false)] {
        let mut state = base();
        profile(&mut state, 3.0);
        let mut args = json!({"sketch":"Sketch1","depth":6,"flip":flip});
        if verb == "cut" {
            args["through_all"] = json!(false);
        }
        call(&mut state, verb, args);
        volume(&state.doc, expected);
        if verb != "intersect" {
            continue;
        }
        let FeatureKind::Extrude(params) = &state.doc.features.last().unwrap().kind else {
            panic!("extrude");
        };
        assert!(params.intersect);
        assert_eq!(state.doc.features.last().unwrap().name, "Intersect-Extrude1");
        call(&mut state, "edit_extrude", json!({"feature":"Intersect-Extrude1","depth":4,"flip":false}));
        volume(&state.doc, 192.0);
        state.doc.undo().unwrap();
        volume(&state.doc, 288.0);
        state.doc.redo().unwrap();
        volume(&state.doc, 192.0);
        let path = std::env::temp_dir().join(format!("soliddraft-intersect-{}.dwprt", std::process::id()));
        state.doc.model().save(&path).unwrap();
        let mut reopened = AppState::new(Document::new("reopen", soliddraft_app::kernel()));
        call(&mut reopened, "open", json!({"path":path}));
        volume(&reopened.doc, 192.0);
        assert!(matches!(&reopened.doc.features.last().unwrap().kind,FeatureKind::Extrude(p) if p.intersect));
        std::fs::remove_file(path).unwrap();
    }
    for (op, word) in [
        (soliddraft_model::ExtrudeOp::Boss, "boss"),
        (soliddraft_model::ExtrudeOp::Cut, "cut"),
        (soliddraft_model::ExtrudeOp::Intersect, "intersect"),
    ] {
        assert_eq!(serde_json::to_value(op).unwrap(), json!(word));
        assert_eq!(serde_json::from_value::<soliddraft_model::ExtrudeOp>(json!(word)).unwrap(), op);
    }
    let mut legacy = serde_json::to_value(ExtrudeParams::default()).unwrap();
    legacy.as_object_mut().unwrap().remove("intersect");
    assert!(!serde_json::from_value::<ExtrudeParams>(legacy).unwrap().intersect);
}

#[test]
fn unsupported_and_empty_intersections_preserve_the_previous_body() {
    for shapes in [
        json!([{"shape":"rectangle","center":[30,0,0],"width":8,"height":8}]),
        json!([{"shape":"rectangle","center":[-3,0,0],"width":2,"height":2},{"shape":"rectangle","center":[3,0,0],"width":2,"height":2}]),
    ] {
        let mut state = base();
        call(&mut state, "sketch", json!({"plane":"top","shapes":shapes}));
        let before = state.doc.display_body();
        let params = ExtrudeParams {
            sketch: Some(state.doc.features.last().unwrap().id),
            intersect: true,
            depth: 6.0,
            ..Default::default()
        };
        let error = state.doc.preview_intersect(&params, before).expect_err("preview must refuse");
        eprintln!("intersection preview refusal: {error}");
        let out = call_checked(
            &mut AppTools { state: &mut state },
            "intersect",
            json!({"sketch":"Sketch1","depth":6,"flip":false}),
            &Caller::Mcp { client: "refusal".into() },
        );
        assert!(out.is_error, "{}", out.content);
        assert_eq!(state.doc.display_body(), before);
        volume(&state.doc, 1000.0);
    }
}

#[test]
fn intersect_requires_a_body_and_rejects_bad_requests_without_changes() {
    let mut empty = AppState::new(Document::new("no body", soliddraft_app::kernel()));
    profile(&mut empty, 0.0);
    let out = call_checked(
        &mut AppTools { state: &mut empty },
        "intersect",
        json!({"sketch":"Sketch1","depth":6,"flip":false}),
        &Caller::Mcp { client: "validation".into() },
    );
    assert!(out.is_error);
    assert!(out.content.contains("intersection needs an existing body"), "{}", out.content);
    assert!(empty.doc.display_body().is_none());
    let mut state = base();
    profile(&mut state, 3.0);
    let before = state.doc.display_body();
    let undo = state.doc.undo_depth();
    for args in [
        json!({"sketch":"Sketch1","depth":0,"flip":false}),
        json!({"sketch":"Sketch1","depth":6}),
        json!({"sketch":"Sketch1","depth":6,"flip":false,"up_to_next":true}),
    ] {
        let out = call_checked(
            &mut AppTools { state: &mut state },
            "intersect",
            args,
            &Caller::Mcp { client: "validation".into() },
        );
        assert!(out.is_error, "{}", out.content);
        assert_eq!(state.doc.display_body(), before);
        assert_eq!(state.doc.undo_depth(), undo);
    }
    let params =
        ExtrudeParams { intersect: true, end: soliddraft_app::document::EndKind::UpToNext, ..Default::default() };
    assert!(state.doc.preview_intersect(&params, before).unwrap_err().contains("fixed depth"));
}

#[test]
fn intersect_ribbon_previews_common_material_and_edits_the_real_feature() {
    use egui_kittest::{Harness, kittest::Queryable};
    let instance = egui_wgpu::wgpu::Instance::default();
    assert!(
        !pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all())).is_empty(),
        "Intersect acceptance requires a renderer"
    );
    let mut state = base();
    profile(&mut state, 3.0);
    state.doc.selection = Some(Selection::Feature(state.doc.features.last().unwrap().id));
    let mut app = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .with_render_every_step(true)
        .wgpu()
        .build_eframe(move |cc| soliddraft_app::app::SolidDraftApp::new(cc, state.doc, Default::default()));
    app.run_steps(4);
    app.get_by_label("Intersect").click();
    app.run_steps(4);
    let editor = app.state().state.editor.as_ref().unwrap();
    assert!(editor.preview.result.is_some(), "{:?}", editor.error);
    assert!(editor.preview.tool.is_none());
    assert!(editor.preview.result.as_ref().unwrap().positions.iter().all(|p| p.x <= 5.0 && p.x >= -1.0));

    app.get_by_label("OK (Enter)").click();
    app.run_steps(4);
    volume(&app.state().state.doc, 480.0);
    assert!(matches!(&app.state().state.doc.features.last().unwrap().kind,FeatureKind::Extrude(p) if p.intersect));
    app.get_all_by_role_and_label(egui::accesskit::Role::Button, "Intersect-Extrude1").next().unwrap().click();
    app.run_steps(4);
    app.get_all_by_role_and_label(egui::accesskit::Role::Button, "Edit feature").next().unwrap().click();
    app.run_steps(4);
    app.get_by_label("Depth value").click();
    app.run_steps(2);
    app.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    app.get_by_label("Depth value").type_text("4");
    app.run_steps(2);
    app.key_press(egui::Key::Enter);
    app.run_steps(4);
    assert!(app.state().state.editor.as_ref().unwrap().preview.result.is_some());
    app.get_by_label("OK (Enter)").click();
    app.run_steps(4);
    volume(&app.state().state.doc, 192.0);
    let out = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/exit-test/l08-intersect");
    std::fs::create_dir_all(&out).unwrap();
    app.render().unwrap().save(out.join("edited-intersection.png")).unwrap();
}
