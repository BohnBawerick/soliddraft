//! V2 batch 2 G2: the sketcher's midpoint and vertical guide, with real pointer input,
//! becomes exactly the relations it showed, and Auto relations off infers nothing.

#[path = "support/measure_window.rs"]
mod window;

use egui::{Key, vec2};
use egui_kittest::kittest::Queryable;
use soliddraft_app::{
    document::{Command, Document, ExtrudeParams, FeatureId, PlaneKind, SketchPlane},
    sketch::{Align, ConstraintKind, DistanceKind, EntityKind, P2, PointRef, Sketch, solve},
    state::{AppAction, Snap, Tool},
};
use std::sync::Arc;
use window::App;

/// The batch 1 block: Sketch1, a centred 40 x 30 rectangle on Top, extruded 10.
fn part() -> (Document, FeatureId) {
    let mut doc = Document::new("Guided block", Arc::new(soliddraft_kernel_catallax::CatallaxKernel::new()));
    let plane = SketchPlane::Reference { plane: PlaneKind::Top, offset: 0.0 };
    let id = doc.apply(Command::AddSketch { plane }).unwrap().unwrap();
    let mut sketch = Sketch::new();
    sketch.rectangle(P2::new(0.0, 0.0), 40.0, 30.0).unwrap();
    solve::solve(&mut sketch, None, true);
    doc.apply(Command::EditSketch { id, sketch }).unwrap();
    let params = ExtrudeParams { sketch: Some(id), depth: 10.0, ..Default::default() };
    doc.apply(Command::AddExtrude { params, via_assistant: false }).unwrap();
    (doc, id)
}

fn sketch(h: &App<'_>, id: FeatureId) -> Sketch {
    h.state().state.doc.feature(id).unwrap().sketch().unwrap().sketch.clone()
}
fn session<'h>(h: &'h App<'_>) -> &'h soliddraft_app::state::SketchSession {
    h.state().state.sketch.as_ref().expect("the sketch is open")
}
fn volume(h: &App<'_>) -> f64 {
    let doc = &h.state().state.doc;
    doc.kernel().mass_properties(doc.display_body().unwrap()).unwrap().volume
}
fn open_line_tool(h: &mut App<'_>, id: FeatureId) {
    h.state_mut().state.push(AppAction::EditSketch(id));
    window::settle(h);
    h.state_mut().state.push(AppAction::SketchTool(Tool::Line));
    h.run_steps(3);
}
/// Where the second click lands: 10 mm above the bottom midpoint, three points to the right.
fn upper(h: &App<'_>) -> egui::Pos2 {
    window::sketch_screen(h, P2::new(0.0, -5.0)) + vec2(3.0, 0.0)
}

/// V2 batch 2 G2.
#[test]
fn midpoint_guide_toggle_commits_the_seen_relation() {
    let (doc, id) = part();
    let Some(mut h) = window::window(doc, "measure-guides") else { return };
    let solid = volume(&h);
    open_line_tool(&mut h, id);
    h.get_by_label("Draw construction").click();
    h.run_steps(3);
    let bottom = sketch(&h, id)
        .entities
        .iter()
        .find(|e| matches!(e.kind, EntityKind::Line { a, b } if a.v == -15.0 && b.v == -15.0))
        .unwrap()
        .id;
    let before = sketch(&h, id);
    let revision = h.state().state.doc.revision();
    let unchanged = |h: &App<'_>| {
        let now = sketch(h, id);
        assert_eq!((now.entities.len(), now.constraints.len()), (before.entities.len(), before.constraints.len()));
        assert_eq!(h.state().state.doc.revision(), revision, "a guide alone changes nothing");
    };

    // Hover the bottom line's midpoint, start there, then aim near (0, -5).
    let start = window::sketch_screen(&h, P2::new(0.0, -15.0));
    window::hover(&mut h, start);
    assert_eq!(session(&h).snaps, [Snap::Midpoint(bottom)]);
    window::click_at(&mut h, start);
    unchanged(&h);
    let at = upper(&h);
    window::hover(&mut h, at);
    assert_eq!(session(&h).snaps, [Snap::Vertical], "the vertical guide at u = 0");
    unchanged(&h);
    window::snapshot(&mut h, "guide-enabled");
    window::click_at(&mut h, at);
    h.key_press(Key::Escape);
    h.run_steps(3);
    h.state_mut().state.push(AppAction::ExitSketch(None));
    window::settle(&mut h);

    let after = sketch(&h, id);
    assert_eq!(after.entities.len(), before.entities.len() + 1, "exactly one new line");
    let line = after.entities.last().unwrap().clone();
    assert!(line.construction);
    let EntityKind::Line { a, b } = line.kind else { panic!("{line:?}") };
    assert!(a.dist(P2::new(0.0, -15.0)) < 1e-9 && b.u.abs() < 1e-9 && (b.v + 5.0).abs() < 0.5, "{a:?} {b:?}");
    let added: Vec<_> = after.constraints[before.constraints.len()..].iter().map(|c| c.kind.clone()).collect();
    assert_eq!(
        added,
        [
            ConstraintKind::Midpoint { point: PointRef::new(line.id, 0), line: bottom },
            ConstraintKind::Vertical { target: Align::Line(line.id) },
        ],
        "the relations the guide showed, and no external reference"
    );
    assert_eq!(after.constraints[..before.constraints.len()], before.constraints[..], "existing relations remain");
    assert_eq!(volume(&h), solid, "a construction line is not profile");

    // Undo, redo and a saved file keep the relations and their ids.
    h.state_mut().state.push(AppAction::Undo);
    window::settle(&mut h);
    assert_eq!(sketch(&h, id).entities.len(), before.entities.len());
    h.state_mut().state.push(AppAction::Redo);
    window::settle(&mut h);
    assert_eq!(sketch(&h, id), after);
    let path = std::env::temp_dir().join(format!("soliddraft-measure-guides-{}.dwprt", std::process::id()));
    h.state_mut().state.push(AppAction::SaveAs(Some(path.clone())));
    window::settle(&mut h);
    h.state_mut().state.push(AppAction::Open(Some(path)));
    window::settle(&mut h);
    assert_eq!(sketch(&h, id), after);

    // The relations hold through a height change, given the line's own length.
    let mut edited = after.clone();
    let height = edited
        .constraints
        .iter()
        .find(
            |c| matches!(c.kind, ConstraintKind::Distance { kind: DistanceKind::Vertical, value, .. } if value == 30.0),
        )
        .unwrap()
        .id;
    edited.constraint_mut(height).unwrap().set_value(36.0);
    edited.constrain(ConstraintKind::Distance {
        a: PointRef::new(line.id, 0),
        b: PointRef::new(line.id, 1),
        kind: DistanceKind::Aligned,
        value: 10.0,
        label: P2::new(5.0, -13.0),
    });
    solve::solve(&mut edited, None, true);
    let EntityKind::Line { a, b } = edited.entity(line.id).unwrap().kind else { panic!() };
    assert!(a.dist(P2::new(0.0, -18.0)) < 1e-6 && b.dist(P2::new(0.0, -8.0)) < 1e-6, "{a:?} {b:?}");

    // Off: the point stays under the pointer and nothing is inferred.
    open_line_tool(&mut h, id);
    h.get_by_label("Auto relations").click();
    h.run_steps(3);
    assert!(!session(&h).auto_relations);
    let before = sketch(&h, id);
    let start = window::sketch_screen(&h, P2::new(0.0, -15.0));
    window::hover(&mut h, start);
    assert!(session(&h).snaps.is_empty());
    window::click_at(&mut h, start);
    let at = upper(&h);
    window::hover(&mut h, at);
    assert!(session(&h).snaps.is_empty(), "no guide while auto relations are off");
    window::snapshot(&mut h, "guide-off");
    window::click_at(&mut h, at);
    h.key_press(Key::Escape);
    h.run_steps(3);
    let off = sketch(&h, id);
    assert_eq!(off.entities.len(), before.entities.len() + 1);
    assert_eq!(off.constraints, before.constraints, "no inferred relation");
    let EntityKind::Line { b, .. } = off.entities.last().unwrap().kind else { panic!() };
    assert!(b.u.abs() > 1e-6, "the end stays where the pointer was: {b:?}");

    // Escape before the second click creates nothing. Escape also leaves the sketch.
    if h.state().state.sketch.as_ref().is_none_or(|s| s.tool != Tool::Line) {
        open_line_tool(&mut h, id);
    }
    if !session(&h).auto_relations {
        h.get_by_label("Auto relations").click();
        h.run_steps(3);
    }
    let before = sketch(&h, id);
    let elsewhere = window::sketch_screen(&h, P2::new(10.0, 0.0));
    window::click_at(&mut h, elsewhere);
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert_eq!(sketch(&h, id), before);
    assert!(h.state().state.sketch.as_ref().is_none_or(|s| s.pending.is_empty()), "the started line is dropped");
}
