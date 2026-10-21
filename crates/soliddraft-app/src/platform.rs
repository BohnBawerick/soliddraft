//! What differs between the desktop and the browser: file dialogs and writing files.
//!
//! On the desktop a dialog asks for a path and files go to disk. The browser has no file
//! system: saving downloads the file under the name it would have had, and opening
//! uploads one, which arrives a frame or more later through [`take_upload`].

use std::path::{Path, PathBuf};

/// Asks where to save a file. In the browser the file downloads to wherever the browser
/// puts downloads, so this returns `default_name` without asking.
pub fn save_path(title: &str, filter: &str, extension: &str, default_name: &str) -> Option<PathBuf> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        rfd::FileDialog::new().add_filter(filter, &[extension]).set_file_name(default_name).set_title(title).save_file()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (title, filter, extension);
        Some(PathBuf::from(default_name))
    }
}

/// Asks for a file with one of `extensions` to open. In the browser this starts an
/// upload and returns `None`; the file arrives later through [`take_upload`].
pub fn open_path(title: &str, filter: &str, extensions: &[&str]) -> Option<PathBuf> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        rfd::FileDialog::new().add_filter(filter, extensions).set_title(title).pick_file()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (title, filter);
        crate::web::upload(extensions);
        None
    }
}

/// Asks for a folder, for File locations. The browser has no folders to pick.
pub fn pick_folder(title: &str) -> Option<PathBuf> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        rfd::FileDialog::new().set_title(title).pick_folder()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = title;
        None
    }
}

/// Writes a file atomically, making its directory first, so a failed write never leaves
/// a half-written file. In the browser the file downloads instead, under the path's file
/// name.
pub fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        soliddraft_document::save_atomic(path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "download".into());
        crate::web::download(&name, bytes)
    }
}

/// The file the user last uploaded, with its name, once it has arrived. Always `None` on
/// the desktop, where opening a file is immediate.
pub fn take_upload() -> Option<(String, Vec<u8>)> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
    #[cfg(target_arch = "wasm32")]
    {
        crate::web::take_upload()
    }
}
