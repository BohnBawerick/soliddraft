//! V2 U1-U8, from the independent test-designer report.

use serde_json::{Value, json};
use soliddraft_kernel_api::*;
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::{
    tracked::{Check, Source},
    *,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn block(kernel: Arc<dyn Kernel>) -> (Document, FeatureId) {
    let mut d = Document::new("Measured block", kernel);
    let mut sketch = sketch::Sketch::new();
    sketch.rectangle(sketch::P2::new(0.0, 0.0), 40.0, 30.0).unwrap();
    sketch::solve::solve(&mut sketch, None, true);
    let s = d
        .execute(Command::AddFeature {
            kind: FeatureKind::Sketch(SketchFeature {
                plane: SketchPlane::Datum { datum: DatumPlane::Top, offset: 0.0 },
                sketch,
            }),
            name: None,
            position: None,
            via_assistant: false,
        })
        .unwrap()
        .created
        .unwrap();
    let feature = d
        .execute(Command::AddFeature {
            kind: FeatureKind::Extrude(ExtrudeFeature {
                profile: ProfileSource::Sketch { feature: s },
                operation: ExtrudeOp::Boss,
                end: EndCondition::Blind { depth: 10.0 },
                reverse: false,
            }),
            name: None,
            position: None,
            via_assistant: false,
        })
        .unwrap()
        .created
        .unwrap();
    (d, feature)
}

fn source(d: &Document) -> Source {
    let t = d.kernel().topology(d.body().unwrap()).unwrap();
    let edge = t.edges.iter().find(|e| (e.bbox.max.z - e.bbox.min.z - 10.0).abs() < 1e-5).unwrap();
    Source::Edge { reference: d.edge_ref(edge.id).unwrap() }
}
fn track(d: &mut Document) {
    d.execute(Command::Track { name: "block_edge".into(), source: source(d), check: Some("< 12 mm".into()) }).unwrap();
}
fn row(d: &Document) -> Value {
    d.values_summary().as_array().unwrap().iter().find(|r| r["name"] == "block_edge").unwrap().clone()
}
fn edit(d: &mut Document, id: FeatureId, depth: f64) {
    let mut kind = d.feature(id).unwrap().kind.clone();
    let FeatureKind::Extrude(e) = &mut kind else { panic!() };
    e.end = EndCondition::Blind { depth };
    d.execute(Command::EditFeature { id, kind }).unwrap();
}
fn refused(d: &mut Document, command: Command, message: &str) {
    let hash = d.semantic_hash();
    let undo = d.undo_depth();
    let error = d.execute(command).unwrap_err().to_string();
    assert!(error.contains(message), "{error}");
    assert_eq!(d.semantic_hash(), hash);
    assert_eq!(d.undo_depth(), undo);
}

#[test]
fn checks_answer_only_what_the_bound_proves() {
    let exact = Measurement { value: 10.0, lo: 10.0, hi: 10.0, certainty: Certainty::Exact };
    let bounded = Measurement { value: 10.0, lo: 9.5, hi: 10.5, certainty: Certainty::Bounded };
    for (m, text, expected) in [
        (exact, "< 12", "pass"),
        (exact, "< 10", "fail"),
        (exact, "<= 10", "pass"),
        (exact, "> 10", "fail"),
        (exact, ">= 10", "pass"),
        (bounded, "< 11", "pass"),
        (bounded, "< 9", "fail"),
        (bounded, "< 10", "unknown"),
        (bounded, "> 10", "unknown"),
        (bounded, ">= 9", "pass"),
    ] {
        let check = Check::parse(text, "mm").unwrap();
        assert_eq!(check.state(Some(&m)), expected, "{text}");
        assert_eq!(check.state(None), "unknown");
    }
}

#[test]
fn bad_track_requests_change_nothing() {
    let (mut d, _) = block(Arc::new(CatallaxKernel::new()));
    d.execute(Command::SetValue { name: "taken".into(), expression: "10".into() }).unwrap();
    for (name, check, message) in [
        ("taken", None, "taken"),
        ("height", Some("< 30 deg"), "length"),
        ("height", Some("< 12 xx"), "xx"),
        ("1a", None, "1a"),
    ] {
        let command = Command::Track { name: name.into(), source: source(&d), check: check.map(str::to_owned) };
        refused(&mut d, command, message);
    }
}

#[test]
fn tracked_values_drive_nothing() {
    let (mut d, _) = block(Arc::new(CatallaxKernel::new()));
    track(&mut d);
    for (name, expression) in [("block_edge", "5"), ("twice", "block_edge * 2")] {
        refused(&mut d, Command::SetValue { name: name.into(), expression: expression.into() }, "tracked");
    }
}

#[test]
fn measuring_changes_neither_hash() {
    let (mut d, id) = block(Arc::new(CatallaxKernel::new()));
    let geometry = d.geometry_hash();
    let revision = d.semantic_hash();
    track(&mut d);
    assert_eq!(d.geometry_hash(), geometry);
    assert_ne!(d.semantic_hash(), revision);
    assert_eq!(row(&d)["tracked"]["geometry"], json!(d.geometry_hash()));
    edit(&mut d, id, 15.0);
    assert_eq!(row(&d)["tracked"]["geometry"], json!(d.geometry_hash()));
    let hashes = (d.semantic_hash(), d.geometry_hash());
    d.rebuild();
    assert_eq!((d.semantic_hash(), d.geometry_hash()), hashes);
    assert_eq!(row(&d)["tracked"]["geometry"], json!(d.geometry_hash()));
}

#[test]
fn a_part_saved_with_a_stale_row_reopens_stale() {
    let kernel = Arc::new(CatallaxKernel::new());
    let (mut d, id) = block(kernel.clone());
    track(&mut d);
    d.execute(Command::SetSuppressed { id, suppressed: true }).unwrap();
    let bytes = d.to_bytes().unwrap();
    let mut reopened = Document::from_bytes(&bytes, kernel).unwrap();
    assert_eq!(row(&reopened)["tracked"]["state"], "stale");
    assert!(row(&reopened)["value"].is_null());
    assert_eq!(row(&reopened)["tracked"]["check"]["state"], "unknown");
    reopened.execute(Command::SetSuppressed { id, suppressed: false }).unwrap();
    assert_eq!(row(&reopened)["tracked"]["state"], "current");
    assert_eq!(row(&reopened)["value"], 10.0);
}

#[test]
fn a_refused_proof_marks_the_row_stale_and_keeps_the_edit() {
    let kernel = Arc::new(RefusingKernel(CatallaxKernel::new(), AtomicBool::new(false)));
    let (mut d, id) = block(kernel.clone());
    track(&mut d);
    kernel.1.store(true, Ordering::SeqCst);
    edit(&mut d, id, 15.0);
    assert_eq!(d.kernel().mass_properties(d.body().unwrap()).unwrap().volume / (40.0 * 30.0), 15.0);
    let r = row(&d);
    assert_eq!(r["tracked"]["state"], "stale");
    assert_eq!(r["value"], 10.0);
    assert!(r["tracked"]["reason"].as_str().unwrap().contains("designed refusal"));
    assert_eq!(r["tracked"]["check"]["state"], "unknown");
    kernel.1.store(false, Ordering::SeqCst);
    d.rebuild();
    assert_eq!(row(&d)["tracked"]["state"], "current");
    assert_eq!(row(&d)["value"], 15.0);
}

#[test]
fn a_file_with_values_and_no_rows_keeps_its_hashes() {
    let bytes = include_bytes!("fixtures/bracket-inch.dwprt");
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let manifest: Value = serde_json::from_reader(zip.by_name("manifest.json").unwrap()).unwrap();
    let d = Document::from_bytes(bytes, Arc::new(CatallaxKernel::new())).unwrap();
    assert!(d.tracked().is_empty());
    assert!(!d.values().is_empty());
    assert_eq!(json!(d.semantic_hash()), manifest["semantic_hash"]);
    assert_eq!(json!(d.geometry_hash()), manifest["geometry_hash"]);
    let saved = d.to_bytes().unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(saved)).unwrap();
    let saved: Value = serde_json::from_reader(zip.by_name("manifest.json").unwrap()).unwrap();
    assert_eq!(saved["semantic_hash"], manifest["semantic_hash"]);
    assert_eq!(saved["geometry_hash"], manifest["geometry_hash"]);
}

#[test]
fn rename_value_renames_a_tracked_row() {
    let (mut d, _) = block(Arc::new(CatallaxKernel::new()));
    track(&mut d);
    d.execute(Command::SetValue { name: "taken".into(), expression: "10".into() }).unwrap();
    let before = d.tracked()[0].clone();
    d.execute(Command::RenameValue { name: "block_edge".into(), to: "edge_h".into() }).unwrap();
    let mut expected = before;
    expected.name = "edge_h".into();
    assert_eq!(d.tracked()[0], expected);
    let values = d.values_summary();
    let r = &values[1];
    assert_eq!(r["name"], "edge_h");
    assert_eq!(r["value"], 10.0);
    assert_eq!(r["tracked"]["check"]["state"], "pass");
    refused(&mut d, Command::RenameValue { name: "edge_h".into(), to: "taken".into() }, "taken");
}
struct RefusingKernel(CatallaxKernel, AtomicBool);
impl Kernel for RefusingKernel {
    fn backend(&self) -> BackendInfo {
        self.0.backend()
    }
    fn create_body_from_profile(&self, tag: &OpTag, profile: &Profile) -> KernelResult<OpResult> {
        self.0.create_body_from_profile(tag, profile)
    }
    fn extrude(&self, tag: &OpTag, profile: BodyId, params: &Extrude) -> KernelResult<OpResult> {
        self.0.extrude(tag, profile, params)
    }
    fn revolve(&self, tag: &OpTag, profile: BodyId, params: &Revolve) -> KernelResult<OpResult> {
        self.0.revolve(tag, profile, params)
    }
    fn boolean(&self, tag: &OpTag, op: BooleanOp, target: BodyId, tool: BodyId) -> KernelResult<OpResult> {
        self.0.boolean(tag, op, target, tool)
    }
    fn fillet(&self, tag: &OpTag, body: BodyId, edges: &[EdgeId], radius: f64) -> KernelResult<OpResult> {
        self.0.fillet(tag, body, edges, radius)
    }
    fn chamfer(&self, tag: &OpTag, body: BodyId, edges: &[EdgeId], distance: f64) -> KernelResult<OpResult> {
        self.0.chamfer(tag, body, edges, distance)
    }
    fn shell(&self, tag: &OpTag, body: BodyId, open_faces: &[FaceId], thickness: f64) -> KernelResult<OpResult> {
        self.0.shell(tag, body, open_faces, thickness)
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
    fn edge_length(&self, body: BodyId, edge: EdgeId) -> KernelResult<Measurement> {
        if self.1.load(Ordering::SeqCst) {
            return Err(KernelError::Unsupported("designed refusal".into()));
        }
        self.0.edge_length(body, edge)
    }
}
