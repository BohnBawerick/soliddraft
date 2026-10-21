//! Finding the document a reference names.
//!
//! The lookup order, for a referencing document and one [`DocRef`]:
//!
//! 1. A document open in this session with the reference's id, because an edited,
//!    unsaved part must win over its bytes on disk.
//! 2. The stored relative path, accepted only when the file there has the reference's
//!    id and kind. When the stored path has a kind's old extension (`.dwpart`), the file
//!    beside it under the current one (`.dwprt`) with the same id comes first: it is the
//!    same document, saved since in the current format.
//! 3. The folder index: the referencing file's folder with its subfolders, and the
//!    search folders from File locations. This finds renamed and moved files.
//! 4. Otherwise the reference is missing, and the resolution lists candidates for the
//!    repair dialog: documents of the same kind, the same file name first.
//!
//! A same-named file with a different id is never taken; it is listed as a candidate.
//! Resolving never rewrites the stored path: a target found elsewhere says so, and the
//! referencing document updates its path when it is next saved.
//!
//! An unbound reference (no id) resolves by its stored path only: whatever document of
//! the right kind is there, or, for a source that is not a document at all (a test
//! recipe), the file itself without an identity.

use std::cell::OnceCell;
use std::path::{Path, PathBuf};

use crate::hash::ContentHash;
use crate::id::DocumentId;
use crate::index::{FolderIndex, IndexEntry};
use crate::kind::DocKind;
use crate::manifest::{Identity, identify_file};
use crate::reference::{DocRef, normalize, relative_path};

/// How many repair candidates a missing reference lists at most.
const MAX_CANDIDATES: usize = 20;

/// A document open in this session, as the resolver sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenDocument {
    /// Its id.
    pub id: DocumentId,
    /// Its kind.
    pub kind: DocKind,
    /// Its file, if it was saved or opened from one.
    pub path: Option<PathBuf>,
    /// Its revision now, including unsaved edits.
    pub revision: ContentHash,
}

/// Where a resolved target is.
#[derive(Clone, Debug, PartialEq)]
pub enum Location {
    /// Open in this session.
    Open(OpenDocument),
    /// A file on disk.
    File(PathBuf),
}

/// Which step of the lookup found the target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    /// A document open in this session.
    OpenDocument,
    /// The stored path.
    StoredPath,
    /// The current-format file beside a stored path with an old extension, such as the
    /// `.dwprt` beside a `.dwpart`.
    SavedSibling,
    /// The folder index: the file was renamed or moved.
    Index,
}

/// A reference's target, found.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    /// Where it is.
    pub location: Location,
    /// How it was found.
    pub via: Via,
    /// What it is; `None` for a source that is not a SolidDraft document, such as a test
    /// recipe an unbound reference points at.
    pub identity: Option<Identity>,
    /// The path the reference should store from now on, relative to the referencing
    /// file, when it differs from the stored one.
    pub new_path: Option<String>,
    /// Other files that hold the same id (copies), which the lookup passed over.
    pub duplicates: Vec<PathBuf>,
}

/// How a found target relates to the revision the reference pins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefStatus {
    /// The target is at the pinned revision.
    Current,
    /// The target moved on: the referencing document shows the pinned revision and
    /// offers a Rebuild.
    NewerAvailable {
        /// The target's revision now.
        now: ContentHash,
    },
    /// The target has no revision to compare: it is not a SolidDraft document.
    Unverified,
}

impl Target {
    /// The target's file, when it has one.
    pub fn path(&self) -> Option<&Path> {
        match &self.location {
            Location::Open(d) => d.path.as_deref(),
            Location::File(p) => Some(p),
        }
    }

    /// The target's id, when it has one.
    pub fn id(&self) -> Option<DocumentId> {
        match &self.location {
            Location::Open(d) => Some(d.id),
            Location::File(_) => self.identity.as_ref().map(|i| i.id),
        }
    }

    /// The target's current revision, when it has one.
    pub fn revision(&self) -> Option<&ContentHash> {
        match &self.location {
            Location::Open(d) => Some(&d.revision),
            Location::File(_) => self.identity.as_ref().map(|i| &i.revision),
        }
    }

    /// Whether the target is at the revision `r` pins.
    pub fn status(&self, r: &DocRef) -> RefStatus {
        match self.revision() {
            None => RefStatus::Unverified,
            Some(now) if *now == r.pinned => RefStatus::Current,
            Some(now) => RefStatus::NewerAvailable { now: now.clone() },
        }
    }
}

/// Why nothing acceptable was at the stored path.
#[derive(Clone, Debug, PartialEq)]
pub enum StoredPathProblem {
    /// No file there.
    NotFound,
    /// A document with another id is there: a different part with the same name.
    DifferentDocument(DocumentId),
    /// A document of another kind is there.
    WrongKind(DocKind),
    /// The file cannot be read as a document.
    Unreadable(String),
}

/// A document the repair dialog can offer for a missing reference.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    /// The file.
    pub path: PathBuf,
    /// What it is.
    pub identity: Identity,
    /// True when its file name matches the stored path's.
    pub same_name: bool,
}

/// A missing reference: what was wrong at the stored path, and what could replace it.
#[derive(Clone, Debug, PartialEq)]
pub struct Missing {
    /// What the stored path held.
    pub stored: StoredPathProblem,
    /// Documents of the reference's kind, the same file name first.
    pub candidates: Vec<Candidate>,
}

/// The outcome of resolving a reference.
#[derive(Clone, Debug, PartialEq)]
pub enum Resolution {
    /// The target was found.
    Found(Target),
    /// It was not: the referencing document keeps its pinned state and the reference
    /// needs a repair.
    Missing(Missing),
}

impl Resolution {
    /// The target, when found.
    pub fn target(&self) -> Option<&Target> {
        match self {
            Resolution::Found(t) => Some(t),
            Resolution::Missing(_) => None,
        }
    }
}

/// Resolves the references of one referencing document.
pub struct Resolver {
    referencing: Option<PathBuf>,
    open: Vec<OpenDocument>,
    search: Vec<PathBuf>,
    index: OnceCell<FolderIndex>,
}

impl Resolver {
    /// A resolver for references held by the document saved at `referencing`, or by an
    /// unsaved one (`None`), which has no stored paths to follow.
    pub fn new(referencing: Option<&Path>) -> Resolver {
        Resolver {
            referencing: referencing.map(Path::to_path_buf),
            open: Vec::new(),
            search: Vec::new(),
            index: OnceCell::new(),
        }
    }

    /// Adds documents open in this session.
    pub fn with_open(mut self, docs: impl IntoIterator<Item = OpenDocument>) -> Resolver {
        self.open.extend(docs);
        self
    }

    /// Adds search folders, such as File locations' "Referenced documents" folders.
    pub fn with_search_folders<P: AsRef<Path>>(mut self, folders: impl IntoIterator<Item = P>) -> Resolver {
        self.search.extend(folders.into_iter().map(|p| p.as_ref().to_path_buf()));
        self
    }

    /// Uses `index` instead of scanning on first need.
    pub fn with_index(self, index: FolderIndex) -> Resolver {
        let _ = self.index.set(index);
        self
    }

    /// The folder index: the referencing file's folder with its subfolders, and the
    /// search folders. Scanned on first use.
    pub fn index(&self) -> &FolderIndex {
        self.index.get_or_init(|| {
            let mut roots: Vec<PathBuf> = Vec::new();
            if let Some(dir) = self.referencing_dir() {
                roots.push(if dir.as_os_str().is_empty() { PathBuf::from(".") } else { dir.to_path_buf() });
            }
            roots.extend(self.search.iter().cloned());
            FolderIndex::scan(roots)
        })
    }

    fn referencing_dir(&self) -> Option<&Path> {
        self.referencing.as_deref().map(|p| p.parent().unwrap_or(Path::new("")))
    }

    /// Resolves `r` (see the module documentation for the order).
    pub fn resolve(&self, r: &DocRef) -> Resolution {
        // 1. An open document.
        if let Some(id) = r.id
            && let Some(doc) = self.open.iter().find(|d| d.id == id && d.kind == r.kind)
        {
            let new_path = doc.path.as_deref().and_then(|p| self.new_path(r, p));
            return Resolution::Found(Target {
                location: Location::Open(doc.clone()),
                via: Via::OpenDocument,
                identity: None,
                new_path,
                duplicates: Vec::new(),
            });
        }
        // 2. The stored path, after a migrated sibling.
        let mut stored = StoredPathProblem::NotFound;
        if let Some(referencing) = &self.referencing {
            let path = r.stored_path(referencing);
            if let Some(id) = r.id
                && let Some(old) = r.kind.legacy_extension()
                && path.extension().is_some_and(|e| e.eq_ignore_ascii_case(old))
            {
                let sibling = path.with_extension(r.kind.extension());
                if let Ok(identity) = identify_file(&sibling)
                    && identity.id == id
                    && identity.kind == r.kind
                {
                    return self.found_file(r, sibling, Via::SavedSibling, Some(identity), Vec::new());
                }
            }
            if path.is_file() {
                match identify_file(&path) {
                    Ok(identity) if identity.kind != r.kind => stored = StoredPathProblem::WrongKind(identity.kind),
                    Ok(identity) => match r.id {
                        Some(id) if identity.id != id => stored = StoredPathProblem::DifferentDocument(identity.id),
                        _ => return self.found_file(r, path, Via::StoredPath, Some(identity), Vec::new()),
                    },
                    // An unbound reference may point at a source that is not a document.
                    Err(_) if r.id.is_none() && DocKind::of_path(&path).is_none() => {
                        return self.found_file(r, path, Via::StoredPath, None, Vec::new());
                    }
                    Err(e) => stored = StoredPathProblem::Unreadable(e.to_string()),
                }
            }
        }
        // 3. The folder index.
        if let Some(id) = r.id {
            let mut hits: Vec<&IndexEntry> = self.index().find(id).filter(|e| e.identity.kind == r.kind).collect();
            if !hits.is_empty() {
                // Copies share an id: prefer the pinned revision, then the nearest file.
                let dir = self.referencing_dir().map(Path::to_path_buf).unwrap_or_default();
                hits.sort_by_key(|e| {
                    let rel = relative_path(&dir, &e.path);
                    (e.identity.revision != r.pinned, rel.matches("../").count(), rel.matches('/').count(), rel)
                });
                let duplicates = hits[1..].iter().map(|e| e.path.clone()).collect();
                let best = hits[0];
                return self.found_file(r, best.path.clone(), Via::Index, Some(best.identity.clone()), duplicates);
            }
        }
        // 4. Missing.
        Resolution::Missing(Missing { stored, candidates: self.candidates(r) })
    }

    fn found_file(
        &self,
        r: &DocRef,
        path: PathBuf,
        via: Via,
        identity: Option<Identity>,
        duplicates: Vec<PathBuf>,
    ) -> Resolution {
        let new_path = self.new_path(r, &path);
        Resolution::Found(Target { location: Location::File(path), via, identity, new_path, duplicates })
    }

    /// The path `r` should store for a target at `target`, if it differs from the
    /// stored one.
    fn new_path(&self, r: &DocRef, target: &Path) -> Option<String> {
        let referencing = self.referencing.as_deref()?;
        let same = normalize(&r.stored_path(referencing)) == normalize(target);
        (!same).then(|| relative_path(referencing.parent().unwrap_or(Path::new("")), target))
    }

    /// Documents of `r`'s kind that could replace it, the same file name first, then
    /// by path.
    pub fn candidates(&self, r: &DocRef) -> Vec<Candidate> {
        let stem = |p: &Path| p.file_stem().map(|s| s.to_string_lossy().to_lowercase());
        let want = stem(Path::new(&r.path));
        let mut out: Vec<Candidate> = self
            .index()
            .of_kind(r.kind)
            .map(|e| Candidate { path: e.path.clone(), identity: e.identity.clone(), same_name: stem(&e.path) == want })
            .collect();
        out.sort_by(|a, b| b.same_name.cmp(&a.same_name).then_with(|| a.path.cmp(&b.path)));
        out.truncate(MAX_CANDIDATES);
        out
    }
}

/// Why a reference could not be re-pointed.
#[derive(Debug, thiserror::Error)]
pub enum RepairError {
    /// The new target cannot be read as a document.
    #[error("{0}")]
    Unreadable(String),
    /// The new target is a document of another kind.
    #[error("{path} is a {found}, the reference needs a {want}")]
    WrongKind {
        /// The new target.
        path: PathBuf,
        /// Its kind.
        found: DocKind,
        /// The reference's kind.
        want: DocKind,
    },
    /// The new target is another document, and replacing was not asked for.
    #[error("{path} is a different document; replace the reference to use it")]
    DifferentDocument {
        /// The new target.
        path: PathBuf,
    },
}

/// Re-points `r`, held by the document saved at `referencing`, at the file `target`: the
/// repair for a missing reference. The target must be a document of the reference's
/// kind. A target with another id is accepted only with `replace` (the user chose a
/// different document on purpose), and the reference then takes its id. The pinned
/// revision is kept, so the referencing document goes on showing what it showed until a
/// Rebuild. Returns the target's identity.
pub fn repair(r: &mut DocRef, referencing: &Path, target: &Path, replace: bool) -> Result<Identity, RepairError> {
    let identity = identify_file(target).map_err(|e| RepairError::Unreadable(format!("{}: {e}", target.display())))?;
    if identity.kind != r.kind {
        return Err(RepairError::WrongKind { path: target.to_path_buf(), found: identity.kind, want: r.kind });
    }
    if r.id.is_some_and(|id| id != identity.id) && !replace {
        return Err(RepairError::DifferentDocument { path: target.to_path_buf() });
    }
    r.id = Some(identity.id);
    r.set_path(referencing, target);
    Ok(identity)
}

/// Points `r` at where its target was found: stores the new path, and binds an
/// unbound reference to the target's id. The pinned revision is not touched. A Rebuild
/// does this, and so does saving the referencing document.
pub fn rebind(r: &mut DocRef, target: &Target) {
    if let Some(path) = &target.new_path {
        r.path = path.clone();
    }
    if r.id.is_none() {
        r.id = target.id();
    }
}

/// Re-pins `r` at a resolved target's current revision, after [`rebind`]: what a
/// Rebuild does to each reference once it has rebuilt what depends on it. Returns false
/// when the target has no revision (a source that is not a document), in which case
/// only the path is updated.
pub fn repin(r: &mut DocRef, target: &Target) -> bool {
    rebind(r, target);
    match target.revision() {
        Some(now) => {
            r.pinned = now.clone();
            true
        }
        None => false,
    }
}
