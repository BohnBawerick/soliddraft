//! Assemblies whose own copies are missing, opened from the display data their part
//! files keep: without a kernel through `FileSource`, and through `KernelSource`, which
//! reads the same data before it rebuilds anything.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use glam::DVec3;
use soliddraft_assembly::{
    AssemblyDocument, DisplaySource, FileSource, KernelSource, LoadState, OccurrenceIndex, OpenOptions, Placement,
};
use soliddraft_document::{DocRef, identify_file};
use soliddraft_kernel_api::Kernel;
use soliddraft_kernel_smlib::SmlibKernel;
use soliddraft_model::sketch::{P2, Sketch, solve::solve};
use soliddraft_model::{
    Command, DatumPlane, Document, EndCondition, ExtrudeFeature, ExtrudeOp, FeatureKind, ProfileSource, SketchFeature,
    SketchPlane, display,
};
use soliddraft_tessellation::{Level, Quality, Settings};

fn kernel() -> Arc<dyn Kernel> {
    Arc::new(SmlibKernel::new())
}

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soliddraft-assembly-display-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A pin 30 mm across and 40 mm long, saved at `path` with `settings`.
fn save_pin(path: &Path, settings: Settings) {
    let mut doc = Document::new("Pin", kernel());
    let mut sketch = Sketch::new();
    sketch.circle_at(P2::new(0.0, 0.0), 30.0).unwrap();
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
    doc.set_settings(settings).unwrap();
    doc.save(path).unwrap();
}

fn reference(from: &Path, to: &Path) -> DocRef {
    let i = identify_file(to).unwrap();
    let mut r = DocRef::new(i.id, i.kind, "", i.revision);
    r.set_path(from, to);
    r
}

/// Two placements of the part at `part`, in an assembly file at `root` that keeps no
/// copies of its own.
fn bare_assembly(root: &Path, part: &Path) {
    let mut doc = AssemblyDocument::new("Rig");
    doc.save(Some(root)).unwrap();
    let options = OpenOptions { display: Some(Arc::new(KernelSource::new(kernel()))), ..OpenOptions::default() };
    let mut doc = AssemblyDocument::open(root, options).unwrap();
    doc.insert(reference(root, part), Placement::IDENTITY).unwrap();
    doc.insert(reference(root, part), Placement::translation(DVec3::new(50.0, 0.0, 0.0))).unwrap();
    doc.finish_loading();
    std::fs::write(root, doc.assembly().to_bytes(doc.id(), None, &[]).unwrap()).unwrap();
}

fn open(root: &Path, display: Option<Arc<dyn DisplaySource>>) -> AssemblyDocument {
    let mut doc = AssemblyDocument::open(root, OpenOptions { display, ..OpenOptions::default() }).unwrap();
    doc.finish_loading();
    doc
}

fn state(doc: &AssemblyDocument, state: LoadState) -> usize {
    doc.store().counts()[LoadState::ALL.iter().position(|s| *s == state).unwrap()]
}

fn part_triangles(doc: &AssemblyDocument) -> usize {
    let def = doc.table().definition(doc.table().children(OccurrenceIndex::ROOT).next().unwrap());
    doc.store().get(def).mesh.as_ref().unwrap().triangle_count()
}

#[test]
fn part_files_keep_their_coarse_and_standard_levels() {
    let dir = folder("levels");
    let part = dir.join("pin.dwprt");
    save_pin(&part, Settings::default());
    let bytes = std::fs::read(&part).unwrap();
    let doc = Document::from_bytes(&bytes, kernel()).unwrap();
    let report = doc.quality_report().unwrap().unwrap();
    for level in display::SAVED {
        let (mesh, made_by) = display::read(&bytes, level).unwrap().expect("display data");
        mesh.check().unwrap();
        assert_eq!(mesh.triangle_count(), report.level(level).triangles, "{level}");
        assert!(made_by.ends_with(&doc.geometry_hash().to_string()), "{made_by}");
        assert!(made_by.contains(&format!(" {level} ")), "{made_by}");
        assert!(mesh.faces.iter().all(|f| !f.name.is_empty()), "every face has a persistent name");
        assert_eq!(mesh.faces.len(), 3, "a cylinder's side and two caps");
        assert!(!mesh.edges.is_empty());
    }
    assert!(display::read(&bytes, Level::Fine).unwrap().is_none(), "the fine level is never saved");
    assert_eq!(doc.to_bytes().unwrap(), bytes, "the same part saves to the same bytes");

    // Without display data the file is smaller and has none.
    let off = dir.join("bare.dwprt");
    save_pin(&off, Settings { save_display_data: false, ..Settings::default() });
    let bare = std::fs::read(&off).unwrap();
    assert!(bare.len() < bytes.len());
    assert!(display::read(&bare, Level::Standard).unwrap().is_none());
    let bare_doc = Document::from_bytes(&bare, kernel()).unwrap();
    assert_eq!(bare_doc.semantic_hash(), doc.semantic_hash(), "display data is not part of the revision");
}

#[test]
fn an_assembly_without_copies_opens_from_part_files_without_a_kernel() {
    let dir = folder("files");
    let part = dir.join("pin.dwprt");
    save_pin(&part, Settings::default());
    let root = dir.join("rig.dwasm");
    bare_assembly(&root, &part);

    let doc = open(&root, Some(Arc::new(FileSource)));
    assert_eq!(doc.table().counts().parts, 2);
    assert_eq!(state(&doc, LoadState::DisplayReady), 1, "one definition, from the part file");
    assert_eq!(state(&doc, LoadState::Failed), 0);
    let (standard, _) = display::read(&std::fs::read(&part).unwrap(), Level::Standard).unwrap().unwrap();
    assert_eq!(part_triangles(&doc), standard.triangle_count());

    // Without any source the copy is missing and the part fails, as before.
    let doc = open(&root, None);
    assert_eq!(state(&doc, LoadState::Failed), 1);
}

#[test]
fn the_kernel_source_reads_display_data_first_and_rebuilds_for_another_quality() {
    let dir = folder("kernel");
    let part = dir.join("pin.dwprt");
    save_pin(&part, Settings { save_display_data: false, ..Settings::default() });
    let root = dir.join("rig.dwasm");
    bare_assembly(&root, &part);

    // No display data in the part: the file source cannot show it, the kernel can.
    let doc = open(&root, Some(Arc::new(FileSource)));
    assert_eq!(state(&doc, LoadState::Failed), 1);
    let doc = open(&root, Some(Arc::new(KernelSource::new(kernel()))));
    assert_eq!(state(&doc, LoadState::DisplayReady), 1);
    let own = part_triangles(&doc);

    // Another quality for every part rebuilds them at it.
    let fine = KernelSource::with_quality(kernel(), Quality::Slider { value: 10 });
    let bytes = std::fs::read(&part).unwrap();
    let (mesh, made_by) = fine.part_mesh(&bytes).unwrap();
    assert!(mesh.triangle_count() > own, "{} vs {own}", mesh.triangle_count());
    assert!(made_by.contains(" standard "), "{made_by}");
}
