//! The imported-body feature: source bytes and decisions in the model, save and reopen,
//! and how the backend's answer becomes the part's body or a refusal.

use std::sync::{Arc, Mutex};

use soliddraft_kernel_api::{
    Approval, BackendInfo, BodyId, BooleanOp, Decision, Draft, EdgeId, EdgePolyline, Extrude, FaceId, HistoryRecord,
    Hole, ImportStepRequest, ImportStepResult, ImportedBody, Kernel, KernelResult, MassProperties, Mesh, OpResult,
    OpTag, PatternLayout, Plane, Point2, Profile, RepairChange, RepairDerivative, RepairRecord, RepairSource, Revolve,
    TessellationParams, Topology, Validation, question_id,
};
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::{
    BuiltinShape, Command, DatumPlane, Document, EndCondition, ExtrudeFeature, ExtrudeOp, FeatureId, FeatureKind,
    FeatureStatus, ImportStepFeature, ProfileSource, sha256_hex,
};

const STEP: &[u8] = b"ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\nENDSEC;\nEND-ISO-10303-21;\n";

/// A backend that answers `import_step` with a given set of solids and open solids, and
/// writes down the decisions it was given. Everything else is Catallax's.
struct Importer {
    inner: CatallaxKernel,
    solids: usize,
    curved: bool,
    left_open: Vec<String>,
    seen: Mutex<Vec<Vec<Decision>>>,
}

impl Importer {
    fn new(solids: usize, left_open: &[&str]) -> Arc<Importer> {
        let left_open = left_open.iter().map(|s| s.to_string()).collect();
        Arc::new(Importer { inner: CatallaxKernel::new(), solids, curved: false, left_open, seen: Mutex::default() })
    }
}

impl Kernel for Importer {
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
    fn hole(&self, tag: &OpTag, body: BodyId, params: &Hole) -> KernelResult<OpResult> {
        self.inner.hole(tag, body, params)
    }
    fn pattern(&self, tag: &OpTag, body: BodyId, layout: &PatternLayout) -> KernelResult<OpResult> {
        self.inner.pattern(tag, body, layout)
    }
    fn draft(&self, tag: &OpTag, body: BodyId, faces: &[FaceId], params: &Draft) -> KernelResult<OpResult> {
        self.inner.draft(tag, body, faces, params)
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

    fn import_step(&self, tag: &OpTag, request: &ImportStepRequest<'_>) -> KernelResult<ImportStepResult> {
        self.seen.lock().unwrap().push(request.decisions.to_vec());
        let mut bodies = Vec::new();
        if self.left_open.is_empty() {
            for i in 0..self.solids {
                let centre = Point2 { u: 0.0, v: 0.0 };
                let profile = if self.curved {
                    Profile::circle(Plane::XY, centre, 5.0)
                } else {
                    Profile::rectangle(Plane::XY, centre, 10.0, 10.0)
                };
                let sheet =
                    self.inner.create_body_from_profile(&OpTag::new(format!("{}.{i}.p", tag.as_str())), &profile)?;
                let solid = self.inner.extrude(
                    &OpTag::new(format!("{}.{i}.e", tag.as_str())),
                    sheet.body,
                    &Extrude { distance: 5.0, direction: None },
                );
                let _ = self.inner.release(sheet.body);
                bodies.push(ImportedBody { name: format!("solid{i}"), result: solid? });
            }
        }
        let change = RepairChange {
            id: "g0".into(),
            entities: vec!["#12".into(), "#40".into()],
            kind: "seam_close".into(),
            bounds_before: [["0".into(), "0".into(), "0".into()], ["10".into(), "10".into(), "5".into()]],
            bounds_after: [["0".into(), "0".into(), "0".into()], ["10".into(), "10".into(), "5".into()]],
            max_movement_mm: "0.004".into(),
            tolerance_mm: "0.01".into(),
            reason: "boundary gap below automatic cap".into(),
            approval: Approval::Automatic,
            approver: None,
        };
        Ok(ImportStepResult {
            bodies,
            left_open: self.left_open.clone(),
            record: RepairRecord {
                format: "catallax-import-repair/1".into(),
                source: RepairSource { name: request.name.into(), sha256: request.sha256.into() },
                derivative: Some(RepairDerivative { sha256: "00".repeat(32) }),
                changes: vec![change],
            },
        })
    }
}

fn add(doc: &mut Document, kind: FeatureKind) -> Result<FeatureId, soliddraft_model::CommandError> {
    doc.execute(Command::AddFeature { kind, name: None, position: None, via_assistant: false })
        .map(|o| o.created.unwrap())
}

fn status(doc: &Document, id: FeatureId) -> FeatureStatus {
    doc.status(id).unwrap().clone()
}

fn failure(doc: &Document, id: FeatureId) -> String {
    match status(doc, id) {
        FeatureStatus::Failed(m) => m,
        other => panic!("expected a failure, got {other:?}"),
    }
}

fn import() -> ImportStepFeature {
    ImportStepFeature::new("bracket.step", STEP.to_vec())
}

#[test]
fn the_import_hashes_its_bytes_and_keeps_them_unchanged() {
    let i = import();
    assert_eq!(i.sha256, sha256_hex(STEP));
    assert_eq!(i.sha256.len(), 64);
    assert_eq!(i.source, STEP);
    assert!(i.hash_matches());
}

#[test]
fn without_a_backend_reader_the_import_fails_in_words_and_keeps_the_source() {
    let mut doc = Document::new("Part1", Arc::new(CatallaxKernel::new()));
    let id = add(&mut doc, FeatureKind::ImportStep(import())).unwrap();
    let message = failure(&doc, id);
    assert!(message.contains("cannot import STEP files yet"), "{message}");
    assert!(message.contains("import_step"), "{message}");
    let Some(FeatureKind::ImportStep(kept)) = doc.feature(id).map(|f| f.kind.clone()) else { panic!() };
    assert_eq!(kept.source, STEP);
}

#[test]
fn a_hash_that_does_not_match_the_bytes_is_refused() {
    let mut doc = Document::new("Part1", Arc::new(CatallaxKernel::new()));
    let mut bad = import();
    bad.sha256 = sha256_hex(b"other");
    assert!(add(&mut doc, FeatureKind::ImportStep(bad)).is_err());
    let mut empty = import();
    empty.source.clear();
    empty.sha256 = sha256_hex(b"");
    assert!(add(&mut doc, FeatureKind::ImportStep(empty)).is_err());
    assert!(doc.features().is_empty());
}

#[test]
fn decisions_must_name_gaps_of_this_file_and_be_readable() {
    let mut doc = Document::new("Part1", Arc::new(CatallaxKernel::new()));
    let i = import();
    let q = question_id(&i.sha256, 2);
    let other = question_id(&sha256_hex(b"other file"), 2);
    assert!(add(&mut doc, FeatureKind::ImportStep(i.declined(&other))).is_err());
    assert!(add(&mut doc, FeatureKind::ImportStep(i.accepted(&q, "0.02", ""))).is_err());
    assert!(add(&mut doc, FeatureKind::ImportStep(i.accepted(&q, "ten", "ana"))).is_err());
    let mut garbled = i.clone();
    garbled.decisions.insert(q.clone(), "maybe".into());
    assert!(add(&mut doc, FeatureKind::ImportStep(garbled)).is_err());
    assert!(add(&mut doc, FeatureKind::ImportStep(i.accepted(&q, "0.02", "ana"))).is_ok());
}

#[test]
fn saved_decisions_reach_the_backend_as_the_wire_shape() {
    let kernel = Importer::new(1, &[]);
    let mut doc = Document::new("Part1", kernel.clone());
    let i = import();
    let (q0, q1) = (question_id(&i.sha256, 0), question_id(&i.sha256, 1));
    let i = i.accepted(&q0, "0.02", "ana").declined(&q1);
    let id = add(&mut doc, FeatureKind::ImportStep(i)).unwrap();
    assert_eq!(status(&doc, id), FeatureStatus::Ok);
    let seen = kernel.seen.lock().unwrap();
    assert_eq!(
        seen[0],
        vec![
            Decision::Accept { id: "g0".into(), tolerance_mm: "0.02".into(), approver: "ana".into() },
            Decision::Decline { id: "g1".into() },
        ]
    );
}

#[test]
fn one_built_solid_is_the_part_body() {
    let mut doc = Document::new("Part1", Importer::new(1, &[]));
    let id = add(&mut doc, FeatureKind::ImportStep(import())).unwrap();
    assert_eq!(status(&doc, id), FeatureStatus::Ok);
    assert!(doc.body().is_some());
}

#[test]
fn separate_solids_are_never_merged_into_one_body() {
    let mut doc = Document::new("Part1", Importer::new(2, &[]));
    let id = add(&mut doc, FeatureKind::ImportStep(import())).unwrap();
    let message = failure(&doc, id);
    assert!(message.contains("2 solids"), "{message}");
    assert!(doc.body().is_none());
}

#[test]
fn a_solid_left_open_is_not_imported() {
    let mut doc = Document::new("Part1", Importer::new(1, &["slot_block"]));
    let id = add(&mut doc, FeatureKind::ImportStep(import())).unwrap();
    let message = failure(&doc, id);
    assert!(message.contains("slot_block not imported until the gap is resolved"), "{message}");
    assert!(message.contains("leaves the solid open and unbuilt"), "{message}");
    assert!(message.contains("original file was never changed"), "{message}");
    assert!(doc.body().is_none());
    // The claim is true: the source bytes and their hash are exactly what was imported.
    let Some(FeatureKind::ImportStep(kept)) = doc.feature(id).map(|f| f.kind.clone()) else { panic!() };
    assert_eq!(kept.source, STEP);
    assert!(kept.hash_matches());
}

#[test]
fn an_import_after_another_body_is_refused_and_keeps_that_body() {
    let mut doc = Document::new("Part1", Importer::new(1, &[]));
    let boss = FeatureKind::Extrude(ExtrudeFeature {
        profile: ProfileSource::Shape {
            datum: DatumPlane::Top,
            shape: BuiltinShape::Rectangle { width: 10.0, height: 10.0 },
        },
        operation: ExtrudeOp::Boss,
        end: EndCondition::Blind { depth: 5.0 },
        reverse: false,
    });
    let boss = add(&mut doc, boss).unwrap();
    let body = doc.body();
    let id = add(&mut doc, FeatureKind::ImportStep(import())).unwrap();
    assert_eq!(status(&doc, boss), FeatureStatus::Ok);
    assert!(failure(&doc, id).contains("must come first"));
    assert_eq!(doc.body(), body);
}

#[test]
fn source_bytes_and_decisions_reopen_identically() {
    let kernel = Importer::new(1, &[]);
    let mut doc = Document::new("Part1", kernel.clone());
    let i = import();
    let q = question_id(&i.sha256, 3);
    let source: Vec<u8> = (0..=255u8).chain(STEP.iter().copied()).collect();
    let mut i = ImportStepFeature::new("odd name \"x\".step", source.clone()).accepted(
        &question_id(&sha256_hex(&source), 0),
        "0.02",
        "ana",
    );
    i = i.declined(&question_id(&sha256_hex(&source), 3));
    let _ = q;
    let id = add(&mut doc, FeatureKind::ImportStep(i.clone())).unwrap();
    let bytes = doc.to_bytes().unwrap();
    assert_eq!(bytes, doc.to_bytes().unwrap(), "saving is deterministic");

    let reopened = Document::from_bytes(&bytes, kernel.clone()).unwrap();
    let Some(FeatureKind::ImportStep(back)) = reopened.feature(id).map(|f| f.kind.clone()) else { panic!() };
    assert_eq!(back, i);
    assert_eq!(back.source, source);
    assert_eq!(reopened.to_bytes().unwrap(), bytes, "saving an opened file reproduces it");
    assert_eq!(status(&reopened, id), FeatureStatus::Ok);
    // Reopening sends the saved answers to the backend; it does not ask again.
    let seen = kernel.seen.lock().unwrap();
    assert_eq!(seen.last().unwrap().len(), 2);
}

#[test]
fn a_file_whose_source_bytes_were_altered_is_refused_on_open() {
    let kernel = Importer::new(1, &[]);
    let mut doc = Document::new("Part1", kernel.clone());
    add(&mut doc, FeatureKind::ImportStep(import())).unwrap();
    let json = doc.part_json();
    let hex: String = STEP.iter().map(|b| format!("{b:02x}")).collect();
    assert!(json.contains(&hex));
    let mut flipped = hex.clone();
    flipped.replace_range(0..2, "00");
    let altered = json.replace(&hex, &flipped);
    assert!(Document::from_part_json(&altered, kernel.clone()).is_err());
    assert!(Document::from_part_json(&json, kernel).is_ok());
}

fn curved_importer() -> Arc<Importer> {
    let mut k = Importer::new(1, &[]);
    Arc::get_mut(&mut k).unwrap().curved = true;
    k
}

#[test]
fn a_curved_body_says_self_intersection_is_not_verified() {
    let mut doc = Document::new("Part1", curved_importer());
    let id = add(&mut doc, FeatureKind::ImportStep(import())).unwrap();
    assert_eq!(status(&doc, id), FeatureStatus::Ok);
    let note = doc.import_check_note(id).expect("a curved body carries a note");
    assert!(note.contains("self-intersection not verified"), "{note}");
    assert!(note.contains("not marked validated"), "{note}");
}

#[test]
fn a_planar_body_makes_no_self_intersection_claim_yet() {
    let mut doc = Document::new("Part1", Importer::new(1, &[]));
    let id = add(&mut doc, FeatureKind::ImportStep(import())).unwrap();
    assert_eq!(status(&doc, id), FeatureStatus::Ok);
    assert_eq!(doc.import_check_note(id), None);
}

#[test]
fn a_failed_import_has_no_check_note() {
    let mut doc = Document::new("Part1", Arc::new(CatallaxKernel::new()));
    let id = add(&mut doc, FeatureKind::ImportStep(import())).unwrap();
    assert_eq!(doc.import_check_note(id), None);
}
