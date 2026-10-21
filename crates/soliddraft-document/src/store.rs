//! The revision store: where the exact data of a pinned revision comes from.
//!
//! References pin the revision their document was saved against. Showing that revision
//! needs only what the referencing document caches (a drawing's view line work, an
//! assembly's display copies). Exact work on it (measuring, mass, projecting a new view)
//! needs the referenced document's bytes at that revision, which is the store's job.
//! Every screen talks to the [`RevisionStore`] trait, so the app behaves the same
//! whichever backend is behind it:
//!
//! - [`CacheOnlyStore`], phase 0's backend and the browser's: it has the revisions this
//!   session saw (opened or saved) and nothing older, so exact work on an older
//!   revision asks for a Rebuild instead of guessing;
//! - a Git backend next, on the desktop, finding a revision in the repository's history
//!   by the hash in the manifest;
//! - a SolidDraft PDM later, as its own project.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::hash::ContentHash;
use crate::id::DocumentId;

/// Why a revision could not be fetched.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The store does not have this revision. The message says what to do instead.
    #[error("{store} does not have revision {} of document {}: {advice}", revision.short(), id.short())]
    NotAvailable {
        /// The store's name.
        store: &'static str,
        /// The document.
        id: DocumentId,
        /// The revision asked for.
        revision: ContentHash,
        /// What the user can do, such as "Rebuild to use the current version".
        advice: &'static str,
    },
    /// The store failed.
    #[error("{0}")]
    Failed(String),
}

/// A source of documents' bytes at given revisions.
pub trait RevisionStore: Send + Sync {
    /// The backend's name, such as "cache-only" or "git".
    fn name(&self) -> &'static str;

    /// The bytes of document `id` at `revision`: a document file whose manifest's
    /// semantic hash is `revision`.
    fn fetch(&self, id: DocumentId, revision: &ContentHash) -> Result<Vec<u8>, StoreError>;

    /// Hands the store a document's bytes at a revision, when the app opens or saves
    /// it. Stores that keep their own history may ignore it.
    fn record(&self, id: DocumentId, revision: &ContentHash, bytes: &[u8]);

    /// True when the store can fetch `revision` of `id` now.
    fn has(&self, id: DocumentId, revision: &ContentHash) -> bool {
        self.fetch(id, revision).is_ok()
    }
}

/// The phase 0 store: revisions this session has seen, in memory, and nothing else.
#[derive(Default)]
pub struct CacheOnlyStore {
    seen: Mutex<HashMap<(DocumentId, ContentHash), Vec<u8>>>,
}

impl CacheOnlyStore {
    /// An empty store.
    pub fn new() -> CacheOnlyStore {
        CacheOnlyStore::default()
    }
}

impl RevisionStore for CacheOnlyStore {
    fn name(&self) -> &'static str {
        "cache-only"
    }

    fn fetch(&self, id: DocumentId, revision: &ContentHash) -> Result<Vec<u8>, StoreError> {
        let seen = self.seen.lock().map_err(|_| StoreError::Failed("the revision cache is poisoned".into()))?;
        seen.get(&(id, revision.clone())).cloned().ok_or_else(|| StoreError::NotAvailable {
            store: "the cache-only store",
            id,
            revision: revision.clone(),
            advice: "it keeps only revisions opened or saved in this session; Rebuild to use the current version",
        })
    }

    fn record(&self, id: DocumentId, revision: &ContentHash, bytes: &[u8]) {
        if let Ok(mut seen) = self.seen.lock() {
            seen.insert((id, revision.clone()), bytes.to_vec());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_only_store_has_what_it_saw() {
        let store = CacheOnlyStore::new();
        let (id, old, new) = (DocumentId::new(), ContentHash::of_bytes(b"1"), ContentHash::of_bytes(b"2"));
        assert!(!store.has(id, &old));
        let err = store.fetch(id, &old).unwrap_err();
        assert!(err.to_string().contains("Rebuild"), "{err}");
        store.record(id, &old, b"old bytes");
        store.record(id, &new, b"new bytes");
        assert_eq!(store.fetch(id, &old).unwrap(), b"old bytes");
        assert_eq!(store.fetch(id, &new).unwrap(), b"new bytes");
        assert!(!store.has(DocumentId::new(), &old));
        let dyn_store: &dyn RevisionStore = &store;
        assert_eq!(dyn_store.name(), "cache-only");
    }
}
