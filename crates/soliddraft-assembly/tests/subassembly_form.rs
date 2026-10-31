//! Forming a separate-file sub-assembly, read back through the production package reader.
//!
//! The fixture is the V6 batch 1 design's: a 40 x 30 x 10 base A and three placements B,
//! C and F of one 20 x 10 x h pin, all centred in plan and rising along +Z. A is on the
//! root's origin, B on A by three plane distances (Z driven by the root value `gap`), C on
//! B by three literal ones, and F free with a half turn about Z. Every expected pose and
//! box below is hand-derived in the design report; none is read from a solver.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use glam::{DQuat, DVec3};
use soliddraft_assembly::{
    Alignment, AssemblyDocument, AssemblyPackage, ComponentId, EndpointSelector, FaceSelector, FormCode, FormRequest,
    GeneratorChoice, KernelSource, Mate, OccurrencePath, OpenOptions, Placement, SolverState, Step,
};
use soliddraft_document::{CacheOnlyStore, DocKind, DocRef, identify_file};
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
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
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

const X: [f64; 3] = [1.0, 0.0, 0.0];
const Y: [f64; 3] = [0.0, 1.0, 0.0];

fn path(steps: &[u32]) -> OccurrencePath {
    OccurrencePath(steps.iter().map(|c| Step::component(ComponentId(*c))).collect())
}

fn half_turn() -> DQuat {
    DQuat::from_xyzw(0.0, 0.0, 1.0, 0.0)
}

fn sha(path: &Path) -> String {
    soliddraft_document::ContentHash::of_bytes(&std::fs::read(path).unwrap()).to_string()
}

struct Fixture {
    dir: PathBuf,
    base: PathBuf,
    pin: PathBuf,
    root: PathBuf,
    pack: PathBuf,
    doc: AssemblyDocument,
    a: ComponentId,
    b: ComponentId,
    c: ComponentId,
    f: ComponentId,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The flat accepted state T0, saved: the design's steps 1 to 23.
fn flat(name: &str) -> Fixture {
    let dir = std::env::temp_dir().join(format!("soliddraft-subassembly-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let work = dir.join("work");
    let (base, pin) = (work.join("parts/base.dwprt"), work.join("parts/pin.dwprt"));
    let (root, pack) = (work.join("root.dwasm"), work.join("modules/pack.dwasm"));
    new_block(&base, 40.0, 30.0, 10.0);
    new_block(&pin, 20.0, 10.0, 6.0);
    let mut doc = AssemblyDocument::new_with("Root", options());
    doc.save(Some(&root)).unwrap();
    let a = doc.insert(reference(&root, &base), Placement::IDENTITY).unwrap();
    let at = |t: [f64; 3]| Placement::translation(DVec3::from_array(t));
    let b = doc.insert(reference(&root, &pin), at([10.0, 10.0, 22.0])).unwrap();
    let c = doc.insert(reference(&root, &pin), at([10.0, 10.0, 31.0])).unwrap();
    let f = doc.insert(reference(&root, &pin), Placement::new(half_turn(), DVec3::new(60.0, -20.0, 7.0))).unwrap();
    doc.finish_loading();
    let top = |c| face(c, GeneratorChoice::EndCap, None);
    let bottom = |c| face(c, GeneratorChoice::StartCap, None);
    doc.mate_origin("O", a, true, false).unwrap();
    doc.mate_distance("Xab", &side(a, X), &side(b, X), "0 mm", Alignment::Aligned, false).unwrap();
    doc.mate_distance("Yab", &side(a, Y), &side(b, Y), "0 mm", Alignment::Aligned, false).unwrap();
    doc.mate_distance("Zab", &top(a), &bottom(b), "12 mm", Alignment::Opposed, false).unwrap();
    doc.mate_distance("Xbc", &side(b, X), &side(c, X), "0 mm", Alignment::Aligned, false).unwrap();
    doc.mate_distance("Ybc", &side(b, Y), &side(c, Y), "0 mm", Alignment::Aligned, false).unwrap();
    doc.mate_distance("Zbc", &top(b), &bottom(c), "3 mm", Alignment::Opposed, false).unwrap();
    doc.set_value("gap", "12 mm").unwrap();
    doc.drive_mate("Zab", "gap", false).unwrap();
    doc.save(None).unwrap();
    Fixture { dir, base, pin, root, pack, doc, a, b, c, f }
}

impl Fixture {
    fn request(&self) -> FormRequest {
        let p = |c: ComponentId| path(&[c.0]);
        FormRequest { paths: vec![p(self.b), p(self.c), p(self.f)], path: self.pack.clone(), origin: p(self.b) }
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

/// The world pose and box of the occurrence at `p`, and its box, against the design's P0 row.
fn leaf(doc: &AssemblyDocument, p: &OccurrencePath, t: [f64; 3], lo: [f64; 3], hi: [f64; 3]) {
    let row = doc.table().find(p).unwrap_or_else(|| panic!("no occurrence at {p}"));
    close(doc.table().world(row).translation, t);
    let b = doc.table().bounds(row);
    close(b.min, lo);
    close(b.max, hi);
}

/// P0: h = 6, gap = 12.
fn p0(doc: &AssemblyDocument, a: &OccurrencePath, b: &OccurrencePath, c: &OccurrencePath, f: &OccurrencePath) {
    leaf(doc, a, [0.0, 0.0, 0.0], [-20.0, -15.0, 0.0], [20.0, 15.0, 10.0]);
    leaf(doc, b, [10.0, 10.0, 22.0], [0.0, 5.0, 22.0], [20.0, 15.0, 28.0]);
    leaf(doc, c, [10.0, 10.0, 31.0], [0.0, 5.0, 31.0], [20.0, 15.0, 37.0]);
    leaf(doc, f, [60.0, -20.0, 7.0], [50.0, -25.0, 7.0], [70.0, -15.0, 13.0]);
    let row = doc.table().find(f).unwrap();
    turned(doc.table().world(row).rotation, [-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]);
    let all = doc.table().bounds(soliddraft_assembly::OccurrenceIndex::ROOT);
    close(all.min, [-20.0, -25.0, 0.0]);
    close(all.max, [70.0, 15.0, 37.0]);
}

/// Steps 23 to 29: the flat root forms B, C and F into pack.dwasm with nothing moving,
/// every reference migrated, and both files reopening functional.
#[test]
fn forming_keeps_references_poses_and_freedoms() {
    let mut fx = flat("form");
    let (a, b, c, f) = (path(&[fx.a.0]), path(&[fx.b.0]), path(&[fx.c.0]), path(&[fx.f.0]));
    p0(&fx.doc, &a, &b, &c, &f);
    let flat_status = fx.doc.solver();
    assert_eq!((flat_status.state, flat_status.remaining_dof), (SolverState::Underconstrained, 6));
    let t0 = fx.doc.assembly().clone();
    let (root_sha, base_sha, pin_sha) = (sha(&fx.root), sha(&fx.base), sha(&fx.pin));
    let root_id = fx.doc.id();
    let pin_id = identify_file(&fx.pin).unwrap().id;

    let formed = fx.doc.form_subassembly(&fx.request()).unwrap();

    // The migration map, total over the selection.
    let m = &formed.migration;
    assert_eq!(m.root, root_id);
    assert!(m.child != root_id && m.child != pin_id && Some(m.child) != t0.sources[0].document.id);
    assert_eq!((m.source.0, m.component.0), (t0.next_id, t0.next_id + 1), "fresh ids from the root's counter");
    let s = m.component;
    let to: Vec<_> = m.occurrences.iter().map(|o| (o.from.clone(), o.to.clone(), o.child.clone())).collect();
    let nested = |x: ComponentId| path(&[s.0, x.0]);
    assert_eq!(
        to,
        [
            (b.clone(), nested(fx.b), b.clone()),
            (c.clone(), nested(fx.c), c.clone()),
            (f.clone(), nested(fx.f), f.clone())
        ]
    );
    for (name, owner) in [("O", root_id), ("Xab", root_id), ("Yab", root_id), ("Zab", root_id)].into_iter().chain([
        ("Xbc", m.child),
        ("Ybc", m.child),
        ("Zbc", m.child),
    ]) {
        let moved = m.mates.iter().find(|x| x.name == name).unwrap();
        let before = t0.known_mates().find(|x| x.name() == name).unwrap();
        assert_eq!((moved.id, moved.owner_before, moved.owner_after), (before.id(), root_id, owner), "{name}");
    }
    assert_eq!(formed.status.root.remaining_dof, 6);
    assert_eq!(formed.status.child.remaining_dof, 12);

    // The open root changed in memory, in one step; nothing it reads changed on disk.
    assert_eq!(fx.doc.undo_label(), Some("Form subassembly"));
    assert!(fx.doc.is_dirty());
    assert_eq!((sha(&fx.root), sha(&fx.base), sha(&fx.pin)), (root_sha.clone(), base_sha.clone(), pin_sha.clone()));
    let truth = fx.doc.assembly();
    let ids: Vec<u32> = truth.components.iter().map(|c| c.id.0).collect();
    assert_eq!(ids, [fx.a.0, s.0]);
    assert_eq!(truth.sources.len(), 2);
    assert_eq!(truth.component(s).unwrap().placement, Placement::translation(DVec3::new(10.0, 10.0, 22.0)));
    let child_ref = &truth.source(m.source).unwrap().document;
    assert_eq!((child_ref.kind, child_ref.path.as_str()), (DocKind::Assembly, "modules/pack.dwasm"));
    assert_eq!(truth.sources[0].document.path, "parts/base.dwprt");
    assert_eq!(truth.values, t0.values, "the root keeps gap with its id and text");
    for name in ["O", "Xab", "Yab", "Zab"] {
        let (now, then) = (
            truth.known_mates().find(|x| x.name() == name).unwrap(),
            t0.known_mates().find(|x| x.name() == name).unwrap(),
        );
        match (now, then) {
            (Mate::Distance { a: na, b: nb, distance: nd, .. }, Mate::Distance { a: ta, b: tb, distance: td, .. }) => {
                assert_eq!((&na.path, &na.document, &na.face), (&ta.path, &ta.document, &ta.face));
                assert_eq!(nb.path, nested(fx.b), "{name}: B's endpoint is nested");
                assert_eq!((&nb.document, &nb.face, &nb.configuration), (&tb.document, &tb.face, &tb.configuration));
                assert_eq!(nd, td, "{name}: the same expression and binding");
            }
            (now, then) => assert_eq!(now, then),
        }
    }
    assert!(truth.known_mates().all(|x| !x.name().ends_with("bc")));
    p0(&fx.doc, &a, &nested(fx.b), &nested(fx.c), &nested(fx.f));
    assert_eq!(fx.doc.solver().remaining_dof, 6);
    assert!(fx.doc.mate_measurements().iter().all(|(_, r)| r.is_ok()), "{:?}", fx.doc.mate_measurements());

    // The new file, read with the production reader.
    let pack = AssemblyPackage::open(&fx.pack).unwrap();
    assert_eq!((pack.id(), pack.manifest().semantic_hash.clone()), (m.child, m.child_revision.clone()));
    let child = pack.assembly();
    assert_eq!(child.sources.len(), 1);
    let pin_ref = &child.sources[0].document;
    assert_eq!(child.sources[0].id, t0.component(fx.b).unwrap().source, "the pin's source keeps its id");
    assert_eq!((pin_ref.id, pin_ref.path.as_str()), (Some(pin_id), "../parts/pin.dwprt"));
    assert_eq!(pin_ref.pinned, t0.source(child.sources[0].id).unwrap().document.pinned);
    let local = |x: ComponentId| child.component(x).unwrap().placement;
    assert_eq!(local(fx.b), Placement::IDENTITY);
    assert_eq!(local(fx.c), Placement::translation(DVec3::new(0.0, 0.0, 9.0)));
    close(local(fx.f).translation, [50.0, -30.0, -15.0]);
    turned(local(fx.f).rotation, [-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]);
    let names: Vec<_> = child.known_mates().map(|x| (x.id(), x.name().to_owned())).collect();
    let expected: Vec<_> =
        t0.known_mates().filter(|x| x.name().ends_with("bc")).map(|x| (x.id(), x.name().to_owned())).collect();
    assert_eq!(names, expected);
    for x in child.known_mates() {
        let Mate::Distance { a, b, distance, .. } = x else { panic!("{x:?}") };
        let then = t0.known_mates().find(|t| t.id() == x.id()).unwrap();
        let Mate::Distance { a: ta, b: tb, .. } = then else { panic!() };
        assert_eq!((a, b), (ta, tb), "endpoints keep their paths, documents and faces");
        assert!(distance.references().is_empty());
    }
    assert!(child.values.is_empty(), "no value is copied or imported");
    let most = child.components.iter().map(|c| c.id.0).chain(child.known_mates().map(|x| x.id().0)).max().unwrap();
    assert!(child.next_id > most && child.next_id > child.sources[0].id.0);
    assert!(pack.copies().iter().any(|(d, r)| *d == pin_id && *r == pin_ref.pinned), "the pin's display copy");

    // Steps 27 and 28: save the formed root and read it back.
    fx.doc.save(None).unwrap();
    assert!(!fx.doc.is_dirty());
    let saved = AssemblyPackage::open(&fx.root).unwrap();
    let mut refs: Vec<_> = saved.manifest().references.iter().map(|r| r.path.clone()).collect();
    refs.sort();
    assert_eq!(refs, ["modules/pack.dwasm", "parts/base.dwprt"]);
    let copies = saved.copies();
    for (doc, rev) in [(m.child, m.child_revision.clone()), (pin_id, pin_ref.pinned.clone())] {
        assert!(copies.contains(&(doc, rev)), "the root carries {doc}'s copy");
    }
    let mut reopened = AssemblyDocument::open(&fx.root, options()).unwrap();
    reopened.finish_loading();
    assert_eq!(reopened.assembly(), fx.doc.assembly());
    assert!(!reopened.is_dirty() && reopened.undo_depth() == 0);
    p0(&reopened, &a, &nested(fx.b), &nested(fx.c), &nested(fx.f));
    assert_eq!(reopened.solver().remaining_dof, 6);
    assert!(reopened.mate_measurements().iter().all(|(_, r)| r.is_ok()));

    // Step 29: the new file on its own, with B at its origin and still free.
    let mut alone = AssemblyDocument::open(&fx.pack, options()).unwrap();
    alone.finish_loading();
    let status = alone.solver();
    assert_eq!((status.state, status.remaining_dof), (SolverState::Underconstrained, 12));
    leaf(&alone, &b, [0.0, 0.0, 0.0], [-10.0, -5.0, 0.0], [10.0, 5.0, 6.0]);
    leaf(&alone, &c, [0.0, 0.0, 9.0], [-10.0, -5.0, 9.0], [10.0, 5.0, 15.0]);
    let all = alone.table().bounds(soliddraft_assembly::OccurrenceIndex::ROOT);
    close(all.min, [-10.0, -35.0, -15.0]);
    close(all.max, [60.0, 5.0, 15.0]);
    assert!(alone.mate_measurements().iter().all(|(_, r)| r.is_ok()));
    assert_eq!((sha(&fx.base), sha(&fx.pin)), (base_sha, pin_sha), "parts are never written");
}

/// What the selection may not be: refused before anything is made.
#[test]
fn malformed_selections_are_refused_whole() {
    let mut fx = flat("malformed");
    let before = (fx.doc.assembly().clone(), fx.doc.undo_depth(), fx.doc.redo_depth(), sha(&fx.root));
    let p = |c: ComponentId| path(&[c.0]);
    let cases = [
        (vec![], p(fx.b), FormCode::Invalid),
        (vec![p(fx.b), p(fx.b)], p(fx.b), FormCode::Invalid),
        (vec![p(fx.b), path(&[99])], p(fx.b), FormCode::Invalid),
        (vec![p(fx.b), p(fx.c)], p(fx.f), FormCode::Invalid),
        (vec![path(&[fx.b.0, 1])], path(&[fx.b.0, 1]), FormCode::Invalid),
        // O holds A on the root's origin: it cannot move into the new file.
        (vec![p(fx.a), p(fx.b)], p(fx.a), FormCode::Unsupported),
    ];
    for (i, (paths, origin, code)) in cases.into_iter().enumerate() {
        let refused = fx.doc.form_subassembly(&FormRequest { paths, path: fx.pack.clone(), origin }).unwrap_err();
        assert_eq!(refused.code, code, "case {i}: {refused}");
        assert!(!fx.pack.exists(), "case {i} made no file");
    }
    // A driven internal mate needs a root value the new file cannot read.
    fx.doc.drive_mate("Zbc", "gap - 9 mm", false).unwrap();
    let driven = (fx.doc.assembly().clone(), fx.doc.undo_depth());
    let refused = fx.doc.form_subassembly(&fx.request()).unwrap_err();
    assert_eq!((refused.code, refused.cause.as_deref()), (FormCode::Unsupported, Some("Zbc")));
    assert_eq!((fx.doc.assembly().clone(), fx.doc.undo_depth()), driven);
    assert!(fx.doc.undo());
    assert_eq!((fx.doc.assembly().clone(), fx.doc.undo_depth(), fx.doc.redo_depth() - 1, sha(&fx.root)), before);
    assert!(!fx.pack.exists());
}

/// An existing file is never replaced by Form.
#[test]
fn an_existing_destination_is_kept() {
    let mut fx = flat("taken");
    std::fs::create_dir_all(fx.pack.parent().unwrap()).unwrap();
    std::fs::write(&fx.pack, b"someone else's").unwrap();
    let before = fx.doc.assembly().clone();
    let refused = fx.doc.form_subassembly(&fx.request()).unwrap_err();
    assert_eq!((refused.code, refused.cause.as_deref()), (FormCode::DestinationExists, Some("pack.dwasm")));
    assert_eq!(std::fs::read(&fx.pack).unwrap(), b"someone else's");
    assert_eq!(fx.doc.assembly(), &before);
    assert_eq!(fx.doc.undo_label(), Some("Drive mate"), "nothing was recorded");
}

/// A part the root finds by id after its file was renamed keeps working once formed: the
/// new file references the part where it is, not where the root's stale path said, and
/// opens on its own with the part found.
#[test]
fn a_renamed_part_is_referenced_where_it_is_found() {
    let dir = std::env::temp_dir().join(format!("soliddraft-subassembly-{}-renamed", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (part, renamed) = (dir.join("parts/p.dwprt"), dir.join("parts/renamed.dwprt"));
    let (root, pack) = (dir.join("root.dwasm"), dir.join("modules/pack.dwasm"));
    new_block(&part, 20.0, 10.0, 6.0);
    let mut doc = AssemblyDocument::new_with("Root", options());
    doc.save(Some(&root)).unwrap();
    let p = doc.insert(reference(&root, &part), Placement::IDENTITY).unwrap();
    doc.finish_loading();
    doc.save(None).unwrap();
    std::fs::rename(&part, &renamed).unwrap();

    let mut doc = AssemblyDocument::open(&root, options()).unwrap();
    doc.finish_loading();
    let row = doc.table().find(&path(&[p.0])).unwrap();
    let freshness = &doc.store().get(doc.table().definition(row)).freshness;
    assert_eq!(*freshness, soliddraft_assembly::Freshness::Current, "the root finds the part by id");
    let request = FormRequest { paths: vec![path(&[p.0])], path: pack.clone(), origin: path(&[p.0]) };
    doc.form_subassembly(&request).unwrap();

    let child = AssemblyPackage::open(&pack).unwrap();
    assert_eq!(child.assembly().sources[0].document.path, "../parts/renamed.dwprt");
    assert_eq!(child.assembly().sources[0].document.id, Some(identify_file(&renamed).unwrap().id));
    let mut alone = AssemblyDocument::open(&pack, options()).unwrap();
    alone.finish_loading();
    let row = alone.table().find(&path(&[p.0])).unwrap();
    let freshness = &alone.store().get(alone.table().definition(row)).freshness;
    assert_eq!(*freshness, soliddraft_assembly::Freshness::Current, "the new file finds the part on its own");
    let _ = std::fs::remove_dir_all(&dir);
}
