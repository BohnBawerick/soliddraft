//! The app's settings file, `settings.json` in the config directory next to the
//! recent-files list. It holds the assistant's provider and model, whether the side area is
//! shown and how wide, the command line's chips and session height, the pinned and recent
//! commands, File locations: the folders and
//! files where reference files are found (`soliddraft_document::locations`), the
//! viewport's default look, and autosave, recovery and backups ([`DocumentSettings`]). The user's own material library sits beside it
//! ([`user_library_path`]).
//!
//! Saving rewrites only the sections the app knows and keeps every other key, so a
//! file written by a newer build survives an older one.

use crate::command_line::BarSettings;
use crate::commands::History;
use crate::panels::Layout;
use crate::viewport::look::Preset;
use serde_json::Value;
use soliddraft_assistant::AssistantSettings;
use soliddraft_document::{AutosavePolicy, Backups, FileLocations};
use soliddraft_materials::{Catalog, Library, Origin};
use std::path::{Path, PathBuf};

fn settings_file() -> Option<PathBuf> {
    crate::file::config_dir().map(|d| d.join("settings.json"))
}

fn read(file: &Path) -> Value {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Default::default()))
}

/// The assistant settings, or the defaults when the file is missing or unreadable.
pub fn assistant() -> AssistantSettings {
    crate::file::config_read_file("settings.json").map(|f| assistant_in(&f)).unwrap_or_default()
}

/// Saves the assistant settings.
pub fn save_assistant(settings: &AssistantSettings) -> Result<(), String> {
    let Some(file) = settings_file() else { return Err("no config directory".into()) };
    save_assistant_in(&file, settings)
}

fn assistant_in(file: &Path) -> AssistantSettings {
    serde_json::from_value(read(file)["assistant"].clone()).unwrap_or_default()
}

fn save_assistant_in(file: &Path, settings: &AssistantSettings) -> Result<(), String> {
    save_section_in(file, "assistant", settings)
}

/// The panel layout, or the default when the file is missing or unreadable.
pub fn layout() -> Layout {
    crate::file::config_read_file("settings.json").map(|f| layout_in(&f)).unwrap_or_default()
}

/// Saves the panel layout.
pub fn save_layout(layout: &Layout) -> Result<(), String> {
    let Some(file) = settings_file() else { return Err("no config directory".into()) };
    save_layout_in(&file, layout)
}

pub(crate) fn layout_in(file: &Path) -> Layout {
    serde_json::from_value::<Layout>(read(file)["layout"].clone()).unwrap_or_default().clamped()
}

pub(crate) fn save_layout_in(file: &Path, layout: &Layout) -> Result<(), String> {
    save_section_in(file, "layout", layout)
}

/// The command line's chips and session height, or the defaults when the file is missing
/// or unreadable. Ids no command has any more are dropped.
pub fn command_line() -> BarSettings {
    crate::file::config_read_file("settings.json").map(|f| command_line_in(&f)).unwrap_or_default()
}

/// Saves the command line's chips and session height.
pub fn save_command_line(bar: &BarSettings) -> Result<(), String> {
    let Some(file) = settings_file() else { return Err("no config directory".into()) };
    save_section_in(&file, "command_line", bar)
}

fn command_line_in(file: &Path) -> BarSettings {
    serde_json::from_value::<BarSettings>(read(file)["command_line"].clone()).unwrap_or_default().cleaned()
}

/// The pinned and recent commands, or none when the file is missing or unreadable.
/// Ids no command has any more are dropped.
pub fn command_history() -> History {
    crate::file::config_read_file("settings.json").map(|f| command_history_in(&f)).unwrap_or_default()
}

/// Saves the pinned and recent commands.
pub fn save_command_history(history: &History) -> Result<(), String> {
    let Some(file) = settings_file() else { return Err("no config directory".into()) };
    save_section_in(&file, "commands", history)
}

fn command_history_in(file: &Path) -> History {
    let mut history: History = serde_json::from_value(read(file)["commands"].clone()).unwrap_or_default();
    history.retain_known();
    history
}

/// File locations, or none when the file is missing or unreadable.
pub fn file_locations() -> FileLocations {
    crate::file::config_read_file("settings.json").map(|f| file_locations_in(&f)).unwrap_or_default()
}

/// Saves File locations.
pub fn save_file_locations(locations: &FileLocations) -> Result<(), String> {
    let Some(file) = settings_file() else { return Err("no config directory".into()) };
    save_file_locations_in(&file, locations)
}

/// File locations from the settings file at `file`.
pub fn file_locations_in(file: &Path) -> FileLocations {
    serde_json::from_value(read(file)["file_locations"].clone()).unwrap_or_default()
}

/// Saves File locations into the settings file at `file`, keeping its other keys.
pub fn save_file_locations_in(file: &Path, locations: &FileLocations) -> Result<(), String> {
    save_section_in(file, "file_locations", locations)
}

/// The user's own material and appearance library, "My library":
/// `libraries/my-library.toml` in the config directory. It may not exist yet; saving
/// it creates it.
pub fn user_library_path() -> Option<PathBuf> {
    crate::file::config_dir().map(|d| d.join("libraries").join("my-library.toml"))
}

/// Every material and appearance library: the built-in ones, the user's own library
/// and those File locations lists.
pub fn material_catalog() -> Catalog {
    Catalog::load(crate::file::config_read_file("libraries/my-library.toml").as_deref(), &file_locations())
}

/// The user's own library, loaded, or a new empty one at its path when it does not
/// exist yet.
pub fn user_library() -> Result<Library, String> {
    let path = user_library_path().ok_or("no config directory")?;
    if let Some(source) = crate::file::config_read_file("libraries/my-library.toml")
        && source.exists()
    {
        let mut lib = Library::load(&source, Origin::User).map_err(|e| e.to_string())?;
        lib.path = Some(path);
        return Ok(lib);
    }
    let mut lib = Library::new("My library", Origin::User);
    lib.path = Some(path);
    Ok(lib)
}

/// Viewport settings: the look the viewport starts in.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ViewSettings {
    pub preset: Preset,
}

/// The look the viewport starts in, Studio when the file does not say.
pub fn default_preset() -> Preset {
    crate::file::config_read_file("settings.json").map(|f| view_in(&f)).unwrap_or_default().preset
}

/// Saves the look the viewport starts in.
pub fn save_default_preset(preset: Preset) -> Result<(), String> {
    let Some(file) = settings_file() else { return Err("no config directory".into()) };
    save_section_in(&file, "view", &ViewSettings { preset })
}

fn view_in(file: &Path) -> ViewSettings {
    serde_json::from_value(read(file)["view"].clone()).unwrap_or_default()
}

/// How documents are protected: autosave's interval and edit count, where recovery
/// copies go, and how many backups a save keeps and where. The `documents` key of the
/// settings file.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct DocumentSettings {
    pub autosave: AutosavePolicy,
    /// Where recovery copies go. `None` is `recovery` in the config directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_folder: Option<PathBuf>,
    pub backups: Backups,
}

impl DocumentSettings {
    /// The recovery folder: the one set, or `recovery` in the config directory.
    pub fn recovery_folder(&self) -> Option<PathBuf> {
        self.recovery_folder.clone().or_else(|| crate::file::config_dir().map(|d| d.join("recovery")))
    }
}

/// The document settings, or the defaults when the file is missing or unreadable.
pub fn documents() -> DocumentSettings {
    crate::file::config_read_file("settings.json").map(|f| documents_in(&f)).unwrap_or_default()
}

/// Saves the document settings.
pub fn save_documents(documents: &DocumentSettings) -> Result<(), String> {
    let Some(file) = settings_file() else { return Err("no config directory".into()) };
    save_section_in(&file, "documents", documents)
}

fn documents_in(file: &Path) -> DocumentSettings {
    serde_json::from_value(read(file)["documents"].clone()).unwrap_or_default()
}

/// Rewrites one section atomically and keeps the others as they are.
fn save_section_in(file: &Path, key: &str, value: &impl serde::Serialize) -> Result<(), String> {
    let source = if settings_file().as_deref() == Some(file) {
        crate::file::config_read_file("settings.json").unwrap_or_else(|| file.to_owned())
    } else {
        file.to_owned()
    };
    let mut all = read(&source);
    all[key] = serde_json::to_value(value).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?;
    soliddraft_document::save_atomic(file, text.as_bytes()).map_err(|e| format!("cannot write {}: {e}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soliddraft_assistant::ProviderKind;

    #[test]
    fn command_history_persists_beside_other_keys() {
        let dir = std::env::temp_dir().join(format!("soliddraft-commands-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        std::fs::write(&file, r#"{"theme":"dark","commands":{"pinned":["fillet","not_a_command"]}}"#).unwrap();
        let mut h = command_history_in(&file);
        assert_eq!(h.pinned, ["fillet"]);
        assert!(h.recent.is_empty());
        h.record("extrude");
        save_section_in(&file, "commands", &h).unwrap();
        assert_eq!(command_history_in(&file), h);
        let all: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(all["theme"], "dark");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_command_line_persists_beside_other_keys() {
        let dir = std::env::temp_dir().join(format!("soliddraft-command-line-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        std::fs::write(&file, r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(command_line_in(&file), BarSettings::default());
        let mut bar = BarSettings::default();
        bar.place("fillet", Some(0));
        bar.session_height = 410.0;
        save_section_in(&file, "command_line", &bar).unwrap();
        assert_eq!(command_line_in(&file), bar);
        std::fs::write(&file, r#"{"command_line":{"tools":["gone","fillet"],"session_height":5}}"#).unwrap();
        let back = command_line_in(&file);
        assert_eq!(back.tools, ["fillet"]);
        assert_eq!(back.session_height, crate::command_line::SESSION_MIN_HEIGHT);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn assistant_settings_persist_beside_other_keys() {
        let dir = std::env::temp_dir().join(format!("soliddraft-settings-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        assert_eq!(assistant_in(&file), AssistantSettings::default());
        std::fs::write(&file, r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(assistant_in(&file), AssistantSettings::default());
        let mut s = AssistantSettings { provider: Some(ProviderKind::OpenRouter), ..Default::default() };
        s.set_model(ProviderKind::OpenRouter, "anthropic/claude-fable-5.1");
        save_assistant_in(&file, &s).unwrap();
        assert_eq!(assistant_in(&file), s);
        let all: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(all["theme"], "dark");
        assert_eq!(all["assistant"]["provider"], "openrouter");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_default_look_is_studio_until_saved() {
        let dir = std::env::temp_dir().join(format!("soliddraft-view-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        assert_eq!(view_in(&file).preset, Preset::Studio);
        std::fs::write(&file, r#"{"view":{"preset":"cinema"},"theme":"dark"}"#).unwrap();
        assert_eq!(view_in(&file).preset, Preset::Studio, "an unknown look falls back to the default");
        save_section_in(&file, "view", &ViewSettings { preset: Preset::Realistic }).unwrap();
        assert_eq!(view_in(&file).preset, Preset::Realistic);
        let all: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(all["view"]["preset"], "realistic");
        assert_eq!(all["theme"], "dark");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn document_settings_default_and_persist() {
        let dir = std::env::temp_dir().join(format!("soliddraft-documents-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        std::fs::write(&file, r#"{"documents":{"autosave":{"interval_seconds":30}},"theme":"dark"}"#).unwrap();
        let mut d = documents_in(&file);
        assert_eq!(d.autosave, AutosavePolicy { interval_seconds: 30, ..Default::default() }, "missing keys default");
        assert_eq!(d.backups, Backups::default());
        d.backups = Backups { count: 3, folder: Some(dir.join("backups")) };
        d.recovery_folder = Some(dir.join("recovery"));
        save_section_in(&file, "documents", &d).unwrap();
        assert_eq!(documents_in(&file), d);
        assert_eq!(d.recovery_folder(), Some(dir.join("recovery")));
        let all: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(all["theme"], "dark");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn file_locations_persist_beside_other_keys() {
        use soliddraft_document::LocationKind;
        let dir = std::env::temp_dir().join(format!("soliddraft-locations-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        std::fs::write(&file, r#"{"assistant":{"provider":"openrouter"}}"#).unwrap();
        assert_eq!(file_locations_in(&file), FileLocations::new());
        let mut f = FileLocations::new();
        f.add(LocationKind::DrawingTemplates, "/shared/templates");
        f.add(LocationKind::ReferencedDocuments, "/shared/parts");
        save_file_locations_in(&file, &f).unwrap();
        assert_eq!(file_locations_in(&file), f);
        assert_eq!(assistant_in(&file).provider, Some(ProviderKind::OpenRouter), "the assistant section is kept");
        let _ = std::fs::remove_dir_all(dir);
    }
}
