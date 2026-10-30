//! V2 batch 2 U2: pair rows follow accepted proof, never a pending body, a refused
//! proof or a reply for an older body.

mod gated;

use serde_json::Value;
use soliddraft_app::{
    document::{Command, Document, ExtrudeParams, ProfileKind, Selection},
    measure,
};
use soliddraft_kernel_api::PairQuantity;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The hang guard every test shares: a wait this long means stuck, not slow.
const HANG: Duration = Duration::from_secs(600);

/// The 40 x 30 block and the E0/E1 diagonal pair, tracked with `< 32 mm`.
fn block(kernel: Arc<gated::Gated>) -> (Document, u64, ExtrudeParams) {
    let mut doc = Document::new("Tracked pair", kernel);
    let params = ExtrudeParams {
        profile: ProfileKind::Rectangle { width: 40.0, height: 30.0 },
        depth: 10.0,
        ..Default::default()
    };
    let id = doc.apply(Command::AddExtrude { params: params.clone(), via_assistant: false }).unwrap().unwrap();
    let source = diagonal(&doc);
    doc.apply(Command::Track { name: "diagonal_gap".into(), source, check: Some("< 32 mm".into()) }).unwrap();
    (doc, id, params)
}
/// E0, the bottom front edge, and E1, the top back edge, at the current depth.
fn diagonal(doc: &Document) -> soliddraft_model::tracked::Source {
    let top = doc.edges.iter().map(|e| e.midpoint.z).fold(0.0, f32::max);
    let find = |y: f32, z: f32| {
        doc.edges
            .iter()
            .find(|e| {
                (e.length - 40.0).abs() < 1e-3 && (e.midpoint.y - y).abs() < 1e-3 && (e.midpoint.z - z).abs() < 1e-3
            })
            .unwrap()
            .id
    };
    let (e0, e1) = (find(-15.0, 0.0), find(15.0, top));
    measure::pair(doc, Selection::Edge(e0), Selection::Edge(e1), PairQuantity::MinimumDistance).unwrap()
}
fn row(doc: &Document) -> Value {
    measure::values(doc)[0].clone()
}
fn settle(doc: &mut Document) {
    let until = Instant::now() + HANG;
    while doc.proof_pending() {
        assert!(Instant::now() < until, "proof did not settle");
        doc.poll_proof();
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn encloses(r: &Value, n: f64) {
    let (lo, hi) = (r["tracked"]["lo"].as_f64().unwrap(), r["tracked"]["hi"].as_f64().unwrap());
    assert!(lo.mul_add(lo, -n) <= 0.0 && hi.mul_add(hi, -n) >= 0.0 && hi - lo <= 1e-8, "sqrt({n}): {r}");
}

#[test]
fn pair_proofs_pending_refused_and_late_do_not_lie() {
    let kernel = Arc::new(gated::Gated::default());
    let (mut doc, id, mut params) = block(kernel.clone());
    encloses(&row(&doc), 1000.0);
    assert_eq!(row(&doc)["tracked"]["check"]["state"], "pass");

    // Pending: the D10 number stays, marked stale, and no new row is accepted.
    kernel.rules.lock().unwrap().hold = Some(15.0);
    params.depth = 15.0;
    doc.apply_interactive(Command::EditExtrude { id, params: params.clone() }).unwrap();
    assert!(doc.proof_pending());
    let r = row(&doc);
    assert_eq!(r["tracked"]["state"], "stale");
    encloses(&r, 1000.0);
    assert_eq!(r["tracked"]["check"]["state"], "unknown");
    let source = doc.model().tracked()[0].source.clone();
    let refused = doc.apply(Command::Track { name: "other".into(), source, check: None });
    kernel.rules.lock().unwrap().hold = None;
    kernel.changed.notify_all();
    assert!(refused.unwrap_err().contains("pending"));
    settle(&mut doc);
    let r = row(&doc);
    assert_eq!(r["tracked"]["state"], "current");
    encloses(&r, 1125.0);
    assert_eq!(r["tracked"]["check"]["state"], "fail");

    // A reply keeps the geometry it answered, so a D15 proof cannot pass for D16's.
    let late = measure::measure_pair(&doc, &doc.model().tracked()[0].source).unwrap();

    // Refused after the edit is accepted: the new geometry stands and the row is stale.
    kernel.rules.lock().unwrap().refuse_pairs = true;
    params.depth = 16.0;
    doc.apply_interactive(Command::EditExtrude { id, params }).unwrap();
    settle(&mut doc);
    assert_eq!(doc.model().body().map(|b| doc.kernel().mass_properties(b).unwrap().volume), Some(40.0 * 30.0 * 16.0));
    let r = row(&doc);
    assert_eq!(r["tracked"]["state"], "stale");
    encloses(&r, 1125.0);
    assert!(r["tracked"]["reason"].as_str().unwrap().contains("measure.bounds-incomplete"), "{r}");
    assert_eq!(r["tracked"]["check"]["state"], "unknown");
    assert_ne!(r["tracked"]["geometry"], serde_json::json!(doc.model().geometry_hash()));
    assert_ne!(late["geometry"], serde_json::json!(doc.model().geometry_hash()), "the late reply is not current");

    // A refused Track keeps the truth, the history and the redo entry.
    doc.undo_interactive().unwrap();
    settle(&mut doc);
    let (hash, undo, redo) = (doc.model().semantic_hash(), doc.undo_depth(), doc.redo_depth());
    assert_eq!(redo, 1);
    let source = diagonal(&doc);
    let refused = doc.apply(Command::Track { name: "other".into(), source, check: None });
    assert!(refused.unwrap_err().contains("measure.bounds-incomplete"));
    assert_eq!((doc.model().semantic_hash(), doc.undo_depth(), doc.redo_depth()), (hash, undo, redo));
    kernel.rules.lock().unwrap().refuse_pairs = false;
    doc.redo_interactive().unwrap();
    settle(&mut doc);
    assert_eq!(row(&doc)["tracked"]["state"], "current");
    encloses(&row(&doc), 900.0 + 256.0);
}
