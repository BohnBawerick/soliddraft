//! Backup files: earlier versions of a document, kept when a save replaces it.
//!
//! Before a save writes over a file whose bytes differ, the file's bytes are copied to
//! backup 1 and the older backups move up one number. Backups past the count are
//! deleted. A save that writes the same bytes keeps the backups as they are, so saving
//! an unchanged document never pushes an older version out.
//!
//! Backups sit beside the document as `<file name>.<n>.bak`, such as
//! `bracket.dwprt.1.bak`, or in a backup folder. In a backup folder the name also
//! carries a tag of the document's folder (`bracket.dwprt.1f9c02ab.1.bak`), so two
//! documents with the same file name in different folders keep separate backups. The
//! `.bak` ending keeps backups out of the folder index: a backup has its document's id,
//! and the resolver must never take it for the document.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::save::save_atomic;

/// How many backups a save keeps, and where.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Backups {
    /// How many earlier versions to keep. 0 keeps none.
    pub count: usize,
    /// The folder backups go to, or `None` for beside the document.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<PathBuf>,
}

impl Default for Backups {
    fn default() -> Backups {
        Backups { count: 1, folder: None }
    }
}

/// What happened to the backups when a document was saved.
#[derive(Debug)]
pub enum BackupOutcome {
    /// The previous version was kept as backup 1.
    Kept(PathBuf),
    /// No backup was made: backups are off, the file is new, or the bytes did not change.
    Unchanged,
    /// The backup could not be made. The document was saved anyway.
    Failed(std::io::Error),
}

impl Backups {
    /// The path of backup `n` (1 is the newest) of the document at `document`.
    pub fn path(&self, document: &Path, n: usize) -> PathBuf {
        let name = document.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match &self.folder {
            None => document.with_file_name(format!("{name}.{n}.bak")),
            Some(folder) => folder.join(format!("{name}.{}.{n}.bak", folder_tag(document))),
        }
    }

    /// The backups of `document` that exist, newest first.
    pub fn list(&self, document: &Path) -> Vec<PathBuf> {
        (1..).map(|n| self.path(document, n)).take_while(|p| p.is_file()).collect()
    }

    /// Saves `bytes` to `document` atomically, keeping the previous version as a backup
    /// first. A backup that cannot be made does not stop the save; the outcome says so.
    pub fn save(&self, document: &Path, bytes: &[u8]) -> std::io::Result<BackupOutcome> {
        let outcome = match self.keep(document, bytes) {
            Ok(Some(path)) => BackupOutcome::Kept(path),
            Ok(None) => BackupOutcome::Unchanged,
            Err(e) => BackupOutcome::Failed(e),
        };
        save_atomic(document, bytes)?;
        Ok(outcome)
    }

    /// Copies the file at `document` to backup 1 when `bytes` differ from it, after
    /// moving the older backups up and deleting those past the count.
    fn keep(&self, document: &Path, bytes: &[u8]) -> std::io::Result<Option<PathBuf>> {
        if self.count == 0 {
            return Ok(None);
        }
        let previous = match std::fs::read(document) {
            Ok(previous) => previous,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        if previous == bytes {
            return Ok(None);
        }
        // Backups past the count, including any left from a larger count, go first.
        let mut n = self.count;
        while self.path(document, n).is_file() {
            n += 1;
        }
        for stale in (self.count..n).rev() {
            std::fs::remove_file(self.path(document, stale))?;
        }
        for k in (1..self.count).rev() {
            let from = self.path(document, k);
            if from.is_file() {
                std::fs::rename(&from, self.path(document, k + 1))?;
            }
        }
        let first = self.path(document, 1);
        save_atomic(&first, &previous)?;
        Ok(Some(first))
    }
}

/// Eight hex digits of the BLAKE3 hash of the document's folder, made absolute.
fn folder_tag(document: &Path) -> String {
    let folder = document.parent().unwrap_or(Path::new(""));
    let folder = std::path::absolute(folder).unwrap_or_else(|_| folder.to_path_buf());
    let hash = blake3::hash(folder.to_string_lossy().as_bytes());
    hash.to_hex()[..8].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("soliddraft-backup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn read(path: &Path) -> Vec<u8> {
        std::fs::read(path).unwrap()
    }

    #[test]
    fn saves_rotate_the_backups_beside_the_document() {
        let dir = folder("beside");
        let part = dir.join("bracket.dwprt");
        let backups = Backups { count: 2, folder: None };
        assert!(matches!(backups.save(&part, b"one").unwrap(), BackupOutcome::Unchanged), "a new file has none");
        assert!(
            matches!(backups.save(&part, b"two").unwrap(), BackupOutcome::Kept(p) if p == dir.join("bracket.dwprt.1.bak"))
        );
        backups.save(&part, b"three").unwrap();
        backups.save(&part, b"four").unwrap();
        assert_eq!(read(&part), b"four");
        let kept = backups.list(&part);
        assert_eq!(kept, [dir.join("bracket.dwprt.1.bak"), dir.join("bracket.dwprt.2.bak")]);
        assert_eq!((read(&kept[0]), read(&kept[1])), (b"three".to_vec(), b"two".to_vec()), "newest first");
        // Saving the same bytes again keeps the older versions.
        assert!(matches!(backups.save(&part, b"four").unwrap(), BackupOutcome::Unchanged));
        assert_eq!(read(&kept[1]), b"two");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_smaller_count_deletes_the_extra_backups_and_zero_keeps_none() {
        let dir = folder("count");
        let part = dir.join("p.dwprt");
        let three = Backups { count: 3, folder: None };
        for bytes in [b"a", b"b", b"c", b"d"] {
            three.save(&part, bytes).unwrap();
        }
        assert_eq!(three.list(&part).len(), 3);
        let one = Backups { count: 1, folder: None };
        one.save(&part, b"e").unwrap();
        assert_eq!(one.list(&part), [dir.join("p.dwprt.1.bak")]);
        assert_eq!(read(&dir.join("p.dwprt.1.bak")), b"d");
        assert!(!dir.join("p.dwprt.2.bak").exists() && !dir.join("p.dwprt.3.bak").exists());
        let none = Backups { count: 0, folder: None };
        assert!(matches!(none.save(&part, b"f").unwrap(), BackupOutcome::Unchanged));
        assert_eq!(read(&dir.join("p.dwprt.1.bak")), b"d", "backups are left alone when off");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_backup_folder_keeps_same_named_documents_apart() {
        let dir = folder("folder");
        let store = dir.join("backups");
        let backups = Backups { count: 1, folder: Some(store.clone()) };
        let (a, b) = (dir.join("a/part.dwprt"), dir.join("b/part.dwprt"));
        for (path, first, second) in [(&a, b"a1", b"a2"), (&b, b"b1", b"b2")] {
            backups.save(path, first).unwrap();
            backups.save(path, second).unwrap();
        }
        let (ka, kb) = (backups.path(&a, 1), backups.path(&b, 1));
        assert_ne!(ka, kb);
        assert!(ka.starts_with(&store) && ka.to_string_lossy().ends_with(".1.bak"));
        assert_eq!((read(&ka), read(&kb)), (b"a1".to_vec(), b"b1".to_vec()));
        assert!(!dir.join("a/part.dwprt.1.bak").exists(), "nothing beside the document");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_backup_that_cannot_be_made_does_not_stop_the_save() {
        let dir = folder("fail");
        let part = dir.join("p.dwprt");
        // The backup folder is a file, so no backup can be written into it.
        let blocker = dir.join("not-a-folder");
        std::fs::write(&blocker, b"").unwrap();
        let backups = Backups { count: 1, folder: Some(blocker) };
        backups.save(&part, b"one").unwrap();
        assert!(matches!(backups.save(&part, b"two").unwrap(), BackupOutcome::Failed(_)));
        assert_eq!(read(&part), b"two");
        let _ = std::fs::remove_dir_all(dir);
    }
}
