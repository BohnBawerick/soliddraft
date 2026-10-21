//! The drawing file: a document envelope (see `soliddraft-document`) with the drawing,
//! its cached line work and a thumbnail.
//!
//! | Entry | Section | Content |
//! |---|---|---|
//! | `manifest.json` | | id, kind, format, writer, kernel, hashes, the model reference |
//! | `drawing.json` | required | the [`Drawing`] without its views' line work |
//! | `cache/views/<id>.json` | optional | each view's [`ViewGeometry`] |
//! | `thumbnail.png` | optional | the first sheet's line work, 512 pixels wide |
//!
//! The line work is a cache, and also what shows the part at the revision the drawing
//! is pinned at: a drawing opens showing it, whatever the part has become since, until
//! a Rebuild. A drawing whose cache entry is missing opens with that view empty and out
//! of date, and the next update regenerates it. Entries carry a fixed timestamp, so the
//! same drawing always gives the same bytes. Writing is atomic.
//!
//! Drawings from before the envelope (`soliddraft-drawing/1`: `meta.json` instead of a
//! manifest, and a model reference holding the part's display name and an older
//! digest) still open. They get an id derived from their content, and their model
//! reference becomes unbound and older than any part revision, so the first Rebuild
//! binds it to the part's id and regenerates the views.

use std::io::Cursor;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use soliddraft_document::manifest::{self, Entry, Manifest, Package, Section};
use soliddraft_document::{DocKind, DocRef, DocumentId, EnvelopeError, kind::format_major};

use crate::view::ViewGeometry;
use crate::{Drawing, DrawingError, Units, display, export};

/// The format of `drawing.json` this version writes.
pub const FORMAT: &str = "soliddraft-drawing/2";
/// The format of drawing files from before the envelope.
pub const LEGACY_FORMAT: &str = "soliddraft-drawing/1";

/// The extension drawings are saved under, without the dot.
pub const EXTENSION: &str = soliddraft_document::DRAWING_EXTENSION;
/// The drawing extension before the envelope. Such files open, and are saved under
/// [`EXTENSION`].
pub const LEGACY_EXTENSION: &str = soliddraft_document::LEGACY_DRAWING_EXTENSION;

/// Thumbnail width in pixels.
const THUMBNAIL_PX: u32 = 512;

const DRAWING: &str = "drawing.json";
const VIEWS: &str = "cache/views/";
const THUMBNAIL: &str = "thumbnail.png";

/// What the envelope says about a drawing file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileMeta {
    /// The drawing's id. Kept through renames and moves; Save as copy mints a new one.
    pub id: DocumentId,
    /// The format of `drawing.json`: [`FORMAT`] for files this version writes.
    pub format: String,
    /// The app that wrote the file, such as "soliddraft 0.1.0".
    pub app: String,
    /// The kernel backend the line work came from, such as "smlib c9c5979".
    pub kernel: String,
    /// Units of dimensions.
    pub units: Units,
}

impl FileMeta {
    /// Metadata for a new drawing this version writes: a new id.
    pub fn new(kernel: &str, units: Units) -> FileMeta {
        FileMeta {
            id: DocumentId::new(),
            format: FORMAT.to_owned(),
            app: manifest::writer(),
            kernel: kernel.to_owned(),
            units,
        }
    }

    /// The same drawing as this version writes it, with line work from `kernel`: the
    /// id is kept.
    pub fn rewritten(&self, kernel: &str, units: Units) -> FileMeta {
        FileMeta { id: self.id, ..FileMeta::new(kernel, units) }
    }
}

/// `meta.json` of a drawing from before the envelope.
#[derive(Deserialize)]
struct LegacyMeta {
    format: String,
    app: String,
    kernel: String,
    units: Units,
}

fn envelope(e: EnvelopeError) -> DrawingError {
    match e {
        EnvelopeError::Json(e) => DrawingError::Json(e),
        EnvelopeError::Io(e) => DrawingError::Io(e.to_string()),
        EnvelopeError::Archive(e) => DrawingError::Format(format!("not a drawing file: {e}")),
        EnvelopeError::Unsupported(m) => DrawingError::Format(m),
    }
}

/// The drawing's manifest as this version writes it.
pub fn manifest(drawing: &Drawing, meta: &FileMeta) -> Result<Manifest, DrawingError> {
    let mut m = Manifest::new(meta.id, DocKind::Drawing, drawing.semantic_hash()?);
    m.kernel = Some(meta.kernel.clone());
    m.sections = vec![Section::required(DRAWING), Section::optional(VIEWS), Section::optional(THUMBNAIL)];
    m.references = vec![drawing.model.clone()];
    Ok(m)
}

/// The file's bytes. Always this version's format, whatever `meta.format` says.
pub fn to_bytes(drawing: &Drawing, meta: &FileMeta) -> Result<Vec<u8>, DrawingError> {
    let mut bare = drawing.clone();
    for v in bare.sheets.iter_mut().flat_map(|s| s.views.iter_mut()) {
        v.geometry = ViewGeometry::default();
    }
    let mut entries = vec![Entry::new(DRAWING, (bare.to_json()? + "\n").into_bytes())];
    for v in drawing.sheets.iter().flat_map(|s| &s.views) {
        entries.push(Entry::new(format!("{VIEWS}{}.json", v.id.0), serde_json::to_vec(&v.geometry)?));
    }
    if !drawing.sheets.is_empty() {
        let png = export::thumbnail::png(&display::layout_sheet(drawing, 0), THUMBNAIL_PX)?;
        entries.push(Entry::stored(THUMBNAIL, png));
    }
    manifest::write_package(&manifest(drawing, meta)?, &entries).map_err(envelope)
}

/// Writes a drawing file atomically: a failed save leaves the previous file whole.
pub fn write(path: &Path, drawing: &Drawing, meta: &FileMeta) -> Result<(), DrawingError> {
    let bytes = to_bytes(drawing, meta)?;
    soliddraft_document::save_atomic(path, &bytes)
        .map_err(|e| DrawingError::Io(format!("cannot write {}: {e}", path.display())))
}

/// Reads a drawing from a file's bytes, in this version's format or the one before.
pub fn from_bytes(bytes: &[u8]) -> Result<(Drawing, FileMeta), DrawingError> {
    let mut package = Package::from_bytes(bytes).map_err(envelope)?;
    let (mut drawing, meta) = match package.manifest().cloned() {
        Some(m) => {
            m.check(DocKind::Drawing, &[DRAWING]).map_err(envelope)?;
            if format_major(&m.format, "soliddraft-drawing") != Some(2) {
                return Err(DrawingError::Format(format!("format is {}, expected {FORMAT}", m.format)));
            }
            let drawing = Drawing::from_json(&text(&mut package, DRAWING)?.ok_or(missing(DRAWING))?)?;
            let units = drawing.meta.units;
            let meta =
                FileMeta { id: m.id, format: m.format, app: m.writer, kernel: m.kernel.unwrap_or_default(), units };
            (drawing, meta)
        }
        None => {
            let old: LegacyMeta = serde_json::from_str(&text(&mut package, "meta.json")?.ok_or(missing("meta.json"))?)?;
            if !matches!(old.format.as_str(), LEGACY_FORMAT | "draftwell-drawing/1") {
                return Err(DrawingError::Format(format!("format is {}, expected {FORMAT}", old.format)));
            }
            let mut truth: Value = serde_json::from_str(&text(&mut package, DRAWING)?.ok_or(missing(DRAWING))?)?;
            let model = DocRef::from_legacy_model(&truth["model"])
                .ok_or_else(|| DrawingError::Format("the drawing's model reference has no path".into()))?;
            let id = manifest::legacy_identity(DocKind::Drawing, &truth, Vec::new()).id;
            truth["model"] = serde_json::to_value(&model)?;
            let drawing: Drawing = serde_json::from_value(truth)?;
            let meta = FileMeta { id, format: old.format, app: old.app, kernel: old.kernel, units: old.units };
            (drawing, meta)
        }
    };
    let ids: Vec<_> = drawing.sheets.iter().flat_map(|s| &s.views).map(|v| v.id).collect();
    for id in ids {
        if let Some(json) = text(&mut package, &format!("{VIEWS}{}.json", id.0))? {
            drawing.view_mut(id).expect("listed").geometry = serde_json::from_str(&json)?;
        }
    }
    Ok((drawing, meta))
}

fn text(package: &mut Package<Cursor<&[u8]>>, name: &str) -> Result<Option<String>, DrawingError> {
    package.read_text(name).map_err(envelope)
}

fn missing(entry: &str) -> DrawingError {
    DrawingError::Format(format!("no {entry}"))
}

/// Reads a drawing file.
pub fn read(path: &Path) -> Result<(Drawing, FileMeta), DrawingError> {
    let bytes = std::fs::read(path).map_err(|e| DrawingError::Io(format!("cannot read {}: {e}", path.display())))?;
    from_bytes(&bytes).map_err(|e| match e {
        DrawingError::Format(m) => DrawingError::Format(format!("{}: {m}", path.display())),
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::bracket_a3;
    use std::io::Write;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    #[test]
    fn files_round_trip_and_are_repeatable() {
        let d = bracket_a3();
        let meta = FileMeta::new("fixture", Units::Millimetre);
        let bytes = to_bytes(&d, &meta).unwrap();
        assert_eq!(to_bytes(&d, &meta).unwrap(), bytes, "same drawing, same bytes");
        let (back, m) = from_bytes(&bytes).unwrap();
        assert_eq!(back, d);
        assert_eq!(m, meta, "the id and the metadata come back");
        let mut p = Package::from_bytes(&bytes).unwrap();
        let manifest = p.manifest().unwrap().clone();
        assert_eq!((manifest.id, manifest.kind, manifest.format.as_str()), (meta.id, DocKind::Drawing, FORMAT));
        assert_eq!(manifest.semantic_hash, d.semantic_hash().unwrap());
        assert_eq!(manifest.references, std::slice::from_ref(&d.model), "the index sees what the drawing references");
        let png = p.read(THUMBNAIL).unwrap().unwrap();
        assert_eq!(&png[1..4], b"PNG");
        assert!(p.read("cache/views/4.json").unwrap().is_some());
        assert!(p.read("meta.json").unwrap().is_none());
        // The line work is not part of the revision.
        let mut moved = d.clone();
        moved.sheets[0].views[0].geometry = ViewGeometry::default();
        assert_eq!(moved.semantic_hash().unwrap(), d.semantic_hash().unwrap());
        assert_eq!(meta.rewritten("other", Units::Inch).id, meta.id);
    }

    #[test]
    fn draftwell_drawings_open_and_save_with_soliddraft_prefixes() {
        let d = bracket_a3();
        let meta = FileMeta::new("fixture", Units::Millimetre);
        let mut m = manifest(&d, &meta).unwrap();
        m.envelope = "draftwell-document/1".into();
        m.format = "draftwell-drawing/2".into();
        m.writer = "draftwell 0.1.0".into();
        let bytes = manifest::write_package(&m, &[Entry::new(DRAWING, d.to_json().unwrap().into_bytes())]).unwrap();
        let (back, old_meta) = from_bytes(&bytes).unwrap();
        assert_eq!(back, d);
        assert_eq!(old_meta.id, meta.id);
        let bytes = to_bytes(&back, &old_meta).unwrap();
        let saved = Package::from_bytes(&bytes).unwrap();
        assert_eq!(saved.manifest().unwrap().envelope, manifest::ENVELOPE);
        assert_eq!(saved.manifest().unwrap().format, FORMAT);
        assert_eq!(from_bytes(&bytes).unwrap().0, d);
        m.format = "draftwell-drawing/3".into();
        let newer = manifest::write_package(&m, &[Entry::new(DRAWING, d.to_json().unwrap().into_bytes())]).unwrap();
        assert!(matches!(from_bytes(&newer), Err(DrawingError::Format(_))));
    }

    /// A drawing file as phase D2 wrote it.
    fn legacy_file(d: &Drawing, with_views: bool) -> Vec<u8> {
        let mut bare = d.clone();
        for v in bare.sheets.iter_mut().flat_map(|s| s.views.iter_mut()) {
            v.geometry = ViewGeometry::default();
        }
        let mut truth = serde_json::to_value(&bare).unwrap();
        truth["model"] = serde_json::json!({
            "document_id": "bracket", "path": "bracket.json", "digest_at_update": "0123456789abcdef", "revision": 7
        });
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        let o = SimpleFileOptions::default();
        zip.start_file("meta.json", o).unwrap();
        let meta =
            r#"{"format": "draftwell-drawing/1", "app": "draftwell 0.1.0", "kernel": "x", "units": "millimetre"}"#;
        zip.write_all(meta.as_bytes()).unwrap();
        zip.start_file("drawing.json", o).unwrap();
        zip.write_all(serde_json::to_string_pretty(&truth).unwrap().as_bytes()).unwrap();
        if with_views {
            for v in d.sheets.iter().flat_map(|s| &s.views) {
                zip.start_file(format!("cache/views/{}.json", v.id.0), o).unwrap();
                zip.write_all(serde_json::to_string(&v.geometry).unwrap().as_bytes()).unwrap();
            }
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn format_1_drawings_open_unbound_and_older_than_their_part() {
        let d = bracket_a3();
        let old = legacy_file(&d, true);
        let (back, meta) = from_bytes(&old).unwrap();
        assert_eq!((meta.format.as_str(), meta.kernel.as_str()), ("draftwell-drawing/1", "x"));
        assert!(meta.id.is_legacy());
        assert_eq!(meta.id, from_bytes(&old).unwrap().1.id, "every open agrees on the id");
        assert_eq!(meta.id, soliddraft_document::identify(Cursor::new(&old)).unwrap().id, "as the index sees it");
        assert!(!back.model.is_bound(), "the old id was the part's name");
        assert_eq!(back.model.path, "bracket.json");
        assert_eq!(back.model.pinned.as_str(), "fnv64:0123456789abcdef");
        assert_eq!(back.sheets, d.sheets, "the views show what they showed");
        assert!(!back.views_match_pin(), "a Rebuild is due");

        // Saved again, it is format 2 with the same id.
        let (again, m) = from_bytes(&to_bytes(&back, &meta.rewritten("x", meta.units)).unwrap()).unwrap();
        assert_eq!((m.id, m.format.as_str()), (meta.id, FORMAT));
        assert_eq!(again, back);
    }

    #[test]
    fn a_missing_cache_leaves_the_view_out_of_date() {
        let (back, _) = from_bytes(&legacy_file(&bracket_a3(), false)).unwrap();
        assert_eq!(back.sheets[0].views.len(), 4);
        assert!(back.sheets[0].views.iter().all(|v| v.geometry.curves.is_empty()));
        assert!(matches!(from_bytes(b"not a zip"), Err(DrawingError::Format(_))));
    }

    #[test]
    fn writes_are_atomic_and_newer_files_are_refused() {
        let dir = std::env::temp_dir().join(format!("soliddraft-drawing-file-{}", std::process::id()));
        let path = dir.join("bracket.dwdrw");
        let d = bracket_a3();
        let meta = FileMeta::new("fixture", Units::Millimetre);
        write(&path, &d, &meta).unwrap();
        assert_eq!(read(&path).unwrap().1.id, meta.id);
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["bracket.dwdrw"]);
        let _ = std::fs::remove_dir_all(&dir);

        let mut m = manifest(&d, &meta).unwrap();
        m.format = "soliddraft-drawing/3".into();
        let bytes = manifest::write_package(&m, &[Entry::new(DRAWING, d.to_json().unwrap().into_bytes())]).unwrap();
        assert!(matches!(from_bytes(&bytes), Err(DrawingError::Format(_))));
    }
}
