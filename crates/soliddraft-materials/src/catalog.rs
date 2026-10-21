//! The catalog: every library the app has loaded, in priority order.
//!
//! The order is SolidDraft's built-in libraries, then the user's own library ("My
//! library", a file in the configuration folder), then the files File locations lists
//! for material and appearance libraries, in the order listed. An id means one entry:
//! when two libraries use the same id the earlier one wins and the later entry is
//! reported as shadowed, never silently swapped in.
//!
//! A document never depends on the catalog to keep its look or its mass: it holds a
//! copy of every material and appearance it uses (see [`crate::Snapshots`]). The catalog
//! answers what is available, and whether a document's copy still matches its library.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use soliddraft_document::{FileLocations, LocationKind};

use crate::appearance::{Appearance, AppearanceId};
use crate::library::{Library, LibraryError, Origin};
use crate::material::{Material, MaterialId};

/// SolidDraft's starter materials.
const BUILT_IN_MATERIALS: &str = include_str!("../libraries/materials.toml");
/// SolidDraft's starter appearances.
const BUILT_IN_APPEARANCES: &str = include_str!("../libraries/appearances.toml");

/// A problem found while loading the catalog. Loading goes on without the entry or file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogIssue {
    /// A library file could not be read.
    Unreadable {
        /// The file.
        path: PathBuf,
        /// Why.
        message: String,
    },
    /// An entry's id is already taken by an earlier library's entry, which wins.
    Shadowed {
        /// "material" or "appearance".
        kind: &'static str,
        /// The id.
        id: String,
        /// The library whose entry is hidden.
        library: String,
        /// The library whose entry wins.
        by: String,
    },
    /// A material names a default appearance no library has.
    MissingAppearance {
        /// The material.
        material: String,
        /// The appearance it names.
        appearance: String,
    },
    /// An entry has a value out of range or another problem.
    Entry {
        /// The library.
        library: String,
        /// The entry's id.
        id: String,
        /// What is wrong.
        message: String,
    },
}

impl std::fmt::Display for CatalogIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CatalogIssue::Unreadable { path, message } => write!(f, "{} could not be read: {message}", path.display()),
            CatalogIssue::Shadowed { kind, id, library, by } => {
                write!(f, "the {kind} {id} in {library} is hidden by the one in {by}")
            }
            CatalogIssue::MissingAppearance { material, appearance } => {
                write!(f, "{material} names the appearance {appearance}, which no library has")
            }
            CatalogIssue::Entry { library, id, message } => write!(f, "{id} in {library}: {message}"),
        }
    }
}

/// How a document's copy of a material or appearance compares with the libraries.
#[derive(Debug, Clone, PartialEq)]
pub enum LibraryMatch<T> {
    /// The library holds the same values.
    Same {
        /// The library's name.
        library: String,
    },
    /// The library's entry has other values; the properties panel says "differs from
    /// library" and offers to update the document.
    Differs {
        /// The library's name.
        library: String,
        /// The library's values.
        current: T,
    },
    /// No library has the id; the document's copy is all there is.
    NotInLibrary,
}

/// Every loaded library, in priority order, with what went wrong loading them.
#[derive(Debug, Clone)]
pub struct Catalog {
    /// The libraries, highest priority first.
    pub libraries: Vec<Library>,
    /// Problems found while loading.
    pub issues: Vec<CatalogIssue>,
    materials: HashMap<MaterialId, (usize, usize)>,
    appearances: HashMap<AppearanceId, (usize, usize)>,
}

impl Catalog {
    /// SolidDraft's built-in libraries, read-only.
    pub fn built_in_libraries() -> Vec<Library> {
        [BUILT_IN_MATERIALS, BUILT_IN_APPEARANCES]
            .into_iter()
            .map(|text| Library::parse(text, Origin::BuiltIn, None).expect("the built-in libraries parse"))
            .collect()
    }

    /// The built-in libraries alone.
    pub fn built_in() -> Catalog {
        Catalog::from_libraries(Catalog::built_in_libraries(), Vec::new())
    }

    /// The built-in libraries, the user's library at `user_library` when it exists,
    /// and the library files File locations lists. A file listed for both materials
    /// and appearances, or the user's library listed again, is read once.
    pub fn load(user_library: Option<&Path>, locations: &FileLocations) -> Catalog {
        let mut libraries = Catalog::built_in_libraries();
        let mut issues = Vec::new();
        let mut seen: Vec<PathBuf> = Vec::new();
        let mut read = |path: &Path, origin: Origin, libraries: &mut Vec<Library>| {
            let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            if seen.contains(&key) {
                return;
            }
            seen.push(key);
            match Library::load(path, origin) {
                Ok(lib) => libraries.push(lib),
                Err(LibraryError::Io { error, .. }) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => issues.push(CatalogIssue::Unreadable { path: path.to_path_buf(), message: e.to_string() }),
            }
        };
        if let Some(p) = user_library {
            read(p, Origin::User, &mut libraries);
        }
        for kind in [LocationKind::MaterialLibraries, LocationKind::AppearanceLibraries] {
            for p in locations.files(kind) {
                read(&p, Origin::Location, &mut libraries);
            }
        }
        Catalog::from_libraries(libraries, issues)
    }

    /// A catalog of these libraries, in this order.
    pub fn from_libraries(libraries: Vec<Library>, mut issues: Vec<CatalogIssue>) -> Catalog {
        let mut materials: HashMap<MaterialId, (usize, usize)> = HashMap::new();
        let mut appearances: HashMap<AppearanceId, (usize, usize)> = HashMap::new();
        for (li, lib) in libraries.iter().enumerate() {
            for p in lib.problems() {
                issues.push(CatalogIssue::Entry { library: lib.name.clone(), id: p.id, message: p.message });
            }
            for (ei, m) in lib.materials.iter().enumerate() {
                if let Some(&(first, _)) = materials.get(&m.id) {
                    if first != li {
                        issues.push(CatalogIssue::Shadowed {
                            kind: "material",
                            id: m.id.0.clone(),
                            library: lib.name.clone(),
                            by: libraries[first].name.clone(),
                        });
                    }
                } else {
                    materials.insert(m.id.clone(), (li, ei));
                }
            }
            for (ei, a) in lib.appearances.iter().enumerate() {
                if let Some(&(first, _)) = appearances.get(&a.id) {
                    if first != li {
                        issues.push(CatalogIssue::Shadowed {
                            kind: "appearance",
                            id: a.id.0.clone(),
                            library: lib.name.clone(),
                            by: libraries[first].name.clone(),
                        });
                    }
                } else {
                    appearances.insert(a.id.clone(), (li, ei));
                }
            }
        }
        for lib in &libraries {
            for m in &lib.materials {
                if let Some(a) = &m.default_appearance
                    && !appearances.contains_key(a)
                {
                    issues.push(CatalogIssue::MissingAppearance { material: m.id.0.clone(), appearance: a.0.clone() });
                }
            }
        }
        Catalog { libraries, issues, materials, appearances }
    }

    /// The material with this id, and the library it comes from.
    pub fn material(&self, id: &MaterialId) -> Option<(&Library, &Material)> {
        self.materials.get(id).map(|&(l, e)| (&self.libraries[l], &self.libraries[l].materials[e]))
    }

    /// The appearance with this id, and the library it comes from.
    pub fn appearance(&self, id: &AppearanceId) -> Option<(&Library, &Appearance)> {
        self.appearances.get(id).map(|&(l, e)| (&self.libraries[l], &self.libraries[l].appearances[e]))
    }

    /// Every material that is not shadowed, by library in priority order, each library's
    /// in file order.
    pub fn materials(&self) -> impl Iterator<Item = (&Library, &Material)> {
        self.libraries.iter().enumerate().flat_map(move |(li, lib)| {
            lib.materials
                .iter()
                .enumerate()
                .filter(move |(ei, m)| self.materials.get(&m.id) == Some(&(li, *ei)))
                .map(move |(_, m)| (lib, m))
        })
    }

    /// Every appearance that is not shadowed, in the same order as
    /// [`Catalog::materials`].
    pub fn appearances(&self) -> impl Iterator<Item = (&Library, &Appearance)> {
        self.libraries.iter().enumerate().flat_map(move |(li, lib)| {
            lib.appearances
                .iter()
                .enumerate()
                .filter(move |(ei, a)| self.appearances.get(&a.id) == Some(&(li, *ei)))
                .map(move |(_, a)| (lib, a))
        })
    }

    /// The appearance a part of this material gets when it has none of its own: the
    /// material's default appearance when a library has it.
    pub fn default_appearance_of(&self, material: &Material) -> Option<&Appearance> {
        material.default_appearance.as_ref().and_then(|id| self.appearance(id)).map(|(_, a)| a)
    }

    /// How a document's copy of a material compares with the libraries.
    pub fn match_material(&self, copy: &Material) -> LibraryMatch<Material> {
        match self.material(&copy.id) {
            None => LibraryMatch::NotInLibrary,
            Some((lib, m)) if m == copy => LibraryMatch::Same { library: lib.name.clone() },
            Some((lib, m)) => LibraryMatch::Differs { library: lib.name.clone(), current: m.clone() },
        }
    }

    /// How a document's copy of an appearance compares with the libraries.
    pub fn match_appearance(&self, copy: &Appearance) -> LibraryMatch<Appearance> {
        match self.appearance(&copy.id) {
            None => LibraryMatch::NotInLibrary,
            Some((lib, a)) if a == copy => LibraryMatch::Same { library: lib.name.clone() },
            Some((lib, a)) => LibraryMatch::Differs { library: lib.name.clone(), current: a.clone() },
        }
    }

    /// The user's library, if it was loaded.
    pub fn user_library(&self) -> Option<&Library> {
        self.libraries.iter().find(|l| l.origin == Origin::User)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib(name: &str, origin: Origin, materials: &[&str], appearances: &[&str]) -> Library {
        let mut l = Library::new(name, origin);
        l.materials = materials.iter().map(|id| Material::new(*id, id.to_uppercase())).collect();
        l.appearances =
            appearances.iter().map(|id| Appearance::new(*id, id.to_uppercase(), crate::Srgb::WHITE)).collect();
        l
    }

    #[test]
    fn the_built_in_libraries_load_cleanly() {
        let c = Catalog::built_in();
        assert!(c.issues.is_empty(), "{:#?}", c.issues);
        let n = c.materials().count();
        assert!((35..=50).contains(&n), "about 40 starter materials, found {n}");
        for (_, m) in c.materials() {
            assert!(m.source.is_some(), "{} has no source", m.id);
            assert!(m.default_appearance.is_some(), "{} has no default appearance", m.id);
            assert!(!m.category.is_empty(), "{} has no category", m.id);
        }
        assert!(c.appearance(&AppearanceId(Appearance::DEFAULT_ID.into())).is_some());
    }

    #[test]
    fn earlier_libraries_win_and_shadowing_is_reported() {
        let mut first = lib("Built-in", Origin::BuiltIn, &["steel"], &["grey"]);
        first.materials[0].default_appearance = Some(AppearanceId("grey".into()));
        let mut mine = lib("Mine", Origin::User, &["steel", "oak"], &[]);
        mine.materials[1].default_appearance = Some(AppearanceId("varnish".into()));
        let c = Catalog::from_libraries(vec![first, mine], Vec::new());
        assert_eq!(c.material(&MaterialId("steel".into())).unwrap().0.name, "Built-in");
        let ids: Vec<&str> = c.materials().map(|(_, m)| m.id.as_str()).collect();
        assert_eq!(ids, ["steel", "oak"]);
        assert!(matches!(&c.issues[0], CatalogIssue::Shadowed { id, by, .. } if id == "steel" && by == "Built-in"));
        assert!(matches!(&c.issues[1], CatalogIssue::MissingAppearance { appearance, .. } if appearance == "varnish"));
        assert_eq!(c.user_library().unwrap().name, "Mine");
    }

    #[test]
    fn copies_compare_with_their_library() {
        let c = Catalog::from_libraries(vec![lib("L", Origin::User, &["steel"], &[])], Vec::new());
        let mut copy = c.material(&MaterialId("steel".into())).unwrap().1.clone();
        assert_eq!(c.match_material(&copy), LibraryMatch::Same { library: "L".into() });
        copy.density_kg_m3 = Some(7850.0);
        assert!(
            matches!(c.match_material(&copy), LibraryMatch::Differs { current, .. } if current.density_kg_m3.is_none())
        );
        assert_eq!(c.match_material(&Material::new("brass", "Brass")), LibraryMatch::NotInLibrary);
    }

    #[test]
    fn files_come_from_the_user_folder_and_file_locations() {
        let dir = std::env::temp_dir().join(format!("soliddraft-catalog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("shared")).unwrap();
        let mut mine = lib("Mine", Origin::User, &["mine-1"], &[]);
        mine.path = Some(dir.join("my-library.toml"));
        mine.save().unwrap();
        std::fs::write(dir.join("shared/team.toml"), lib("Team", Origin::Location, &["team-1"], &[]).to_toml())
            .unwrap();
        std::fs::write(dir.join("shared/broken.toml"), "format = 3").unwrap();
        let mut locations = FileLocations::new();
        locations.add(LocationKind::MaterialLibraries, dir.join("shared"));
        locations.add(LocationKind::AppearanceLibraries, dir.join("shared/team.toml"));
        locations.add(LocationKind::MaterialLibraries, dir.join("my-library.toml"));
        let c = Catalog::load(Some(&dir.join("my-library.toml")), &locations);
        let names: Vec<&str> = c.libraries.iter().skip(2).map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["Mine", "Team"], "each file once, the user's first");
        assert_eq!(c.libraries[2].origin, Origin::User);
        assert_eq!(c.libraries[3].origin, Origin::Location);
        assert!(matches!(&c.issues[..], [CatalogIssue::Unreadable { path, .. }] if path.ends_with("broken.toml")));
        let missing = Catalog::load(Some(&dir.join("nothing-yet.toml")), &FileLocations::new());
        assert!(missing.issues.is_empty(), "a user library not made yet is not a problem");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
