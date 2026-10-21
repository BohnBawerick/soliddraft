//! The envelope every document file shares: a zip whose first entry is `manifest.json`.
//!
//! The manifest says what the file is (id, kind, format of its truth entry), who wrote
//! it, the semantic and geometry hashes of its content, the documents it references,
//! and which sections the file holds and whether each is required. The rule for old
//! readers: refuse a file that has a required section they do not know (they would
//! damage it), and drop optional sections they do not know (caches) without a word.
//!
//! Files written before the envelope have no manifest. [`identify`] still gives them an
//! identity, derived from their content (see [`DocumentId::legacy`]).

use std::io::{Cursor, Read, Seek, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

use crate::hash::{ContentHash, without_format};
use crate::id::DocumentId;
use crate::kind::{DocKind, format_major};
use crate::reference::DocRef;

/// The envelope's format tag.
pub const ENVELOPE: &str = "soliddraft-document/1";
/// The manifest's entry name.
pub const MANIFEST: &str = "manifest.json";

/// Why a document file could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum EnvelopeError {
    /// Reading or writing failed.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// The file is not a zip archive.
    #[error("not a SolidDraft document: {0}")]
    Archive(#[from] zip::result::ZipError),
    /// A JSON entry is not valid.
    #[error("not valid: {0}")]
    Json(#[from] serde_json::Error),
    /// The file was written by a newer SolidDraft, or is not a document this version
    /// knows.
    #[error("{0}")]
    Unsupported(String),
}

/// `manifest.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// Always [`ENVELOPE`] for files this version writes.
    pub envelope: String,
    /// The document's id.
    pub id: DocumentId,
    /// The document's kind.
    pub kind: DocKind,
    /// The format tag of the truth entry, such as "soliddraft-part/2".
    pub format: String,
    /// The application that wrote the file, such as "soliddraft 0.1.0".
    pub writer: String,
    /// The kernel backend whose results the file's caches hold, such as "smlib c9c5979".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel: Option<String>,
    /// BLAKE3 over the canonical truth: the revision references pin.
    pub semantic_hash: ContentHash,
    /// BLAKE3 over the canonical truth without names, colours and materials, for kinds
    /// that have geometry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry_hash: Option<ContentHash>,
    /// The sections the file holds.
    pub sections: Vec<Section>,
    /// The documents this one references, as its truth holds them, so an index can
    /// answer "where used" from manifests alone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<DocRef>,
}

/// One section of a document file: an entry, or every entry under a folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    /// The entry name, or a folder ending in "/" for every entry under it.
    pub name: String,
    /// True when a reader that does not know the section must refuse the file.
    pub required: bool,
}

impl Section {
    /// A section a reader must understand.
    pub fn required(name: &str) -> Section {
        Section { name: name.to_owned(), required: true }
    }

    /// A section a reader may drop, such as a cache.
    pub fn optional(name: &str) -> Section {
        Section { name: name.to_owned(), required: false }
    }

    /// True when the section holds `entry`.
    pub fn holds(&self, entry: &str) -> bool {
        if self.name.ends_with('/') { entry.starts_with(&self.name) } else { entry == self.name }
    }
}

impl Manifest {
    /// A manifest for a file this version writes.
    pub fn new(id: DocumentId, kind: DocKind, semantic_hash: ContentHash) -> Manifest {
        Manifest {
            envelope: ENVELOPE.to_owned(),
            id,
            kind,
            format: kind.format().to_owned(),
            writer: writer(),
            kernel: None,
            semantic_hash,
            geometry_hash: None,
            sections: Vec::new(),
            references: Vec::new(),
        }
    }

    /// Checks that this version can read the file: the envelope's major version, the
    /// kind, and that every required section is one of `known`.
    pub fn check(&self, kind: DocKind, known: &[&str]) -> Result<(), EnvelopeError> {
        if format_major(&self.envelope, "soliddraft-document") != Some(1) {
            return Err(EnvelopeError::Unsupported(format!(
                "the file's envelope is {}, this version reads {ENVELOPE}",
                self.envelope
            )));
        }
        if self.kind != kind {
            return Err(EnvelopeError::Unsupported(format!("the file is a {}, not a {kind}", self.kind)));
        }
        if let Some(s) = self.sections.iter().find(|s| s.required && !known.contains(&s.name.as_str())) {
            return Err(EnvelopeError::Unsupported(format!(
                "the file needs a newer SolidDraft: it has a required section {} this version does not know",
                s.name
            )));
        }
        Ok(())
    }
}

/// The writer string of this build: "soliddraft 0.1.0".
pub fn writer() -> String {
    format!("soliddraft {}", env!("CARGO_PKG_VERSION"))
}

/// The semantic hash of a document whose truth is one JSON entry: the hash of that
/// entry without its format tag, under the kind's truth entry name.
pub fn semantic_hash(kind: DocKind, truth: &Value) -> ContentHash {
    ContentHash::of_entries([(kind.truth_entry(), &without_format(truth))])
}

/// One entry to write into a document file.
pub struct Entry<'a> {
    /// The entry name.
    pub name: String,
    /// Its bytes.
    pub bytes: std::borrow::Cow<'a, [u8]>,
    /// Deflate it; already compressed data such as PNG is stored as is.
    pub compress: bool,
}

impl<'a> Entry<'a> {
    /// A deflated entry.
    pub fn new(name: impl Into<String>, bytes: impl Into<std::borrow::Cow<'a, [u8]>>) -> Entry<'a> {
        Entry { name: name.into(), bytes: bytes.into(), compress: true }
    }

    /// An entry stored without compression.
    pub fn stored(name: impl Into<String>, bytes: impl Into<std::borrow::Cow<'a, [u8]>>) -> Entry<'a> {
        Entry { name: name.into(), bytes: bytes.into(), compress: false }
    }
}

/// A document file's bytes: the manifest first, then `entries` in order. Entries carry
/// a fixed timestamp, so the same content always gives the same bytes. Every entry must
/// belong to a section the manifest lists.
pub fn write_package(manifest: &Manifest, entries: &[Entry<'_>]) -> Result<Vec<u8>, EnvelopeError> {
    if let Some(e) = entries.iter().find(|e| !manifest.sections.iter().any(|s| s.holds(&e.name))) {
        return Err(EnvelopeError::Unsupported(format!("entry {} is in no section of the manifest", e.name)));
    }
    let deflated = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(DateTime::default());
    let stored = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(DateTime::default());
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(MANIFEST, deflated)?;
    zip.write_all((serde_json::to_string_pretty(manifest)? + "\n").as_bytes())?;
    for e in entries {
        zip.start_file(e.name.as_str(), if e.compress { deflated } else { stored })?;
        zip.write_all(&e.bytes)?;
    }
    Ok(zip.finish()?.into_inner())
}

/// A document file opened for reading. Clones share the parsed entry list, so a
/// reader per thread costs no second parse.
#[derive(Clone)]
pub struct Package<R: Read + Seek> {
    zip: ZipArchive<R>,
    manifest: Option<Manifest>,
}

impl<'a> Package<Cursor<&'a [u8]>> {
    /// Opens a document file from its bytes.
    pub fn from_bytes(bytes: &'a [u8]) -> Result<Self, EnvelopeError> {
        Package::new(Cursor::new(bytes))
    }
}

impl<R: Read + Seek> Package<R> {
    /// Opens a document file. A file without a manifest opens with
    /// [`Package::manifest`] `None`: it predates the envelope.
    pub fn new(reader: R) -> Result<Self, EnvelopeError> {
        let mut zip = ZipArchive::new(reader)?;
        let manifest = match read_entry(&mut zip, MANIFEST)? {
            Some(bytes) => Some(serde_json::from_slice::<Manifest>(&bytes)?),
            None => None,
        };
        Ok(Package { zip, manifest })
    }

    /// The manifest, or `None` for a file from before the envelope.
    pub fn manifest(&self) -> Option<&Manifest> {
        self.manifest.as_ref()
    }

    /// An entry's bytes, or `None` when the file has no such entry.
    pub fn read(&mut self, name: &str) -> Result<Option<Vec<u8>>, EnvelopeError> {
        read_entry(&mut self.zip, name)
    }

    /// An entry as text.
    pub fn read_text(&mut self, name: &str) -> Result<Option<String>, EnvelopeError> {
        match self.read(name)? {
            Some(bytes) => Ok(Some(
                String::from_utf8(bytes).map_err(|_| EnvelopeError::Unsupported(format!("{name} is not UTF-8")))?,
            )),
            None => Ok(None),
        }
    }

    /// Every entry name, in file order.
    pub fn names(&self) -> Vec<String> {
        self.zip.file_names().filter_map(Result::ok).map(|n| n.into_owned()).collect()
    }
}

fn read_entry<R: Read + Seek>(zip: &mut ZipArchive<R>, name: &str) -> Result<Option<Vec<u8>>, EnvelopeError> {
    match zip.by_name(name) {
        Ok(mut f) => {
            let mut bytes = Vec::new();
            f.read_to_end(&mut bytes)?;
            Ok(Some(bytes))
        }
        Err(zip::result::ZipError::FileNotFound) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// What the index and the resolver need to know about a document file without opening
/// it as a document.
#[derive(Clone, Debug, PartialEq)]
pub struct Identity {
    /// The document's id.
    pub id: DocumentId,
    /// Its kind.
    pub kind: DocKind,
    /// Its current revision: the semantic hash.
    pub revision: ContentHash,
    /// The documents it references.
    pub references: Vec<DocRef>,
    /// True for a file from before the envelope, whose id is derived from its content.
    pub legacy: bool,
}

/// The identity of a document file, read from its manifest, or derived from the content
/// of a file from before the envelope.
pub fn identify<R: Read + Seek>(reader: R) -> Result<Identity, EnvelopeError> {
    let mut package = Package::new(reader)?;
    if let Some(m) = package.manifest() {
        return Ok(Identity {
            id: m.id,
            kind: m.kind,
            revision: m.semantic_hash.clone(),
            references: m.references.clone(),
            legacy: false,
        });
    }
    // A part before the envelope: recipe.json and meta.json.
    if let Some(text) = package.read(LEGACY_PART_TRUTH)? {
        let truth: Value = serde_json::from_slice(&text)?;
        return Ok(legacy_identity(DocKind::Part, &truth, Vec::new()));
    }
    // A drawing before the envelope: meta.json and drawing.json with a model reference.
    if let Some(text) = package.read(DocKind::Drawing.truth_entry())? {
        let truth: Value = serde_json::from_slice(&text)?;
        let references = DocRef::from_legacy_model(&truth["model"]).into_iter().collect();
        return Ok(legacy_identity(DocKind::Drawing, &truth, references));
    }
    Err(EnvelopeError::Unsupported("not a SolidDraft document: no manifest".into()))
}

/// The identity of a document file on disk.
pub fn identify_file(path: &Path) -> Result<Identity, EnvelopeError> {
    identify(std::io::BufReader::new(std::fs::File::open(path)?))
}

/// The truth entry of a part file from before the envelope.
pub const LEGACY_PART_TRUTH: &str = "recipe.json";

/// The identity of a pre-envelope file of `kind` whose truth is `truth`: its revision
/// is the semantic hash of that truth, and its id derives from the revision.
pub fn legacy_identity(kind: DocKind, truth: &Value, references: Vec<DocRef>) -> Identity {
    let revision = semantic_hash(kind, truth);
    Identity { id: DocumentId::legacy(kind, &revision), kind, revision, references, legacy: true }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest() -> Manifest {
        let mut m = Manifest::new(DocumentId::new(), DocKind::Part, ContentHash::of_bytes(b"x"));
        m.sections = vec![Section::required("part.json"), Section::optional("cache/")];
        m
    }

    #[test]
    fn packages_round_trip_and_repeat() {
        let m = manifest();
        let entries = [Entry::new("part.json", b"{}".as_slice()), Entry::stored("cache/mesh.bin", vec![1, 2, 3])];
        let bytes = write_package(&m, &entries).unwrap();
        assert_eq!(write_package(&m, &entries).unwrap(), bytes, "same content, same bytes");
        let mut p = Package::from_bytes(&bytes).unwrap();
        assert_eq!(p.manifest(), Some(&m));
        assert_eq!(p.names(), ["manifest.json", "part.json", "cache/mesh.bin"]);
        assert_eq!(p.read("cache/mesh.bin").unwrap(), Some(vec![1, 2, 3]));
        assert_eq!(p.read("nothing").unwrap(), None);
        let id = identify(Cursor::new(&bytes)).unwrap();
        assert_eq!((id.id, id.kind, id.legacy), (m.id, DocKind::Part, false));
    }

    #[test]
    fn entries_outside_the_sections_are_refused() {
        let err = write_package(&manifest(), &[Entry::new("stray.json", b"{}".as_slice())]).unwrap_err();
        assert!(err.to_string().contains("stray.json"), "{err}");
    }

    #[test]
    fn unknown_required_sections_refuse_and_unknown_optional_ones_do_not() {
        let mut m = manifest();
        assert!(m.check(DocKind::Part, &["part.json"]).is_ok());
        m.sections.push(Section::optional("cache/shiny/"));
        assert!(m.check(DocKind::Part, &["part.json"]).is_ok());
        m.sections.push(Section::required("mates.json"));
        let err = m.check(DocKind::Part, &["part.json"]).unwrap_err();
        assert!(err.to_string().contains("mates.json"), "{err}");
        assert!(manifest().check(DocKind::Drawing, &["part.json"]).is_err(), "wrong kind");
        let mut newer = manifest();
        newer.envelope = "soliddraft-document/2".into();
        assert!(newer.check(DocKind::Part, &["part.json"]).is_err(), "newer envelope");
    }

    #[test]
    fn files_without_a_manifest_get_a_derived_identity() {
        let recipe = json!({"format": "soliddraft-part/1", "name": "p", "features": []});
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(LEGACY_PART_TRUTH, SimpleFileOptions::default()).unwrap();
        zip.write_all(serde_json::to_string_pretty(&recipe).unwrap().as_bytes()).unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let a = identify(Cursor::new(&bytes)).unwrap();
        assert!(a.legacy && a.id.is_legacy());
        assert_eq!(a.kind, DocKind::Part);
        assert_eq!(a.id, identify(Cursor::new(&bytes)).unwrap().id, "the same file, the same id");
        // The revision ignores the format tag, so the migrated part has the same one.
        let migrated = json!({"format": "soliddraft-part/2", "name": "p", "features": []});
        assert_eq!(a.revision, semantic_hash(DocKind::Part, &migrated));
    }
}
