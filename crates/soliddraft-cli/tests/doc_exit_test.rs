//! The phase 0 exit test, through the `soliddraft` binary as separate processes:
//!
//! - a drawing made of a saved part pins the part's revision;
//! - editing and saving the part changes nothing in the drawing: the drawing file keeps
//!   its bytes and its views, and `doc refs` reports a newer version;
//! - `doc rebuild` moves the drawing to the new revision, with the changed value;
//! - the drawing follows the part into a subfolder, reports it missing when it is gone,
//!   and is repaired only with the same part unless a replacement is asked for;
//! - a part and a drawing from before the envelope migrate, and the drawing binds to the
//!   part's id on its first Rebuild.

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::Arc;

use serde_json::Value;
use soliddraft_document::{ContentHash, DocKind, Package};
use soliddraft_kernel_api::Kernel;
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::sketch::{P2, Sketch, solve::solve};
use soliddraft_model::{
    Command, DatumPlane, Document, EndCondition, ExtrudeFeature, ExtrudeOp, FeatureId, FeatureKind, ProfileSource,
    SketchFeature, SketchPlane,
};

/// 2026-10-14, so the title block's date is fixed.
const DATE_EPOCH: &str = "1791936000";

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let dir = std::env::temp_dir().join(format!("soliddraft-doc-exit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }

    /// Runs `soliddraft` in this folder with its own settings; returns success and stdout.
    fn run(&self, args: &[&str]) -> (bool, String, String) {
        let out = Process::new(env!("CARGO_BIN_EXE_soliddraft"))
            .args(args)
            .current_dir(&self.0)
            .env("SOURCE_DATE_EPOCH", DATE_EPOCH)
            .env("SOLIDDRAFT_CONFIG_DIR", self.0.join(".config"))
            .output()
            .expect("running soliddraft");
        (out.status.success(), String::from_utf8(out.stdout).unwrap(), String::from_utf8(out.stderr).unwrap())
    }

    fn json(&self, args: &[&str]) -> Value {
        let (ok, stdout, stderr) = self.run(args);
        assert!(ok, "soliddraft {args:?} failed: {stderr}");
        serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("{e}: {stdout}"))
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn kernel() -> Arc<dyn Kernel> {
    Arc::new(CatallaxKernel::new())
}

/// A 40 x 30 block `depth` mm thick; returns it with its extrusion's id.
fn block(depth: f64) -> (Document, FeatureId) {
    let mut doc = Document::new("Block", kernel());
    let mut sketch = Sketch::new();
    sketch.rectangle(P2::new(0.0, 0.0), 40.0, 30.0).unwrap();
    solve(&mut sketch, None, true);
    let plane = SketchPlane::Datum { datum: DatumPlane::Top, offset: 0.0 };
    let add = |doc: &mut Document, kind| {
        doc.execute(Command::AddFeature { kind, name: None, position: None, via_assistant: false })
            .unwrap()
            .created
            .unwrap()
    };
    let s = add(&mut doc, FeatureKind::Sketch(SketchFeature { plane, sketch }));
    let e = add(&mut doc, FeatureKind::Extrude(extrude(s, depth)));
    (doc, e)
}

fn extrude(sketch: FeatureId, depth: f64) -> ExtrudeFeature {
    ExtrudeFeature {
        profile: ProfileSource::Sketch { feature: sketch },
        operation: ExtrudeOp::Boss,
        end: EndCondition::Blind { depth },
        reverse: false,
    }
}

/// Opens the saved block, sets its depth and saves it.
fn set_depth(path: &Path, depth: f64) -> ContentHash {
    let mut doc = Document::open(path, kernel()).unwrap();
    let e = doc.features().iter().find(|f| matches!(f.kind, FeatureKind::Extrude(_))).unwrap().id;
    let FeatureKind::Extrude(old) = &doc.feature(e).unwrap().kind else { unreachable!() };
    let ProfileSource::Sketch { feature: s } = old.profile else { unreachable!() };
    doc.execute(Command::EditFeature { id: e, kind: FeatureKind::Extrude(extrude(s, depth)) }).unwrap();
    doc.save(path).unwrap();
    doc.semantic_hash()
}

fn drawing(path: &Path) -> soliddraft_drawing::Drawing {
    soliddraft_drawing::file::read(path).expect("the drawing opens").0
}

fn dimension_texts(d: &soliddraft_drawing::Drawing) -> Vec<String> {
    let symbols = &soliddraft_drawing::library::template("iso-a3").style_file().symbols;
    d.sheets
        .iter()
        .flat_map(|s| &s.annotations)
        .filter_map(|a| match &a.body {
            soliddraft_drawing::AnnotationKind::Dimension(dim) => {
                Some(soliddraft_drawing::measure::display_text(dim, symbols))
            }
            _ => None,
        })
        .collect()
}

fn reference(dir: &Dir, file: &str) -> Value {
    let refs = dir.json(&["doc", "refs", file, "--json"]);
    let list = refs["references"].as_array().unwrap();
    assert_eq!(list.len(), 1, "{refs:#}");
    list[0].clone()
}

#[test]
fn an_edited_part_changes_nothing_until_a_rebuild() {
    let dir = Dir::new("pinned");
    let (doc, _) = block(10.0);
    doc.save(dir.path("block.dwprt")).unwrap();
    let first = doc.semantic_hash();

    // The drawing pins the part's revision.
    let made = dir.json(&["draw", "block.dwprt", "--out", "block.dwdrw", "--json"]);
    assert!(made["dimensions_placed"].as_u64().unwrap() > 0, "{made:#}");
    let d = drawing(&dir.path("block.dwdrw"));
    assert_eq!(d.model.id, Some(doc.id()), "the reference holds the part's id, not its name");
    assert_eq!(d.model.kind, DocKind::Part);
    assert_eq!(d.model.path, "block.dwprt");
    assert_eq!(d.model.pinned, first);
    assert!(d.views_match_pin());
    assert!(dimension_texts(&d).iter().any(|t| t == "10"), "{:?}", dimension_texts(&d));
    let r = reference(&dir, "block.dwdrw");
    assert_eq!(r["status"], "current", "{r:#}");
    let info = dir.json(&["doc", "info", "block.dwprt", "--json"]);
    assert_eq!(info["id"], doc.id().to_string());
    assert_eq!(info["revision"], first.as_str());

    // The part gets thicker. The drawing file is untouched, still shows 10, and says a
    // newer version is available.
    let before = std::fs::read(dir.path("block.dwdrw")).unwrap();
    let second = set_depth(&dir.path("block.dwprt"), 12.0);
    assert_ne!(second, first);
    let r = reference(&dir, "block.dwdrw");
    assert_eq!(r["status"], "newer_available", "{r:#}");
    assert_eq!(r["now"], second.as_str());
    assert_eq!(r["reference"]["pinned"], first.as_str());
    assert_eq!(std::fs::read(dir.path("block.dwdrw")).unwrap(), before, "nothing wrote the drawing");
    let d = drawing(&dir.path("block.dwdrw"));
    assert_eq!(d.model.pinned, first);
    assert!(d.views_match_pin(), "the views still show the pinned revision");
    assert!(dimension_texts(&d).iter().any(|t| t == "10"));

    // Rebuild moves it: the value changes and the reference is current again.
    let rebuilt = dir.json(&["doc", "rebuild", "block.dwdrw", "--json"]);
    let changed = rebuilt["changed"].as_array().unwrap();
    assert_eq!(changed.len(), 1, "{rebuilt:#}");
    assert_eq!((changed[0]["previous"].as_str(), changed[0]["now"].as_str()), (Some("10"), Some("12")));
    assert_eq!(rebuilt["digest_before"], first.as_str());
    assert_eq!(rebuilt["digest_after"], second.as_str());
    let d = drawing(&dir.path("block.dwdrw"));
    assert_eq!(d.model.pinned, second);
    assert!(d.views_match_pin());
    assert_eq!(reference(&dir, "block.dwdrw")["status"], "current");

    // The drawing keeps its id through the Rebuild.
    let id_of = |p: &Path| soliddraft_document::identify_file(p).unwrap().id;
    let drawing_id = id_of(&dir.path("block.dwdrw"));
    assert!(Package::from_bytes(&before).unwrap().manifest().is_some_and(|m| m.id == drawing_id));
}

#[test]
fn a_reference_follows_a_moved_part_and_is_repaired_only_on_purpose() {
    let dir = Dir::new("moved");
    let (doc, _) = block(10.0);
    doc.save(dir.path("block.dwprt")).unwrap();
    dir.json(&["draw", "block.dwprt", "--out", "block.dwdrw", "--json"]);

    // Moved into a subfolder: found by id, and the Rebuild stores the new path.
    std::fs::create_dir_all(dir.path("parts")).unwrap();
    std::fs::rename(dir.path("block.dwprt"), dir.path("parts/plate.dwprt")).unwrap();
    let r = reference(&dir, "block.dwdrw");
    assert_eq!((r["status"].as_str(), r["via"].as_str()), (Some("current"), Some("index")), "{r:#}");
    assert_eq!(r["new_path"], "parts/plate.dwprt");
    let rebuilt = dir.json(&["doc", "rebuild", "block.dwdrw", "--json"]);
    assert_eq!(rebuilt["model"], "parts/plate.dwprt");
    assert_eq!(rebuilt["changed"].as_array().unwrap().len(), 0, "the same part, nothing changed");
    assert_eq!(
        dir.json(&["doc", "where-used", "parts/plate.dwprt", "--search", ".", "--json"])["used_by"][0]["file"]
            .as_str()
            .map(|f| f.ends_with("block.dwdrw")),
        Some(true)
    );

    // Gone, with a different part under its old name: missing, never taken.
    std::fs::rename(dir.path("parts/plate.dwprt"), dir.path("kept.bin")).unwrap();
    let (other, _) = block(20.0);
    other.save(dir.path("parts/plate.dwprt")).unwrap();
    let (ok, stdout, _) = dir.run(&["doc", "refs", "block.dwdrw", "--json"]);
    assert!(!ok, "a missing reference fails doc refs");
    let refs: Value = serde_json::from_str(stdout.trim()).unwrap();
    let r = &refs["references"][0];
    assert_eq!(r["status"], "missing", "{r:#}");
    assert!(r["problem"].as_str().unwrap().contains("different document"), "{r:#}");
    assert_eq!(r["candidates"][0]["id"], other.id().to_string());
    let (ok, _, stderr) = dir.run(&["doc", "rebuild", "block.dwdrw"]);
    assert!(!ok && stderr.contains("missing"), "{stderr}");

    // Repair with the other part is refused unless asked for.
    let (ok, _, stderr) = dir.run(&["doc", "repair", "block.dwdrw", "--to", "parts/plate.dwprt"]);
    assert!(!ok && stderr.contains("different document"), "{stderr}");
    // The original part, restored somewhere else, is accepted.
    std::fs::rename(dir.path("kept.bin"), dir.path("restored.dwprt")).unwrap();
    let repaired = dir.json(&["doc", "repair", "block.dwdrw", "--to", "restored.dwprt", "--json"]);
    assert_eq!(repaired["status"], "current", "{repaired:#}");
    assert_eq!(reference(&dir, "block.dwdrw")["status"], "current");
    // Replacing it with the other part is a choice; the drawing then shows its old
    // revision until a Rebuild.
    let replaced = dir.json(&["doc", "repair", "block.dwdrw", "--to", "parts/plate.dwprt", "--replace", "--json"]);
    assert_eq!(replaced["status"], "newer_available", "{replaced:#}");
    assert_eq!(replaced["reference"]["id"], other.id().to_string());
}

#[test]
fn files_from_before_the_envelope_migrate_and_bind_on_rebuild() {
    let dir = Dir::new("migrate");
    let (doc, _) = block(10.0);
    doc.save(dir.path("new.dwprt")).unwrap();
    dir.json(&["draw", "new.dwprt", "--out", "block.dwdrw", "--json"]);

    // Rewrite both files the way phase D2 wrote them.
    let part_json = doc.part_json().replace("soliddraft-part/2", "soliddraft-part/1");
    write_zip(&dir.path("block.dwpart"), &[("recipe.json".into(), part_json), ("meta.json".into(), "{}".into())]);
    std::fs::remove_file(dir.path("new.dwprt")).unwrap();
    let bytes = std::fs::read(dir.path("block.dwdrw")).unwrap();
    let mut p = Package::from_bytes(&bytes).unwrap();
    let mut truth: Value = serde_json::from_str(&p.read_text("drawing.json").unwrap().unwrap()).unwrap();
    truth["model"] = serde_json::json!({
        "document_id": "Block", "path": "block.dwpart", "digest_at_update": "0123456789abcdef", "revision": 3
    });
    let meta = r#"{"format":"soliddraft-drawing/1","app":"soliddraft 0.1.0","kernel":"x","units":"millimetre"}"#;
    let mut entries = vec![
        ("meta.json".to_owned(), meta.to_owned()),
        ("drawing.json".to_owned(), serde_json::to_string_pretty(&truth).unwrap()),
    ];
    for name in p.names().into_iter().filter(|n| n.starts_with("cache/views/")) {
        let text = p.read_text(&name).unwrap().unwrap();
        entries.push((name, text));
    }
    write_zip(&dir.path("block.dwdrawing"), &entries);
    std::fs::remove_file(dir.path("block.dwdrw")).unwrap();

    // The old drawing opens, unbound and older than its part.
    let r = reference(&dir, "block.dwdrawing");
    assert_eq!(r["status"], "newer_available", "{r:#}");
    assert!(r["reference"].get("id").is_none(), "the old id was the part's name");

    // Migration writes .dwprt and .dwdrw beside the old files and binds the reference.
    let migrated = dir.json(&["doc", "migrate", "block.dwpart", "block.dwdrawing", "--json"]);
    assert_eq!(migrated[0]["written"].as_str().map(|p| p.ends_with("block.dwprt")), Some(true), "{migrated:#}");
    assert_eq!(migrated[1]["written"].as_str().map(|p| p.ends_with("block.dwdrw")), Some(true), "{migrated:#}");
    let part_id = soliddraft_document::identify_file(&dir.path("block.dwprt")).unwrap().id;
    assert!(part_id.is_legacy());
    assert_eq!(part_id, soliddraft_document::identify_file(&dir.path("block.dwpart")).unwrap().id);
    let d = drawing(&dir.path("block.dwdrw"));
    assert_eq!(d.model.id, Some(part_id), "migration bound the reference");
    assert!(!d.views_match_pin());

    // The Rebuild follows the reference to the .dwprt and pins it.
    let rebuilt = dir.json(&["doc", "rebuild", "block.dwdrw", "--json"]);
    assert_eq!(rebuilt["model"], "block.dwprt", "{rebuilt:#}");
    let r = reference(&dir, "block.dwdrw");
    assert_eq!(r["status"], "current", "{r:#}");
    assert!(drawing(&dir.path("block.dwdrw")).views_match_pin());

    // A Rebuild of an old-named drawing writes the current name beside it.
    std::fs::copy(dir.path("block.dwdrawing"), dir.path("old.dwdrawing")).unwrap();
    let rebuilt = dir.json(&["draw", "--update", "old.dwdrawing", "--json"]);
    assert_eq!(rebuilt["drawing"].as_str().map(|p| p.ends_with("old.dwdrw")), Some(true), "{rebuilt:#}");
}

fn write_zip(path: &Path, entries: &[(String, String)]) {
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, text) in entries {
        zip.start_file(name.as_str(), zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(text.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}
