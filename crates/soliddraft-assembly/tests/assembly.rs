//! Assemblies of real parts: insert, pattern, save, reopen from the display copies,
//! override occurrences inside sub-assemblies, undo, and Rebuild after a part changes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use glam::DVec3;
use soliddraft_assembly::{
    AssemblyDocument, Freshness, KernelSource, LoadState, OccurrencePath, OpenOptions, PatternRule, Placement,
    RefChange,
};
use soliddraft_document::{DocKind, DocRef, identify_file};
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
    let dir = std::env::temp_dir().join(format!("soliddraft-assembly-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A block `w` x 20 x `h` centred on the origin in plan, saved at `path`, keeping its id when the file exists.
fn save_block(path: &Path, w: f64, h: f64) {
    let mut doc = match path.exists() {
        true => Document::open(path, kernel()).unwrap(),
        false => Document::new("Block", kernel()),
    };
    let ids: Vec<_> = doc.features().iter().map(|f| f.id).collect();
    for id in ids.into_iter().rev() {
        doc.execute(Command::DeleteFeature { id }).unwrap();
    }
    let mut sketch = Sketch::new();
    sketch.rectangle(P2::new(0.0, 0.0), w, 20.0).unwrap();
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

fn reference(from: &Path, to: &Path) -> DocRef {
    let i = identify_file(to).unwrap();
    let mut r = DocRef::new(i.id, i.kind, "", i.revision);
    r.set_path(from, to);
    r
}

fn with_kernel() -> OpenOptions {
    OpenOptions {
        display: Some(Arc::new(KernelSource::new(kernel()))),
        check_freshness: true,
        ..OpenOptions::default()
    }
}

#[test]
fn assemblies_open_from_their_copies_and_rebuild_when_parts_change() {
    let dir = folder("rebuild");
    let part = dir.join("parts/block.dwprt");
    std::fs::create_dir_all(part.parent().unwrap()).unwrap();
    save_block(&part, 40.0, 10.0);

    // Build: two blocks and a pattern of the second; meshes come from the kernel.
    let root = dir.join("rig.dwasm");
    let mut doc = AssemblyDocument::new("Rig");
    doc.save(Some(&root)).unwrap();
    let mut doc = AssemblyDocument::open(&root, with_kernel()).unwrap();
    let a = doc.insert(reference(&root, &part), Placement::IDENTITY).unwrap();
    let b = doc.insert(reference(&root, &part), Placement::translation(DVec3::new(100.0, 0.0, 0.0))).unwrap();
    doc.add_pattern(vec![b], PatternRule::Linear { direction: [0.0, 1.0, 0.0], spacing: 50.0, count: 3, second: None })
        .unwrap();
    doc.finish_loading();
    assert_eq!(doc.assembly().sources.len(), 1, "two placements of one part share a source");
    assert_eq!(doc.table().counts().parts, 4);
    assert_eq!(doc.store().counts()[2], 1, "one display-ready definition");
    let member: OccurrencePath = format!("{b}:{}.2", doc.assembly().patterns[0].id).parse().unwrap();
    let row = doc.table().find(&member).unwrap();
    assert_eq!(doc.table().world(row).translation, DVec3::new(100.0, 100.0, 0.0));
    let bounds = doc.table().bounds(row);
    assert!((bounds.max.x - 120.0).abs() < 1e-3 && (bounds.max.z - 10.0).abs() < 1e-3, "{bounds:?}");
    doc.save(None).unwrap();

    // Reopen with no kernel at all: everything shows from the file's own copies.
    let mut doc =
        AssemblyDocument::open(&root, OpenOptions { check_freshness: true, ..OpenOptions::default() }).unwrap();
    doc.finish_loading();
    let d = doc.store().iter().find(|(_, d)| d.kind == DocKind::Part).unwrap().1;
    assert_eq!(d.state, LoadState::DisplayReady);
    assert_eq!(d.freshness, Freshness::Current);

    // The part changes: the assembly still shows the pinned revision and says so.
    save_block(&part, 60.0, 10.0);
    let mut doc = AssemblyDocument::open(&root, with_kernel()).unwrap();
    doc.finish_loading();
    let row = doc.table().find(&member).unwrap();
    assert!((doc.table().bounds(row).max.x - 120.0).abs() < 1e-3, "still the pinned revision");
    let d = doc.store().iter().find(|(_, d)| d.kind == DocKind::Part).unwrap().1;
    assert!(matches!(d.freshness, Freshness::NewerAvailable { .. }), "{:?}", d.freshness);

    // Rebuild moves it on, and undo takes it back.
    let report = doc.rebuild();
    assert_eq!(report.updated(), 1);
    assert_eq!(report.sources[0].status, RefChange::Updated);
    doc.finish_loading();
    let row = doc.table().find(&member).unwrap();
    assert!((doc.table().bounds(row).max.x - 130.0).abs() < 1e-3, "{:?}", doc.table().bounds(row));
    doc.save(None).unwrap();
    assert!(doc.undo());
    doc.finish_loading();
    let row = doc.table().find(&member).unwrap();
    assert!((doc.table().bounds(row).max.x - 120.0).abs() < 1e-3);
    let _ = a;
}

#[test]
fn overrides_change_occurrences_inside_sub_assemblies() {
    let dir = folder("overrides");
    let part = dir.join("block.dwprt");
    save_block(&part, 10.0, 10.0);
    let sub = dir.join("sub.dwasm");
    let mut s = AssemblyDocument::new("Sub");
    s.save(Some(&sub)).unwrap();
    let mut s = AssemblyDocument::open(&sub, with_kernel()).unwrap();
    let inner = s.insert(reference(&sub, &part), Placement::translation(DVec3::new(0.0, 0.0, 5.0))).unwrap();
    s.finish_loading();
    s.save(None).unwrap();

    let root = dir.join("top.dwasm");
    let mut t = AssemblyDocument::new("Top");
    t.save(Some(&root)).unwrap();
    let mut t = AssemblyDocument::open(&root, with_kernel()).unwrap();
    let left = t.insert(reference(&root, &sub), Placement::IDENTITY).unwrap();
    let right = t.insert(reference(&root, &sub), Placement::translation(DVec3::new(50.0, 0.0, 0.0))).unwrap();
    t.finish_loading();
    assert_eq!(t.table().counts().parts, 2);
    assert_eq!(t.table().counts().assemblies, 3, "the root and two sub-assemblies");

    // Hide and move one part of one sub-assembly; the other is untouched.
    let p: OccurrencePath = format!("{right}/{inner}").parse().unwrap();
    t.set_hidden(std::slice::from_ref(&p), true).unwrap();
    t.move_to(&[(p.clone(), Placement::translation(DVec3::new(0.0, 7.0, 0.0)))]).unwrap();
    let row = t.table().find(&p).unwrap();
    assert_eq!(t.table().world(row).translation, DVec3::new(50.0, 7.0, 0.0));
    assert_eq!(t.table().counts().hidden, 1);
    assert!(t.assembly().override_at(&p).is_some(), "the change lives in the top assembly");
    let other: OccurrencePath = format!("{left}/{inner}").parse().unwrap();
    assert_eq!(t.table().world(t.table().find(&other).unwrap()).translation, DVec3::new(0.0, 0.0, 5.0));

    // Moving the sub-assembly carries its part along.
    t.move_by(&[format!("{right}").parse().unwrap()], &Placement::translation(DVec3::new(0.0, 0.0, 100.0))).unwrap();
    assert_eq!(t.table().world(row).translation, DVec3::new(50.0, 7.0, 100.0));
    assert!(t.undo());
    assert_eq!(t.table().world(row).translation, DVec3::new(50.0, 7.0, 0.0));
    assert!(t.redo());
    assert_eq!(t.table().world(row).translation, DVec3::new(50.0, 7.0, 100.0));

    // Saved and reopened, the overrides apply again, and the file opens on its own.
    t.save(None).unwrap();
    let mut back = AssemblyDocument::open(&root, OpenOptions::default()).unwrap();
    back.finish_loading();
    let row = back.table().find(&p).unwrap();
    assert_eq!(back.table().world(row).translation, DVec3::new(50.0, 7.0, 100.0));
    assert_eq!(back.table().counts().hidden, 1);
    assert_eq!(back.store().counts()[2], 1, "the part's copy came along with the sub-assembly's");
    assert_eq!(back.store().counts()[5], 0, "nothing failed");

    // Undo the hide: back to visible, and the override goes away with the move undone.
    let mut t = AssemblyDocument::open(&root, OpenOptions::default()).unwrap();
    t.set_hidden(std::slice::from_ref(&p), false).unwrap();
    assert_eq!(t.table().counts().hidden, 0);
    t.undo();
    assert_eq!(t.table().counts().hidden, 1);
}

#[test]
fn deleting_and_suppressing_keep_the_rest_in_place() {
    let dir = folder("delete");
    let part = dir.join("block.dwprt");
    save_block(&part, 10.0, 10.0);
    let root = dir.join("a.dwasm");
    let mut doc = AssemblyDocument::new("A");
    doc.save(Some(&root)).unwrap();
    let mut doc = AssemblyDocument::open(&root, with_kernel()).unwrap();
    let ids: Vec<_> = (0..5)
        .map(|i| {
            doc.insert(reference(&root, &part), Placement::translation(DVec3::new(i as f64 * 20.0, 0.0, 0.0))).unwrap()
        })
        .collect();
    doc.finish_loading();
    let last: OccurrencePath = format!("{}", ids[4]).parse().unwrap();
    let before = doc.table().world(doc.table().find(&last).unwrap());
    doc.delete(&ids[1..3]).unwrap();
    assert_eq!(doc.table().counts().parts, 3);
    assert_eq!(doc.table().world(doc.table().find(&last).unwrap()), before);
    doc.set_suppressed(std::slice::from_ref(&last), true).unwrap();
    assert_eq!(doc.table().counts().drawn, 2);
    assert!(doc.undo());
    assert_eq!(doc.table().counts().drawn, 3);
    assert!(doc.undo());
    doc.finish_loading();
    assert_eq!(doc.table().counts().parts, 5, "undo brings the deleted components back");
    assert!(doc.is_dirty());
}

#[test]
fn references_resolve_from_the_file_that_holds_them() {
    // The sub-assembly and its part live outside the root's folder: the part's path is
    // relative to the sub-assembly, so it must be followed from there.
    let dir = folder("nested-paths");
    let part = dir.join("lib/parts/block.dwprt");
    std::fs::create_dir_all(part.parent().unwrap()).unwrap();
    save_block(&part, 10.0, 10.0);
    let sub = dir.join("lib/sub.dwasm");
    let mut s = AssemblyDocument::new("Sub");
    s.save(Some(&sub)).unwrap();
    let mut s = AssemblyDocument::open(&sub, with_kernel()).unwrap();
    s.insert(reference(&sub, &part), Placement::IDENTITY).unwrap();
    s.finish_loading();
    s.save(None).unwrap();
    let root = dir.join("top/top.dwasm");
    std::fs::create_dir_all(root.parent().unwrap()).unwrap();
    let mut t = AssemblyDocument::new("Top");
    t.save(Some(&root)).unwrap();
    let mut t = AssemblyDocument::open(&root, with_kernel()).unwrap();
    t.insert(reference(&root, &sub), Placement::IDENTITY).unwrap();
    t.finish_loading();
    t.save(None).unwrap();

    let mut back =
        AssemblyDocument::open(&root, OpenOptions { check_freshness: true, ..OpenOptions::default() }).unwrap();
    back.finish_loading();
    for (_, d) in back.store().iter().filter(|(i, _)| *i != back.root()) {
        assert_eq!(d.freshness, Freshness::Current, "{}", d.label);
    }
}

#[test]
fn an_update_is_refused_whole_while_one_part_is_missing() {
    let dir = folder("atomic-update");
    let elsewhere = folder("missing-update-source");
    let a = dir.join("a.dwprt");
    let b = dir.join("b.dwprt");
    let root = dir.join("rig.dwasm");
    save_block(&a, 40.0, 10.0);
    save_block(&b, 10.0, 10.0);
    let mut doc = AssemblyDocument::new_with("Rig", with_kernel());
    doc.save(Some(&root)).unwrap();
    let component = doc.insert(reference(&root, &a), Placement::IDENTITY).unwrap();
    doc.insert(reference(&root, &b), Placement::translation(DVec3::new(50.0, 0.0, 0.0))).unwrap();
    doc.finish_loading();
    doc.save(None).unwrap();
    let revision = doc.revision();
    let pins: Vec<_> = doc.assembly().sources.iter().map(|s| s.document.pinned.clone()).collect();
    let undo = doc.undo_label().map(str::to_owned);
    save_block(&a, 60.0, 10.0);
    std::fs::rename(&b, elsewhere.join("b.dwprt")).unwrap();
    let report = doc.rebuild();
    assert_eq!(report.missing(), 1, "{report:?}");
    assert!(report.refusal.as_ref().is_some_and(|r| r.contains("b.dwprt")), "{report:?}");
    assert!(report.sources.iter().any(|s| s.label == "b" && s.status == RefChange::Missing));
    assert_eq!(doc.revision(), revision, "a missing source must refuse the whole Update");
    assert_eq!(doc.assembly().sources.iter().map(|s| s.document.pinned.clone()).collect::<Vec<_>>(), pins);
    assert_eq!(doc.undo_label(), undo.as_deref());
    assert!(!doc.is_dirty());
    doc.finish_loading();
    let path = OccurrencePath::root().child(soliddraft_assembly::Step::component(component));
    let row = doc.table().find(&path).unwrap();
    assert!((doc.table().bounds(row).max.x - 20.0).abs() < 1e-3);
    std::fs::rename(elsewhere.join("b.dwprt"), &b).unwrap();
    let report = doc.rebuild();
    assert_eq!(report.updated(), 1);
    assert_eq!(report.sources.iter().filter(|s| s.status == RefChange::Unchanged).count(), 1);
    doc.finish_loading();
    let row = doc.table().find(&path).unwrap();
    assert!((doc.table().bounds(row).max.x - 30.0).abs() < 1e-3);
    assert_eq!(doc.undo_label(), Some("Update"));
}

#[test]
fn an_update_whose_new_revision_cannot_be_shown_keeps_the_old_one() {
    let dir = folder("unshowable-update");
    let a = dir.join("a.dwprt");
    let root = dir.join("rig.dwasm");
    save_block(&a, 40.0, 10.0);
    let mut doc = AssemblyDocument::new_with("Rig", with_kernel());
    doc.save(Some(&root)).unwrap();
    let component = doc.insert(reference(&root, &a), Placement::IDENTITY).unwrap();
    doc.finish_loading();
    doc.save(None).unwrap();
    let mut doc =
        AssemblyDocument::open(&root, OpenOptions { check_freshness: true, ..OpenOptions::default() }).unwrap();
    doc.finish_loading();
    let revision = doc.revision();
    let pin = doc.assembly().sources[0].document.pinned.clone();
    let undo = doc.undo_label().map(str::to_owned);
    save_block(&a, 60.0, 10.0);
    let part = Document::open(&a, kernel()).unwrap();
    let bytes =
        part.to_bytes_with(&soliddraft_model::file::Extras { no_display_data: true, ..Default::default() }).unwrap();
    std::fs::write(&a, bytes).unwrap();
    let report = doc.rebuild();
    assert!(
        report.refusal.as_ref().is_some_and(|r| r.contains("a.dwprt") && r.contains("cannot be shown")),
        "{report:?}"
    );
    assert_eq!(doc.revision(), revision, "an unshowable revision must refuse Update: {report:?}");
    assert_eq!(doc.assembly().sources[0].document.pinned, pin);
    assert_eq!(doc.undo_label(), undo.as_deref());
    assert!(!doc.is_dirty());
    let path = OccurrencePath::root().child(soliddraft_assembly::Step::component(component));
    let row = doc.table().find(&path).unwrap();
    assert_eq!(doc.store().get(doc.table().definition(row)).state, LoadState::DisplayReady);
    assert!((doc.table().bounds(row).max.x - 20.0).abs() < 1e-3);
}
