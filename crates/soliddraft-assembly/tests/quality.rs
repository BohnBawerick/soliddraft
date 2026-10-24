//! Per-component quality: two placements of one part at two qualities are two
//! definitions with their own meshes, drawn by the same scene; the override is saved
//! outside the revision, reopens from the file's own copies, and undoes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use glam::DVec3;
use soliddraft_assembly::{
    AssemblyDocument, FileSource, KernelSource, LoadState, OccurrencePath, OpenOptions, Placement, Scene,
};
use soliddraft_document::{DocRef, identify_file};
use soliddraft_kernel_api::Kernel;
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::sketch::{P2, Sketch, solve::solve};
use soliddraft_model::{
    Command, DatumPlane, Document, EndCondition, ExtrudeFeature, ExtrudeOp, FeatureKind, ProfileSource, SketchFeature,
    SketchPlane,
};
use soliddraft_tessellation::Quality;

fn kernel() -> Arc<dyn Kernel> {
    Arc::new(CatallaxKernel::new())
}

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soliddraft-assembly-quality-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A pin 60 mm across and 40 mm long: large enough that the slider changes its mesh.
fn save_pin(path: &Path) {
    let mut doc = Document::new("Pin", kernel());
    let mut sketch = Sketch::new();
    sketch.circle_at(P2::new(0.0, 0.0), 60.0).unwrap();
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
            end: EndCondition::Blind { depth: 40.0 },
            reverse: false,
        }),
    );
    doc.save(path).unwrap();
}

fn with_kernel() -> OpenOptions {
    OpenOptions { display: Some(Arc::new(KernelSource::new(kernel()))), ..OpenOptions::default() }
}

/// Two placements of one pin. Returns the assembly and the two occurrence paths.
fn rig(dir: &Path) -> (PathBuf, OccurrencePath, OccurrencePath) {
    let part = dir.join("pin.dwprt");
    save_pin(&part);
    let root = dir.join("rig.dwasm");
    AssemblyDocument::new("Rig").save(Some(&root)).unwrap();
    let mut doc = AssemblyDocument::open(&root, with_kernel()).unwrap();
    let i = identify_file(&part).unwrap();
    let mut r = DocRef::new(i.id, i.kind, "", i.revision);
    r.set_path(&root, &part);
    let a = doc.insert(r.clone(), Placement::IDENTITY).unwrap();
    let b = doc.insert(r, Placement::translation(DVec3::new(100.0, 0.0, 0.0))).unwrap();
    doc.finish_loading();
    doc.save(None).unwrap();
    (root, a.to_string().parse().unwrap(), b.to_string().parse().unwrap())
}

fn part_definitions(doc: &AssemblyDocument) -> usize {
    doc.store().iter().filter(|(_, d)| d.kind == soliddraft_document::DocKind::Part).count()
}

fn triangles(doc: &AssemblyDocument, path: &OccurrencePath) -> usize {
    let def = doc.table().definition(doc.table().find(path).unwrap());
    doc.store().get(def).mesh.as_ref().expect("a mesh").triangle_count()
}

#[test]
fn a_component_at_its_own_quality_gets_its_own_mesh() {
    let dir = folder("override");
    let (root, a, b) = rig(&dir);
    let mut doc = AssemblyDocument::open(&root, with_kernel()).unwrap();
    doc.finish_loading();
    assert_eq!(part_definitions(&doc), 1, "two placements share one definition");
    let own = triangles(&doc, &a);
    let revision = doc.revision();

    doc.set_quality(std::slice::from_ref(&a), Some(Quality::Slider { value: 10 })).unwrap();
    doc.finish_loading();
    assert_eq!(part_definitions(&doc), 2);
    assert_eq!(doc.quality_at(&a), Some(&Quality::Slider { value: 10 }));
    assert_eq!(doc.quality_at(&b), None);
    assert!(triangles(&doc, &a) > own, "{} vs {own}", triangles(&doc, &a));
    assert_eq!(triangles(&doc, &b), own, "the other placement keeps the part's own mesh");
    assert_eq!(doc.revision(), revision, "a quality is not a revision");
    assert!(doc.is_dirty(), "but it is an unsaved change");
    assert_eq!(doc.store().counts()[LoadState::ALL.iter().position(|s| *s == LoadState::Failed).unwrap()], 0);

    // The scene draws the two definitions like any two.
    let mut scene = Scene::new(&mut doc);
    scene.update(&mut doc);
    assert_eq!(scene.draws(|_| true).len(), 2);

    // Undo and redo.
    assert!(doc.undo());
    doc.finish_loading();
    assert_eq!(doc.quality_at(&a), None);
    assert_eq!(triangles(&doc, &a), own);
    assert!(!doc.is_dirty());
    assert!(doc.redo());
    doc.finish_loading();
    assert!(triangles(&doc, &a) > own);

    // Saved, it reopens without a kernel: the fine mesh comes from the file's copy.
    doc.save(None).unwrap();
    assert!(!doc.is_dirty());
    let mut reopened = AssemblyDocument::open(&root, OpenOptions::default()).unwrap();
    reopened.finish_loading();
    assert_eq!(reopened.settings(), doc.settings());
    assert_eq!(reopened.revision(), revision);
    assert_eq!(triangles(&reopened, &a), triangles(&doc, &a));
    assert_eq!(triangles(&reopened, &b), own);

    // A source that cannot make another quality shows the part at its own instead of
    // failing, when the copies are gone.
    let bare = dir.join("bare.dwasm");
    std::fs::write(&bare, doc.assembly().to_bytes(doc.id(), None, &[]).unwrap()).unwrap();
    let mut files_only =
        AssemblyDocument::open(&bare, OpenOptions { display: Some(Arc::new(FileSource)), ..OpenOptions::default() })
            .unwrap();
    files_only.finish_loading();
    assert_eq!(files_only.settings().overrides.len(), 1);
    assert_eq!(triangles(&files_only, &a), own);
}

#[test]
fn a_quality_for_every_part_yields_to_component_overrides() {
    let dir = folder("every");
    let (root, a, b) = rig(&dir);
    let mut doc = AssemblyDocument::open(&root, with_kernel()).unwrap();
    doc.finish_loading();
    let own = triangles(&doc, &b);
    doc.set_viewport_quality(Quality::Slider { value: 10 }).unwrap();
    doc.set_quality(std::slice::from_ref(&a), Some(Quality::Slider { value: 1 })).unwrap();
    doc.finish_loading();
    assert_eq!(doc.quality_at(&b), Some(&Quality::Slider { value: 10 }));
    assert_eq!(doc.quality_at(&a), Some(&Quality::Slider { value: 1 }), "the override wins");
    assert!(triangles(&doc, &b) > own);
    assert!(triangles(&doc, &a) <= own);
    assert_eq!(part_definitions(&doc), 3, "the part's own definition stays known");

    // Back to each part's own.
    doc.set_viewport_quality(Quality::Auto).unwrap();
    doc.set_quality(std::slice::from_ref(&a), None).unwrap();
    doc.finish_loading();
    assert_eq!(doc.quality_at(&a), None);
    assert_eq!(triangles(&doc, &b), own);
    assert!(doc.settings().is_default());
    assert!(doc.set_quality(std::slice::from_ref(&a), Some(Quality::Slider { value: 0 })).is_err());
}

#[test]
fn coarse_and_fine_levels_load_on_request() {
    use soliddraft_tessellation::Level;
    let dir = folder("levels");
    let (root, a, _) = rig(&dir);
    let mut doc = AssemblyDocument::open(&root, with_kernel()).unwrap();
    doc.finish_loading();
    let def = doc.table().definition(doc.table().find(&a).unwrap());
    let standard = triangles(&doc, &a);
    let _ = doc.take_changes();

    doc.request_levels(&[(def, Level::Coarse), (def, Level::Fine), (def, Level::Standard)]);
    assert_eq!(doc.levels_pending(), 2, "the standard level is already there");
    doc.request_levels(&[(def, Level::Coarse)]);
    assert_eq!(doc.levels_pending(), 2, "asking again queues nothing");
    doc.finish_levels();
    let d = doc.store().get(def);
    let coarse = d.coarse.as_ref().expect("coarse, from the part file's display data").triangle_count();
    let fine = d.fine.as_ref().expect("fine, rebuilt on the kernel").triangle_count();
    assert!(coarse <= standard && standard < fine, "{coarse} {standard} {fine}");
    let dev = d.deviations();
    assert!(dev[0] > dev[1] && dev[1] > dev[2] && dev[2] > 0.0, "{dev:?}");
    let changes = doc.take_changes();
    assert!(changes.levels.contains(&(def, Level::Coarse)) && changes.levels.contains(&(def, Level::Fine)));

    // The coarse level is saved with the copies; reopened with no kernel it comes
    // from there, and the fine level cannot be made, once.
    doc.save(None).unwrap();
    let mut bare = AssemblyDocument::open(&root, OpenOptions::default()).unwrap();
    bare.finish_loading();
    let def = bare.table().definition(bare.table().find(&a).unwrap());
    bare.request_levels(&[(def, Level::Coarse), (def, Level::Fine)]);
    bare.finish_levels();
    assert_eq!(bare.store().get(def).coarse.as_ref().unwrap().triangle_count(), coarse);
    assert!(bare.store().get(def).fine.is_none());
    bare.request_levels(&[(def, Level::Fine)]);
    assert_eq!(bare.levels_pending(), 0, "a failed level is not asked for again");

    // A new structure starts over with new definitions.
    bare.set_quality(std::slice::from_ref(&a), Some(Quality::Slider { value: 9 })).unwrap();
    bare.finish_loading();
    let variant = bare.table().definition(bare.table().find(&a).unwrap());
    bare.request_levels(&[(variant, Level::Coarse)]);
    bare.finish_levels();
    assert!(bare.store().get(variant).coarse.is_none(), "no kernel and no copy at that quality");
}
