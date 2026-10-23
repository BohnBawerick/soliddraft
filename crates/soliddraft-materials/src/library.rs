//! Library files: materials and appearances people pick from.
//!
//! A library is a TOML file:
//!
//! ```toml
//! format = "soliddraft-library/1"
//! name = "Workshop stock"
//! source = "Supplier data sheets, 2026"   # the default for entries without their own
//! licence = "CC0-1.0"                       # likewise
//!
//! [[material]]
//! id = "workshop-6082-t6"
//! name = "Aluminium 6082-T6"
//! category = "Aluminium alloys"
//! density_kg_m3 = 2700.0
//! default_appearance = "aluminium-brushed"
//!
//! [[appearance]]
//! id = "workshop-blue"
//! name = "Workshop blue"
//! base_color = "#1f4e8c"
//! ```
//!
//! One file can hold materials, appearances or both. SolidDraft's own libraries are
//! compiled in and read-only; to change an entry, duplicate it into a library of your
//! own. Libraries are found through File locations (see [`crate::catalog`]).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::appearance::{Appearance, AppearanceId};
use crate::material::{Material, MaterialId};

/// The format tag of library files this version writes.
pub const FORMAT: &str = "soliddraft-library/1";

/// Why a library could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    /// Reading or writing the file failed.
    #[error("{path}: {error}")]
    Io {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        error: std::io::Error,
    },
    /// The file is not a library this version reads.
    #[error("{path}: {message}")]
    Parse {
        /// The file, or the library's name for one not read from a file.
        path: PathBuf,
        /// What is wrong, with the line when TOML knows it.
        message: String,
    },
    /// The library is SolidDraft's own.
    #[error("{0} is read-only; duplicate its entries into a library of your own")]
    ReadOnly(String),
    /// The library has no file to save to yet.
    #[error("{0} has no file yet")]
    NoFile(String),
}

/// Where a library came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// Compiled into SolidDraft. Read-only.
    BuiltIn,
    /// The user's own library in the configuration folder: "My library".
    User,
    /// A file listed under File locations, such as a team's shared library.
    Location,
    /// Read from a file another way, such as an import not saved yet.
    File,
}

impl Origin {
    /// How the materials panel heads the library's group.
    pub fn label(self) -> &'static str {
        match self {
            Origin::BuiltIn => "Built-in",
            Origin::User => "My library",
            Origin::Location => "File locations",
            Origin::File => "File",
        }
    }
}

/// A library's file form.
#[derive(Serialize, Deserialize)]
struct LibraryFile {
    format: String,
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    licence: Option<String>,
    #[serde(default, rename = "material", skip_serializing_if = "Vec::is_empty")]
    materials: Vec<Material>,
    #[serde(default, rename = "appearance", skip_serializing_if = "Vec::is_empty")]
    appearances: Vec<Appearance>,
}

/// A library of materials and appearances.
#[derive(Clone, Debug, PartialEq)]
pub struct Library {
    /// The library's name: "SolidDraft materials", "Workshop stock".
    pub name: String,
    /// Where it came from.
    pub origin: Origin,
    /// Its file; `None` for a built-in library or one not saved yet.
    pub path: Option<PathBuf>,
    /// Where entries without their own source come from.
    pub source: Option<String>,
    /// The licence of entries without their own.
    pub licence: Option<String>,
    /// The materials, in file order. Each carries its source and licence, the
    /// library's when it has none of its own.
    pub materials: Vec<Material>,
    /// The appearances, in file order.
    pub appearances: Vec<Appearance>,
}

/// Something wrong inside a library that does not stop it loading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryProblem {
    /// The entry's id.
    pub id: String,
    /// What is wrong.
    pub message: String,
}

impl Library {
    /// An empty library, not saved anywhere yet.
    pub fn new(name: impl Into<String>, origin: Origin) -> Library {
        Library {
            name: name.into(),
            origin,
            path: None,
            source: None,
            licence: None,
            materials: Vec::new(),
            appearances: Vec::new(),
        }
    }

    /// Reads a library from TOML text. `path` names it in errors and is kept.
    pub fn parse(text: &str, origin: Origin, path: Option<&Path>) -> Result<Library, LibraryError> {
        let shown = || path.map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("library"));
        let file: LibraryFile =
            toml::from_str(text).map_err(|e| LibraryError::Parse { path: shown(), message: e.to_string() })?;
        let major = file
            .format
            .strip_prefix("soliddraft-library/")
            .or_else(|| file.format.strip_prefix("draftwell-library/"))
            .and_then(|v| v.parse::<u32>().ok());
        if major != Some(1) {
            let message = format!("the file is {:?}; this version reads {FORMAT}", file.format);
            return Err(LibraryError::Parse { path: shown(), message });
        }
        let mut materials = file.materials;
        for m in &mut materials {
            if m.source.is_none() {
                m.source.clone_from(&file.source);
            }
            if m.licence.is_none() {
                m.licence.clone_from(&file.licence);
            }
        }
        Ok(Library {
            name: file.name,
            origin,
            path: path.map(Path::to_path_buf),
            source: file.source,
            licence: file.licence,
            materials,
            appearances: file.appearances,
        })
    }

    /// Reads a library file.
    pub fn load(path: &Path, origin: Origin) -> Result<Library, LibraryError> {
        let text =
            std::fs::read_to_string(path).map_err(|error| LibraryError::Io { path: path.to_path_buf(), error })?;
        Library::parse(&text, origin, Some(path))
    }

    /// The library as TOML. An entry's source and licence are left out where they are
    /// the library's, so reading and writing a file keeps it as it was.
    pub fn to_toml(&self) -> String {
        let materials = self
            .materials
            .iter()
            .map(|m| {
                let mut m = m.clone();
                if m.source == self.source {
                    m.source = None;
                }
                if m.licence == self.licence {
                    m.licence = None;
                }
                m
            })
            .collect();
        let file = LibraryFile {
            format: FORMAT.into(),
            name: self.name.clone(),
            source: self.source.clone(),
            licence: self.licence.clone(),
            materials,
            appearances: self.appearances.clone(),
        };
        toml::to_string(&file).expect("a library always serialises")
    }

    /// True for a library that cannot be saved: SolidDraft's own.
    pub fn is_read_only(&self) -> bool {
        self.origin == Origin::BuiltIn
    }

    /// Writes the library to its file, atomically.
    pub fn save(&self) -> Result<(), LibraryError> {
        if self.is_read_only() {
            return Err(LibraryError::ReadOnly(self.name.clone()));
        }
        let Some(path) = &self.path else { return Err(LibraryError::NoFile(self.name.clone())) };
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|error| LibraryError::Io { path: dir.to_path_buf(), error })?;
        }
        soliddraft_document::save_atomic(path, self.to_toml().as_bytes())
            .map_err(|error| LibraryError::Io { path: path.clone(), error })
    }

    /// A material by id.
    pub fn material(&self, id: &MaterialId) -> Option<&Material> {
        self.materials.iter().find(|m| &m.id == id)
    }

    /// An appearance by id.
    pub fn appearance(&self, id: &AppearanceId) -> Option<&Appearance> {
        self.appearances.iter().find(|a| &a.id == id)
    }

    /// Adds a material, or replaces the one with its id. Returns true when it replaced
    /// one.
    pub fn put_material(&mut self, material: Material) -> bool {
        match self.materials.iter_mut().find(|m| m.id == material.id) {
            Some(slot) => {
                *slot = material;
                true
            }
            None => {
                self.materials.push(material);
                false
            }
        }
    }

    /// Adds an appearance, or replaces the one with its id. Returns true when it
    /// replaced one.
    pub fn put_appearance(&mut self, appearance: Appearance) -> bool {
        match self.appearances.iter_mut().find(|a| a.id == appearance.id) {
            Some(slot) => {
                *slot = appearance;
                true
            }
            None => {
                self.appearances.push(appearance);
                false
            }
        }
    }

    /// Removes a material. Returns false when the library has no such material.
    pub fn remove_material(&mut self, id: &MaterialId) -> bool {
        let before = self.materials.len();
        self.materials.retain(|m| &m.id != id);
        self.materials.len() != before
    }

    /// Removes an appearance. Returns false when the library has no such appearance.
    pub fn remove_appearance(&mut self, id: &AppearanceId) -> bool {
        let before = self.appearances.len();
        self.appearances.retain(|a| &a.id != id);
        self.appearances.len() != before
    }

    /// Adds another library's entries, as importing into this library does. An entry
    /// whose id this library already uses, or that `material_taken` or
    /// `appearance_taken` says another library uses, gets a fresh id made from its
    /// name, and a material naming a renamed appearance follows it. Entries keep their
    /// source and licence. Returns each renamed id as (old, new).
    pub fn merge(
        &mut self,
        incoming: Library,
        material_taken: &dyn Fn(&MaterialId) -> bool,
        appearance_taken: &dyn Fn(&AppearanceId) -> bool,
    ) -> Vec<(String, String)> {
        let mut renamed = Vec::new();
        let mut appearance_ids = Vec::new();
        for mut a in incoming.appearances {
            let base = a.id.0.clone();
            let free = |id: &str| {
                let id = AppearanceId(id.to_owned());
                self.appearance(&id).is_none() && !appearance_taken(&id)
            };
            let id = unique(&base, |c| !free(c));
            if id != base {
                renamed.push((base.clone(), id.clone()));
            }
            appearance_ids.push((base, id.clone()));
            a.id = AppearanceId(id);
            self.appearances.push(a);
        }
        for mut m in incoming.materials {
            let base = m.id.0.clone();
            let free = |id: &str| {
                let id = MaterialId(id.to_owned());
                self.material(&id).is_none() && !material_taken(&id)
            };
            let id = unique(&base, |c| !free(c));
            if id != base {
                renamed.push((base, id.clone()));
            }
            m.id = MaterialId(id);
            if let Some(d) = &mut m.default_appearance
                && let Some((_, new)) = appearance_ids.iter().find(|(old, _)| *old == d.0)
            {
                d.0.clone_from(new);
            }
            if m.source.is_none() {
                m.source.clone_from(&incoming.source);
            }
            if m.licence.is_none() {
                m.licence.clone_from(&incoming.licence);
            }
            self.materials.push(m);
        }
        renamed
    }

    /// What is wrong with the library's entries: an id used twice, or a field out of
    /// range. Entries load anyway; the panel shows the problems beside them.
    pub fn problems(&self) -> Vec<EntryProblem> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for m in &self.materials {
            if !seen.insert(("material", m.id.as_str())) {
                out.push(EntryProblem { id: m.id.0.clone(), message: "another material has this id".into() });
            }
            for p in m.problems() {
                out.push(EntryProblem { id: m.id.0.clone(), message: p.to_string() });
            }
        }
        for a in &self.appearances {
            if !seen.insert(("appearance", a.id.as_str())) {
                out.push(EntryProblem { id: a.id.0.clone(), message: "another appearance has this id".into() });
            }
            for p in a.problems() {
                out.push(EntryProblem { id: a.id.0.clone(), message: p.to_string() });
            }
        }
        out
    }

    /// An id not used by any material in the library, made from `name`: lower case,
    /// words joined by hyphens, with "-2", "-3" and so on added when needed.
    pub fn fresh_material_id(&self, name: &str) -> MaterialId {
        let base = slug(name);
        let taken = |id: &str| self.materials.iter().any(|m| m.id.0 == id);
        MaterialId(unique(&base, taken))
    }

    /// An id not used by any appearance in the library, made from `name` as
    /// [`Library::fresh_material_id`] makes one.
    pub fn fresh_appearance_id(&self, name: &str) -> AppearanceId {
        let base = slug(name);
        let taken = |id: &str| self.appearances.iter().any(|a| a.id.0 == id);
        AppearanceId(unique(&base, taken))
    }
}

/// `name` as an id: lower-case ASCII letters and digits, other runs replaced by one
/// hyphen. "Aluminium 6061-T6 (sheet)" becomes "aluminium-6061-t6-sheet".
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_owned();
    if out.is_empty() { "entry".into() } else { out }
}

pub(crate) fn unique(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_owned();
    }
    (2..).map(|n| format!("{base}-{n}")).find(|c| !taken(c)).expect("some suffix is free")
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = r##"
format = "soliddraft-library/1"
name = "Workshop stock"
source = "Supplier data sheets"
licence = "CC0-1.0"

[[material]]
id = "w-6082"
name = "Aluminium 6082-T6"
density_kg_m3 = 2700.0

[[material]]
id = "w-pom"
name = "POM"
source = "Datasheet 12"

[[appearance]]
id = "w-blue"
name = "Workshop blue"
base_color = "#1f4e8c"
"##;

    #[test]
    fn entries_inherit_the_library_source_and_round_trip() {
        let lib = Library::parse(TEXT, Origin::Location, None).unwrap();
        assert_eq!(lib.materials[0].source.as_deref(), Some("Supplier data sheets"));
        assert_eq!(lib.materials[0].licence.as_deref(), Some("CC0-1.0"));
        assert_eq!(lib.materials[1].source.as_deref(), Some("Datasheet 12"), "its own source wins");
        let back = Library::parse(&lib.to_toml(), Origin::Location, None).unwrap();
        assert_eq!(back, lib);
        assert_eq!(lib.to_toml().matches("Supplier data sheets").count(), 1, "inherited values are not repeated");
        assert!(lib.problems().is_empty());
    }

    #[test]
    fn sources_and_conditions_round_trip_through_a_library_file() {
        use crate::provenance::{Conditions, Property, PropertySource, Provenance};
        let text = format!(
            "{TEXT}\n{}",
            r#"
[[material]]
id = "w-2024"
name = "Aluminium 2024-T3"
density_kg_m3 = 2768.0
yield_pa = 3.31e8

[material.provenance.yield_pa]
source = { kind = "reference", citation = "MIL-HDBK-5J, table 3.2.3.0(b)" }
conditions = { temperature_k = 294.0, form = "sheet", treatment = "T3", direction = "longitudinal" }

[material.provenance.density_kg_m3.source]
kind = "user_override"
note = "weighed"
"#
        );
        let lib = Library::parse(&text, Origin::User, None).unwrap();
        let m = lib.material(&MaterialId("w-2024".into())).unwrap();
        assert_eq!(
            m.provenance_of(Property::Yield),
            Provenance {
                source: PropertySource::Reference { citation: "MIL-HDBK-5J, table 3.2.3.0(b)".into() },
                conditions: Conditions {
                    temperature_k: Some(294.0),
                    form: Some("sheet".into()),
                    treatment: Some("T3".into()),
                    direction: Some("longitudinal".into()),
                    ..Conditions::default()
                },
            }
        );
        assert_eq!(m.source_of(Property::Density), &PropertySource::UserOverride { note: Some("weighed".into()) });
        assert_eq!(m.source, Some("Supplier data sheets".into()), "the entry's source text is still inherited");
        assert!(
            lib.materials[0].source_of(Property::Density).is_unknown(),
            "the library's source text is not a value's"
        );
        let back = Library::parse(&lib.to_toml(), Origin::User, None).unwrap();
        assert_eq!(back, lib);
        assert!(lib.problems().is_empty());
    }

    #[test]
    fn built_in_entries_write_as_before() {
        for lib in crate::Catalog::built_in_libraries() {
            assert!(!lib.to_toml().contains("provenance"), "{} gained provenance", lib.name);
        }
    }

    #[test]
    fn other_formats_and_bad_toml_are_refused() {
        let old = TEXT.replace(FORMAT, "draftwell-library/1");
        let lib = Library::parse(&old, Origin::User, None).unwrap();
        assert!(lib.to_toml().contains(FORMAT));
        assert_eq!(Library::parse(TEXT, Origin::User, None).unwrap(), lib);
        assert!(
            Library::parse(&old.replace("draftwell-library/1", "draftwell-library/2"), Origin::User, None).is_err()
        );
        let newer = TEXT.replace("soliddraft-library/1", "soliddraft-library/2");
        assert!(matches!(Library::parse(&newer, Origin::File, None), Err(LibraryError::Parse { .. })));
        let err = Library::parse("format = ", Origin::File, Some(Path::new("x.toml"))).unwrap_err();
        assert!(err.to_string().starts_with("x.toml: "), "{err}");
    }

    #[test]
    fn editing_keeps_ids_unique() {
        let mut lib = Library::parse(TEXT, Origin::User, None).unwrap();
        let mut copy = lib.materials[0].clone();
        copy.id = lib.fresh_material_id(&copy.name);
        assert_eq!(copy.id.0, "aluminium-6082-t6");
        assert!(!lib.put_material(copy.clone()));
        assert_eq!(lib.fresh_material_id("Aluminium 6082 T6").0, "aluminium-6082-t6-2");
        assert!(lib.put_material(copy.clone()), "same id replaces");
        assert!(lib.remove_material(&copy.id));
        assert!(!lib.remove_material(&copy.id));
        lib.materials.push(lib.materials[0].clone());
        assert_eq!(lib.problems()[0].message, "another material has this id");
        assert_eq!(slug("  Ti-6Al-4V (annealed) "), "ti-6al-4v-annealed");
        assert_eq!(slug("§"), "entry");
    }

    #[test]
    fn merging_renames_taken_ids_and_follows_appearances() {
        let mut mine = Library::parse(TEXT, Origin::User, None).unwrap();
        let mut incoming = Library::new("Imported", Origin::File);
        incoming.source = Some("Imported from x.sldmat".into());
        let mut m = Material::new("w-6082", "Clash");
        m.default_appearance = Some(AppearanceId("w-blue".into()));
        incoming.materials = vec![m, Material::new("abs", "ABS")];
        incoming.appearances = vec![Appearance::new("w-blue", "Clash blue", crate::Srgb::WHITE)];
        let built_in = |id: &MaterialId| id.0 == "abs";
        let renamed = mine.merge(incoming, &built_in, &|_| false);
        assert_eq!(
            renamed,
            [
                ("w-blue".into(), "w-blue-2".into()),
                ("w-6082".into(), "w-6082-2".into()),
                ("abs".into(), "abs-2".into())
            ]
        );
        let clash = mine.material(&MaterialId("w-6082-2".into())).unwrap();
        assert_eq!(clash.default_appearance, Some(AppearanceId("w-blue-2".into())));
        assert_eq!(clash.source.as_deref(), Some("Imported from x.sldmat"));
        assert!(mine.problems().is_empty());
    }

    #[test]
    fn built_in_libraries_cannot_be_saved() {
        let lib = Library::new("SolidDraft materials", Origin::BuiltIn);
        assert!(matches!(lib.save(), Err(LibraryError::ReadOnly(_))));
        assert!(matches!(Library::new("Unsaved", Origin::User).save(), Err(LibraryError::NoFile(_))));
    }
}
