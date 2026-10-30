//! V2 batch 2 pair rows and angle limits, ahead of the native pair query. The stand-in
//! kernel answers pairs from display boxes, so these tests prove row policy only; the
//! designed U1-U3 tests prove values on the real query.

use serde_json::Value;
use soliddraft_kernel_api::*;
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::{
    tracked::{Check, Endpoint, Source},
    *,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn add(d: &mut Document, kind: FeatureKind) -> FeatureId {
    d.execute(Command::AddFeature { kind, name: None, position: None, via_assistant: false }).unwrap().created.unwrap()
}

/// The batch 1 block without its bore: [-20,20] x [-15,15] x [0,10].
fn block(kernel: Arc<dyn Kernel>) -> (Document, FeatureId) {
    let mut d = Document::new("Paired block", kernel);
    let mut sketch = sketch::Sketch::new();
    sketch.rectangle(sketch::P2::new(0.0, 0.0), 40.0, 30.0).unwrap();
    sketch::solve::solve(&mut sketch, None, true);
    let plane = SketchPlane::Datum { datum: DatumPlane::Top, offset: 0.0 };
    let s = add(&mut d, FeatureKind::Sketch(SketchFeature { plane, sketch }));
    let e = add(
        &mut d,
        FeatureKind::Extrude(ExtrudeFeature {
            profile: ProfileSource::Sketch { feature: s },
            operation: ExtrudeOp::Boss,
            end: EndCondition::Blind { depth: 10.0 },
            reverse: false,
        }),
    );
    (d, e)
}

/// The x-running edge at `y` on the bottom (`top` false) or top cap.
fn edge(d: &Document, y: f64, top: bool) -> EdgeRef {
    let t = d.kernel().topology(d.body().unwrap()).unwrap();
    let z = if top { t.bbox.max.z } else { 0.0 };
    let e = t
        .edges
        .iter()
        .find(|e| {
            e.bbox.max.x - e.bbox.min.x > 39.0
                && (e.bbox.min.y - y).abs() < 0.01
                && (e.bbox.max.y - y).abs() < 0.01
                && (e.bbox.min.z - z).abs() < 0.01
        })
        .unwrap();
    d.edge_ref(e.id).unwrap()
}

/// E0 and E1 of the designer's aliases: bottom front and top back.
fn diagonal(d: &Document) -> Source {
    Source::Pair {
        quantity: PairQuantity::MinimumDistance,
        a: Box::new(Endpoint::Edge { reference: edge(d, -15.0, false) }),
        b: Box::new(Endpoint::Edge { reference: edge(d, 15.0, true) }),
    }
}

fn row(d: &Document, name: &str) -> Value {
    d.values_summary().as_array().unwrap().iter().find(|r| r["name"] == name).unwrap().clone()
}

fn refused(d: &mut Document, command: Command, message: &str) {
    let hash = d.semantic_hash();
    let undo = d.undo_depth();
    let error = d.execute(command).unwrap_err().to_string();
    assert!(error.contains(message), "{error}");
    assert_eq!(d.semantic_hash(), hash);
    assert_eq!(d.undo_depth(), undo);
}

fn fillet(d: &mut Document, reference: EdgeRef) -> FeatureId {
    add(d, FeatureKind::Fillet(BlendFeature { edges: vec![reference], size: 2.0 }))
}

fn depth(d: &mut Document, id: FeatureId, depth: f64) {
    let mut kind = d.feature(id).unwrap().kind.clone();
    let FeatureKind::Extrude(e) = &mut kind else { panic!() };
    e.end = EndCondition::Blind { depth };
    d.execute(Command::EditFeature { id, kind }).unwrap();
}

#[test]
fn angle_limits_use_the_shared_parser() {
    let right = Measurement { value: 90.0, lo: 90.0, hi: 90.0, certainty: Certainty::Exact };
    for (text, expected) in [(">= 89 deg", "pass"), ("< 90 deg", "fail"), ("<= 90", "pass"), ("> 89.5 deg", "pass")] {
        assert_eq!(Check::parse(text, "deg").unwrap().state(Some(&right)), expected, "{text}");
    }
    // A bare limit takes the row's unit: 90 means 90 deg.
    assert_eq!(Check::parse("< 90", "deg").unwrap().state(Some(&right)), "fail");
    assert_eq!(Check::parse("< 90.5", "deg").unwrap().state(Some(&right)), "pass");
    assert!(Check::parse("< 12 mm", "deg").unwrap_err().contains("angle"));
    assert!(Check::parse("< 12 deg", "mm").unwrap_err().contains("length"));
    assert!(Check::parse("< 12 deg", "mm2").unwrap_err().contains("area"));
}

#[test]
fn pair_points_must_suit_the_quantity() {
    let (mut d, _) = block(Arc::new(Pairs::default()));
    let (e0, e1) = (edge(&d, -15.0, false), edge(&d, 15.0, true));
    let whole = Endpoint::Edge { reference: e0.clone() };
    let point = Endpoint::EdgeMidpoint { reference: e1.clone() };
    for (quantity, a, b, message) in [
        (PairQuantity::PointDistance, whole.clone(), point.clone(), "midpoint on both"),
        (PairQuantity::MinimumDistance, point.clone(), whole.clone(), "takes no midpoint"),
        (PairQuantity::DirectionAngle, whole.clone(), point.clone(), "takes no midpoint"),
    ] {
        let source = Source::Pair { quantity, a: Box::new(a), b: Box::new(b) };
        refused(&mut d, Command::Track { name: "gap".into(), source, check: None }, message);
    }
    let source = Source::Pair { quantity: PairQuantity::PointDistance, a: Box::new(point.clone()), b: Box::new(point) };
    d.execute(Command::Track { name: "gap".into(), source, check: None }).unwrap();
    let r = row(&d, "gap");
    assert_eq!(r["tracked"]["quantity"], "point_distance");
    assert_eq!(r["tracked"]["sources"][0]["point"], "midpoint");
}

#[test]
fn pair_rows_follow_both_references() {
    let kernel = Arc::new(Pairs::default());
    let (mut d, id) = block(kernel.clone());
    let geometry = d.geometry_hash();
    let undo = d.undo_depth();
    d.execute(Command::Track { name: "diagonal_gap".into(), source: diagonal(&d), check: Some("< 32 mm".into()) })
        .unwrap();
    assert_eq!(d.undo_depth(), undo + 1);
    assert_eq!(d.geometry_hash(), geometry);
    let r = row(&d, "diagonal_gap");
    assert_eq!(r["unit"], "mm");
    assert_eq!(r["tracked"]["quantity"], "minimum_distance");
    assert_eq!(r["tracked"]["state"], "current");
    assert_eq!(r["tracked"]["check"]["state"], "pass");
    assert_eq!(r["tracked"]["sources"].as_array().unwrap().len(), 2);
    assert!(r["tracked"]["sources"][1]["point"].is_null());
    assert!((r["value"].as_f64().unwrap() - 1000f64.sqrt()).abs() < 1e-9);

    depth(&mut d, id, 15.0);
    let r = row(&d, "diagonal_gap");
    assert!((r["value"].as_f64().unwrap() - 1125f64.sqrt()).abs() < 1e-9);
    assert_eq!(r["tracked"]["check"]["state"], "fail");
    let definition = d.tracked()[0].clone();

    // Rounding either end removes the edge the row names; no nearby edge stands in.
    let e0 = edge(&d, -15.0, false);
    fillet(&mut d, e0.clone());
    let r = row(&d, "diagonal_gap");
    assert_eq!(r["tracked"]["state"], "stale");
    assert!((r["value"].as_f64().unwrap() - 1125f64.sqrt()).abs() < 1e-9);
    assert!(r["tracked"]["reason"].as_str().unwrap().contains(&e0.label));
    assert_eq!(r["tracked"]["check"]["state"], "unknown");
    d.undo().unwrap();
    assert_eq!(row(&d, "diagonal_gap")["tracked"]["state"], "current");
    assert_eq!(d.tracked()[0], definition);

    let e1 = edge(&d, 15.0, true);
    fillet(&mut d, e1.clone());
    let r = row(&d, "diagonal_gap");
    assert_eq!(r["tracked"]["state"], "stale");
    assert!(r["tracked"]["reason"].as_str().unwrap().contains(&e1.label));

    let reopened = Document::from_bytes(&d.to_bytes().unwrap(), kernel).unwrap();
    assert_eq!(reopened.tracked()[0], definition);
    let r = row(&reopened, "diagonal_gap");
    assert_eq!(r["tracked"]["state"], "stale");
    assert!(r["value"].is_null());
    assert!(r["tracked"]["geometry"].is_null());
}

#[test]
fn a_refused_pair_changes_nothing_and_never_reads_zero() {
    let kernel = Arc::new(Pairs::default());
    let (mut d, id) = block(kernel.clone());
    kernel.refuse.store(true, Ordering::SeqCst);
    let source = diagonal(&d);
    refused(&mut d, Command::Track { name: "gap".into(), source, check: None }, "designed refusal");
    kernel.refuse.store(false, Ordering::SeqCst);
    d.execute(Command::Track { name: "gap".into(), source: diagonal(&d), check: Some("< 32 mm".into()) }).unwrap();
    kernel.refuse.store(true, Ordering::SeqCst);
    depth(&mut d, id, 15.0);
    let r = row(&d, "gap");
    assert_eq!(r["tracked"]["state"], "stale");
    assert!((r["value"].as_f64().unwrap() - 1000f64.sqrt()).abs() < 1e-9);
    assert_eq!(r["tracked"]["check"]["state"], "unknown");
    assert!(r["tracked"]["reason"].as_str().unwrap().contains("designed refusal"));
}

/// Catallax, answering pair requests from the distance between display box centres.
/// For two straight edges of the block that is their midpoint distance, which equals
/// E0/E1's minimum distance. It stands in for the native query and proves nothing.
#[derive(Default)]
struct Pairs {
    inner: CatallaxKernel,
    refuse: AtomicBool,
}

impl Kernel for Pairs {
    fn backend(&self) -> BackendInfo {
        self.inner.backend()
    }
    fn create_body_from_profile(&self, tag: &OpTag, profile: &Profile) -> KernelResult<OpResult> {
        self.inner.create_body_from_profile(tag, profile)
    }
    fn extrude(&self, tag: &OpTag, profile: BodyId, params: &Extrude) -> KernelResult<OpResult> {
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
    fn shell(&self, tag: &OpTag, body: BodyId, open_faces: &[FaceId], thickness: f64) -> KernelResult<OpResult> {
        self.inner.shell(tag, body, open_faces, thickness)
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
    fn measure_pair(
        &self,
        body: BodyId,
        a: PairEndpoint,
        b: PairEndpoint,
        _quantity: PairQuantity,
    ) -> KernelResult<Measurement> {
        if self.refuse.load(Ordering::SeqCst) {
            return Err(KernelError::Unsupported("designed refusal".into()));
        }
        let t = self.inner.topology(body)?;
        let centre = |e: PairEndpoint| {
            match e {
                PairEndpoint::Edge(id) | PairEndpoint::EdgeMidpoint(id) => t.edge(id).map(|e| e.bbox),
                PairEndpoint::Face(id) => t.face(id).map(|f| f.bbox),
            }
            .map(|b| (b.min + b.max) * 0.5)
            .ok_or_else(|| KernelError::Unsupported("unknown entity".into()))
        };
        let d = (centre(a)? - centre(b)?).length();
        Ok(Measurement { value: d, lo: d, hi: d, certainty: Certainty::Bounded })
    }
}
