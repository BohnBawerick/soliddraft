//! P2 through the real command layer and revolve editor.
use serde_json::{Value, json};
use soliddraft_app::command_tools::{CommandTools, ProposalBook, Surface};
use soliddraft_app::document::{Document, FeatureKind, PickEntry, Selection};
use soliddraft_assistant::{Caller, ToolHost};

fn call(doc: &mut Document, name: &str, input: Value) -> Value {
    let mut book = ProposalBook::default();
    let result = CommandTools { doc, book: &mut book, surface: Surface::Mcp, drawing: None }.call(
        name,
        input,
        &Caller::Mcp { client: "P2 regression".into() },
    );
    assert!(!result.is_error, "{name}: {}", result.content);
    serde_json::from_str(&result.content).unwrap()
}

fn meridian() -> Document {
    let mut doc = Document::new("P2 collar", soliddraft_app::kernel());
    call(
        &mut doc,
        "sketch",
        json!({"plane":"top","shapes":[{"shape":"rectangle","center":[9,6,0],"width":6,"height":12}]}),
    );
    doc
}

fn selectable(doc: &Document) {
    assert_eq!((doc.faces.len(), doc.edges.len()), (6, 6));
    assert!(doc.features.iter().all(|f| matches!(f.status, soliddraft_app::document::FeatureStatus::Ok)));
    let mesh = &doc.meshes[&doc.display_body().unwrap()];
    assert!(!mesh.indices.is_empty());
    for face in &doc.faces {
        let pick = doc.pick_of_face(face.id).expect("face pick");
        assert!(matches!(doc.pick(pick), Some(PickEntry::Face(_, id)) if id == face.id));
        assert!(mesh.face_picks.contains(&pick));
    }
    for edge in &doc.edges {
        let pick = doc.pick_of_edge(edge.id).expect("edge pick");
        assert!(matches!(doc.pick(pick), Some(PickEntry::Edge(_, id)) if id == edge.id));
        assert!(mesh.edges.iter().any(|e| e.pick == pick && !e.segments.is_empty()));
    }
}

#[test]
fn collar_commands_keep_picking_names_through_edit_undo_and_reopen() {
    let mut doc = meridian();
    call(
        &mut doc,
        "revolve",
        json!({"sketch":"Sketch1","axis_origin":[0,0,0],"axis_direction":[0,1,0],"angle_deg":360}),
    );
    assert_eq!((doc.faces.len(), doc.edges.len()), (4, 4));
    // Rectangle line 2 is the outer meridian, x=12. Select by persistent
    // construction origin, never by nearest sampled coordinate.
    let edges: Vec<_> = doc
        .edges
        .iter()
        .filter_map(|edge| {
            let reference = doc.edge_ref(edge.id)?;
            reference
                .faces
                .iter()
                .any(|face| matches!(face.generator, soliddraft_model::Generator::Side { entity } if entity == 2))
                .then(|| format!("Revolve1 · {}", edge.label))
        })
        .collect();
    assert_eq!(edges.len(), 2);
    eprintln!("P2 outer rim names: {edges:?}");
    call(&mut doc, "chamfer", json!({"edges":edges,"distance":1}));
    selectable(&doc);
    let names: Vec<_> = doc.faces.iter().map(|face| face.label.clone()).collect();
    call(&mut doc, "edit_chamfer", json!({"feature":"Chamfer1","distance":0.75}));
    selectable(&doc);
    assert_eq!(names, doc.faces.iter().map(|face| face.label.clone()).collect::<Vec<_>>());
    call(&mut doc, "undo", json!({}));
    selectable(&doc);
    call(&mut doc, "redo", json!({}));
    selectable(&doc);
    let path = std::env::temp_dir().join(format!("soliddraft-p2-{}.dwprt", std::process::id()));
    doc.model().save(&path).unwrap();
    let mut reopened = Document::new("reopened", soliddraft_app::kernel());
    call(&mut reopened, "open", json!({"path":path}));
    selectable(&reopened);
    assert!(matches!(reopened.features[1].kind, FeatureKind::Revolve(_)));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn revolve_ribbon_opens_and_commits_the_real_editor() {
    use egui_kittest::{Harness, kittest::Queryable};
    let instance = egui_wgpu::wgpu::Instance::default();
    if pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all())).is_empty() {
        eprintln!("P2 UI NOT RUN: no GPU adapter");
        return;
    }
    let config = std::env::temp_dir().join("soliddraft-p2-ui-config");
    std::fs::create_dir_all(&config).unwrap();
    soliddraft_app::file::set_config_dir(Some(config));
    let mut doc = meridian();
    doc.selection = Some(Selection::Feature(doc.features[0].id));
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .with_render_every_step(true)
        .wgpu()
        .build_eframe(move |cc| soliddraft_app::app::SolidDraftApp::new(cc, doc, Default::default()));
    harness.run_steps(4);
    harness.get_by_label("Revolve").click();
    harness.run_steps(4);
    assert!(matches!(
        harness.state().state.editor.as_ref().unwrap().params,
        soliddraft_app::state::EditorParams::Revolve(_)
    ));
    harness.get_by_label("OK (Enter)").click();
    harness.run_steps(4);
    assert!(harness.state().state.editor.is_none());
    assert!(matches!(harness.state().state.doc.features[1].kind, FeatureKind::Revolve(_)));
    assert_eq!(harness.state().state.doc.faces.len(), 4);
    let path = std::env::temp_dir().join("soliddraft-p2-revolve-ui.png");
    harness.render().unwrap().save(path).unwrap();
    for (variant, tool, args) in [
        ("original", "chamfer", json!({"edges":["Revolve1 · Edge 1","Revolve1 · Edge 3"],"distance":1})),
        ("edited", "edit_chamfer", json!({"feature":"Chamfer1","distance":0.75})),
    ] {
        let start = std::time::Instant::now();
        call(&mut harness.state_mut().state.doc, tool, args);
        harness.run_steps(4);
        let frame = harness.render().unwrap();
        eprintln!(
            "P2 {variant} command through rendered frame: {:.3} ms (GUI test harness)",
            start.elapsed().as_secs_f64() * 1000.0
        );
        selectable(&harness.state().state.doc);
        frame.save(std::env::temp_dir().join(format!("soliddraft-p2-{variant}-ui.png"))).unwrap();
        let path = std::env::temp_dir().join(format!("soliddraft-p2-{variant}.step"));
        let result = soliddraft_app::app_tools::AppTools { state: &mut harness.state_mut().state }.call(
            "export_step",
            json!({"path":path}),
            &Caller::Mcp { client: "P2 output regression".into() },
        );
        assert!(!result.is_error, "{variant} STEP: {}", result.content);
        assert!(std::fs::metadata(&path).unwrap().len() > 0);
    }
    harness.get_by_label("Make drawing").click();
    harness.run_steps(4);
    harness.get_by_label("Create drawing (Enter)").click();
    harness.run_steps(4);
    assert!(harness.state().state.drawing.is_active());
    let drawing = &harness.state().state.drawing.session.as_ref().unwrap().doc.drawing;
    assert_eq!(drawing.sheets[0].views.len(), 4);
    assert!(drawing.sheets[0].views.iter().all(|v| v.geometry.curves.iter().any(|c| c.visible)));
    harness.render().unwrap().save(std::env::temp_dir().join("soliddraft-p2-drawing-ui.png")).unwrap();
}
