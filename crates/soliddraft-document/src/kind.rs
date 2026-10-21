//! Document kinds, their file extensions and their format tags.
//!
//! Each kind is saved as its own file: parts `.dwprt`, assemblies `.dwasm`, drawings
//! `.dwdrw`. Files under the names before phase 0 (`.dwpart` parts, `.dwdrawing`
//! drawings) still open, and saving one writes the current name beside it.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// The extension parts are saved under.
pub const PART_EXTENSION: &str = "dwprt";
/// The part extension before phase 0. Such files open and are saved as `.dwprt`.
pub const LEGACY_PART_EXTENSION: &str = "dwpart";
/// The extension assemblies are saved under (see `soliddraft-assembly`).
pub const ASSEMBLY_EXTENSION: &str = "dwasm";
/// The extension drawings are saved under.
pub const DRAWING_EXTENSION: &str = "dwdrw";
/// The drawing extension before phase 0. Such files open and are saved as `.dwdrw`.
pub const LEGACY_DRAWING_EXTENSION: &str = "dwdrawing";

/// The kind of a SolidDraft document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocKind {
    /// A part: features that build one body.
    Part,
    /// An assembly: placed parts and sub-assemblies.
    Assembly,
    /// A drawing: sheets of views of parts and assemblies.
    Drawing,
}

impl DocKind {
    /// Every kind.
    pub const ALL: [DocKind; 3] = [DocKind::Part, DocKind::Assembly, DocKind::Drawing];

    /// The kind as the manifest writes it: "part", "assembly", "drawing".
    pub fn as_str(self) -> &'static str {
        match self {
            DocKind::Part => "part",
            DocKind::Assembly => "assembly",
            DocKind::Drawing => "drawing",
        }
    }

    /// The kind in words for dialogs: "SolidDraft part".
    pub fn label(self) -> &'static str {
        match self {
            DocKind::Part => "SolidDraft part",
            DocKind::Assembly => "SolidDraft assembly",
            DocKind::Drawing => "SolidDraft drawing",
        }
    }

    /// The extension files of this kind are saved under, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            DocKind::Part => PART_EXTENSION,
            DocKind::Assembly => ASSEMBLY_EXTENSION,
            DocKind::Drawing => DRAWING_EXTENSION,
        }
    }

    /// The extension files of this kind had before phase 0, if any.
    pub fn legacy_extension(self) -> Option<&'static str> {
        match self {
            DocKind::Part => Some(LEGACY_PART_EXTENSION),
            DocKind::Assembly => None,
            DocKind::Drawing => Some(LEGACY_DRAWING_EXTENSION),
        }
    }

    /// Every extension files of this kind open under, the saved one first.
    pub fn read_extensions(self) -> &'static [&'static str] {
        match self {
            DocKind::Part => &[PART_EXTENSION, LEGACY_PART_EXTENSION],
            DocKind::Assembly => &[ASSEMBLY_EXTENSION],
            DocKind::Drawing => &[DRAWING_EXTENSION, LEGACY_DRAWING_EXTENSION],
        }
    }

    /// The kind a file extension opens as, ignoring case.
    pub fn from_extension(extension: &str) -> Option<DocKind> {
        let e = extension.to_ascii_lowercase();
        DocKind::ALL.into_iter().find(|k| k.read_extensions().contains(&e.as_str()))
    }

    /// The kind a path's extension opens as.
    pub fn of_path(path: &Path) -> Option<DocKind> {
        DocKind::from_extension(path.extension()?.to_str()?)
    }

    /// The format tag of the truth entry this version writes, such as
    /// "soliddraft-part/2".
    pub fn format(self) -> &'static str {
        match self {
            DocKind::Part => "soliddraft-part/2",
            DocKind::Assembly => "soliddraft-assembly/1",
            DocKind::Drawing => "soliddraft-drawing/2",
        }
    }

    /// The name of the truth entry in this version's files.
    pub fn truth_entry(self) -> &'static str {
        match self {
            DocKind::Part => "part.json",
            DocKind::Assembly => "assembly.json",
            DocKind::Drawing => "drawing.json",
        }
    }
}

impl std::fmt::Display for DocKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The major version of a format tag, such as 2 for "soliddraft-part/2" or
/// "soliddraft-part/2.1", accepting the earlier Draftwell prefix too.
pub fn format_major(tag: &str, prefix: &str) -> Option<u32> {
    let version = tag.strip_prefix(prefix).or_else(|| {
        let kind = prefix.strip_prefix("soliddraft-")?;
        tag.strip_prefix("draftwell-")?.strip_prefix(kind)
    })?;
    version.strip_prefix('/')?.split('.').next()?.parse().ok()
}

/// `path` with the extension its kind is saved under: a `.dwpart` path becomes `.dwprt`,
/// a `.dwdrawing` path `.dwdrw`, a path without an extension gets the kind's.
pub fn saved_path(path: &Path, kind: DocKind) -> std::path::PathBuf {
    match DocKind::of_path(path) {
        Some(k) if k == kind => path.with_extension(kind.extension()),
        _ if path.extension().is_none() => path.with_extension(kind.extension()),
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_map_to_kinds() {
        assert_eq!(DocKind::from_extension("dwprt"), Some(DocKind::Part));
        assert_eq!(DocKind::from_extension("DWPART"), Some(DocKind::Part));
        assert_eq!(DocKind::from_extension("dwasm"), Some(DocKind::Assembly));
        assert_eq!(DocKind::from_extension("dwdrawing"), Some(DocKind::Drawing));
        assert_eq!(DocKind::from_extension("dwdrw"), Some(DocKind::Drawing));
        assert_eq!(DocKind::Drawing.extension(), "dwdrw");
        assert_eq!(DocKind::from_extension("json"), None);
        assert_eq!(DocKind::Part.extension(), "dwprt");
        assert_eq!(DocKind::Drawing.extension(), DRAWING_EXTENSION);
    }

    #[test]
    fn saved_paths_take_the_current_extension() {
        assert_eq!(saved_path(Path::new("a/b.dwpart"), DocKind::Part), Path::new("a/b.dwprt"));
        assert_eq!(saved_path(Path::new("b"), DocKind::Part), Path::new("b.dwprt"));
        assert_eq!(saved_path(Path::new("b.dwdrawing"), DocKind::Drawing), Path::new("b.dwdrw"));
        assert_eq!(saved_path(Path::new("b.zip"), DocKind::Part), Path::new("b.zip"));
    }

    #[test]
    fn format_majors() {
        assert_eq!(format_major("soliddraft-part/2", "soliddraft-part"), Some(2));
        assert_eq!(format_major("soliddraft-part/1.3", "soliddraft-part"), Some(1));
        assert_eq!(format_major("soliddraft-drawing/1", "soliddraft-part"), None);
        assert_eq!(format_major("soliddraft-part", "soliddraft-part"), None);
        assert_eq!(format_major("draftwell-part/2.1", "soliddraft-part"), Some(2));
        assert_eq!(format_major("draftwell-drawing/1", "soliddraft-part"), None);
        assert_eq!(format_major("draftwell-parts/2", "soliddraft-part"), None);
    }
}
