//! Drag preview scenarios, driven through the same drag steps the PropertyManager's
//! dimension field pushes, with no window: continuous input, cancellation, a change of
//! setup in the middle of a drag, errors that arrive after their value was passed, a
//! failed final value, and one undo step per drag.
//!
//! The kernel is Catallax behind a wrapper that can hold an extrusion of one chosen depth
//! until the scenario lets it go, or refuse one, so the order in which answers arrive is
//! fixed by the scenario rather than by timing.

use soliddraft_app::document::{Command, Document, FeatureId, FeatureKind, FeatureStatus, Selection};
use soliddraft_app::drag::{Dimension, Shown};
use soliddraft_app::state::{AppState, DragStep};
mod gated;
use gated::Gated;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The hang guard every test shares: a wait this long means stuck, not slow.
const HANG: Duration = Duration::from_secs(600);

impl Gated {
    fn hold(&self, depth: f64) {
        self.rules.lock().unwrap().hold = Some(depth);
    }

    fn refuse(&self, depth: f64) {
        self.rules.lock().unwrap().refuse.push(depth);
    }

    /// Waits until an extrusion is held.
    fn wait_held(&self) {
        let rules = self.rules.lock().unwrap();
        let (_rules, timeout) = self.changed.wait_timeout_while(rules, HANG, |r| r.holding.is_none()).unwrap();
        assert!(!timeout.timed_out(), "no extrusion reached the hold");
    }

    fn release(&self) {
        let mut rules = self.rules.lock().unwrap();
        rules.hold = None;
        self.changed.notify_all();
    }

    fn release_into_hold(&self, depth: f64) {
        self.rules.lock().unwrap().hold = Some(depth);
        self.changed.notify_all();
        let rules = self.rules.lock().unwrap();
        let (_rules, timeout) = self.changed.wait_timeout_while(rules, HANG, |r| r.holding != Some(depth)).unwrap();
        assert!(!timeout.timed_out(), "the newest extrusion never reached its hold");
    }
}

/// The sample part (plate, flange, boss) on a gated kernel, with the plate selected.
fn scenario() -> (AppState, Arc<Gated>, FeatureId) {
    let dir = std::env::temp_dir().join(format!("soliddraft-drag-preview-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    soliddraft_app::file::set_config_dir(Some(dir));
    let kernel = Arc::new(Gated::default());
    let mut doc = Document::new("Part1", kernel.clone());
    soliddraft_app::populate_demo(&mut doc);
    assert_eq!(doc.error_count(), 0, "the sample part builds");
    let plate = doc.features[0].id;
    doc.selection = Some(Selection::Feature(plate));
    (AppState::new(doc), kernel, plate)
}

fn depth(state: &AppState, id: FeatureId) -> f64 {
    match &state.doc.feature(id).unwrap().kind {
        FeatureKind::Extrude(p) => p.depth,
        other => panic!("not an extrude: {other:?}"),
    }
}

fn volume(state: &AppState) -> f64 {
    let body = state.doc.display_body().expect("a body");
    state.doc.kernel().mass_properties(body).unwrap().volume
}

/// Polls once per frame until the drag has answered its newest value.
fn settle(state: &mut AppState) {
    let deadline = Instant::now() + HANG;
    while state.poll_drag() {
        assert!(Instant::now() < deadline, "the drag preview never answered");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Polls while an exact proof is held until `shown` is true. Construction runs beside
/// its proof, so the early display can arrive after the proof reaches the hold.
fn poll_until(state: &mut AppState, shown: impl Fn(&AppState) -> bool) {
    let deadline = Instant::now() + HANG;
    while !shown(state) {
        assert!(Instant::now() < deadline, "the construction display never arrived while its proof was held");
        state.poll_drag();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn start(state: &mut AppState, feature: FeatureId) {
    state.drag_dimension(DragStep::Start { feature, dimension: Dimension::Depth });
    assert!(state.drag_preview.is_some(), "the drag started");
}

fn shown_value(state: &AppState) -> Option<f64> {
    match state.drag_preview.as_ref()?.shown() {
        Shown::Result(r) => Some(r.value),
        _ => None,
    }
}

/// A scripted drag of the plate from 10 mm to 30 mm in 0.5 mm steps, one per 60 Hz
/// frame, keeps the interface free and shows results as the worker makes them; every
/// result shown is newer than the one before; letting go applies 30 mm exactly, once.
#[test]
fn continuous_drag_delivers_intermediates_and_applies_the_final_value_once() {
    let (mut state, _kernel, plate) = scenario();
    let undo_before = state.doc.undo_depth();
    let volume_before = volume(&state);
    start(&mut state, plate);
    let mut delivered = 0;
    let mut last_shown = None;
    let mut slowest_frame = Duration::ZERO;
    for step in 1..=40 {
        let frame = Instant::now();
        state.drag_dimension(DragStep::Move(10.0 + 0.5 * step as f64));
        let pending = state.poll_drag();
        slowest_frame = slowest_frame.max(frame.elapsed());
        if let Some(v) = shown_value(&state)
            && last_shown != Some(v)
        {
            assert!(last_shown.is_none_or(|l| v > l), "a result older than the one shown came back: {v}");
            assert!(state.preview_body().is_some(), "a shown result is drawn");
            last_shown = Some(v);
            delivered += 1;
        }
        assert!(pending || shown_value(&state) == Some(10.0 + 0.5 * step as f64));
        std::thread::sleep(Duration::from_millis(16));
    }
    settle(&mut state);
    if shown_value(&state) != last_shown {
        delivered += 1;
    }
    assert_eq!(shown_value(&state), Some(30.0), "the preview converges on the value the drag is at");
    // The document has not moved during the drag.
    assert_eq!(depth(&state, plate), 10.0);
    assert_eq!(state.doc.undo_depth(), undo_before);
    eprintln!(
        "scripted drag: 40 moves over {} ms of frames delivered {delivered} intermediate results; \
         slowest frame spent {} ms in the drag",
        40 * 16,
        slowest_frame.as_millis()
    );

    state.drag_dimension(DragStep::Finish);
    assert!(state.drag_preview.is_none());
    assert!(state.preview_body().is_none(), "after the drag the part itself is shown");
    assert_eq!(depth(&state, plate), 30.0);
    assert_eq!(state.doc.undo_depth(), undo_before + 1, "one drag is one undo step");
    assert!(volume(&state) > volume_before);
    state.doc.undo().unwrap();
    assert_eq!(depth(&state, plate), 10.0, "one undo returns to where the drag started");
    assert!((volume(&state) - volume_before).abs() < 1e-6);
}

/// Escape in the middle of a drag drops it: nothing is applied, nothing is left on
/// screen, and the answer still owed is dropped when it comes.
#[test]
fn cancelling_a_drag_applies_nothing_and_drops_late_answers() {
    let (mut state, kernel, plate) = scenario();
    let undo_before = state.doc.undo_depth();
    let edits_before = state.doc.edits;
    start(&mut state, plate);
    state.drag_dimension(DragStep::Move(14.5));
    settle(&mut state);
    assert_eq!(shown_value(&state), Some(14.5));
    kernel.hold(17.25);
    state.drag_dimension(DragStep::Move(17.25));
    kernel.wait_held();
    state.drag_dimension(DragStep::Cancel);
    assert!(state.drag_preview.is_none());
    assert!(state.preview_body().is_none());
    kernel.release();
    // A finish after the cancel, as the pointer lets go, finds no drag.
    state.drag_dimension(DragStep::Finish);
    assert!(!state.poll_drag());
    assert_eq!(depth(&state, plate), 10.0);
    assert_eq!(state.doc.undo_depth(), undo_before);
    assert_eq!(state.doc.edits, edits_before, "a cancelled drag never touches the part");
}

/// A change of setup in the middle of a drag makes it obsolete: an undo, a command from
/// elsewhere, or a new selection. Its owed answer is dropped and letting go applies
/// nothing.
#[test]
fn a_setup_change_mid_drag_drops_the_drag() {
    type Change = fn(&mut AppState, FeatureId);
    let changes: [(&str, Change); 3] = [
        ("selection", |s, _| s.doc.selection = Some(Selection::Feature(s.doc.features[1].id))),
        ("rename", |s, plate| {
            s.doc.apply(Command::Rename { id: plate, name: "Plate".into() }).unwrap();
        }),
        ("undo", |s, _| {
            s.doc.undo().unwrap();
        }),
    ];
    for (what, change) in changes {
        let (mut state, kernel, plate) = scenario();
        start(&mut state, plate);
        kernel.hold(21.75);
        state.drag_dimension(DragStep::Move(21.75));
        kernel.wait_held();
        change(&mut state, plate);
        let undo_before = state.doc.undo_depth();
        assert!(!state.drag_preview.as_ref().unwrap().is_current(&state.doc), "{what} changes the setup");
        assert!(!state.poll_drag(), "{what}: an obsolete drag waits for nothing");
        assert!(state.drag_preview.is_none(), "{what}: the obsolete drag is dropped");
        assert!(state.preview_body().is_none());
        kernel.release();
        state.drag_dimension(DragStep::Finish);
        assert_eq!(state.doc.undo_depth(), undo_before, "{what}: nothing applied after the change");
        if what != "undo" {
            assert_eq!(depth(&state, plate), 10.0, "{what}: the depth stays");
        }
    }
}

/// An error for a value the drag has already passed is dropped, and the newer value's
/// result is shown; an error for the value the drag is at is shown as a failed preview,
/// with the part itself left on screen rather than the last good preview.
#[test]
fn obsolete_errors_are_dropped_and_a_current_error_is_shown() {
    let (mut state, kernel, plate) = scenario();
    kernel.refuse(18.5);
    kernel.hold(18.5);
    start(&mut state, plate);
    state.drag_dimension(DragStep::Move(12.0));
    settle(&mut state);
    assert_eq!(shown_value(&state), Some(12.0));
    // 18.5 fails, but the drag moves on to 19.5 before the failure arrives.
    state.drag_dimension(DragStep::Move(18.5));
    kernel.wait_held();
    state.drag_dimension(DragStep::Move(19.5));
    kernel.release();
    settle(&mut state);
    assert_eq!(shown_value(&state), Some(19.5), "the passed value's error was dropped");
    // The drag comes back to rest on the failing value.
    state.drag_dimension(DragStep::Move(18.5));
    settle(&mut state);
    match state.drag_preview.as_ref().unwrap().shown() {
        Shown::Failed { value, .. } => {
            assert_eq!(*value, 18.5);
        }
        other => panic!("the current error is shown, not {other:?}"),
    }
    assert!(state.preview_body().is_none(), "a failed preview shows the part, not an older preview");
}

/// Letting go on a value that does not build never shows the last good preview as
/// success: the edit lands as one undo step, the feature shows its error, the preview is
/// gone and one undo brings back the part as it was.
#[test]
fn a_failed_final_value_shows_the_failure_not_the_last_preview() {
    let (mut state, kernel, plate) = scenario();
    let undo_before = state.doc.undo_depth();
    let volume_before = volume(&state);
    let proved_body = state.doc.display_body();
    let proved_revision = state.doc.revision();
    kernel.refuse(33.5);
    start(&mut state, plate);
    state.drag_dimension(DragStep::Move(25.0));
    settle(&mut state);
    assert_eq!(shown_value(&state), Some(25.0));
    // Let go before the worker has answered the failing value.
    state.drag_dimension(DragStep::Move(33.5));
    state.drag_dimension(DragStep::Finish);
    assert!(state.drag_preview.is_none());
    assert!(state.preview_body().is_none(), "the last good preview is not left on screen");
    assert!(state.doc.proof_pending());
    assert!(state.export_part(soliddraft_exchange::Format::Step).is_err());
    settle(&mut state);
    assert!(
        matches!(&state.doc.feature(plate).unwrap().status, FeatureStatus::Error(_)),
        "the feature shows its error"
    );
    assert_eq!(state.doc.display_body(), proved_body);
    assert_eq!(state.doc.displayed_revision(), proved_revision);
    assert!(state.doc.revision() > proved_revision);
    assert!(state.export_part(soliddraft_exchange::Format::Step).is_err());
    assert_eq!(state.doc.undo_depth(), undo_before + 1, "one drag is one undo step");
    state.doc.undo().unwrap();
    assert_eq!(depth(&state, plate), 10.0);
    assert_eq!(state.doc.error_count(), 0);
    assert!((volume(&state) - volume_before).abs() < 1e-6);
    state.doc.redo().unwrap();
    assert_eq!(state.doc.display_body(), proved_body);
    assert!(state.doc.displayed_revision() < state.doc.revision());
    assert!(state.export_part(soliddraft_exchange::Format::Step).is_err());
}

/// Dragging back over values already built shows them at once from the drag's cache,
/// and a drag that ends where it started applies nothing.
#[test]
fn dragging_back_reuses_built_values_and_an_unchanged_drag_applies_nothing() {
    let (mut state, _kernel, plate) = scenario();
    let undo_before = state.doc.undo_depth();
    start(&mut state, plate);
    state.drag_dimension(DragStep::Move(15.0));
    settle(&mut state);
    state.drag_dimension(DragStep::Move(10.0));
    assert!(!state.poll_drag(), "the starting value is the part's own mesh");
    assert_eq!(shown_value(&state), Some(10.0));
    state.drag_dimension(DragStep::Move(15.0));
    assert!(!state.poll_drag(), "a value built before is shown without the worker");
    assert_eq!(shown_value(&state), Some(15.0));
    state.drag_dimension(DragStep::Move(10.0));
    state.drag_dimension(DragStep::Finish);
    assert_eq!(state.doc.undo_depth(), undo_before);
    assert_eq!(depth(&state, plate), 10.0);
}

#[test]
fn late_success_never_displays_a_superseded_value_or_enables_export() {
    let (mut state, kernel, plate) = scenario();
    let original = state.doc.display_body();
    start(&mut state, plate);
    state.drag_dimension(DragStep::Move(12.0));
    settle(&mut state);
    assert_eq!(shown_value(&state), Some(12.0));

    kernel.hold(14.0);
    state.drag_dimension(DragStep::Move(14.0));
    kernel.wait_held();
    state.drag_dimension(DragStep::Move(18.0));
    kernel.release_into_hold(18.0);
    poll_until(&mut state, |state| shown_value(state) == Some(18.0));
    assert!(state.poll_drag());
    assert_eq!(shown_value(&state), Some(18.0), "the newer construction is visible while its proof is held");
    assert!(matches!(state.drag_preview.as_ref().unwrap().shown(), Shown::Result(result) if !result.proved));
    assert!(state.preview_body().is_some());
    assert!(state.export_part(soliddraft_exchange::Format::Step).is_err());
    assert_eq!(state.doc.display_body(), original);

    kernel.release();
    settle(&mut state);
    assert_eq!(shown_value(&state), Some(18.0));
    assert!(state.export_part(soliddraft_exchange::Format::Step).is_err());
    state.drag_dimension(DragStep::Cancel);
    assert_eq!(state.doc.display_body(), original);
    assert_eq!(depth(&state, plate), 10.0);
}

fn block_scenario() -> (AppState, Arc<Gated>, FeatureId) {
    use soliddraft_app::document::{ExtrudeParams, ProfileKind};
    let dir = std::env::temp_dir().join(format!("soliddraft-drag-preview-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    soliddraft_app::file::set_config_dir(Some(dir));
    let kernel = Arc::new(Gated::default());
    let mut doc = Document::new("Block", kernel.clone());
    let feature = doc
        .apply(Command::AddExtrude {
            params: ExtrudeParams {
                profile: ProfileKind::Rectangle { width: 40.0, height: 30.0 },
                depth: 10.0,
                ..Default::default()
            },
            via_assistant: false,
        })
        .unwrap()
        .unwrap();
    doc.selection = Some(Selection::Feature(feature));
    (AppState::new(doc), kernel, feature)
}

fn edit_depth_interactive(state: &mut AppState, feature: FeatureId, value: f64) {
    let kind = Dimension::Depth.with(&state.doc.feature(feature).unwrap().kind, value);
    let command = soliddraft_app::drag::edit_command(feature, kind).unwrap();
    state.doc.apply_interactive(command).unwrap();
}

#[test]
fn cancelling_a_newer_drag_resumes_the_original_proof_and_resolves_its_selected_origin() {
    use soliddraft_app::document::PickEntry;
    let (mut state, kernel, block) = block_scenario();
    let original_volume = volume(&state);
    let undo_before = state.doc.undo_depth();
    kernel.hold(14.0);
    edit_depth_interactive(&mut state, block, 14.0);
    kernel.wait_held();
    let committed = state.doc.revision();
    assert!(state.export_part(soliddraft_exchange::Format::Step).is_err());

    start(&mut state, block);
    state.drag_dimension(DragStep::Move(18.0));
    let deadline = Instant::now() + HANG;
    while shown_value(&state) != Some(18.0) {
        assert!(state.poll_drag());
        assert!(Instant::now() < deadline, "the newer construction was held behind the old proof");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(state.preview_body().unwrap().bounds().unwrap().1.z, 18.0);
    state.drag_dimension(DragStep::Cancel);
    assert_eq!(depth(&state, block), 14.0);
    assert_eq!(state.doc.revision(), committed);
    assert_eq!(state.doc.undo_depth(), undo_before + 1);
    let mesh = state.preview_body().expect("the committed construction is restored");
    assert_eq!(mesh.bounds().unwrap().1.z, 14.0);
    let PickEntry::Face(_, provisional) = state.doc.pick(mesh.face_picks[0]).unwrap() else {
        panic!("the provisional face is selectable");
    };
    state.doc.selection = Some(Selection::Face(provisional));
    assert!(state.doc.any_face_ref(provisional).is_none());
    assert!(state.export_part(soliddraft_exchange::Format::Step).is_err());
    kernel.release();
    settle(&mut state);
    assert_eq!(state.doc.displayed_revision(), committed);
    let Some(Selection::Face(proved)) = state.doc.selection else {
        panic!("the same origin resolves after proof");
    };
    assert_ne!(proved, provisional);
    assert!(state.doc.any_face_ref(proved).is_some());
    assert_eq!(volume(&state), 40.0 * 30.0 * 14.0);
    state.export_part(soliddraft_exchange::Format::Step).unwrap();
    state.doc.undo_interactive().unwrap();
    assert!(state.export_part(soliddraft_exchange::Format::Step).is_err());
    settle(&mut state);
    assert_eq!(volume(&state), original_volume);
}

#[test]
fn pending_edits_undo_and_redo_ignore_an_older_committed_failure() {
    let (mut state, kernel, block) = block_scenario();
    let undo_before = state.doc.undo_depth();
    kernel.refuse(14.0);
    kernel.hold(14.0);
    edit_depth_interactive(&mut state, block, 14.0);
    kernel.wait_held();
    edit_depth_interactive(&mut state, block, 18.0);
    state.doc.undo_interactive().unwrap();
    assert_eq!(depth(&state, block), 14.0);
    state.doc.redo_interactive().unwrap();
    assert_eq!(depth(&state, block), 18.0);
    let current = state.doc.revision();
    assert!(state.export_part(soliddraft_exchange::Format::Step).is_err());
    kernel.release_into_hold(18.0);
    poll_until(&mut state, |state| state.doc.displayed_revision() == current);
    assert!(state.poll_drag());
    assert_eq!(state.doc.error_count(), 0);
    assert!(state.doc.proof_pending());
    assert_eq!(state.doc.displayed_revision(), current);
    assert_eq!(state.preview_body().unwrap().bounds().unwrap().1.z, 18.0);
    assert!(state.export_part(soliddraft_exchange::Format::Step).is_err());
    kernel.release();
    settle(&mut state);
    assert_eq!(state.doc.revision(), current);
    assert_eq!(state.doc.displayed_revision(), current);
    assert_eq!(state.doc.feature(block).unwrap().status, FeatureStatus::Ok);
    assert_eq!(state.doc.undo_depth(), undo_before + 2);
    assert_eq!(volume(&state), 40.0 * 30.0 * 18.0);
    state.export_part(soliddraft_exchange::Format::Step).unwrap();
}

/// A sketch that needs a repair adds no geometry while every feature that uses it is
/// suppressed. Its part is the current exact part: its faces can be referenced, and a
/// later scalar edit is adopted rather than snapped back for the old repair.
#[test]
fn a_sketch_needing_repair_with_no_active_user_does_not_fail_the_part() {
    let mut doc = soliddraft_app::command_tools::board_bracket();
    let named = |doc: &Document, name: &str| doc.features.iter().find(|f| f.name == name).unwrap().id;
    for name in ["Cut-Extrude1", "Boss-Extrude3"] {
        let id = named(&doc, name);
        doc.apply(Command::Suppress { id, suppressed: true }).unwrap();
    }
    let sketch = named(&doc, "Sketch4");
    assert!(matches!(doc.feature(sketch).unwrap().status, FeatureStatus::NeedsRepair(_)));
    let mut state = AppState::new(doc);
    assert!(state.doc.proof_state().current_proved_display());
    let face = state.doc.faces[0].id;
    assert!(state.doc.any_face_ref(face).is_some(), "the current part's faces can be referenced");

    let plate = named(&state.doc, "Boss-Extrude1");
    let before = volume(&state);
    edit_depth_interactive(&mut state, plate, 12.0);
    settle(&mut state);
    assert!(!state.doc.proof_pending());
    assert!(!state.doc.proof_state().failed, "the old repair is not this edit's failure");
    assert_eq!(state.doc.displayed_revision(), state.doc.revision());
    assert!(matches!(state.doc.feature(sketch).unwrap().status, FeatureStatus::NeedsRepair(_)));
    assert!(volume(&state) > before, "the deeper plate is shown");
}
