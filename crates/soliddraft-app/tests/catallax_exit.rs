//! The Catallax bracket target displayed and picked by the real application renderer.

use std::collections::BTreeMap;

use egui::{Key, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soliddraft_app::app::{Automation, SolidDraftApp};
use soliddraft_app::document::{self, FeatureKind, Selection};
use soliddraft_kernel_api::{BooleanOp, Vec3};
use soliddraft_model::{
    self as model, Command, DatumPlane, EndCondition, ExtrudeFeature, ExtrudeOp, FeatureId, ProfileSource,
};
use soliddraft_recipe::{Command as RecipeCommand, Shape};
use soliddraft_sketch::{P2, Sketch};

fn bracket() -> document::Document {
    let recipe = soliddraft_recipe::parse(include_str!("../../../recipes/bracket.json")).unwrap();
    let mut part = model::Document::new("Catallax bracket", soliddraft_app::kernel());
    let mut profiles = BTreeMap::new();
    for step in &recipe.steps {
        match &step.command {
            RecipeCommand::Profile { plane, shape } => {
                profiles.insert(step.id.clone(), (plane.plane(), shape.clone()));
            }
            RecipeCommand::Extrude { profile, distance, .. } => {
                let (plane, shape) = &profiles[profile];
                let datum = if plane.normal == Vec3::Z {
                    DatumPlane::Top
                } else {
                    assert_eq!(plane.normal, Vec3::X);
                    DatumPlane::Right
                };
                let frame = datum.plane();
                let shift = P2::new(plane.origin.dot(frame.x_dir), plane.origin.dot(frame.y_dir()));
                let mut sketch = Sketch::new();
                match shape {
                    Shape::Rectangle { center, width, height } => {
                        sketch.rectangle(P2::new(center.u + shift.u, center.v + shift.v), *width, *height).unwrap();
                    }
                    Shape::Circle { center, radius } => {
                        sketch.circle_at(P2::new(center.u + shift.u, center.v + shift.v), radius * 2.0).unwrap();
                    }
                    _ => panic!("bracket fixture changed"),
                }
                let sketch = part
                    .execute(Command::AddFeature {
                        kind: model::FeatureKind::Sketch(model::SketchFeature {
                            plane: model::SketchPlane::Datum { datum, offset: plane.origin.dot(frame.normal) },
                            sketch,
                        }),
                        name: Some(profile.clone()),
                        position: None,
                        via_assistant: false,
                    })
                    .unwrap()
                    .created
                    .unwrap();
                let cut = recipe.steps.iter().any(|s| matches!(&s.command, RecipeCommand::Boolean { kind: BooleanOp::Subtract, tool, .. } if tool == &step.id));
                part.execute(Command::AddFeature {
                    kind: model::FeatureKind::Extrude(ExtrudeFeature {
                        profile: ProfileSource::Sketch { feature: sketch },
                        operation: if cut { ExtrudeOp::Cut } else { ExtrudeOp::Boss },
                        end: EndCondition::Blind { depth: *distance },
                        reverse: false,
                    }),
                    name: Some(step.id.clone()),
                    position: None,
                    via_assistant: false,
                })
                .unwrap();
                let last = part.features().last().unwrap();
                assert!(
                    matches!(part.status(last.id), Some(model::FeatureStatus::Ok)),
                    "{}: {:?}",
                    step.id,
                    part.status(last.id)
                );
            }
            RecipeCommand::Boolean { .. } => {}
            _ => panic!("bracket fixture changed"),
        }
    }
    document::Document::from_model(part)
}

fn volume(doc: &document::Document) -> f64 {
    doc.kernel().mass_properties(doc.display_body().unwrap()).unwrap().volume
}

#[test]
fn bracket_renders_picks_a_named_face_edits_and_undoes_on_catallax() {
    let instance = egui_wgpu::wgpu::Instance::default();
    let available = !pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all())).is_empty();
    if !available {
        assert!(std::env::var_os("SOLIDDRAFT_REQUIRE_GPU").is_none(), "CI requires its software Vulkan adapter");
        return;
    }
    assert_eq!(soliddraft_app::kernel().backend().name, "catallax");
    let doc = bracket();
    let original = volume(&doc);
    let plate = doc.features.iter().find(|f| f.name == "plate").unwrap().id;
    let flange = doc.features.iter().find(|f| f.name == "flange").unwrap().id;
    let mut harness = Harness::builder()
        .with_size(vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .with_render_every_step(true)
        .wgpu()
        .build_eframe(move |cc| SolidDraftApp::new(cc, doc, Automation::default()));
    harness.run_steps(5);
    harness.key_press(Key::Z);
    harness.run_steps(30);
    let image = harness.render().expect("bracket frame renders");
    assert!(image.width() > 1000 && image.height() > 600);
    assert!(harness.state().state.doc.meshes.values().any(|m| m.triangle_count() > 100));
    let state = &harness.state().state;
    let p = state.viewport.camera.project(glam::Vec3::new(55.0, 8.0, 10.0), state.viewport.rect).unwrap();
    harness.hover_at(p);
    harness.run_steps(4);
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos: pos2(p.x, p.y),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        });
        harness.step();
    }
    harness.run_steps(4);
    let doc = &harness.state().state.doc;
    let Some(Selection::Face(face)) = doc.selection else {
        panic!("the rendered plate face must be pickable: {:?}", doc.selection)
    };
    let picked = doc.named_face(face).unwrap();
    assert_eq!(picked.feature, plate, "picked {picked:?}");
    assert_eq!(picked.label, "End cap");
    assert!(doc.describe(Selection::Face(face)).unwrap().title.contains("plate"));
    let doc = &mut harness.state_mut().state.doc;
    let FeatureKind::Extrude(mut params) = doc.features.iter().find(|f| f.id == flange).unwrap().kind.clone() else {
        panic!()
    };
    params.depth = 72.0;
    doc.apply(document::Command::EditExtrude { id: flange, params }).unwrap();
    assert!((volume(doc) - original - 10800.0).abs() < 1e-4);
    assert!(
        doc.faces.iter().any(|f| f.feature == flange && f.label == "End cap" && (f.centroid.z - 72.0).abs() < 1e-5)
    );
    assert!(matches!(doc.model().status(FeatureId(flange.try_into().unwrap())), Some(model::FeatureStatus::Ok)));
    doc.undo().unwrap();
    assert!((volume(doc) - original).abs() < 1e-4);
    harness.run_steps(5);
    harness.render().expect("rebuilt bracket renders");
    drawing_workspace(&mut harness);
}

fn click_button(harness: &mut Harness<'_, SolidDraftApp>, label: &str) {
    harness
        .get_all_by_role_and_label(egui::accesskit::Role::Button, label)
        .next()
        .unwrap_or_else(|| panic!("no button labelled {label}"))
        .click();
    harness.run_steps(4);
}

fn click_at(harness: &mut Harness<'_, SolidDraftApp>, pos: egui::Pos2) {
    harness.hover_at(pos);
    harness.run_steps(3);
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        });
        harness.step();
    }
    harness.run_steps(4);
}

fn drawing_workspace(harness: &mut Harness<'_, SolidDraftApp>) {
    use soliddraft_drawing::ViewKind;

    harness.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::D);
    harness.run_steps(4);
    click_button(harness, "Create drawing (Enter)");
    harness.run_steps(12);
    assert!(harness.state().state.drawing.is_active());
    let session = harness.state().state.drawing.session.as_ref().unwrap();
    assert_eq!(session.doc.drawing.sheets[0].views.len(), 4);
    assert!(session.doc.drawing.sheets[0].views.iter().all(|v| !v.geometry.curves.is_empty()));
    {
        let ctx = soliddraft_app::drawing::part::PartContext::new(&harness.state().state.doc).unwrap();
        let part = ctx.part();
        let index = soliddraft_drawing::PartIndex::build(&part).unwrap();
        for curve in session.doc.drawing.sheets[0].views.iter().flat_map(|v| &v.geometry.curves) {
            assert!(
                matches!(index.resolve(&curve.source), soliddraft_drawing::Resolution::Found(_)),
                "new view reference does not resolve: {:?}",
                curve.source
            );
        }
    }
    let top = session.doc.drawing.sheets[0]
        .views
        .iter()
        .find(|v| soliddraft_drawing::project::view_label(&session.doc.drawing.sheets[0], v) == "Top view")
        .unwrap()
        .id;
    let boss = session.screen_of_model(top, Vec3::new(70.0, 45.0, 30.0)).unwrap();
    click_button(harness, "Section");
    click_at(harness, boss);
    click_at(harness, boss + vec2(70.0, 2.0));
    assert!(harness.state().state.drawing.session.as_ref().unwrap().draft.pending.is_some());
    let session = harness.state().state.drawing.session.as_ref().unwrap();
    let top_pos = session.doc.drawing.view(top).unwrap().position;
    let place = session.screen_of(soliddraft_kernel_api::Point2::new(top_pos.u + 130.0, top_pos.v));
    click_at(harness, place);
    let session = harness.state().state.drawing.session.as_ref().unwrap();
    let section = session.doc.drawing.sheets[0]
        .views
        .iter()
        .find(|v| matches!(v.kind, ViewKind::Section { .. }))
        .expect("the section was placed in the drawing workspace");
    assert_eq!(section.geometry.hatch.len(), 3);
    assert!(section.geometry.curves.iter().any(|c| c.visible));
    harness.render().expect("Catallax drawing workspace renders its section");

    let doc = &mut harness.state_mut().state.doc;
    let plate = doc.features.iter().find(|f| f.name == "plate").unwrap();
    let id = plate.id;
    let FeatureKind::Extrude(mut params) = plate.kind.clone() else { panic!() };
    // The hole tools end at Z=11; keep this edit through-hole to preserve the referenced rims.
    params.depth = 10.5;
    doc.apply(document::Command::EditExtrude { id, params }).unwrap();
    harness.run_steps(12);
    let session = harness.state().state.drawing.session.as_ref().unwrap();
    let update = session.doc.last_update.as_ref().expect("the drawing regenerated after the plate edit");
    assert_eq!(update.views, 5);
    assert!(
        update.dangling.is_empty(),
        "annotation states: {:?}",
        session.doc.drawing.sheets[0].annotations.iter().map(|a| (a.id, &a.state)).collect::<Vec<_>>()
    );
    assert!(session.is_up_to_date());
    let section =
        session.doc.drawing.sheets[0].views.iter().find(|v| matches!(v.kind, ViewKind::Section { .. })).unwrap();
    assert_eq!(section.geometry.hatch.len(), 3);
    let image = harness.render().expect("regenerated Catallax drawing workspace renders");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/exit-test/catallax-drawing.png");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    image.save(path).expect("save the Catallax drawing workspace proof");
}

#[test]
fn polygon_controls_commit_undo_redo_and_extrude_on_catallax() {
    use soliddraft_app::document::{Command, ExtrudeParams, PlaneKind, SketchPlane};
    use soliddraft_app::state::AppAction;

    let instance = egui_wgpu::wgpu::Instance::default();
    if pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all())).is_empty() {
        assert!(std::env::var_os("SOLIDDRAFT_REQUIRE_GPU").is_none(), "CI requires a GPU adapter");
        return;
    }
    let mut harness = Harness::builder()
        .with_size(vec2(1440.0, 1000.0))
        .with_pixels_per_point(1.0)
        .with_render_every_step(true)
        .wgpu()
        .build_eframe(|cc| SolidDraftApp::new(cc, soliddraft_app::new_document(), Automation::default()));
    harness
        .state_mut()
        .state
        .push(AppAction::StartSketchOn(SketchPlane::Reference { plane: PlaneKind::Top, offset: 0.0 }));
    harness.run_steps(5);
    harness.get_by_label("Polygon and modify").scroll_to_me();
    harness.run_steps(3);
    harness.get_by_label("Polygon and modify").click();
    harness.run_steps(4);
    harness.get_by_label("Apply sketch tool").scroll_to_me();
    harness.run_steps(3);
    harness.get_by_label("Apply sketch tool").click();
    harness.run_steps(5);
    let sketch_id = harness.state().state.sketch.as_ref().unwrap().feature;
    let area = 150.0 * 3.0_f64.sqrt();
    let sketch_area = |h: &Harness<'_, SolidDraftApp>| {
        h.state().state.sketch.as_ref().unwrap().sketch.regions().unwrap().iter().map(|r| r.area()).sum::<f64>()
    };
    assert!((sketch_area(&harness) - area).abs() < 1e-5);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
    harness.run_steps(4);
    assert!(harness.state().state.sketch.as_ref().unwrap().sketch.is_empty());
    harness.key_press_modifiers(egui::Modifiers::COMMAND, Key::Y);
    harness.run_steps(4);
    assert!((sketch_area(&harness) - area).abs() < 1e-5);
    let image = harness.render().expect("polygon numeric controls render");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/exit-test/sketch-polygon.png");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    image.save(path).unwrap();
    harness.state_mut().state.push(AppAction::ExitSketch(None));
    harness.run_steps(4);
    let doc = &mut harness.state_mut().state.doc;
    doc.apply(Command::AddExtrude {
        params: ExtrudeParams { sketch: Some(sketch_id), depth: 2.0, ..Default::default() },
        via_assistant: false,
    })
    .unwrap();
    assert!((volume(doc) - area * 2.0).abs() < 0.01);
}
