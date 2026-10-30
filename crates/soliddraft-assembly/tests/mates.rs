//! Mates on real parts through the commands the panel and the MCP tools call: what stays
//! free, how alignment and Origin work, what a failed Update leaves behind, and what old
//! and future files keep.
//!
//! Expected positions are hand-worked from the plane equations of a 40 x 30 x D block A
//! and a 20 x 10 x 6 block B, both centred on the origin in plan and rising along +Z.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use glam::{DQuat, DVec3};
use serde_json::{Value, json};
use soliddraft_assembly::{
    Alignment, Assembly, AssemblyDocument, AssemblyPackage, ComponentId, EndpointSelector, FaceSelector, FileError,
    GeneratorChoice, KernelSource, MateEdit, OccurrencePath, OpenOptions, Placement, RefusalCode, SolverState, Step,
};
use soliddraft_document::{CacheOnlyStore, DocRef, identify_file};
use soliddraft_kernel_api::Kernel;
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::sketch::{P2, Sketch, solve::solve};
use soliddraft_model::{
    Command, DatumPlane, Document, EndCondition, ExtrudeFeature, ExtrudeOp, FeatureKind, ProfileSource, SketchFeature,
    SketchPlane,
};

fn kernel() -> Arc<dyn Kernel> {
    Arc::new(CatallaxKernel::new())
}

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soliddraft-mates-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn options() -> OpenOptions {
    OpenOptions {
        display: Some(Arc::new(KernelSource::new(kernel()))),
        revisions: Some(Arc::new(CacheOnlyStore::new())),
        check_freshness: true,
        ..OpenOptions::default()
    }
}

/// A `w` x `d` x `h` block, centred in plan and rising from z = 0.
fn new_block(path: &Path, w: f64, d: f64, h: f64) {
    let mut doc = Document::new("Block", kernel());
    let mut sketch = Sketch::new();
    sketch.rectangle(P2::new(0.0, 0.0), w, d).unwrap();
    solve(&mut sketch, None, true);
    let plane = SketchPlane::Datum { datum: DatumPlane::Top, offset: 0.0 };
    let add = |doc: &mut Document, kind| {
        doc.execute(Command::AddFeature { kind, name: None, position: None, via_assistant: false })
            .unwrap()
            .created
            .unwrap()
    };
    let s = add(&mut doc, FeatureKind::Sketch(SketchFeature { plane, sketch }));
    add(
        &mut doc,
        FeatureKind::Extrude(ExtrudeFeature {
            profile: ProfileSource::Sketch { feature: s },
            operation: ExtrudeOp::Boss,
            end: EndCondition::Blind { depth: h },
            reverse: false,
        }),
    );
    doc.save(path).unwrap();
}

/// Sets a saved block's height, keeping its id and its extrusion.
fn set_height(path: &Path, h: f64) {
    let mut doc = Document::open(path, kernel()).unwrap();
    let id = doc.features()[1].id;
    let FeatureKind::Extrude(mut extrude) = doc.feature(id).unwrap().kind.clone() else { panic!("an extrusion") };
    extrude.end = EndCondition::Blind { depth: h };
    doc.execute(Command::EditFeature { id, kind: FeatureKind::Extrude(extrude) }).unwrap();
    doc.save(path).unwrap();
}

/// Replaces a saved block's extrusion with a new feature making the same box.
fn regenerate(path: &Path) {
    let mut doc = Document::open(path, kernel()).unwrap();
    let old = doc.features()[1].clone();
    doc.execute(Command::DeleteFeature { id: old.id }).unwrap();
    let kind = old.kind.clone();
    doc.execute(Command::AddFeature { kind, name: Some(old.name.clone()), position: None, via_assistant: false })
        .unwrap();
    assert_ne!(doc.features()[1].id, old.id);
    doc.save(path).unwrap();
}

fn reference(from: &Path, to: &Path) -> DocRef {
    let i = identify_file(to).unwrap();
    let mut r = DocRef::new(i.id, i.kind, "", i.revision);
    r.set_path(from, to);
    r
}

fn face(component: ComponentId, generator: GeneratorChoice, normal: Option<[f64; 3]>) -> EndpointSelector {
    let feature = "Boss-Extrude1".into();
    EndpointSelector { component, face: FaceSelector::Generator { feature, generator, normal } }
}

fn side(component: ComponentId, n: [f64; 3]) -> EndpointSelector {
    face(component, GeneratorChoice::Side, Some(n))
}

fn top(component: ComponentId) -> EndpointSelector {
    face(component, GeneratorChoice::EndCap, None)
}

fn bottom(component: ComponentId) -> EndpointSelector {
    face(component, GeneratorChoice::StartCap, None)
}

const X: [f64; 3] = [1.0, 0.0, 0.0];
const Y: [f64; 3] = [0.0, 1.0, 0.0];

struct Rig {
    dir: PathBuf,
    a_file: PathBuf,
    b_file: PathBuf,
    root: PathBuf,
    doc: AssemblyDocument,
    a: ComponentId,
    b: ComponentId,
}

/// Part A of height `depth` and B inserted at [10, 10, 30], both floating, saved.
fn rig(name: &str, depth: f64) -> Rig {
    let dir = folder(name);
    let (a_file, b_file, root) = (dir.join("a.dwprt"), dir.join("b.dwprt"), dir.join("rig.dwasm"));
    new_block(&a_file, 40.0, 30.0, depth);
    new_block(&b_file, 20.0, 10.0, 6.0);
    let mut doc = AssemblyDocument::new_with("Rig", options());
    doc.save(Some(&root)).unwrap();
    let a = doc.insert(reference(&root, &a_file), Placement::IDENTITY).unwrap();
    let b = doc.insert(reference(&root, &b_file), Placement::translation(DVec3::new(10.0, 10.0, 30.0))).unwrap();
    doc.finish_loading();
    doc.save(None).unwrap();
    Rig { dir, a_file, b_file, root, doc, a, b }
}

impl Rig {
    fn x(&mut self, alignment: Alignment) {
        self.doc.mate_distance("X", &side(self.a, X), &side(self.b, X), "0 mm", alignment, false).unwrap();
    }

    fn y(&mut self, alignment: Alignment) {
        self.doc.mate_distance("Y", &side(self.a, Y), &side(self.b, Y), "0 mm", alignment, false).unwrap();
    }

    fn z(&mut self, distance: &str, alignment: Alignment) {
        self.doc.mate_distance("Z", &top(self.a), &bottom(self.b), distance, alignment, false).unwrap();
    }

    /// O, X, Y and Z of the main graph, Z at 5 mm.
    fn graph(&mut self) {
        self.doc.mate_origin("O", self.a, true, false).unwrap();
        self.x(Alignment::Aligned);
        self.y(Alignment::Aligned);
        self.z("5 mm", Alignment::Opposed);
    }

    fn placement(&self, c: ComponentId) -> Placement {
        self.doc.assembly().component(c).unwrap().placement
    }

    fn row(&self, c: ComponentId) -> soliddraft_assembly::OccurrenceIndex {
        self.doc.table().find(&OccurrencePath::root().child(Step::component(c))).unwrap()
    }

    /// The whole accepted state a refusal must leave alone.
    fn state(&self) -> Value {
        let table = self.doc.table();
        let rows: Vec<_> = (0..table.len() as u32)
            .map(soliddraft_assembly::OccurrenceIndex)
            .map(|r| json!([table.path(r), table.local(r), table.world(r)]))
            .collect();
        let keys: Vec<_> = self.doc.store().iter().map(|(_, d)| json!([d.key.document, d.key.revision])).collect();
        json!({
            "truth": self.doc.assembly(), "revision": self.doc.revision(), "dirty": self.doc.is_dirty(),
            "undo": [self.doc.undo_label(), self.doc.undo_depth()], "redo": [self.doc.redo_label(), self.doc.redo_depth()],
            "rows": rows, "definitions": keys,
        })
    }

    /// The top of B as the occurrence table shows it.
    fn b_top(&self) -> f64 {
        self.doc.table().bounds(self.row(self.b)).max.z
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn close(a: DVec3, b: [f64; 3]) {
    assert!(a.distance(DVec3::from_array(b)) <= 1e-9, "{a} against {b:?}");
}

/// Rotations match by what they do to the basis, allowing q and -q.
fn turned(q: DQuat, x: [f64; 3], y: [f64; 3], z: [f64; 3]) {
    for (axis, wanted) in [(DVec3::X, x), (DVec3::Y, y), (DVec3::Z, z)] {
        assert!((q * axis).distance(DVec3::from_array(wanted)) <= 1e-12, "{q} on {axis}");
    }
}

/// S1: refusal against partial freedom, and no hidden fixing of A.
#[test]
fn partial_freedom_and_strict_failure_are_distinct() {
    let mut r = rig("s1", 10.0);
    r.doc.mate_origin("O", r.a, true, false).unwrap();
    let before = r.state();
    // A held redo survives a refusal too.
    r.doc.mate_origin("Spare", r.b, false, false).unwrap();
    r.doc.undo();
    let held = r.state();
    let a_bytes = std::fs::read(&r.a_file).unwrap();
    let b_bytes = std::fs::read(&r.b_file).unwrap();

    let refused = r.doc.mate_distance("Z", &top(r.a), &bottom(r.b), "5 mm", Alignment::Opposed, true).unwrap_err();
    assert_eq!(refused.code, RefusalCode::UnderConstrained);
    assert_eq!(refused.remaining_dof, Some(3));
    let motions: Vec<_> = refused.free.iter().map(|f| (f.component, f.motion.as_str())).collect();
    assert_eq!(motions, [(r.b, "translate_x"), (r.b, "translate_y"), (r.b, "spin_z")]);
    assert_eq!(r.state(), held, "a refused proposal allocates and records nothing, and keeps redo");
    assert_eq!(std::fs::read(&r.a_file).unwrap(), a_bytes);
    assert_eq!(std::fs::read(&r.b_file).unwrap(), b_bytes);
    assert_eq!(before["truth"], held["truth"]);

    // Ordinary insertion of the same partial mate commits, and keeps authored coordinates.
    r.z("5 mm", Alignment::Opposed);
    let status = r.doc.solver();
    assert_eq!((status.state, status.remaining_dof), (SolverState::Underconstrained, 3));
    assert_eq!(status.free.len(), 3);
    assert_eq!(r.placement(r.b).translation, DVec3::new(10.0, 10.0, 15.0));

    // Without O the six remaining motions are the whole group's, and A is not fixed.
    let mut r = rig("s1-float", 10.0);
    r.x(Alignment::Aligned);
    r.y(Alignment::Aligned);
    r.z("5 mm", Alignment::Opposed);
    assert_eq!(r.doc.solver().remaining_dof, 6);
    assert!(r.doc.solver().free.is_empty(), "no one component moves alone: {:?}", r.doc.solver());
    assert_eq!(r.doc.solver().state, SolverState::Underconstrained);
    let refused = r.doc.mate_distance("Again", &top(r.a), &bottom(r.b), "5 mm", Alignment::Opposed, true).unwrap_err();
    assert_eq!((refused.code, refused.remaining_dof), (RefusalCode::UnderConstrained, Some(6)));
}

/// S2: alignment is a vector relation, and every family goes through one solver.
#[test]
fn alignment_and_origin_use_the_same_solver() {
    let mut r = rig("s2", 14.0);
    r.graph();
    let before = r.state();
    let refused = r.doc.edit_mates(&[("Z".into(), MateEdit::Alignment(Alignment::Aligned))]).unwrap_err();
    assert_eq!(refused.code, RefusalCode::OverConstrained, "{refused}");
    assert_eq!(r.state(), before);

    // Y and Z flip together, as one candidate and one undo step.
    r.doc.edit_mates(&[("Z".into(), MateEdit::Distance("12 mm".into()))]).unwrap();
    let depth = r.doc.undo_depth();
    r.doc
        .edit_mates(&[
            ("Y".into(), MateEdit::Alignment(Alignment::Opposed)),
            ("Z".into(), MateEdit::Alignment(Alignment::Aligned)),
        ])
        .unwrap();
    assert_eq!(r.doc.undo_depth(), depth + 1);
    let p = r.placement(r.b);
    assert_eq!(p.rotation.to_array(), [1.0, 0.0, 0.0, 0.0]);
    turned(p.rotation, [1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]);
    close(p.translation, [10.0, 20.0, 26.0]);
    let bounds = r.doc.table().bounds(r.row(r.b));
    close(bounds.min, [0.0, 15.0, 20.0]);
    close(bounds.max, [20.0, 25.0, 26.0]);
    let measured = r.doc.mate_measurements();
    let z = measured.last().unwrap().1.as_ref().unwrap();
    assert_eq!((z.value, z.rational.as_str()), (12.0, "12"));

    r.doc.save(None).unwrap();
    let mut reopened = AssemblyDocument::open(&r.root, options()).unwrap();
    reopened.finish_loading();
    reopened.edit_mates(&[("Z".into(), MateEdit::Distance("8 mm".into()))]).unwrap();
    let c = reopened.assembly().component(r.b).unwrap();
    close(c.placement.translation, [10.0, 20.0, 22.0]);
    let row = reopened.table().find(&OccurrencePath::root().child(Step::component(r.b))).unwrap();
    close(reopened.table().bounds(row).min, [0.0, 15.0, 16.0]);
    close(reopened.table().bounds(row).max, [20.0, 25.0, 22.0]);
}

/// S3: an Origin mate anchors what it says it anchors.
#[test]
fn origin_axis_alignment_is_authored() {
    let mut r = rig("s3", 14.0);
    r.doc.mate_origin("O", r.a, false, false).unwrap();
    r.x(Alignment::Aligned);
    r.y(Alignment::Aligned);
    r.z("12 mm", Alignment::Opposed);
    assert_eq!(r.doc.solver().remaining_dof, 3, "A's origin is anchored and its turn is not");
    let before = r.state();
    let refused = r.doc.mate_origin("Strict", r.a, false, true).unwrap_err();
    assert_eq!((refused.code, refused.remaining_dof), (RefusalCode::UnderConstrained, Some(3)));
    assert_eq!(r.state(), before);

    let depth = r.doc.undo_depth();
    r.doc.edit_mates(&[("O".into(), MateEdit::AlignAxes(true))]).unwrap();
    assert_eq!(r.doc.undo_depth(), depth + 1);
    assert_eq!(r.doc.solver().remaining_dof, 0);
    assert!(r.placement(r.a).is_identity());
    close(r.placement(r.b).translation, [10.0, 10.0, 26.0]);
    assert!(r.doc.undo());
    assert_eq!(r.doc.solver().remaining_dof, 3);
    assert!(r.doc.redo());
    assert_eq!(r.doc.solver().remaining_dof, 0);
    r.doc.save(None).unwrap();
    let mut reopened = AssemblyDocument::open(&r.root, options()).unwrap();
    reopened.finish_loading();
    assert_eq!(reopened.solver().remaining_dof, 0);
    assert_eq!(reopened.assembly(), r.doc.assembly());

    // A first component at zero with no Origin mate floats.
    let mut r = rig("s3-float", 14.0);
    assert_eq!(r.doc.solver().remaining_dof, 12);
    assert_eq!(r.doc.solver().state, SolverState::Underconstrained);
}

/// S4: a failed Update rolls back every candidate resource, whichever step failed.
#[test]
fn failed_update_rolls_back_every_candidate_resource() {
    let mut r = rig("s4", 14.0);
    r.graph();
    r.doc.set_value("gap", "12 mm").unwrap();
    r.doc.drive_mate("Z", "gap", false).unwrap();
    close(r.placement(r.b).translation, [10.0, 10.0, 26.0]);
    r.doc.save(None).unwrap();
    let saved_a = std::fs::read(&r.a_file).unwrap();

    // B has a valid new depth; A's new revision has lost the faces the mates name.
    set_height(&r.b_file, 8.0);
    regenerate(&r.a_file);
    let before = r.state();
    let top_before = r.b_top();
    let report = r.doc.rebuild();
    let refusal = report.mate_refusal.clone().unwrap();
    assert_eq!(refusal.code, RefusalCode::DanglingReference);
    assert_eq!(refusal.mates, ["X", "Y", "Z"]);
    assert!(refusal.message.contains("a.dwprt"), "{refusal}");
    assert!(report.refusal.as_ref().unwrap().contains("dangling_reference"));
    r.doc.finish_loading();
    assert_eq!(r.state(), before);
    assert_eq!(r.b_top(), top_before, "B's six-mm display stays");

    // Restoring the file repairs it, and the good B comes in: 26 + 8.
    std::fs::write(&r.a_file, &saved_a).unwrap();
    let report = r.doc.rebuild();
    assert!(report.refusal.is_none(), "{report:?}");
    r.doc.finish_loading();
    close(r.placement(r.b).translation, [10.0, 10.0, 26.0]);
    assert!((r.b_top() - 34.0).abs() <= 1e-9, "{}", r.b_top());
    assert_eq!(r.doc.undo_label(), Some("Update"));

    // A refusal from the solve itself, after the candidate has loaded: B is anchored too,
    // so A's new height leaves the distance mate nothing to move.
    let mut r = rig("s4-solve", 10.0);
    r.doc.mate_origin("OA", r.a, true, false).unwrap();
    r.doc.mate_origin("OB", r.b, true, false).unwrap();
    r.doc.mate_distance("Z", &top(r.a), &bottom(r.b), "-10 mm", Alignment::Opposed, false).unwrap();
    r.doc.finish_loading();
    r.doc.save(None).unwrap();
    set_height(&r.a_file, 14.0);
    let before = r.state();
    let report = r.doc.rebuild();
    assert_eq!(report.mate_refusal.unwrap().code, RefusalCode::OverConstrained);
    r.doc.finish_loading();
    assert_eq!(r.state(), before);
}

/// P1: an old file, and a mate from a later version.
#[test]
fn old_and_future_assembly_truth_survives() {
    // The file V4's qualified scenario wrote, as hosted CI uploaded it. V5 did not build it.
    let dir = folder("p1");
    let root = dir.join("rig.dwasm");
    std::fs::write(&root, include_bytes!("fixtures/v4-unmated/rig.dwasm")).unwrap();
    std::fs::write(dir.join("a.dwprt"), include_bytes!("fixtures/v4-unmated/a.dwprt")).unwrap();
    let written = AssemblyPackage::open(&root).unwrap();
    let (id, revision) = (written.id(), written.manifest().semantic_hash.clone());
    let geometry = written.manifest().geometry_hash.clone().unwrap();
    assert_eq!(id.to_string(), "0f4f3914-b756-4be9-b74d-e4d703894e84");
    assert_eq!(revision.to_string(), "blake3:eadfa6d96ef1d2ff8868d89f54e001bd72daa2c64940f2a5334b9a3c15c1755d");
    assert_eq!(geometry.to_string(), "blake3:3cdcde0e867af058eb07e3e0c53250413d528785a91947655681a5231fe1f103");
    assert_eq!(written.assembly().semantic_hash(), revision, "the hash V4 wrote is the hash V5 computes");
    assert_eq!(written.assembly().geometry_hash(), geometry);
    let truth = written.assembly().truth_entries().json.to_vec();
    let text = String::from_utf8(truth.clone()).unwrap();
    assert!(!text.contains("mates") && !text.contains("values"), "an empty design writes nothing: {text}");

    let mut doc = AssemblyDocument::open(&root, options()).unwrap();
    doc.finish_loading();
    assert_ne!(doc.solver().state, SolverState::Error);
    let floating = doc.assembly().components[0].clone();
    assert!(floating.placement.is_identity());
    assert_eq!(doc.assembly(), &**written.assembly(), "opening solves nothing and moves nothing");
    assert!(!doc.is_dirty());
    doc.save(None).unwrap();
    let saved = AssemblyPackage::open(&root).unwrap();
    assert_eq!((saved.id(), &saved.manifest().semantic_hash), (id, &revision));
    assert_eq!(saved.manifest().geometry_hash.as_ref(), Some(&geometry));
    assert_eq!(saved.assembly().truth_entries().json.to_vec(), truth);
    assert_eq!(saved.assembly().components[0], floating);
    let _ = std::fs::remove_dir_all(&dir);

    // A mate of a later version round-trips untouched, and nothing solves past it.
    let mut v: Value = serde_json::from_str(&text).unwrap();
    let future = json!({ "kind": "gear", "id": 40, "ratio": 2.5, "future": { "nested": [1, 2] } });
    v["mates"] = json!([future]);
    let kept = Assembly::from_truth(v.to_string().as_bytes(), None).unwrap();
    assert_eq!(serde_json::to_value(&kept).unwrap()["mates"][0], future);
    let dir = folder("p1-future");
    let root = dir.join("future.dwasm");
    kept.save(&root, id, None, &[]).unwrap();
    let mut doc = AssemblyDocument::open(&root, options()).unwrap();
    doc.finish_loading();
    let status = doc.solver();
    assert_eq!(status.state, SolverState::Error);
    assert!(status.diagnostics[0].contains("unknown_mate"), "{status:?}");
    let refused = doc.set_value("gap", "5 mm").unwrap_err();
    assert_eq!(refused.code, RefusalCode::UnknownMate);
    assert_eq!(doc.assembly(), &kept);
    assert!(!doc.is_dirty(), "a proof or a status never changes truth");
    let _ = std::fs::remove_dir_all(dir);

    // A known mate that names what is not there is refused, not dropped.
    let mut v: Value = serde_json::from_str(&text).unwrap();
    v["mates"] = json!([{ "kind": "origin", "id": 9, "name": "O", "component": 77, "align_axes": true }]);
    v["next_id"] = json!(10);
    assert!(matches!(Assembly::from_truth(v.to_string().as_bytes(), None), Err(FileError::Invalid(_))));
}

/// A proof names the accepted revision, and a part that moved on leaves it retained.
#[test]
fn a_proof_stays_with_its_revision() {
    let mut r = rig("retained", 10.0);
    r.graph();
    let before = r.doc.mate_measurements();
    let pinned = before.last().unwrap().1.as_ref().unwrap().clone();
    assert_eq!((pinned.value, pinned.rational.as_str()), (5.0, "5"));
    assert_eq!(pinned.certainty, soliddraft_kernel_api::Certainty::Exact);
    assert!(!pinned.a.retained);
    set_height(&r.a_file, 14.0);
    r.doc.refresh_references(Vec::new());
    r.doc.finish_loading();
    let later = r.doc.mate_measurements();
    let held = later.last().unwrap().1.as_ref().unwrap();
    assert!(held.a.retained && !held.b.retained, "{held:?}");
    assert_eq!((held.value, &held.a.revision), (5.0, &pinned.a.revision));
}

/// A mate that moves a pattern's seed takes the members with it, and undo brings them back.
#[test]
fn a_mated_seed_takes_its_pattern_along() {
    use soliddraft_assembly::PatternRule;
    let mut r = rig("pattern", 10.0);
    let rule = PatternRule::Linear { direction: [0.0, 1.0, 0.0], spacing: 50.0, count: 2, second: None };
    let pattern = r.doc.add_pattern(vec![r.b], rule).unwrap();
    r.doc.finish_loading();
    let member = OccurrencePath::root().child(Step::member(r.b, pattern, 1));
    r.graph();
    r.doc.finish_loading();
    let row = r.doc.table().find(&member).unwrap();
    close(r.doc.table().world(row).translation, [10.0, 60.0, 15.0]);
    while r.doc.undo_label() != Some("Pattern") {
        r.doc.undo();
    }
    r.doc.finish_loading();
    let row = r.doc.table().find(&member).unwrap();
    close(r.doc.table().world(row).translation, [10.0, 60.0, 30.0]);
}

/// A move proposes a pose; a mated component keeps only what its mates leave free.
#[test]
fn moves_and_deletes_respect_mates() {
    let mut r = rig("moves", 10.0);
    r.doc.mate_origin("O", r.a, true, false).unwrap();
    r.z("5 mm", Alignment::Opposed);
    let path = OccurrencePath::root().child(Step::component(r.b));
    r.doc.move_to(&[(path.clone(), Placement::translation(DVec3::new(1.0, 2.0, 99.0)))]).unwrap();
    close(r.placement(r.b).translation, [1.0, 2.0, 15.0]);
    assert_eq!(r.doc.undo_label(), Some("Move"));

    r.x(Alignment::Aligned);
    r.y(Alignment::Aligned);
    let before = r.state();
    assert!(r.doc.move_to(&[(path, Placement::translation(DVec3::new(1.0, 2.0, 99.0)))]).is_err());
    assert_eq!(r.state(), before);

    let refused = r.doc.delete(&[r.b]).unwrap_err();
    assert!(refused.to_string().contains("mates first"), "{refused}");
    assert_eq!(r.state(), before);
    r.doc.delete_mate("Z").unwrap();
    assert_eq!(r.doc.solver().remaining_dof, 1);
    close(r.placement(r.b).translation, [10.0, 10.0, 15.0]);
}
