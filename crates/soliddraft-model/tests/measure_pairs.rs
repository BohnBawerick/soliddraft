//! V2 batch 2 pair rows and angle limits. The designed U1 and U3 tests run the real
//! Catallax query on the batch 1 part. The stand-in kernel below answers pairs from
//! display boxes, so the tests that use it prove row policy only.

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

/// The batch 1 part: the block with a radius 6 bore about Z through it.
fn holed(kernel: Arc<dyn Kernel>) -> (Document, FeatureId) {
    let (mut d, e) = block(kernel);
    let mut sketch = sketch::Sketch::new();
    sketch.circle_at(sketch::P2::new(0.0, 0.0), 12.0).unwrap();
    sketch::solve::solve(&mut sketch, None, true);
    let plane = SketchPlane::Datum { datum: DatumPlane::Top, offset: 0.0 };
    let s = add(&mut d, FeatureKind::Sketch(SketchFeature { plane, sketch }));
    let cut = add(
        &mut d,
        FeatureKind::Extrude(ExtrudeFeature {
            profile: ProfileSource::Sketch { feature: s },
            operation: ExtrudeOp::Cut,
            end: EndCondition::ThroughAll,
            reverse: false,
        }),
    );
    assert!(d.status(cut).unwrap().is_ok(), "{:?}", d.status(cut));
    let volume = d.kernel().mass_properties(d.body().unwrap()).unwrap().volume;
    assert!((volume - (12000.0 - 360.0 * std::f64::consts::PI)).abs() < 1e-3, "the bore goes through: {volume}");
    (d, e)
}

/// E2: the top edge along y at x = 20.
fn right_top(d: &Document) -> EdgeRef {
    let t = d.kernel().topology(d.body().unwrap()).unwrap();
    let top = t.bbox.max.z;
    let e = t
        .edges
        .iter()
        .find(|e| {
            e.bbox.max.y - e.bbox.min.y > 29.0
                && (e.bbox.min.x - 20.0).abs() < 0.01
                && (e.bbox.max.x - 20.0).abs() < 0.01
                && (e.bbox.min.z - top).abs() < 0.01
        })
        .unwrap();
    d.edge_ref(e.id).unwrap()
}

/// F0 and F1: the holed caps.
fn caps(d: &Document) -> Source {
    let t = d.kernel().topology(d.body().unwrap()).unwrap();
    let cap = |z: f64| {
        let f = t.faces.iter().find(|f| f.surface == SurfaceKind::Plane && (f.centroid.z - z).abs() < 1e-3).unwrap();
        Box::new(Endpoint::Face { reference: d.face_ref(f.id).unwrap() })
    };
    let top = t.faces.iter().map(|f| f.centroid.z).fold(0.0, f64::max);
    Source::Pair { quantity: PairQuantity::MinimumDistance, a: cap(0.0), b: cap(top) }
}

/// An outward enclosure of sqrt(`n`): `mul_add` rounds once, so the signs are exact.
fn root(r: &Value, n: f64) {
    let (lo, hi) = (r["tracked"]["lo"].as_f64().unwrap(), r["tracked"]["hi"].as_f64().unwrap());
    assert!(lo.mul_add(lo, -n) <= 0.0 && hi.mul_add(hi, -n) >= 0.0 && lo < hi && hi - lo <= 1e-8, "sqrt({n}): {r}");
    assert!((r["value"].as_f64().unwrap() - n.sqrt()).abs() <= 1e-8, "{r}");
    assert_eq!(r["tracked"]["certainty"], "bounded");
}

fn about(r: &Value, v: f64) {
    let (lo, hi) = (r["tracked"]["lo"].as_f64().unwrap(), r["tracked"]["hi"].as_f64().unwrap());
    assert!(lo <= v && v <= hi && hi - lo <= 1e-8, "{v}: {r}");
    assert!(["exact", "bounded"].contains(&r["tracked"]["certainty"].as_str().unwrap()), "{r}");
}

fn current(d: &Document, r: &Value, check: &str) {
    assert_eq!(r["tracked"]["state"], "current", "{r}");
    assert_eq!(r["tracked"]["check"]["state"], check, "{r}");
    assert_eq!(r["tracked"]["geometry"], serde_json::json!(d.geometry_hash()), "a current row is on this geometry");
}

/// V2 batch 2 U1.
#[test]
fn pair_definitions_follow_both_refs_and_quantity() {
    let kernel: Arc<dyn Kernel> = Arc::new(CatallaxKernel::new());
    let (mut d, id) = holed(kernel.clone());
    let geometry = d.geometry_hash();
    for (name, source, check) in [("diagonal_gap", diagonal(&d), "< 32 mm"), ("cap_gap", caps(&d), "< 12 mm")] {
        d.execute(Command::Track { name: name.into(), source, check: Some(check.into()) }).unwrap();
    }
    assert_eq!(d.geometry_hash(), geometry, "tracking builds nothing");
    let (diag, cap) = (row(&d, "diagonal_gap"), row(&d, "cap_gap"));
    root(&diag, 1000.0);
    about(&cap, 10.0);
    current(&d, &diag, "pass");
    current(&d, &cap, "pass");

    depth(&mut d, id, 15.0);
    let (diag, cap) = (row(&d, "diagonal_gap"), row(&d, "cap_gap"));
    root(&diag, 1125.0);
    about(&cap, 15.0);
    current(&d, &diag, "fail");
    current(&d, &cap, "fail");
    let definitions = d.tracked().to_vec();

    let e0 = edge(&d, -15.0, false);
    fillet(&mut d, e0.clone());
    let diag = row(&d, "diagonal_gap");
    assert_eq!(diag["tracked"]["state"], "stale");
    root(&diag, 1125.0);
    assert_eq!(diag["tracked"]["check"]["state"], "unknown");
    let reason = diag["tracked"]["reason"].as_str().unwrap();
    assert!(reason.contains("measure.unresolved-reference") && reason.contains(&e0.label), "{reason}");
    let cap = row(&d, "cap_gap");
    about(&cap, 15.0);
    current(&d, &cap, "fail");
    d.undo().unwrap();
    root(&row(&d, "diagonal_gap"), 1125.0);
    current(&d, &row(&d, "diagonal_gap"), "fail");
    about(&row(&d, "cap_gap"), 15.0);
    assert_eq!(d.tracked(), definitions);

    // The saved recipes are the typed references, and they prove again after reopening.
    let bytes = d.to_bytes().unwrap();
    let reopened = Document::from_bytes(&bytes, kernel.clone()).unwrap();
    assert_eq!(reopened.tracked(), definitions);
    assert_eq!(reopened.semantic_hash(), d.semantic_hash(), "reopening changes nothing");
    root(&row(&reopened, "diagonal_gap"), 1125.0);
    current(&reopened, &row(&reopened, "diagonal_gap"), "fail");

    // Removing the other end alone also makes the row stale.
    let (mut other, _) = holed(kernel.clone());
    let source = diagonal(&other);
    other.execute(Command::Track { name: "diagonal_gap".into(), source, check: None }).unwrap();
    let e1 = edge(&other, 15.0, true);
    fillet(&mut other, e1.clone());
    let diag = row(&other, "diagonal_gap");
    assert_eq!(diag["tracked"]["state"], "stale");
    assert!(diag["tracked"]["reason"].as_str().unwrap().contains(&e1.label), "{diag}");

    // One pair of edges, three quantities: none reuses another's number.
    let (plain, _) = holed(kernel);
    let (poisoned, _) = holed(Arc::new(Poisoned(CatallaxKernel::new())));
    for d in [&plain, &poisoned] {
        let (a, b) = (edge(d, -15.0, false), right_top(d));
        let pair = |quantity, point: bool| {
            let end = |reference: &EdgeRef| {
                Box::new(if point {
                    Endpoint::EdgeMidpoint { reference: reference.clone() }
                } else {
                    Endpoint::Edge { reference: reference.clone() }
                })
            };
            d.measure_source(&Source::Pair { quantity, a: end(&a), b: end(&b) }).unwrap()
        };
        let minimum = pair(PairQuantity::MinimumDistance, false);
        let points = pair(PairQuantity::PointDistance, true);
        let angle = pair(PairQuantity::DirectionAngle, false);
        assert!(minimum.lo <= 10.0 && 10.0 <= minimum.hi && minimum.hi - minimum.lo <= 1e-8, "{minimum:?}");
        assert!(points.lo.mul_add(points.lo, -725.0) <= 0.0 && points.hi.mul_add(points.hi, -725.0) >= 0.0);
        assert_eq!((angle.value, angle.certainty), (90.0, Certainty::Exact));
        let m = d.measure_midpoint(&a).unwrap();
        assert_eq!(m.coordinates.map(|c| c.value), [0.0, -15.0, 0.0], "the proof, not the display");
    }
    let shown = |d: &Document| d.kernel().topology(d.body().unwrap()).unwrap().edges[0].midpoint.x;
    assert_eq!(shown(&poisoned) - shown(&plain), 5.0, "the display really was poisoned");
}

/// V2 batch 2 U3.
#[test]
fn new_row_kinds_keep_units_and_old_truth() {
    let (mut d, _) = holed(Arc::new(CatallaxKernel::new()));
    let angle = |d: &Document| Source::Pair {
        quantity: PairQuantity::DirectionAngle,
        a: Box::new(Endpoint::Edge { reference: edge(d, -15.0, false) }),
        b: Box::new(Endpoint::Edge { reference: right_top(d) }),
    };
    for (name, check, expected) in
        [("square", ">= 89 deg", "pass"), ("under", "< 90 deg", "fail"), ("bare", "< 90.5", "pass")]
    {
        let source = angle(&d);
        d.execute(Command::Track { name: name.into(), source, check: Some(check.into()) }).unwrap();
        let r = row(&d, name);
        assert_eq!((r["unit"].as_str(), r["value"].as_f64()), (Some("deg"), Some(90.0)), "{r}");
        assert_eq!(r["tracked"]["certainty"], "exact");
        assert_eq!(r["tracked"]["check"]["state"], expected, "{check}");
    }
    let source = angle(&d);
    refused(&mut d, Command::Track { name: "mixed".into(), source, check: Some("< 12 mm".into()) }, "angle");
    let source = diagonal(&d);
    refused(&mut d, Command::Track { name: "mixed".into(), source, check: Some("< 12 deg".into()) }, "length");
    for expression in ["square * 2", "square"] {
        refused(&mut d, Command::SetValue { name: "driven".into(), expression: expression.into() }, "tracked");
    }

    // A batch 1 file, saved by the merged batch 1 scenario, opens and saves unchanged.
    let bytes = include_bytes!("fixtures/measure-batch1.dwprt");
    let read = |bytes: &[u8]| {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
        let manifest: Value = serde_json::from_reader(zip.by_name("manifest.json").unwrap()).unwrap();
        let part: Value = serde_json::from_reader(zip.by_name("part.json").unwrap()).unwrap();
        (manifest, part)
    };
    let (manifest, part) = read(bytes);
    let opened = Document::from_bytes(bytes, Arc::new(CatallaxKernel::new())).unwrap();
    assert_eq!(opened.tracked().len(), 2);
    assert!(opened.tracked().iter().all(|r| matches!(r.source, Source::Edge { .. } | Source::Face { .. })));
    assert_eq!(serde_json::json!(opened.semantic_hash()), manifest["semantic_hash"]);
    assert_eq!(serde_json::json!(opened.geometry_hash()), manifest["geometry_hash"]);
    let (resaved, repart) = read(&opened.to_bytes().unwrap());
    assert_eq!(resaved["semantic_hash"], manifest["semantic_hash"]);
    assert_eq!(resaved["geometry_hash"], manifest["geometry_hash"]);
    assert_eq!(repart["values"], part["values"], "rows, ids and definitions survive");
    assert_eq!(repart["features"], part["features"]);
}

/// Catallax with every display midpoint and centroid moved 5 mm along x. Proofs are
/// forwarded untouched, so a proven value that moved would have come from the display.
struct Poisoned(CatallaxKernel);

impl Poisoned {
    fn shift(mut t: Topology) -> Topology {
        let by = Vec3::new(5.0, 0.0, 0.0);
        for e in &mut t.edges {
            e.midpoint = e.midpoint + by;
        }
        for f in &mut t.faces {
            f.centroid = f.centroid + by;
        }
        t
    }
}

impl Kernel for Poisoned {
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
        self.0.topology(body).map(Self::shift)
    }
    fn measured_topology(&self, body: BodyId) -> KernelResult<Topology> {
        self.0.measured_topology(body).map(Self::shift)
    }
    fn history(&self, body: BodyId) -> KernelResult<Vec<HistoryRecord>> {
        self.0.history(body)
    }
    fn release(&self, body: BodyId) -> KernelResult<()> {
        self.0.release(body)
    }
    fn edge_midpoint(&self, body: BodyId, edge: EdgeId) -> KernelResult<PointProof> {
        self.0.edge_midpoint(body, edge)
    }
    fn measure_pair(
        &self,
        body: BodyId,
        a: PairEndpoint,
        b: PairEndpoint,
        quantity: PairQuantity,
    ) -> KernelResult<Measurement> {
        self.0.measure_pair(body, a, b, quantity)
    }
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
