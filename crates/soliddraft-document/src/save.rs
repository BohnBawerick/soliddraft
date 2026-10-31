//! Atomic saves: every document writer goes through [`save_atomic`].
//!
//! The bytes go to a temporary file in the target's folder, are flushed to disk, and
//! the temporary file is renamed over the target. A crash or a full disk during a save
//! leaves the previous file whole; the rename is atomic on every platform SolidDraft
//! runs on when both names are in one folder.
//!
//! [`save_new`] publishes a file that must not exist yet: the complete temporary file is
//! linked under the target's name, which fails when the name is taken, even by a file
//! made a moment before. Nothing is ever replaced.

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

/// Writes `bytes` to `path`, which must not exist, making its folder first. The file
/// appears whole or not at all, and an existing file, even one made while this runs, is
/// left alone: that is an [`std::io::ErrorKind::AlreadyExists`] error.
pub fn save_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
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
        // A hard link never replaces its target, unlike a rename.
        std::fs::hard_link(&tmp, path)
    })();
    let _ = std::fs::remove_file(&tmp);
    written?;
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
    fn a_new_file_never_replaces_one_already_there() {
        let dir = std::env::temp_dir().join(format!("soliddraft-save-new-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("sub/new.dwasm");
        save_new(&path, b"one").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"one");
        let refused = save_new(&path, b"two").unwrap_err();
        assert_eq!(refused.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&path).unwrap(), b"one", "the file already there is kept");
        let names: Vec<_> = std::fs::read_dir(dir.join("sub")).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["new.dwasm"], "no temporary file is left");
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
