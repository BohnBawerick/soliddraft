//! A child application session panics during an edit. Its unclosed journal must
//! reproduce the document, the earlier refusal and the panic without any saved part.

use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};
use soliddraft_app::command_tools::{CommandTools, ProposalBook, Surface};
use soliddraft_app::document::{Command, Document, ExtrudeParams};
use soliddraft_app::drawing::document::DrawingDoc;
use soliddraft_app::journal::Replay;
use soliddraft_assistant::{Caller, ToolHost};
use soliddraft_document::journal::{self, Session};
use soliddraft_kernel_api::*;
use soliddraft_kernel_catallax::CatallaxKernel;

#[derive(Default)]
struct Crashing(CatallaxKernel);
impl Kernel for Crashing {
    fn backend(&self) -> BackendInfo {
        self.0.backend()
    }
    fn create_body_from_profile(&self, tag: &OpTag, profile: &Profile) -> KernelResult<OpResult> {
        self.0.create_body_from_profile(tag, profile)
    }
    fn extrude(&self, tag: &OpTag, profile: BodyId, params: &Extrude) -> KernelResult<OpResult> {
        assert!(params.distance != 13.0, "deliberate journal crash at depth 13");
        self.0.extrude(tag, profile, params)
    }
    fn revolve(&self, tag: &OpTag, profile: BodyId, params: &Revolve) -> KernelResult<OpResult> {
        self.0.revolve(tag, profile, params)
    }
    fn boolean(&self, tag: &OpTag, op: BooleanOp, a: BodyId, b: BodyId) -> KernelResult<OpResult> {
        self.0.boolean(tag, op, a, b)
    }
    fn fillet(&self, tag: &OpTag, body: BodyId, edges: &[EdgeId], radius: f64) -> KernelResult<OpResult> {
        self.0.fillet(tag, body, edges, radius)
    }
    fn chamfer(&self, tag: &OpTag, body: BodyId, edges: &[EdgeId], distance: f64) -> KernelResult<OpResult> {
        self.0.chamfer(tag, body, edges, distance)
    }
    fn shell(&self, tag: &OpTag, body: BodyId, open: &[FaceId], thickness: f64) -> KernelResult<OpResult> {
        self.0.shell(tag, body, open, thickness)
    }
    fn tessellate(&self, body: BodyId, params: &TessellationParams) -> KernelResult<Mesh> {
        self.0.tessellate(body, params)
    }
    fn tessellate_edges(&self, body: BodyId, params: &TessellationParams) -> KernelResult<Vec<EdgePolyline>> {
        self.0.tessellate_edges(body, params)
    }
    fn mass_properties(&self, body: BodyId) -> KernelResult<MassProperties> {
        self.0.mass_properties(body)
    }
    fn validate(&self, body: BodyId) -> KernelResult<Validation> {
        self.0.validate(body)
    }
    fn topology(&self, body: BodyId) -> KernelResult<Topology> {
        self.0.topology(body)
    }
    fn history(&self, body: BodyId) -> KernelResult<Vec<HistoryRecord>> {
        self.0.history(body)
    }
    fn release(&self, body: BodyId) -> KernelResult<()> {
        self.0.release(body)
    }
}

#[test]
fn crash_child() {
    let Some(root) = std::env::var_os("SOLIDDRAFT_JOURNAL_CRASH_CHILD") else { return };
    let root = Path::new(&root);
    let session = Session::start(&root.join("journals")).unwrap();
    std::fs::write(root.join("journal-path"), session.path().to_str().unwrap()).unwrap();
    let mut doc = Document::new("Crash scenario", Arc::new(Crashing::default()));
    let params = ExtrudeParams { depth: 10.0, ..Default::default() };
    let feature = doc.apply(Command::AddExtrude { params, via_assistant: false }).unwrap().unwrap();
    doc.apply(Command::Rename { id: feature, name: "Plate".into() }).unwrap();
    doc.undo().unwrap();
    doc.redo().unwrap();
    let mut book = ProposalBook::default();
    let refusal = CommandTools { doc: &mut doc, book: &mut book, surface: Surface::Mcp, drawing: None }.call(
        "delete_feature",
        json!({"feature":"Missing"}),
        &Caller::Mcp { client: "crash test".into() },
    );
    assert!(refusal.is_error);
    let mut drawing = DrawingDoc::new(soliddraft_drawing::fixture::bracket_a3());
    drawing
        .apply(
            soliddraft_drawing::Command::MoveView { view: soliddraft_drawing::ViewId(1), to: Point2::new(70.0, 80.0) },
            None,
        )
        .unwrap();
    drawing.undo().unwrap();
    drawing.redo().unwrap();
    // The test observes the actual state after the failing edit has mutated the
    // model, then exits without invoking the application's clean-session close.
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        doc.apply(Command::EditExtrude { id: feature, params: ExtrudeParams { depth: 13.0, ..params } })
    }));
    assert!(failed.is_err(), "the scenario must reach its injected kernel panic");
    std::fs::write(
        root.join("expected.json"),
        serde_json::to_vec(&json!({
            "part":doc.model().truth(), "drawing":drawing.drawing, "refusal":refusal.content,
            "panic":"deliberate journal crash at depth 13"
        }))
        .unwrap(),
    )
    .unwrap();
    std::process::exit(101);
}

#[test]
fn a_crashed_session_replays_the_document_refusal_and_failure() {
    let root =
        std::env::temp_dir().join(format!("soliddraft-journal-crash-{}", soliddraft_document::DocumentId::new()));
    std::fs::create_dir_all(&root).unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_child", "--nocapture"])
        .env("SOLIDDRAFT_JOURNAL_CRASH_CHILD", &root)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(101));
    let expected: Value = serde_json::from_slice(&std::fs::read(root.join("expected.json")).unwrap()).unwrap();
    let path = std::fs::read_to_string(root.join("journal-path")).unwrap();
    assert_eq!(journal::abandoned(&root.join("journals")), [Path::new(&path)]);
    let replay = Replay::read(Path::new(&path), Arc::new(Crashing::default())).unwrap();
    let report = replay.report();
    assert_eq!(report["parts"][0]["part"], expected["part"]);
    assert_eq!(report["drawings"][0]["drawing"], expected["drawing"]);
    assert_eq!(report["outcomes"][4]["error"], expected["refusal"]);
    assert_eq!(report["outcomes"][8]["panic"], expected["panic"]);
    assert_eq!(report["interrupted"], 9);
    assert_eq!(report["failure"], "command 9 panicked: deliberate journal crash at depth 13");
    std::fs::remove_dir_all(root).unwrap();
}
