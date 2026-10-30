//! A body the viewport cannot draw never replaces the part on screen, and the status never
//! calls such a part rebuilt. `Gated` refuses every display mesh on request, so this holds
//! whatever the kernel can mesh today.

mod gated;
#[allow(dead_code)]
#[path = "support/ribbon.rs"]
mod ribbon;

use gated::Gated;
use soliddraft_app::document::{Command, Document, ExtrudeParams, FeatureId, FeatureKind, ProfileKind};
use soliddraft_app::state::AppAction;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The hang guard every test shares: a wait this long means stuck, not slow.
const HANG: Duration = Duration::from_secs(600);
const REASON: &str = "mesh.precision-or-budget";

fn block(depth: f64) -> ExtrudeParams {
    ExtrudeParams { profile: ProfileKind::Rectangle { width: 40.0, height: 30.0 }, depth, ..Default::default() }
}

fn add(depth: f64) -> Command {
    Command::AddExtrude { params: block(depth), via_assistant: false }
}

fn part(kernel: Arc<Gated>) -> (Document, FeatureId) {
    let mut doc = Document::new("Display failure", kernel);
    let id = doc.apply(add(10.0)).unwrap().unwrap();
    (doc, id)
}

fn refuse_display(kernel: &Gated, refuse: bool) {
    kernel.rules.lock().unwrap().refuse_display = refuse;
}

fn volume(doc: &Document) -> f64 {
    doc.kernel().mass_properties(doc.display_body().unwrap()).unwrap().volume
}

fn settle(doc: &mut Document) {
    let until = Instant::now() + HANG;
    while doc.proof_pending() {
        assert!(Instant::now() < until, "proof did not settle");
        doc.poll_proof();
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_new_feature_without_a_display_mesh_is_rolled_back() {
    let kernel = Arc::new(Gated::default());
    let (mut doc, _) = part(kernel.clone());
    let before = (doc.features.len(), doc.undo_depth(), doc.display_body(), doc.triangle_count());
    assert!(before.3 > 0);

    refuse_display(&kernel, true);
    let refused = doc.apply(add(25.0)).unwrap_err();
    assert!(refused.contains(REASON), "{refused}");
    assert_eq!(
        (doc.features.len(), doc.undo_depth(), doc.display_body(), doc.triangle_count()),
        before,
        "the part stays on screen"
    );
    assert_eq!(doc.display_failure(), Some(refused.as_str()));

    // The next change the viewport can draw clears it.
    refuse_display(&kernel, false);
    doc.apply(add(25.0)).unwrap();
    assert_eq!(doc.display_failure(), None);
    assert!(doc.triangle_count() > 0);
}

#[test]
fn the_first_feature_without_a_display_mesh_leaves_an_empty_part() {
    let kernel = Arc::new(Gated::default());
    let mut doc = Document::new("Display failure", kernel.clone());
    refuse_display(&kernel, true);
    let refused = doc.apply(add(10.0)).unwrap_err();
    assert!(refused.contains(REASON), "{refused}");
    assert!(doc.features.is_empty() && doc.display_body().is_none());
    assert_eq!(doc.display_failure(), Some(refused.as_str()));
}

#[test]
fn a_proved_edit_without_a_display_mesh_is_rolled_back() {
    let kernel = Arc::new(Gated::default());
    let (mut doc, id) = part(kernel.clone());
    let before = (doc.undo_depth(), volume(&doc), doc.triangle_count());

    refuse_display(&kernel, true);
    doc.apply_interactive(Command::EditExtrude { id, params: block(15.0) }).unwrap();
    assert!(doc.proof_pending());
    settle(&mut doc);

    assert_eq!((doc.undo_depth(), volume(&doc), doc.triangle_count()), before, "the part stays on screen");
    let Some(FeatureKind::Extrude(params)) = doc.feature(id).map(|f| &f.kind) else { panic!("the extrude stays") };
    assert_eq!(params.depth, 10.0);
    let message = doc.display_failure().expect("the status has the reason");
    assert!(message.contains(REASON), "{message}");
}

#[test]
fn a_part_that_cannot_roll_back_keeps_its_model_and_says_why() {
    let kernel = Arc::new(Gated::default());
    let (doc, _) = part(kernel.clone());
    refuse_display(&kernel, true);
    let reopened = Document::from_model(doc.model().fork());
    assert_eq!(reopened.triangle_count(), 0);
    assert_eq!(reopened.features.len(), doc.features.len(), "nothing the user made is lost");
    let message = reopened.display_failure().expect("the status has the reason");
    assert!(message.contains(REASON), "{message}");
    assert_eq!(reopened.proof_message(), Some((message, false)), "the exact model is still proved");
}

fn assert_failed(lines: &[String], added: bool) {
    assert!(!lines.iter().any(|line| line.contains("Rebuild OK")), "{lines:?}");
    assert!(lines.iter().any(|line| line == "Display failed"), "{lines:?}");
    assert!(lines.iter().any(|line| line.contains(REASON)), "{lines:?}");
    assert_eq!(lines.iter().any(|line| line == "Feature added"), added, "{lines:?}");
}

/// The case that shipped once: an empty viewport under a green status.
#[test]
fn the_status_never_calls_a_part_with_no_mesh_rebuilt() {
    let case = ribbon::case("display-failure-blank");
    let kernel = Arc::new(Gated::default());
    let (doc, _) = part(kernel.clone());
    refuse_display(&kernel, true);
    let mut app = ribbon::app(Document::from_model(doc.model().fork()));
    case.run(&mut app, |app| {
        assert_eq!(app.state().state.doc.triangle_count(), 0);
        assert_failed(&ribbon::status_lines(app), false);

        // The same probe does see the green status once the part is drawn.
        refuse_display(&kernel, false);
        let _ = app.state_mut().state.doc.rebuild();
        app.run_steps(5);
        assert!(app.state().state.doc.triangle_count() > 0);
        let lines = ribbon::status_lines(app);
        assert!(lines.iter().any(|line| line == "Rebuild OK"), "{lines:?}");
    });
}

#[test]
fn a_rolled_back_feature_stays_on_the_status_bar() {
    let case = ribbon::case("display-failure-rollback");
    let kernel = Arc::new(Gated::default());
    let (doc, _) = part(kernel.clone());
    let mut app = ribbon::app(doc);
    case.run(&mut app, |app| {
        let triangles = app.state().state.doc.triangle_count();
        refuse_display(&kernel, true);
        app.state_mut().state.push(AppAction::Command(add(25.0)));
        app.run_steps(5);
        assert_eq!(app.state().state.doc.triangle_count(), triangles, "the part stays on screen");
        assert_failed(&ribbon::status_lines(app), false);
    });
}
