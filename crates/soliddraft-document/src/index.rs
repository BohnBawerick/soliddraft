//! The folder index: which documents are where, by id.
//!
//! The resolver falls back to it when a reference's stored path no longer holds its
//! target (the file was renamed or moved), and the repair dialog lists its entries as
//! candidates. Because every manifest lists the document's references, the same index
//! answers "where used": which documents reference a given one.
//!
//! An index is a scan of folders and their subfolders, reading only each file's
//! manifest. Hidden folders (a leading dot) are skipped.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::id::DocumentId;
use crate::kind::DocKind;
use crate::manifest::{Identity, identify_file};
use crate::reference::normalize;

/// How deep a scan goes below each root.
const MAX_DEPTH: usize = 16;

/// One document file the index found.
#[derive(Clone, Debug, PartialEq)]
pub struct IndexEntry {
    /// The file.
    pub path: PathBuf,
    /// What it is.
    pub identity: Identity,
}

/// Documents found under a set of folders.
#[derive(Clone, Debug, Default)]
pub struct FolderIndex {
    entries: Vec<IndexEntry>,
    unreadable: Vec<(PathBuf, String)>,
    seen: HashSet<PathBuf>,
}

impl FolderIndex {
    /// An empty index.
    pub fn new() -> FolderIndex {
        FolderIndex::default()
    }

    /// Indexes every document file under `roots` and their subfolders. A root may also
    /// be a single file. Roots that do not exist are skipped.
    pub fn scan<P: AsRef<Path>>(roots: impl IntoIterator<Item = P>) -> FolderIndex {
        let mut index = FolderIndex::new();
        for root in roots {
            index.add(root.as_ref());
        }
        index
    }

    /// Adds a folder (with its subfolders) or a single file to the index.
    pub fn add(&mut self, root: &Path) {
        if root.is_file() {
            self.add_file(root);
        } else {
            self.walk(root, 0);
        }
    }

    fn walk(&mut self, dir: &Path, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let Ok(read) = std::fs::read_dir(dir) else { return };
        let mut paths: Vec<PathBuf> = read.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for p in paths {
            let hidden = p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'));
            if hidden {
                continue;
            }
            if p.is_dir() {
                self.walk(&p, depth + 1);
            } else {
                self.add_file(&p);
            }
        }
    }

    /// Adds one file if it has a document extension. Returns true when it was indexed.
    pub fn add_file(&mut self, path: &Path) -> bool {
        if DocKind::of_path(path).is_none() {
            return false;
        }
        let key = std::fs::canonicalize(path).unwrap_or_else(|_| normalize(path));
        if !self.seen.insert(key) {
            return false;
        }
        match identify_file(path) {
            Ok(identity) => {
                self.entries.push(IndexEntry { path: path.to_path_buf(), identity });
                true
            }
            Err(e) => {
                self.unreadable.push((path.to_path_buf(), e.to_string()));
                false
            }
        }
    }

    /// Every indexed document, in scan order.
    pub fn entries(&self) -> &[IndexEntry] {
        &self.entries
    }

    /// Files with a document extension that could not be read, with why.
    pub fn unreadable(&self) -> &[(PathBuf, String)] {
        &self.unreadable
    }

    /// The files that hold document `id`. Usually one; copies of a file share its id.
    pub fn find(&self, id: DocumentId) -> impl Iterator<Item = &IndexEntry> {
        self.entries.iter().filter(move |e| e.identity.id == id)
    }

    /// The documents of one kind.
    pub fn of_kind(&self, kind: DocKind) -> impl Iterator<Item = &IndexEntry> {
        self.entries.iter().filter(move |e| e.identity.kind == kind)
    }

    /// The documents that reference document `id`: by id, or, for unbound references,
    /// by a stored path that leads to a file holding `id`.
    pub fn where_used(&self, id: DocumentId) -> Vec<&IndexEntry> {
        let holders: Vec<PathBuf> = self.find(id).map(|e| canonical(&e.path)).collect();
        self.entries
            .iter()
            .filter(|e| {
                e.identity.references.iter().any(|r| match r.id {
                    Some(rid) => rid == id,
                    None => holders.contains(&canonical(&r.stored_path(&e.path))),
                })
            })
            .collect()
    }
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| normalize(path))
}
