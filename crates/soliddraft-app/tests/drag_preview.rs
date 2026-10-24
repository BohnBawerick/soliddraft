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
use soliddraft_kernel_api::{
    BackendInfo, BodyId, BooleanOp, EdgeId, EdgePolyline, Extrude, FaceId, HistoryRecord, Kernel, KernelError,
    KernelResult, MassProperties, Mesh, OpKind, OpResult, OpTag, Profile, Revolve, TessellationParams, Topology,
    Validation,
};
use soliddraft_kernel_catallax::CatallaxKernel;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Catallax, with extrusions of one depth held back and others refused on request.
#[derive(Default)]
struct Gated {
    inner: CatallaxKernel,
    rules: Mutex<Rules>,
    changed: Condvar,
}

#[derive(Default)]
struct Rules {
    /// Extrusions this deep wait until the hold is lifted.
    hold: Option<f64>,
    /// True while an extrusion waits on the hold.
    holding: bool,
    /// Extrusions this deep fail.
    refuse: Vec<f64>,
}

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
        let (_rules, timeout) =
            self.changed.wait_timeout_while(rules, Duration::from_secs(120), |r| !r.holding).unwrap();
        assert!(!timeout.timed_out(), "no extrusion reached the hold");
    }

    fn release(&self) {
        let mut rules = self.rules.lock().unwrap();
        rules.hold = None;
        self.changed.notify_all();
    }
}

impl Kernel for Gated {
    fn backend(&self) -> BackendInfo {
        self.inner.backend()
    }
    fn create_body_from_profile(&self, tag: &OpTag, profile: &Profile) -> KernelResult<OpResult> {
        self.inner.create_body_from_profile(tag, profile)
    }
    fn extrude(&self, tag: &OpTag, profile: BodyId, params: &Extrude) -> KernelResult<OpResult> {
        {
            let mut rules = self.rules.lock().unwrap();
            if rules.hold == Some(params.distance) {
                rules.holding = true;
                self.changed.notify_all();
                rules = self.changed.wait_while(rules, |r| r.hold == Some(params.distance)).unwrap();
                rules.holding = false;
            }
            if rules.refuse.contains(&params.distance) {
                return Err(KernelError::Failed {
                    op: OpKind::Extrude,
                    code: 0,
                    message: format!("the scenario refuses a depth of {}", params.distance),
                });
            }
        }
        self.inner.extrude(tag, profile, params)
    }
    fn revolve(&self, tag: &OpTag, profile: BodyId, params: &Revolve) -> KernelResult<OpResult> {
        self.inner.revolve(tag, profile, params)
    }
    fn boolean(&self, tag: &OpTag, op: BooleanOp, target: BodyId, tool: BodyId) -> KernelResult<OpResult> {
        self.inner.boolean(tag, op, target, tool)
    }
    fn fillet(&self, tag: &OpTag, body: BodyId, edges: &[EdgeId], radius: f64) -> KernelResult<OpResult> {
        self.inner.fillet(tag, body, edges, radius)
    }
    fn chamfer(&self, tag: &OpTag, body: BodyId, edges: &[EdgeId], distance: f64) -> KernelResult<OpResult> {
        self.inner.chamfer(tag, body, edges, distance)
    }
    fn shell(&self, tag: &OpTag, body: BodyId, open: &[FaceId], thickness: f64) -> KernelResult<OpResult> {
        self.inner.shell(tag, body, open, thickness)
    }
    fn tessellate(&self, body: BodyId, params: &TessellationParams) -> KernelResult<Mesh> {
        self.inner.tessellate(body, params)
    }
    fn tessellate_edges(&self, body: BodyId, params: &TessellationParams) -> KernelResult<Vec<EdgePolyline>> {
        self.inner.tessellate_edges(body, params)
    }
    fn mass_properties(&self, body: BodyId) -> KernelResult<MassProperties> {
        self.inner.mass_properties(body)
    }
    fn validate(&self, body: BodyId) -> KernelResult<Validation> {
        self.inner.validate(body)
    }
    fn topology(&self, body: BodyId) -> KernelResult<Topology> {
        self.inner.topology(body)
    }
    fn history(&self, body: BodyId) -> KernelResult<Vec<HistoryRecord>> {
        self.inner.history(body)
    }
    fn release(&self, body: BodyId) -> KernelResult<()> {
        self.inner.release(body)
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
    let deadline = Instant::now() + Duration::from_secs(120);
    while state.poll_drag() {
        assert!(Instant::now() < deadline, "the drag preview never answered");
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
    assert!(delivered >= 1);

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
        Shown::Failed { value, message } => {
            assert_eq!(*value, 18.5);
            assert!(message.contains("refuses"), "{message}");
        }
        other => panic!("the current error is shown, not {other:?}"),
    }
    assert!(state.preview_body().is_none(), "a failed preview shows the part, not an older preview");
    let (label, failed) = state.drag_preview.as_ref().unwrap().label();
    assert!(failed && label.starts_with("Preview failed"), "{label}");
}

/// Letting go on a value that does not build never shows the last good preview as
/// success: the edit lands as one undo step, the feature shows its error, the preview is
/// gone and one undo brings back the part as it was.
#[test]
fn a_failed_final_value_shows_the_failure_not_the_last_preview() {
    let (mut state, kernel, plate) = scenario();
    let undo_before = state.doc.undo_depth();
    let volume_before = volume(&state);
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
    assert!(
        matches!(&state.doc.feature(plate).unwrap().status, FeatureStatus::Error(e) if e.contains("refuses")),
        "the feature shows its error"
    );
    let note = state.note.as_ref().expect("a note");
    assert!(note.text.contains("did not build"), "{}", note.text);
    assert_eq!(state.doc.undo_depth(), undo_before + 1, "one drag is one undo step");
    state.doc.undo().unwrap();
    assert_eq!(depth(&state, plate), 10.0);
    assert_eq!(state.doc.error_count(), 0);
    assert!((volume(&state) - volume_before).abs() < 1e-6);
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
