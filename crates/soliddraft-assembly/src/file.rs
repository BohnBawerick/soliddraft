//! Assembly files (`.dwasm`).
//!
//! An assembly file is a document envelope (see `soliddraft-document`) with:
//!
//! - `manifest.json`: id, kind, format, writer, the semantic and geometry hashes, the
//!   sections, and a copy of every source reference so an index can answer "where
//!   used";
//! - `assembly.json` (required): the truth ([`Assembly`]);
//! - `placements.bin` (required when present): the unnamed components of a large
//!   assembly as fixed records ([`crate::bulk`]);
//! - `settings.json` (optional): display settings, outside both hashes
//!   ([`Assembly::settings`]);
//! - `cache/refs/<document id>/<revision>/` (optional): display copies of every
//!   document the assembly shows, at the revision it pins, so the assembly opens and
//!   looks as saved without opening any other file. A part's copy is its
//!   [`DisplayMesh`] (`display.bin`); a sub-assembly's copy is its own truth entries
//!   (`assembly.json`, `placements.bin`), whose parts' copies sit beside it in the same
//!   cache. A part shown at a quality of its own has its mesh at that quality beside
//!   the usual one, as `display-<code>.bin` ([`display_entry`]); a part's coarse
//!   level, once loaded, is kept as `coarse.bin` ([`level_entry`]). Copies are keyed
//!   by revision, so two sources pinning one revision share one copy; deleting the
//!   whole cache never changes the design.
//!
//! Writing is deterministic: the same assembly and copies give the same bytes.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use soliddraft_document::hash::without_format;
use soliddraft_document::manifest::{self, Entry, Manifest, Package, Section};
use soliddraft_document::{
    ContentHash, DocKind, DocumentId, EnvelopeError,
    kind::{format_major, found_tag},
};
use soliddraft_materials::Snapshots;
use soliddraft_tessellation::{Level, Quality, Settings, settings};

use crate::binary::BinaryError;
use crate::bulk::{self, PLACEMENTS};
use crate::mesh::{DISPLAY_MESH, DisplayMesh};
use crate::schema::{Assembly, Component, FORMAT, Invalid, OccurrenceAppearance, Override, Pattern, Source, Units};

/// The truth entry.
pub const ASSEMBLY: &str = "assembly.json";
/// The folder of display copies.
pub const REFS: &str = "cache/refs/";
/// The file name extension assemblies are saved under.
pub const EXTENSION: &str = soliddraft_document::kind::ASSEMBLY_EXTENSION;

/// Why an assembly file could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum FileError {
    /// Reading or writing failed.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// The envelope is not valid or is too new.
    #[error("{0}")]
    Envelope(#[from] EnvelopeError),
    /// `assembly.json` is not valid.
    #[error("the assembly is not valid: {0}")]
    Json(#[from] serde_json::Error),
    /// A binary entry is not valid.
    #[error("{0}")]
    Binary(#[from] BinaryError),
    /// The truth parses but is inconsistent.
    #[error("{0}")]
    Invalid(#[from] Invalid),
    /// The format tag is not one this version reads.
    #[error("the file {}, this version reads {FORMAT}", found_tag(.found))]
    Format {
        /// The tag in the file.
        found: String,
    },
}

/// The name, within a part's copy folder, of its display mesh at `quality`: the usual
/// `display.bin` for the part's own quality (`None`), `display-<code>.bin` otherwise.
pub fn display_entry(quality: Option<&Quality>) -> String {
    match quality {
        None => DISPLAY_MESH.to_owned(),
        Some(q) => format!("display-{}.bin", q.code()),
    }
}

/// The name, within a part's copy folder, of its mesh at `level` and `quality`: the
/// standard level is [`display_entry`]; the coarse one `coarse.bin` (or
/// `coarse-<code>.bin` at a quality of its own). Fine meshes are made on demand and
/// never saved, but are named the same way (`fine.bin`).
pub fn level_entry(quality: Option<&Quality>, level: Level) -> String {
    match (level, quality) {
        (Level::Standard, q) => display_entry(q),
        (l, None) => format!("{l}.bin"),
        (l, Some(q)) => format!("{l}-{}.bin", q.code()),
    }
}

/// The folder in `cache/refs` that holds the copy of document `id` at `revision`.
pub fn copy_folder(id: DocumentId, revision: &ContentHash) -> String {
    format!("{REFS}{id}/{}/", revision.as_str().replace(':', "-"))
}

/// A display copy to write into an assembly file: one document at one revision.
#[derive(Clone, Debug, PartialEq)]
pub struct RefCopy {
    /// The document.
    pub document: DocumentId,
    /// Its revision.
    pub revision: ContentHash,
    /// The entries, by name within the copy's folder: `display.bin` for a part;
    /// `assembly.json` and maybe `placements.bin` for a sub-assembly.
    pub entries: Vec<(String, Arc<[u8]>)>,
}

impl RefCopy {
    /// The copy of a part revision: its display mesh.
    pub fn part(document: DocumentId, revision: ContentHash, mesh: &DisplayMesh, made_by: &str) -> RefCopy {
        RefCopy { document, revision, entries: vec![(DISPLAY_MESH.to_owned(), mesh.encode(made_by).into())] }
    }

    /// The copy of a sub-assembly revision: its truth entries as its file holds them.
    pub fn assembly(document: DocumentId, revision: ContentHash, truth: &TruthEntries) -> RefCopy {
        let mut entries = vec![(ASSEMBLY.to_owned(), truth.json.clone())];
        if let Some(b) = &truth.bulk {
            entries.push((PLACEMENTS.to_owned(), b.clone()));
        }
        RefCopy { document, revision, entries }
    }
}

/// An assembly's truth as the bytes of its entries.
#[derive(Clone, Debug, PartialEq)]
pub struct TruthEntries {
    /// `assembly.json`.
    pub json: Arc<[u8]>,
    /// `placements.bin`, for an assembly that has one.
    pub bulk: Option<Arc<[u8]>>,
}

impl Assembly {
    /// The truth entries this assembly writes: named components and, for a large
    /// assembly, the rest in `placements.bin`. The lists must be in canonical order.
    pub fn truth_entries(&self) -> TruthEntries {
        let (json, bulk) = self.split_truth(false);
        TruthEntries {
            json: (serde_json::to_string_pretty(&json).expect("an assembly always serialises") + "\n")
                .into_bytes()
                .into(),
            bulk: bulk.map(Into::into),
        }
    }

    /// The JSON truth with the components it keeps, and `placements.bin` if any.
    fn split_truth(&self, geometry: bool) -> (Value, Option<Vec<u8>>) {
        let total = self.components.len();
        let (bulk, named): (Vec<&Component>, Vec<&Component>) =
            self.components.iter().partition(|c| bulk::goes_to_bulk(c, total));
        let out = TruthOut {
            format: &self.format,
            name: &self.name,
            units: self.units,
            sources: &self.sources,
            components: named,
            patterns: &self.patterns,
            overrides: &self.overrides,
            appearances: &self.appearances,
            snapshots: &self.snapshots,
            configurations: &self.configurations,
            display_states: &self.display_states,
            mates: &self.mates,
            next_id: self.next_id,
        };
        let json = serde_json::to_value(&out).expect("an assembly always serialises");
        let bulk = (!bulk.is_empty()).then(|| bulk::encode(bulk.into_iter(), geometry));
        (json, bulk)
    }

    /// The semantic hash: the assembly's revision, unsaved edits included. BLAKE3 over
    /// the canonical `assembly.json` without its format tag, and over the bytes of
    /// `placements.bin` when there is one.
    pub fn semantic_hash(&self) -> ContentHash {
        let (json, bulk) = self.split_truth(false);
        hash_truth(without_format(&json), bulk.as_deref())
    }

    /// The hash of what decides the assembly's shape: the truth without names, hidden
    /// flags, colours, display states and reference paths. Hiding, colouring or
    /// renaming a component changes the semantic hash and not this one.
    pub fn geometry_hash(&self) -> ContentHash {
        let (mut json, bulk) = self.split_truth(true);
        json = without_format(&json);
        if let Value::Object(map) = &mut json {
            map.remove("name");
            map.remove("display_states");
            map.remove("appearances");
            map.remove("snapshots");
            for list in ["components", "patterns"] {
                if let Some(Value::Array(items)) = map.get_mut(list) {
                    for item in items.iter_mut().filter_map(Value::as_object_mut) {
                        item.remove("name");
                        item.remove("hidden");
                    }
                }
            }
            if let Some(Value::Array(items)) = map.get_mut("overrides") {
                for item in items.iter_mut().filter_map(Value::as_object_mut) {
                    item.remove("hidden");
                }
                items.retain(|o| o.as_object().is_some_and(|m| m.len() > 1));
            }
            if let Some(Value::Array(items)) = map.get_mut("sources") {
                for doc in items.iter_mut().filter_map(|s| s.get_mut("document")).filter_map(Value::as_object_mut) {
                    doc.remove("path");
                    doc.remove("locator");
                }
            }
        }
        hash_truth(json, bulk.as_deref())
    }

    /// The assembly's manifest as this version writes it, with `kernel` naming what
    /// made the display copies.
    pub fn manifest(&self, id: DocumentId, kernel: Option<String>, has_copies: bool) -> Manifest {
        let mut m = Manifest::new(id, DocKind::Assembly, self.semantic_hash());
        m.kernel = kernel;
        m.geometry_hash = Some(self.geometry_hash());
        m.sections = vec![Section::required(ASSEMBLY)];
        if self.components.iter().any(|c| bulk::goes_to_bulk(c, self.components.len())) {
            m.sections.push(Section::required(PLACEMENTS));
        }
        if !self.settings.is_default() {
            m.sections.push(Section::optional(settings::ENTRY));
        }
        if has_copies {
            m.sections.push(Section::optional(REFS));
        }
        m.references = self.sources.iter().map(|s| s.document.clone()).collect();
        m
    }

    /// The assembly file's bytes, with `copies` in its cache.
    pub fn to_bytes(&self, id: DocumentId, kernel: Option<String>, copies: &[RefCopy]) -> Result<Vec<u8>, FileError> {
        let mut a = self.clone();
        a.normalize();
        a.validate()?;
        let truth = a.truth_entries();
        let manifest = a.manifest(id, kernel, !copies.is_empty());
        let mut entries = vec![Entry::new(ASSEMBLY, &*truth.json)];
        if let Some(b) = &truth.bulk {
            entries.push(Entry::new(PLACEMENTS, &**b));
        }
        let settings_json = a.settings.to_json();
        if !a.settings.is_default() {
            entries.push(Entry::new(settings::ENTRY, settings_json.as_bytes()));
        }
        // Copies in folder order, each copy's entries in name order, once each.
        let mut sorted: BTreeMap<String, &Arc<[u8]>> = BTreeMap::new();
        for c in copies {
            let folder = copy_folder(c.document, &c.revision);
            for (name, bytes) in &c.entries {
                sorted.insert(format!("{folder}{name}"), bytes);
            }
        }
        entries.extend(sorted.into_iter().map(|(name, bytes)| Entry::new(name, &**bytes)));
        Ok(manifest::write_package(&manifest, &entries)?)
    }

    /// Writes the assembly file atomically.
    pub fn save(
        &self,
        path: impl AsRef<Path>,
        id: DocumentId,
        kernel: Option<String>,
        copies: &[RefCopy],
    ) -> Result<(), FileError> {
        Ok(soliddraft_document::save_atomic(path.as_ref(), &self.to_bytes(id, kernel, copies)?)?)
    }

    /// An assembly from its truth entries' bytes: checks the format, merges the
    /// components of `placements.bin` into the list, and validates.
    pub fn from_truth(json: &[u8], bulk: Option<&[u8]>) -> Result<Assembly, FileError> {
        // One pass in the common case; a file that does not parse is probed for its format
        // tag, so a newer file says so instead of naming a field this version lacks.
        let mut a: Assembly = match serde_json::from_slice(json) {
            Ok(a) => a,
            Err(e) => {
                #[derive(serde::Deserialize)]
                struct Probe {
                    #[serde(default)]
                    format: String,
                }
                // Text that is not JSON at all is damage, not another version.
                let Ok(Probe { format: found }) = serde_json::from_slice::<Probe>(json) else {
                    return Err(e.into());
                };
                if format_major(&found, "soliddraft-assembly") != Some(1) {
                    return Err(FileError::Format { found });
                }
                return Err(e.into());
            }
        };
        if format_major(&a.format, "soliddraft-assembly") != Some(1) {
            return Err(FileError::Format { found: a.format });
        }
        a.format = FORMAT.into();
        if let Some(b) = bulk {
            a.components.extend(bulk::decode(b)?);
            a.components.sort_by_key(|c| c.id);
        }
        a.validate()?;
        Ok(a)
    }
}

/// [`Assembly`] as written, borrowing, with only the components `assembly.json` keeps.
/// The fields and their attributes mirror [`Assembly`]'s.
#[derive(Serialize)]
struct TruthOut<'a> {
    format: &'a str,
    name: &'a str,
    units: Units,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    sources: &'a [Source],
    #[serde(skip_serializing_if = "Vec::is_empty")]
    components: Vec<&'a Component>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    patterns: &'a [Pattern],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    overrides: &'a [Override],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    appearances: &'a [OccurrenceAppearance],
    #[serde(skip_serializing_if = "Snapshots::is_empty")]
    snapshots: &'a Snapshots,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    configurations: &'a [Value],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    display_states: &'a [Value],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    mates: &'a [Value],
    next_id: u32,
}

fn hash_truth(json: Value, bulk: Option<&[u8]>) -> ContentHash {
    match bulk {
        None => ContentHash::of_entries([(ASSEMBLY, &json)]),
        Some(b) => {
            let digest = Value::String(ContentHash::of_bytes(b).to_string());
            ContentHash::of_entries([(ASSEMBLY, &json), (PLACEMENTS, &digest)])
        }
    }
}

/// An assembly file opened for reading. The truth is read at once; display copies are
/// read on demand, so a loader can take them in the order it needs them, from as many
/// threads as it likes ([`AssemblyPackage::reader`]).
#[derive(Clone)]
pub struct AssemblyPackage {
    manifest: Manifest,
    assembly: Arc<Assembly>,
    package: Package<Cursor<Arc<[u8]>>>,
}

impl AssemblyPackage {
    /// Opens an assembly file from its bytes.
    pub fn from_bytes(bytes: impl Into<Arc<[u8]>>) -> Result<AssemblyPackage, FileError> {
        let mut package = Package::new(Cursor::new(bytes.into()))?;
        let manifest = package
            .manifest()
            .cloned()
            .ok_or_else(|| EnvelopeError::Unsupported("not a SolidDraft assembly: no manifest".into()))?;
        manifest.check(DocKind::Assembly, &[ASSEMBLY, PLACEMENTS])?;
        let json = package.read(ASSEMBLY)?.ok_or_else(|| EnvelopeError::Unsupported("no assembly.json".into()))?;
        let bulk = package.read(PLACEMENTS)?;
        let mut assembly = Assembly::from_truth(&json, bulk.as_deref())?;
        // Optional: settings this version cannot read open as the defaults.
        if let Ok(Some(text)) = package.read_text(settings::ENTRY) {
            assembly.settings = Settings::from_json(&text).unwrap_or_default();
        }
        Ok(AssemblyPackage { manifest, assembly: Arc::new(assembly), package })
    }

    /// Opens an assembly file.
    pub fn open(path: impl AsRef<Path>) -> Result<AssemblyPackage, FileError> {
        AssemblyPackage::from_bytes(std::fs::read(path)?)
    }

    /// The manifest.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The assembly's id.
    pub fn id(&self) -> DocumentId {
        self.manifest.id
    }

    /// The truth.
    pub fn assembly(&self) -> &Arc<Assembly> {
        &self.assembly
    }

    /// A reader of the display copies for another thread. Cheap: the entry list is
    /// shared.
    pub fn reader(&self) -> CopyReader {
        CopyReader { package: self.package.clone() }
    }

    /// Every display copy in the file: document and revision.
    pub fn copies(&self) -> Vec<(DocumentId, ContentHash)> {
        let mut out: Vec<(DocumentId, ContentHash)> = self
            .package
            .names()
            .iter()
            .filter_map(|n| n.strip_prefix(REFS))
            .filter_map(|rest| {
                let mut parts = rest.split('/');
                let id = parts.next()?.parse().ok()?;
                let rev = parts.next()?;
                let (scheme, digest) = rev.split_once('-')?;
                Some((id, ContentHash::foreign(scheme, digest)))
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

/// Reads display copies out of an assembly file.
pub struct CopyReader {
    package: Package<Cursor<Arc<[u8]>>>,
}

impl CopyReader {
    /// The display mesh of part `id` at `revision`, with what made it, if the file
    /// holds it.
    pub fn display_mesh(
        &mut self,
        id: DocumentId,
        revision: &ContentHash,
    ) -> Result<Option<(DisplayMesh, String)>, FileError> {
        self.display_mesh_at(id, revision, None)
    }

    /// The display mesh of part `id` at `revision` made at `quality` (`None` for the
    /// part's own), with what made it, if the file holds it.
    pub fn display_mesh_at(
        &mut self,
        id: DocumentId,
        revision: &ContentHash,
        quality: Option<&Quality>,
    ) -> Result<Option<(DisplayMesh, String)>, FileError> {
        let name = format!("{}{}", copy_folder(id, revision), display_entry(quality));
        match self.package.read(&name)? {
            Some(bytes) => Ok(Some(DisplayMesh::decode(&bytes)?)),
            None => Ok(None),
        }
    }

    /// The truth entries of sub-assembly `id` at `revision`, if the file holds them.
    pub fn assembly_truth(
        &mut self,
        id: DocumentId,
        revision: &ContentHash,
    ) -> Result<Option<TruthEntries>, FileError> {
        let folder = copy_folder(id, revision);
        let Some(json) = self.package.read(&format!("{folder}{ASSEMBLY}"))? else {
            return Ok(None);
        };
        let bulk = self.package.read(&format!("{folder}{PLACEMENTS}"))?;
        Ok(Some(TruthEntries { json: json.into(), bulk: bulk.map(Into::into) }))
    }

    /// The raw bytes of one entry of the copy of `id` at `revision`.
    pub fn entry(&mut self, id: DocumentId, revision: &ContentHash, name: &str) -> Result<Option<Vec<u8>>, FileError> {
        Ok(self.package.read(&format!("{}{name}", copy_folder(id, revision)))?)
    }

    /// The raw entries of the copy of `id` at `revision`, to carry into another file.
    pub fn copy(&mut self, id: DocumentId, revision: &ContentHash) -> Result<Option<RefCopy>, FileError> {
        let folder = copy_folder(id, revision);
        let names: Vec<String> = self.package.names().into_iter().filter(|n| n.starts_with(&folder)).collect();
        if names.is_empty() {
            return Ok(None);
        }
        let mut entries = Vec::with_capacity(names.len());
        for n in names {
            let bytes = self.package.read(&n)?.unwrap_or_default();
            entries.push((n[folder.len()..].to_owned(), bytes.into()));
        }
        Ok(Some(RefCopy { document: id, revision: revision.clone(), entries }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::{ComponentId, SourceId};
    use crate::placement::Placement;
    use crate::schema::PatternRule;
    use glam::DVec3;
    use soliddraft_document::DocRef;

    fn sample(components: usize) -> (Assembly, DocumentId, ContentHash) {
        let part = DocumentId::new();
        let rev = ContentHash::of_bytes(b"part rev");
        let mut a = Assembly::new("Rack");
        let s = a.add_source(DocRef::new(part, DocKind::Part, "parts/p.dwprt", rev.clone()));
        for i in 0..components {
            a.add_component(s, Placement::translation(DVec3::new(i as f64 * 10.0, 0.0, 0.0)));
        }
        if components > 0 {
            a.components[0].name = Some("First".into());
            a.add_pattern(
                vec![ComponentId(2)],
                PatternRule::Linear { direction: [0.0, 1.0, 0.0], spacing: 5.0, count: 3, second: None },
            );
        }
        (a, part, rev)
    }

    #[test]
    fn small_assemblies_round_trip_with_their_copies() {
        let (a, part, rev) = sample(3);
        let id = DocumentId::new();
        let copy = RefCopy::part(part, rev.clone(), &DisplayMesh::cuboid(DVec3::ZERO, DVec3::ONE), "test");
        let bytes = a.to_bytes(id, Some("test 1".into()), std::slice::from_ref(&copy)).unwrap();
        assert_eq!(a.to_bytes(id, Some("test 1".into()), std::slice::from_ref(&copy)).unwrap(), bytes, "deterministic");
        let p = AssemblyPackage::from_bytes(bytes).unwrap();
        assert_eq!(**p.assembly(), a);
        assert_eq!(p.id(), id);
        assert_eq!(p.manifest().semantic_hash, a.semantic_hash());
        assert_eq!(p.manifest().references.len(), 1);
        assert_eq!(p.copies(), vec![(part, rev.clone())]);
        let mut r = p.reader();
        let (mesh, made_by) = r.display_mesh(part, &rev).unwrap().unwrap();
        assert_eq!((mesh.triangle_count(), made_by.as_str()), (12, "test"));
        assert!(r.display_mesh(part, &ContentHash::of_bytes(b"other")).unwrap().is_none());
        assert_eq!(r.copy(part, &rev).unwrap().unwrap(), copy);
        assert!(p.manifest().sections.iter().all(|s| s.name != PLACEMENTS));
    }

    #[test]
    fn draftwell_assemblies_open_and_save_with_soliddraft_prefixes() {
        let (a, ..) = sample(3);
        let id = DocumentId::new();
        let mut m = a.manifest(id, None, false);
        m.envelope = "draftwell-document/1".into();
        m.format = "draftwell-assembly/1".into();
        m.writer = "draftwell 0.1.0".into();
        let truth = String::from_utf8(a.truth_entries().json.to_vec())
            .unwrap()
            .replace("soliddraft-assembly/1", "draftwell-assembly/1");
        let bytes = manifest::write_package(&m, &[Entry::new(ASSEMBLY, truth.as_bytes())]).unwrap();
        let old = AssemblyPackage::from_bytes(bytes).unwrap();
        assert_eq!(old.id(), id);
        assert_eq!(**old.assembly(), a);
        assert_eq!(old.assembly().semantic_hash(), m.semantic_hash);
        let bytes = old.assembly().to_bytes(id, None, &[]).unwrap();
        let mut saved = Package::from_bytes(&bytes).unwrap();
        assert_eq!(saved.manifest().unwrap().envelope, manifest::ENVELOPE);
        assert_eq!(saved.manifest().unwrap().format, FORMAT);
        let truth: Value = serde_json::from_slice(&saved.read(ASSEMBLY).unwrap().unwrap()).unwrap();
        assert_eq!(truth["format"], FORMAT);
        let newer = truth.to_string().replace(FORMAT, "draftwell-assembly/2");
        assert!(matches!(Assembly::from_truth(newer.as_bytes(), None), Err(FileError::Format { .. })));
    }

    #[test]
    fn a_truth_that_is_not_json_is_damage_and_one_without_a_tag_says_so() {
        // Text that is not JSON is not "a file of another format".
        for text in ["", "{ not json", "\u{0}\u{0}"] {
            assert!(matches!(Assembly::from_truth(text.as_bytes(), None), Err(FileError::Json(_))), "{text:?}");
        }
        // A tagless truth is refused as one, in words, not with a blank where the tag goes.
        let untagged = Assembly::from_truth(br#"{"name": "x"}"#, None).unwrap_err();
        assert!(matches!(untagged, FileError::Format { .. }), "{untagged}");
        assert!(untagged.to_string().contains("the file has no format tag"), "{untagged}");
        let newer = Assembly::from_truth(br#"{"format": "soliddraft-assembly/2"}"#, None).unwrap_err();
        assert!(newer.to_string().starts_with("the file is soliddraft-assembly/2, this version reads"), "{newer}");
    }

    #[test]
    fn large_assemblies_keep_unnamed_components_in_placements_bin() {
        let (mut a, ..) = sample(bulk::BULK_THRESHOLD + 5);
        a.components[3].hidden = true;
        let truth = a.truth_entries();
        let json: Value = serde_json::from_slice(&truth.json).unwrap();
        assert_eq!(json["components"].as_array().unwrap().len(), 1, "only the named one");
        assert!(truth.bulk.is_some());
        let bytes = a.to_bytes(DocumentId::new(), None, &[]).unwrap();
        let p = AssemblyPackage::from_bytes(bytes).unwrap();
        assert_eq!(**p.assembly(), a);
        assert!(p.manifest().sections.iter().any(|s| s.name == PLACEMENTS && s.required));
        let back = Assembly::from_truth(&truth.json, truth.bulk.as_deref()).unwrap();
        assert_eq!(back, a);
    }

    #[test]
    fn hiding_and_renaming_change_the_revision_and_not_the_geometry() {
        for n in [3, bulk::BULK_THRESHOLD + 2] {
            let (a, ..) = sample(n);
            let mut hidden = a.clone();
            hidden.components[1].hidden = true;
            hidden.name = "Renamed".into();
            hidden.sources[0].document.path = "elsewhere/p.dwprt".into();
            assert_ne!(hidden.semantic_hash(), a.semantic_hash());
            assert_eq!(hidden.geometry_hash(), a.geometry_hash(), "{n} components");
            let mut moved = a.clone();
            moved.components[1].placement = Placement::translation(DVec3::new(1.0, 2.0, 3.0));
            assert_ne!(moved.geometry_hash(), a.geometry_hash());
        }
    }

    #[test]
    fn the_revision_ignores_the_format_tag() {
        let (a, ..) = sample(2);
        let mut v = serde_json::to_value(&a).unwrap();
        v["format"] = "soliddraft-assembly/1.1".into();
        let b = Assembly::from_truth(serde_json::to_string(&v).unwrap().as_bytes(), None).unwrap();
        assert_eq!(b.semantic_hash(), a.semantic_hash());
        v["format"] = "soliddraft-assembly/2".into();
        assert!(matches!(
            Assembly::from_truth(serde_json::to_string(&v).unwrap().as_bytes(), None),
            Err(FileError::Format { .. })
        ));
    }

    #[test]
    fn inconsistent_files_are_refused() {
        let (mut a, ..) = sample(2);
        a.components.push(Component::new(ComponentId(50), SourceId(77), Placement::IDENTITY));
        a.next_id = 51;
        assert!(matches!(a.to_bytes(DocumentId::new(), None, &[]), Err(FileError::Invalid(_))));
    }
}
