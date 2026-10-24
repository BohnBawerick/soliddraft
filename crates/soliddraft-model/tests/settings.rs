//! A part's display and accuracy settings: saved in `settings.json`, outside the part's
//! truth, so changing them never changes its revision.

use std::sync::Arc;

use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::sketch::{P2, Sketch, SolveStatus, solve::solve};
use soliddraft_model::{
    Command, DatumPlane, Document, EndCondition, ExtrudeFeature, ExtrudeOp, FeatureKind, ProfileSource, SketchFeature,
    SketchPlane,
};
use soliddraft_tessellation::{ExportTolerance, Level, Quality, Settings};

/// A cylinder of radius 10 and height 20, so the quality setting shows.
fn cylinder() -> Document {
    let mut doc = Document::new("Pin", Arc::new(CatallaxKernel::new()));
    let mut s = Sketch::new();
    s.circle_at(P2::new(0.0, 0.0), 20.0).unwrap();
    solve(&mut s, None, true);
    assert_eq!(s.status, SolveStatus::FullyDefined);
    let plane = SketchPlane::Datum { datum: DatumPlane::Top, offset: 0.0 };
    let kind = FeatureKind::Sketch(SketchFeature { plane, sketch: s });
    let sketch = doc.execute(Command::AddFeature { kind, name: None, position: None, via_assistant: false }).unwrap();
    let kind = FeatureKind::Extrude(ExtrudeFeature {
        profile: ProfileSource::Sketch { feature: sketch.created.unwrap() },
        operation: ExtrudeOp::Boss,
        end: EndCondition::Blind { depth: 20.0 },
        reverse: false,
    });
    doc.execute(Command::AddFeature { kind, name: None, position: None, via_assistant: false }).unwrap();
    doc
}

#[test]
fn a_recovery_snapshot_releases_bodies_and_restores_its_captured_revision_and_settings() {
    let mut doc = cylinder();
    let settings = Settings { viewport: Quality::Slider { value: 9 }, ..Settings::default() };
    doc.set_settings(settings.clone()).unwrap();
    let revision = doc.semantic_hash();
    let geometry = doc.geometry_hash();
    let id = doc.id();
    let body = doc.body().unwrap();
    let kernel = doc.kernel().clone();
    let snapshot = doc.snapshot();
    drop(doc);
    assert!(matches!(kernel.topology(body), Err(soliddraft_kernel_api::KernelError::UnknownBody(_))));

    // Encoding must need only the copied recipe, even after every old body is gone.
    let (manifest, bytes) = snapshot.encode().unwrap();
    assert_eq!(manifest.id, id);
    assert_eq!(manifest.semantic_hash, revision);
    assert_eq!(manifest.geometry_hash, Some(geometry));
    let restored = Document::from_bytes(&bytes, kernel).unwrap();
    assert_eq!(restored.id(), id);
    assert_eq!(restored.semantic_hash(), revision);
    assert_eq!(restored.settings(), &settings);
    let volume = restored.kernel().mass_properties(restored.body().unwrap()).unwrap().volume;
    assert!((volume - 2000.0 * std::f64::consts::PI).abs() < 1e-6, "{volume}");
}

#[test]
fn settings_change_neither_revision_nor_geometry() {
    let mut doc = cylinder();
    let kernel = doc.kernel().clone();
    let (revision, geometry) = (doc.semantic_hash(), doc.geometry_hash());
    let plain = doc.to_bytes().unwrap();
    let names = |bytes: &[u8]| {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
        (0..zip.len()).map(|i| zip.by_index(i).unwrap().name().unwrap().into_owned()).collect::<Vec<_>>()
    };
    assert!(!names(&plain).contains(&"settings.json".to_owned()), "default settings are not written");

    let custom = Settings {
        viewport: Quality::Slider { value: 9 },
        export: ExportTolerance { chord_height: 0.002, angle_deg: 5.0 },
        save_display_data: false,
        ..Settings::default()
    };
    doc.set_settings(custom.clone()).unwrap();
    assert_eq!((doc.semantic_hash(), doc.geometry_hash()), (revision.clone(), geometry));
    let bytes = doc.to_bytes().unwrap();
    assert!(names(&bytes).contains(&"settings.json".to_owned()));
    assert_eq!(doc.manifest().semantic_hash, revision);
    assert!(doc.manifest().sections.iter().any(|s| s.name == "settings.json" && !s.required));

    let reopened = Document::from_bytes(&bytes, kernel.clone()).unwrap();
    assert_eq!(reopened.settings(), &custom);
    assert_eq!(reopened.to_bytes().unwrap(), bytes, "saving an opened file reproduces it");
    assert_eq!(reopened.semantic_hash(), revision);

    // Back to the defaults, the file is what it was.
    doc.set_settings(Settings::default()).unwrap();
    assert_eq!(doc.to_bytes().unwrap(), plain);

    // Values out of range are refused and change nothing.
    let bad = Settings { viewport: Quality::Slider { value: 0 }, ..Settings::default() };
    assert!(doc.set_settings(bad).is_err());
    assert!(doc.settings().is_default());
}

#[test]
fn unreadable_settings_open_with_the_defaults() {
    let mut doc = cylinder();
    doc.set_settings(Settings { viewport: Quality::Slider { value: 2 }, ..Settings::default() }).unwrap();
    let bytes = doc.to_bytes().unwrap();
    // Rewrite settings.json as a format from the future.
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).unwrap();
        let name = entry.name().unwrap().into_owned();
        let mut data = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut data).unwrap();
        if name == "settings.json" {
            data = br#"{"format": "soliddraft-settings/9", "hologram": true}"#.to_vec();
        }
        out.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        std::io::Write::write_all(&mut out, &data).unwrap();
    }
    let bytes = out.finish().unwrap().into_inner();
    let reopened = Document::from_bytes(&bytes, doc.kernel().clone()).unwrap();
    assert!(reopened.settings().is_default());
    assert_eq!(reopened.semantic_hash(), doc.semantic_hash());
}

#[test]
fn the_quality_report_follows_the_setting() {
    let mut doc = cylinder();
    let auto = doc.quality_report().unwrap().unwrap();
    // A 20 x 20 x 20 box around the cylinder: diagonal 20 * sqrt(3).
    let chord = doc.kernel().topology(doc.body().unwrap()).unwrap().display_chord_height.unwrap();
    assert!((auto.diagonal_mm - 20.0 * 3f64.sqrt()).abs() <= 2.0 * 3f64.sqrt() * chord + 1e-6, "{}", auto.diagonal_mm);
    let standard = auto.level(Level::Standard);
    assert!((standard.deviation_mm.unwrap() - 0.002 * auto.diagonal_mm).abs() < 1e-12);
    assert!(auto.level(Level::Fine).triangles > standard.triangles);

    doc.set_settings(Settings { viewport: Quality::Slider { value: 10 }, ..Settings::default() }).unwrap();
    let fine = doc.quality_report().unwrap().unwrap();
    assert!(fine.level(Level::Standard).triangles > standard.triangles);
    assert!(fine.level(Level::Standard).deviation_mm.unwrap() < standard.deviation_mm.unwrap());

    let empty = Document::new("Empty", doc.kernel().clone());
    assert!(empty.quality_report().is_none());
}
