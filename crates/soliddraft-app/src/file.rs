//! Opening and saving part files, and the recent-files list.
//!
//! The format is the model's (`soliddraft_model::file`): a document envelope with the
//! part's manifest and its feature recipe. Parts are saved as `.dwprt`; `.dwpart`
//! files from before the envelope open, and saving one writes a `.dwprt` beside it.

use crate::document::Document;
use soliddraft_kernel_api::Kernel;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use soliddraft_document::DocKind;
pub use soliddraft_model::file::{EXTENSION, FORMAT, LEGACY_EXTENSION};

/// The extensions the Open dialog offers: parts and drawings, current and old names.
pub const OPEN_EXTENSIONS: [&str; 4] = [
    EXTENSION,
    soliddraft_document::DRAWING_EXTENSION,
    LEGACY_EXTENSION,
    soliddraft_document::LEGACY_DRAWING_EXTENSION,
];
const RECENT_LIMIT: usize = 8;

/// Saves the part atomically, under `.dwprt` when `path` names a `.dwpart` or has no
/// extension. The part keeps its id, whatever the name.
pub fn save(doc: &mut Document, path: &Path) -> Result<(), String> {
    let path = &with_extension(path);
    #[cfg(not(target_arch = "wasm32"))]
    doc.model().save(path).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    // The browser downloads the file instead.
    #[cfg(target_arch = "wasm32")]
    crate::platform::write_file(path, &doc.model().to_bytes().map_err(|e| format!("cannot save the part: {e}"))?)?;
    doc.path = Some(path.to_path_buf());
    doc.name = part_name(path);
    doc.dirty = false;
    remember(path);
    Ok(())
}

/// Opens a part file and rebuilds it. A part with a broken feature still opens;
/// the tree shows what failed.
pub fn open(path: &Path, kernel: Arc<dyn Kernel>) -> Result<Document, String> {
    let model = soliddraft_model::Document::open(path, kernel).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut doc = Document::from_model(model);
    doc.name = part_name(path);
    doc.path = Some(path.to_path_buf());
    doc.dirty = false;
    remember(path);
    Ok(doc)
}

/// Opens a part from the bytes of a file called `name`, such as one uploaded to the
/// browser, and rebuilds it.
pub fn open_bytes(name: &str, bytes: &[u8], kernel: Arc<dyn Kernel>) -> Result<Document, String> {
    let model = soliddraft_model::Document::from_bytes(bytes, kernel).map_err(|e| format!("{name}: {e}"))?;
    let mut doc = Document::from_model(model);
    doc.name = part_name(Path::new(name));
    doc.dirty = false;
    Ok(doc)
}

/// The part name shown in the title and the tree: the file stem.
pub fn part_name(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Part1".to_owned())
}

/// The path a part is saved at: `.dwprt` added when the user typed no extension, and
/// in place of `.dwpart`.
pub fn with_extension(path: &Path) -> PathBuf {
    soliddraft_document::kind::saved_path(path, DocKind::Part)
}

static CONFIG_DIR: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Points the recent-files list and the settings file somewhere else, for tests and automation.
pub fn set_config_dir(dir: Option<PathBuf>) {
    *CONFIG_DIR.lock().unwrap() = dir;
}

/// Where the recent-files list and the settings file live.
pub(crate) fn config_dir() -> Option<PathBuf> {
    config_dirs().map(|(current, _)| current)
}

fn config_dirs() -> Option<(PathBuf, Option<PathBuf>)> {
    if let Some(d) = CONFIG_DIR.lock().unwrap().clone() {
        return Some((d, None));
    }
    if let Some(d) = soliddraft_assistant::environment::var_os("SOLIDDRAFT_CONFIG_DIR") {
        return Some((PathBuf::from(d), None));
    }
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    base.map(|b| (b.join("soliddraft"), Some(b.join("draftwell"))))
}

/// Reads an old config file only when its SolidDraft counterpart is absent.
pub(crate) fn config_read_file(relative: &str) -> Option<PathBuf> {
    let (current, legacy) = config_dirs()?;
    Some(config_read_path(current.join(relative), legacy.map(|d| d.join(relative))))
}

fn config_read_path(current: PathBuf, legacy: Option<PathBuf>) -> PathBuf {
    if !current.try_exists().unwrap_or(true)
        && let Some(old) = legacy
        && old.try_exists().unwrap_or(false)
    {
        return old;
    }
    current
}

fn recent_file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("recent.json"))
}

/// The most recently used part and drawing files, newest first, existing files only.
pub fn recent() -> Vec<PathBuf> {
    let Some(file) = config_read_file("recent.json") else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(file) else { return Vec::new() };
    let list: Vec<PathBuf> = serde_json::from_str(&text).unwrap_or_default();
    list.into_iter().filter(|p| p.exists()).collect()
}

pub fn remember(path: &Path) {
    let Some(file) = recent_file() else { return };
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut list = recent();
    list.retain(|p| *p != path);
    list.insert(0, path);
    list.truncate(RECENT_LIMIT);
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(&list) {
        let _ = std::fs::write(file, text);
    }
}

pub fn forget_all() {
    if let Some(file) = recent_file() {
        let _ = soliddraft_document::save_atomic(&file, b"[]");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Command, ExtrudeParams, FeatureKind};

    #[test]
    fn config_falls_back_per_file_and_preserves_current_values() {
        let dir = std::env::temp_dir().join(format!("soliddraft-config-fallback-{}", std::process::id()));
        let current = dir.join("soliddraft/settings.json");
        let legacy = dir.join("draftwell/settings.json");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, b"old settings").unwrap();
        assert_eq!(config_read_path(current.clone(), Some(legacy.clone())), legacy);
        assert_eq!(config_read_path(current.clone(), None), current, "explicit directories are isolated");
        soliddraft_document::save_atomic(&current, b"").unwrap();
        assert_eq!(config_read_path(current.clone(), Some(legacy)), current, "an empty current file wins");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_part_round_trips_through_its_file() {
        let dir = std::env::temp_dir().join(format!("soliddraft-file-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        set_config_dir(Some(dir.clone()));
        let kernel: Arc<dyn Kernel> = Arc::new(soliddraft_kernel_smlib::SmlibKernel::new());
        let mut doc = Document::new("Bracket", kernel.clone());
        doc.apply(Command::AddExtrude { params: ExtrudeParams::default(), via_assistant: false }).unwrap();
        let path = dir.join(format!("bracket.{EXTENSION}"));
        save(&mut doc, &path).unwrap();
        assert!(!doc.dirty);
        let back = open(&path, kernel.clone()).unwrap();
        assert_eq!(back.name, "bracket");
        assert_eq!(back.model().id(), doc.model().id(), "the part keeps its id");
        assert_eq!(back.features.len(), 1);
        assert!(matches!(back.features[0].kind, FeatureKind::Extrude(_)));
        assert_eq!(back.triangle_count(), doc.triangle_count());
        assert_eq!(recent().first().map(|p| p.file_name().unwrap().to_owned()), path.file_name().map(|s| s.to_owned()));

        // A part opened from a .dwpart is saved as .dwprt with the same id.
        let old = dir.join("old.dwpart");
        std::fs::copy(&path, &old).unwrap();
        let mut reopened = open(&old, kernel).unwrap();
        save(&mut reopened, &old).unwrap();
        assert_eq!(reopened.path.as_deref(), Some(dir.join("old.dwprt").as_path()));
        assert_eq!(soliddraft_model::file::FORMAT, "soliddraft-part/2");
        let saved = std::fs::read(dir.join("old.dwprt")).unwrap();
        assert_eq!(soliddraft_document::identify(std::io::Cursor::new(&saved)).unwrap().id, doc.model().id());
        assert_eq!(with_extension(Path::new("x")), Path::new("x.dwprt"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
