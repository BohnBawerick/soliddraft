//! V2 A1-A2: tracked rows follow accepted proof, never a pending body.

mod gated;

use serde_json::Value;
use soliddraft_app::{
    document::{Command, Document, ExtrudeParams, ProfileKind, Selection},
    measure,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The hang guard every test shares: a wait this long means stuck, not slow.
const HANG: Duration = Duration::from_secs(600);

fn block(kernel: Arc<gated::Gated>) -> (Document, u64, ExtrudeParams) {
    let mut doc = Document::new("Tracked proof", kernel);
    let params = ExtrudeParams {
        profile: ProfileKind::Rectangle { width: 40.0, height: 30.0 },
        depth: 10.0,
        ..Default::default()
    };
    let id = doc.apply(Command::AddExtrude { params: params.clone(), via_assistant: false }).unwrap().unwrap();
    let edge = doc.edges.iter().find(|e| (e.length - 10.0).abs() < 1e-5).unwrap().id;
    let source = measure::source(&doc, Selection::Edge(edge)).unwrap();
    doc.apply(Command::Track { name: "block_edge".into(), source, check: Some("< 12 mm".into()) }).unwrap();
    (doc, id, params)
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

#[test]
fn a_row_is_stale_until_the_edit_is_proved() {
    let kernel = Arc::new(gated::Gated::default());
    let (mut doc, id, mut params) = block(kernel.clone());
    let source = doc.model().tracked()[0].source.clone();
    kernel.rules.lock().unwrap().hold = Some(15.0);
    params.depth = 15.0;
    doc.apply_interactive(Command::EditExtrude { id, params }).unwrap();
    assert!(doc.proof_pending());
    let r = row(&doc);
    assert_eq!(r["tracked"]["state"], "stale");
    assert_eq!(r["value"], 10.0);
    assert!(r["tracked"]["reason"].as_str().unwrap().contains("pending"));
    assert_eq!(r["tracked"]["check"]["state"], "unknown");
    let refused = doc.apply(Command::Track { name: "other".into(), source, check: None });
    // Release even if the assertion fails, so a failed test cannot hold a worker forever.
    kernel.rules.lock().unwrap().hold = None;
    kernel.changed.notify_all();
    assert!(refused.unwrap_err().contains("pending"));
    settle(&mut doc);
    assert_eq!(row(&doc)["tracked"]["state"], "current");
    assert_eq!(row(&doc)["value"], 15.0);
}

#[test]
fn a_failed_proof_leaves_the_row_stale() {
    let kernel = Arc::new(gated::Gated::default());
    let (mut doc, id, mut params) = block(kernel.clone());
    kernel.rules.lock().unwrap().refuse = vec![15.0];
    params.depth = 15.0;
    doc.apply_interactive(Command::EditExtrude { id, params }).unwrap();
    settle(&mut doc);
    let r = row(&doc);
    assert_eq!(r["tracked"]["state"], "stale");
    assert_eq!(r["value"], 10.0);
    assert_eq!(r["tracked"]["check"]["state"], "unknown");
    doc.undo_interactive().unwrap();
    settle(&mut doc);
    assert_eq!(row(&doc)["tracked"]["state"], "current");
    assert_eq!(row(&doc)["value"], 10.0);
}
