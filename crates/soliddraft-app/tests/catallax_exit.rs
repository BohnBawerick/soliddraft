//! The Catallax bracket target displayed and picked by the real application renderer.

use std::collections::BTreeMap;

use egui::{Key, pos2, vec2};
use egui_kittest::Harness;
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
}
