//! Native part files.
//!
//! A part file (`.dwprt`) is a document envelope (see `soliddraft-document`) with two
//! entries:
//!
//! - `manifest.json`: the part's id, kind and format, the application and kernel that
//!   wrote it, its semantic hash (the revision drawings and assemblies pin) and its
//!   geometry hash, and its sections.
//! - `part.json`: the truth. The format tag (`soliddraft-part/2`), the part's name and
//!   units, every feature in tree order with its parameters, sketch and persistent
//!   references, the rollback bar and the next free feature id, and when it has them
//!   the part's material and appearance assignments with copies of what they use (see
//!   [`materials`](crate::materials)). It never holds kernel ids or geometry the
//!   kernel computes, so it rebuilds on any backend.
//! - `settings.json`, when any setting differs from its default: viewport quality,
//!   export accuracy and whether to save display data (see `soliddraft-tessellation`).
//!   It is an optional section outside both hashes, so a quality change never gives the
//!   part a new revision. A reader that cannot read it opens the part with the default
//!   settings.
//! - `cache/mesh/coarse.bin` and `cache/mesh/standard.bin`, when the settings save
//!   display data and the part has a body: its meshes at two display levels, so it can
//!   be shown without a rebuild (see [`display`](crate::display)). Optional, outside both
//!   hashes.
//! - `thumbnail.png`, when the app saved the part with one: a small picture of the part
//!   for a start page or a file browser. Optional, outside both hashes.
//!
//! Files from before the envelope (`.dwpart`, `soliddraft-part/1`) hold the same truth
//! as `recipe.json` beside a `meta.json`. They still open, with an id derived from their
//! content, and are written in the current format when saved.
//!
//! Writing is deterministic: the same document always gives the same bytes, so saving
//! an opened file reproduces it exactly. Floating-point values round-trip bit for bit.
//! The format's major version is the number after the slash; a file with a newer major
//! version, or with a required section this version does not know, is refused rather
//! than misread.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use soliddraft_document::manifest::{self, Entry, LEGACY_PART_TRUTH, Manifest, Package, Section};
use soliddraft_document::{ContentHash, DocKind, EnvelopeError, kind::format_major};
use soliddraft_kernel_api::Kernel;

use soliddraft_materials::Snapshots;
use soliddraft_tessellation::{Settings, settings};

use crate::display;
use crate::document::Document;
use crate::feature::{DatumPlane, Feature, FeatureId};
use crate::materials::{AppearanceAssignment, MaterialAssignment, PartMaterials};

/// The format tag of the current version.
pub const FORMAT: &str = "soliddraft-part/2";
/// The format tag of part files from before the document envelope.
pub const LEGACY_FORMAT: &str = "soliddraft-part/1";
/// The file name extension parts are saved under.
pub const EXTENSION: &str = soliddraft_document::PART_EXTENSION;
/// The extension of part files from before the envelope, which still open.
pub const LEGACY_EXTENSION: &str = soliddraft_document::LEGACY_PART_EXTENSION;

/// The truth entry.
const PART: &str = "part.json";

/// Why a part file could not be written or read.
#[derive(Debug, thiserror::Error)]
pub enum FileError {
    /// Reading or writing the file failed.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// The file is not a zip archive, or an entry is missing.
    #[error("not a SolidDraft part file: {0}")]
    Archive(String),
    /// `part.json` is not valid.
    #[error("the part's recipe is not valid: {0}")]
    Json(#[from] serde_json::Error),
    /// The format tag is not one this version reads.
    #[error("the file is {found}, this version reads {FORMAT} and {LEGACY_FORMAT}")]
    Format {
        /// The tag in the file.
        found: String,
    },
    /// The file needs a newer SolidDraft, or is another kind of document.
    #[error("{0}")]
    Unsupported(String),
    /// The recipe parses but is inconsistent.
    #[error("the part's recipe is inconsistent: {0}")]
    Invalid(String),
}

impl From<EnvelopeError> for FileError {
    fn from(e: EnvelopeError) -> FileError {
        match e {
            EnvelopeError::Io(e) => FileError::Io(e),
            EnvelopeError::Archive(e) => FileError::Archive(e.to_string()),
            EnvelopeError::Json(e) => FileError::Json(e),
            EnvelopeError::Unsupported(m) => FileError::Unsupported(m),
        }
    }
}

/// `part.json`, and `recipe.json` before the envelope.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recipe {
    format: String,
    name: String,
    units: Units,
    features: Vec<Feature>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    datum_names: BTreeMap<DatumPlane, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rollback: Option<FeatureId>,
    next_feature: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    material: Vec<MaterialAssignment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    appearances: Vec<AppearanceAssignment>,
    #[serde(default, skip_serializing_if = "Snapshots::is_empty")]
    snapshots: Snapshots,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Units {
    /// Millimetres and degrees.
    Mm,
}

impl Document {
    fn recipe(&self) -> Recipe {
        Recipe {
            format: FORMAT.into(),
            name: self.name.clone(),
            units: Units::Mm,
            features: self.features.clone(),
            datum_names: self.datum_names.clone(),
            rollback: self.rollback,
            next_feature: self.next_id,
            material: self.materials.material.clone(),
            appearances: self.materials.appearances.clone(),
            snapshots: self.materials.snapshots.clone(),
        }
    }

    /// The part's `part.json`: features, parameters and references, no kernel state.
    pub fn part_json(&self) -> String {
        serde_json::to_string_pretty(&self.recipe()).expect("a recipe always serialises") + "\n"
    }

    /// The part's truth as JSON, what its hashes are computed over.
    pub fn truth(&self) -> Value {
        serde_json::to_value(self.recipe()).expect("a recipe always serialises")
    }

    /// The part's revision: BLAKE3 over its canonical truth, unsaved edits included.
    /// Equal while nothing about the part changed; a drawing or assembly pins it.
    pub fn semantic_hash(&self) -> ContentHash {
        manifest::semantic_hash(DocKind::Part, &self.truth())
    }

    /// The hash of what decides the part's shape: the truth without the part's and
    /// features' names, repair-prompt labels, provenance, material and appearances. Renaming a feature changes
    /// the semantic hash and not this one.
    pub fn geometry_hash(&self) -> ContentHash {
        let mut truth = soliddraft_document::hash::without_format(&self.truth());
        if let Value::Object(map) = &mut truth {
            for key in ["name", "datum_names", "material", "appearances", "snapshots"] {
                map.remove(key);
            }
            if let Some(Value::Array(features)) = map.get_mut("features") {
                for f in features.iter_mut().filter_map(Value::as_object_mut) {
                    f.remove("name");
                    f.remove("via_assistant");
                }
            }
        }
        strip_labels(&mut truth);
        ContentHash::of_entries([(PART, &truth)])
    }

    /// The part's manifest as this version writes it.
    pub fn manifest(&self) -> Manifest {
        self.manifest_for(&Extras::default())
    }

    fn manifest_for(&self, extras: &Extras<'_>) -> Manifest {
        let backend = self.kernel.backend();
        let mut m = Manifest::new(self.id, DocKind::Part, self.semantic_hash());
        m.kernel = Some(format!("{} {}", backend.name, backend.version));
        m.geometry_hash = Some(self.geometry_hash());
        m.sections = vec![Section::required(PART)];
        if !self.settings.is_default() {
            m.sections.push(Section::optional(settings::ENTRY));
        }
        if self.saves_display_data(extras) {
            m.sections.push(Section::optional(display::FOLDER));
        }
        if extras.thumbnail.is_some() {
            m.sections.push(Section::optional(manifest::THUMBNAIL));
        }
        m
    }

    /// True when saving writes display data: the settings ask for it, the save does not
    /// leave it out and there is a body.
    fn saves_display_data(&self, extras: &Extras<'_>) -> bool {
        self.settings.save_display_data && !extras.no_display_data && self.body().is_some()
    }

    /// The part file's bytes. With display data on, the part is tessellated at its
    /// coarse and standard levels; a kernel that cannot do it leaves the data out
    /// rather than failing the save.
    pub fn to_bytes(&self) -> Result<Vec<u8>, FileError> {
        self.to_bytes_with(&Extras::default())
    }

    /// The part file's bytes, with what `extras` adds or leaves out.
    pub fn to_bytes_with(&self, extras: &Extras<'_>) -> Result<Vec<u8>, FileError> {
        let mut entries = vec![Entry::new(PART, self.part_json().into_bytes())];
        if !self.settings.is_default() {
            entries.push(Entry::new(settings::ENTRY, self.settings.to_json().into_bytes()));
        }
        let mut m = self.manifest_for(extras);
        if self.saves_display_data(extras) {
            match self.display_data(&self.settings.viewport, &display::SAVED) {
                Some(Ok(data)) => {
                    for d in data {
                        entries.push(Entry::new(display::entry(d.level), d.mesh.encode(&d.made_by)));
                    }
                }
                _ => m.sections.retain(|s| s.name != display::FOLDER),
            }
        }
        if let Some(png) = extras.thumbnail {
            entries.push(Entry::stored(manifest::THUMBNAIL, png));
        }
        Ok(manifest::write_package(&m, &entries)?)
    }

    /// Writes the part file atomically: a failed save never leaves a half-written part.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), FileError> {
        Ok(soliddraft_document::save_atomic(path.as_ref(), &self.to_bytes()?)?)
    }

    /// Reads a part from a file's bytes and rebuilds it on `kernel`. A file from before
    /// the envelope gets the id derived from its content.
    pub fn from_bytes(bytes: &[u8], kernel: Arc<dyn Kernel>) -> Result<Document, FileError> {
        let mut package = Package::from_bytes(bytes)?;
        let Some(m) = package.manifest().cloned() else {
            let text = package
                .read_text(LEGACY_PART_TRUTH)?
                .ok_or_else(|| FileError::Archive("no manifest.json or recipe.json".into()))?;
            return Self::read_truth(&text, kernel, 1);
        };
        m.check(DocKind::Part, &[PART])?;
        let text = package.read_text(PART)?.ok_or_else(|| FileError::Archive("no part.json".into()))?;
        let mut doc = Self::read_truth(&text, kernel, 2)?;
        doc.id = m.id;
        if let Ok(Some(text)) = package.read_text(settings::ENTRY) {
            doc.settings = Settings::from_json(&text).unwrap_or_default();
        }
        Ok(doc)
    }

    /// Reads a part from its `part.json` (or a pre-envelope `recipe.json`) text and
    /// rebuilds it on `kernel`. Without a manifest there is no recorded id, so the part
    /// gets the id derived from its content, as a pre-envelope file does.
    pub fn from_part_json(text: &str, kernel: Arc<dyn Kernel>) -> Result<Document, FileError> {
        Self::read_truth(text, kernel, 2)
    }

    /// Reads truth text whose format's major version is at most `newest`.
    fn read_truth(text: &str, kernel: Arc<dyn Kernel>, newest: u32) -> Result<Document, FileError> {
        let probe: Value = serde_json::from_str(text)?;
        let found = probe.get("format").and_then(|f| f.as_str()).unwrap_or_default().to_string();
        if !format_major(&found, "soliddraft-part").is_some_and(|m| (1..=newest).contains(&m)) {
            return Err(FileError::Format { found });
        }
        let recipe: Recipe = serde_json::from_str(text)?;
        let mut ids: Vec<FeatureId> = recipe.features.iter().map(|f| f.id).collect();
        ids.sort();
        if ids.windows(2).any(|w| w[0] == w[1]) {
            return Err(FileError::Invalid("two features share an id".into()));
        }
        if ids.last().is_some_and(|id| id.0 >= recipe.next_feature) {
            return Err(FileError::Invalid("next_feature is not above every feature id".into()));
        }
        if let Some(r) = recipe.rollback
            && !ids.contains(&r)
        {
            return Err(FileError::Invalid(format!("the rollback bar names {r}, which is not a feature")));
        }
        // A feature may read one that was deleted (it reports that when it rebuilds),
        // but never one below it.
        for (i, f) in recipe.features.iter().enumerate() {
            for dep in f.kind.dependencies() {
                if recipe.features[i..].iter().any(|g| g.id == dep) {
                    return Err(FileError::Invalid(format!("{} reads {dep}, which comes after it", f.id)));
                }
            }
        }
        let mut materials =
            PartMaterials { material: recipe.material, appearances: recipe.appearances, snapshots: recipe.snapshots };
        materials.snapshots.normalise();
        materials.check().map_err(FileError::Invalid)?;
        let mut doc = Document::new(recipe.name, kernel);
        doc.id = manifest::legacy_identity(DocKind::Part, &probe, Vec::new()).id;
        doc.materials = materials;
        for (datum, name) in recipe.datum_names {
            doc.rename_datum(datum, &name).map_err(|e| FileError::Invalid(e.to_string()))?;
        }
        doc.features = recipe.features;
        doc.rollback = recipe.rollback;
        doc.next_id = recipe.next_feature;
        doc.rebuild();
        Ok(doc)
    }

    /// Opens a part file and rebuilds it on `kernel`.
    pub fn open(path: impl AsRef<Path>, kernel: Arc<dyn Kernel>) -> Result<Document, FileError> {
        Self::from_bytes(&std::fs::read(path)?, kernel)
    }
}

/// What a save adds to the part file or leaves out of it, beyond the truth.
#[derive(Clone, Copy, Debug, Default)]
pub struct Extras<'a> {
    /// A PNG of the part to store as `thumbnail.png`.
    pub thumbnail: Option<&'a [u8]>,
    /// Leave out the display meshes whatever the settings say, for a quick copy such as
    /// autosave's recovery copy. The part opens from it with a rebuild.
    pub no_display_data: bool,
}

/// Removes every `label` key: the words a reference keeps for repair prompts.
fn strip_labels(v: &mut Value) {
    match v {
        Value::Object(map) => {
            map.remove("label");
            map.values_mut().for_each(strip_labels);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_labels),
        _ => {}
    }
}
