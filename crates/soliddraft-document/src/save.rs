//! Atomic saves: every document writer goes through [`save_atomic`].
//!
//! The bytes go to a temporary file in the target's folder, are flushed to disk, and
//! the temporary file is renamed over the target. A crash or a full disk during a save
//! leaves the previous file whole; the rename is atomic on every platform SolidDraft
//! runs on when both names are in one folder.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Writes `bytes` to `path` atomically, making its folder first.
pub fn save_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
    if let Some(dir) = dir {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = temporary_name(path);
    let written = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    // Make the rename itself durable. Folders cannot be opened for this on Windows.
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(dir.unwrap_or(Path::new("."))) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// A hidden name beside `path` that no other save in this process uses at the same time.
fn temporary_name(path: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{name}.{}-{n}.saving", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_replace_the_file_and_leave_nothing_behind() {
        let dir = std::env::temp_dir().join(format!("soliddraft-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("sub/part.dwprt");
        save_atomic(&path, b"one").unwrap();
        save_atomic(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        let names: Vec<_> = std::fs::read_dir(dir.join("sub")).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["part.dwprt"], "no temporary file is left");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_save_keeps_the_old_file() {
        let dir = std::env::temp_dir().join(format!("soliddraft-save-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // The target is a folder, so the rename fails after the bytes were written.
        let path = dir.join("taken");
        std::fs::create_dir_all(path.join("inside")).unwrap();
        assert!(save_atomic(&path, b"new").is_err());
        assert!(path.join("inside").is_dir(), "the old content is untouched");
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["taken"], "the temporary file is removed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
