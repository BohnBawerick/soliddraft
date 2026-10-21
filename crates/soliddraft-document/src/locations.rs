//! File locations: where the app finds reference files.
//!
//! One setting holds, per kind of reference file, an ordered list of folders and
//! files: material and appearance libraries, part, assembly and drawing templates,
//! sheet formats and title blocks, drafting styles, configuration tables, and the
//! folders searched for referenced documents. Built-in content stays compiled in; these
//! add to it, earlier entries first. A team shares a library or a template by listing
//! the same shared folder.
//!
//! The setting is plain data with a JSON form; the app keeps it in its settings file.
//! Kinds a newer build added are kept as they are, so an older build does not drop
//! them when it saves.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A kind of reference file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LocationKind {
    /// Folders searched for referenced documents that are not where a reference says.
    ReferencedDocuments,
    /// Material libraries.
    MaterialLibraries,
    /// Appearance libraries.
    AppearanceLibraries,
    /// Templates for new parts.
    PartTemplates,
    /// Templates for new assemblies.
    AssemblyTemplates,
    /// Templates for new drawings.
    DrawingTemplates,
    /// Sheet formats and title blocks.
    SheetFormats,
    /// Drafting (dimension) styles.
    DraftingStyles,
    /// Configuration tables.
    ConfigurationTables,
}

impl LocationKind {
    /// Every kind, in the order the settings page lists them.
    pub const ALL: [LocationKind; 9] = [
        LocationKind::ReferencedDocuments,
        LocationKind::MaterialLibraries,
        LocationKind::AppearanceLibraries,
        LocationKind::PartTemplates,
        LocationKind::AssemblyTemplates,
        LocationKind::DrawingTemplates,
        LocationKind::SheetFormats,
        LocationKind::DraftingStyles,
        LocationKind::ConfigurationTables,
    ];

    /// The key in the setting's JSON and on the command line: "drawing_templates".
    pub fn key(self) -> &'static str {
        match self {
            LocationKind::ReferencedDocuments => "referenced_documents",
            LocationKind::MaterialLibraries => "material_libraries",
            LocationKind::AppearanceLibraries => "appearance_libraries",
            LocationKind::PartTemplates => "part_templates",
            LocationKind::AssemblyTemplates => "assembly_templates",
            LocationKind::DrawingTemplates => "drawing_templates",
            LocationKind::SheetFormats => "sheet_formats",
            LocationKind::DraftingStyles => "drafting_styles",
            LocationKind::ConfigurationTables => "configuration_tables",
        }
    }

    /// The kind with this key.
    pub fn from_key(key: &str) -> Option<LocationKind> {
        LocationKind::ALL.into_iter().find(|k| k.key() == key.replace('-', "_"))
    }

    /// The kind's name on the settings page: "Drawing templates".
    pub fn label(self) -> &'static str {
        match self {
            LocationKind::ReferencedDocuments => "Referenced documents",
            LocationKind::MaterialLibraries => "Material libraries",
            LocationKind::AppearanceLibraries => "Appearance libraries",
            LocationKind::PartTemplates => "Part templates",
            LocationKind::AssemblyTemplates => "Assembly templates",
            LocationKind::DrawingTemplates => "Drawing templates",
            LocationKind::SheetFormats => "Sheet formats and title blocks",
            LocationKind::DraftingStyles => "Drafting styles",
            LocationKind::ConfigurationTables => "Configuration tables",
        }
    }

    /// The extensions of this kind's files. A folder listed for the kind contributes
    /// its files with these extensions. Empty for referenced documents, whose folders
    /// are searched, not listed.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            LocationKind::ReferencedDocuments => &[],
            LocationKind::MaterialLibraries | LocationKind::AppearanceLibraries => &["toml"],
            LocationKind::PartTemplates => &["dwprt"],
            LocationKind::AssemblyTemplates => &["dwasm"],
            LocationKind::DrawingTemplates | LocationKind::SheetFormats | LocationKind::DraftingStyles => &["toml"],
            LocationKind::ConfigurationTables => &["csv", "toml"],
        }
    }
}

impl std::fmt::Display for LocationKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.key())
    }
}

/// The File locations setting: folders and files per kind, in priority order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FileLocations {
    lists: BTreeMap<String, Vec<PathBuf>>,
}

impl FileLocations {
    /// No locations: built-in content only.
    pub fn new() -> FileLocations {
        FileLocations::default()
    }

    /// The folders and files listed for `kind`, in order.
    pub fn get(&self, kind: LocationKind) -> &[PathBuf] {
        self.lists.get(kind.key()).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Replaces the list for `kind`.
    pub fn set(&mut self, kind: LocationKind, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            self.lists.remove(kind.key());
        } else {
            self.lists.insert(kind.key().to_owned(), paths);
        }
    }

    /// Appends `path` to the list for `kind`. Returns false when it is already listed.
    pub fn add(&mut self, kind: LocationKind, path: impl Into<PathBuf>) -> bool {
        let path = path.into();
        let list = self.lists.entry(kind.key().to_owned()).or_default();
        if list.contains(&path) {
            return false;
        }
        list.push(path);
        true
    }

    /// Removes `path` from the list for `kind`. Returns false when it was not listed.
    pub fn remove(&mut self, kind: LocationKind, path: &Path) -> bool {
        let Some(list) = self.lists.get_mut(kind.key()) else { return false };
        let before = list.len();
        list.retain(|p| p != path);
        let removed = list.len() != before;
        if list.is_empty() {
            self.lists.remove(kind.key());
        }
        removed
    }

    /// Moves `path` one place earlier (`up`) or later in the list for `kind`. Returns
    /// false when it cannot move.
    pub fn shift(&mut self, kind: LocationKind, path: &Path, up: bool) -> bool {
        let Some(list) = self.lists.get_mut(kind.key()) else { return false };
        let Some(i) = list.iter().position(|p| p == path) else { return false };
        let j = if up { i.checked_sub(1) } else { (i + 1 < list.len()).then_some(i + 1) };
        let Some(j) = j else { return false };
        list.swap(i, j);
        true
    }

    /// The reference files of `kind`, in priority order: each listed file with one of
    /// the kind's extensions, and the files with those extensions directly inside each
    /// listed folder, sorted by name. Entries that do not exist are skipped.
    pub fn files(&self, kind: LocationKind) -> Vec<PathBuf> {
        let wanted = |p: &Path| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| kind.extensions().iter().any(|w| w.eq_ignore_ascii_case(e)))
        };
        let mut out = Vec::new();
        for entry in self.get(kind) {
            if entry.is_file() {
                if wanted(entry) && !out.contains(entry) {
                    out.push(entry.clone());
                }
            } else if let Ok(read) = std::fs::read_dir(entry) {
                let mut files: Vec<PathBuf> =
                    read.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_file() && wanted(p)).collect();
                files.sort();
                for f in files {
                    if !out.contains(&f) {
                        out.push(f);
                    }
                }
            }
        }
        out
    }

    /// The folders searched for referenced documents.
    pub fn search_folders(&self) -> &[PathBuf] {
        self.get(LocationKind::ReferencedDocuments)
    }

    /// Keys in the setting this build does not know, written by a newer one.
    pub fn unknown_keys(&self) -> Vec<&str> {
        self.lists.keys().map(String::as_str).filter(|k| LocationKind::from_key(k).is_none()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_keep_order_and_unknown_kinds() {
        let mut f: FileLocations =
            serde_json::from_str(r#"{"drawing_templates": ["/a", "/b"], "render_presets": ["/x"]}"#).unwrap();
        assert_eq!(f.get(LocationKind::DrawingTemplates), [PathBuf::from("/a"), PathBuf::from("/b")]);
        assert!(f.add(LocationKind::DrawingTemplates, "/c"));
        assert!(!f.add(LocationKind::DrawingTemplates, "/c"), "listed once");
        assert!(f.shift(LocationKind::DrawingTemplates, Path::new("/c"), true));
        assert_eq!(f.get(LocationKind::DrawingTemplates)[1], PathBuf::from("/c"));
        assert!(!f.shift(LocationKind::DrawingTemplates, Path::new("/a"), true), "already first");
        assert!(f.remove(LocationKind::DrawingTemplates, Path::new("/a")));
        assert!(!f.remove(LocationKind::MaterialLibraries, Path::new("/a")));
        assert_eq!(f.unknown_keys(), ["render_presets"]);
        let json = serde_json::to_value(&f).unwrap();
        assert_eq!(json["render_presets"][0], "/x", "a newer build's kinds survive");
        for k in LocationKind::ALL {
            assert_eq!(LocationKind::from_key(k.key()), Some(k));
        }
        assert_eq!(LocationKind::from_key("drawing-templates"), Some(LocationKind::DrawingTemplates));
    }

    #[test]
    fn folders_contribute_their_files_of_the_kind() {
        let dir = std::env::temp_dir().join(format!("soliddraft-locations-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("shared")).unwrap();
        for name in ["shared/b.toml", "shared/a.toml", "shared/notes.txt", "own.toml"] {
            std::fs::write(dir.join(name), "").unwrap();
        }
        let mut f = FileLocations::new();
        f.add(LocationKind::DrawingTemplates, dir.join("own.toml"));
        f.add(LocationKind::DrawingTemplates, dir.join("shared"));
        f.add(LocationKind::DrawingTemplates, dir.join("missing"));
        let files = f.files(LocationKind::DrawingTemplates);
        assert_eq!(files, [dir.join("own.toml"), dir.join("shared/a.toml"), dir.join("shared/b.toml")]);
        assert!(f.files(LocationKind::PartTemplates).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
