//! References from one document to another.
//!
//! A [`DocRef`] names the target by id and kind, says where it was (a path relative to
//! the referencing file), and pins the revision the referencing document was saved
//! against. Opening resolves it (see [`crate::resolve`]); a target whose revision moved
//! on is shown at the pinned revision and flagged, and a Rebuild re-pins it.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::hash::ContentHash;
use crate::id::{ConfigurationId, DisplayStateId, DocumentId};
use crate::kind::DocKind;

/// A reference to another document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocRef {
    /// What the target must be. `None` for an unbound reference: one migrated from a
    /// file that recorded no id, or one to a source that has none (a test recipe). An
    /// unbound reference resolves by its path only and is bound the first time it
    /// resolves to a document with an id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<DocumentId>,
    /// The target's kind, checked on resolve.
    pub kind: DocKind,
    /// The target's path relative to the referencing file's folder, with forward
    /// slashes.
    pub path: String,
    /// The target's configuration; `None` for its default. Reserved: phase 0 writes
    /// `None` and keeps any value it reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<ConfigurationId>,
    /// The target's display state; `None` for its default. Reserved like
    /// `configuration`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_state: Option<DisplayStateId>,
    /// The target's semantic hash when the referencing document was last rebuilt
    /// against it: the revision it shows until the next Rebuild.
    pub pinned: ContentHash,
    /// Where a revision store can fetch the pinned revision, when it can: a store's own
    /// locator such as a commit. `None` in phase 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<String>,
}

impl DocRef {
    /// A bound reference to `id` at `path`, pinned at `pinned`.
    pub fn new(id: DocumentId, kind: DocKind, path: impl Into<String>, pinned: ContentHash) -> DocRef {
        DocRef {
            id: Some(id),
            kind,
            path: path.into(),
            configuration: None,
            display_state: None,
            pinned,
            locator: None,
        }
    }

    /// An unbound reference to whatever is at `path`.
    pub fn unbound(kind: DocKind, path: impl Into<String>, pinned: ContentHash) -> DocRef {
        DocRef { id: None, ..DocRef::new(DocumentId::new(), kind, path, pinned) }
    }

    /// The reference a pre-phase-0 drawing's `model` object becomes: unbound (the old
    /// id was the part's display name, which identifies nothing), the same path, and
    /// the old digest kept as a foreign hash that never matches a current revision, so
    /// the drawing reads as older than its part until a Rebuild.
    pub fn from_legacy_model(model: &Value) -> Option<DocRef> {
        let path = model.get("path")?.as_str()?;
        let digest = model.get("digest_at_update").and_then(Value::as_str).unwrap_or("");
        Some(DocRef::unbound(DocKind::Part, path, ContentHash::foreign("fnv64", digest)))
    }

    /// True when the reference knows its target's id.
    pub fn is_bound(&self) -> bool {
        self.id.is_some()
    }

    /// Where the target was, for a referencing file at `referencing`.
    pub fn stored_path(&self, referencing: &Path) -> PathBuf {
        let dir = referencing.parent().unwrap_or(Path::new(""));
        normalize(&dir.join(&self.path))
    }

    /// Re-points the reference at `target`, as seen from a referencing file at
    /// `referencing`.
    pub fn set_path(&mut self, referencing: &Path, target: &Path) {
        self.path = relative_path(referencing.parent().unwrap_or(Path::new("")), target);
    }
}

/// `target` relative to the folder `from`, with forward slashes, such as
/// "../parts/bracket.dwprt". Both are made absolute against the current directory
/// first; the result is lexical, so neither needs to exist. On different drive roots
/// the absolute target is returned.
pub fn relative_path(from: &Path, target: &Path) -> String {
    // A file name alone has the empty path as its folder: the current one.
    let from = if from.as_os_str().is_empty() { Path::new(".") } else { from };
    let abs = |p: &Path| normalize(&std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()));
    let (from, target) = (abs(from), abs(target));
    let f: Vec<Component> = from.components().collect();
    let t: Vec<Component> = target.components().collect();
    if f.first() != t.first() {
        return slashes(&target);
    }
    let common = f.iter().zip(&t).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_owned(), f.len() - common).collect();
    parts.extend(t[common..].iter().map(|c| c.as_os_str().to_string_lossy().into_owned()));
    parts.join("/")
}

/// `path` with `.` removed and `..` folded into its parent where there is one.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                let pops = matches!(out.components().next_back(), Some(Component::Normal(_)));
                if pops {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn slashes(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn references_serialise_with_reserved_fields_left_out() {
        let id = DocumentId::new();
        let r = DocRef::new(id, DocKind::Part, "bracket.dwprt", ContentHash::of_bytes(b"x"));
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["id"], json!(id.to_string()));
        assert_eq!(v["kind"], "part");
        assert!(v.get("configuration").is_none() && v.get("display_state").is_none() && v.get("locator").is_none());
        let mut with = r.clone();
        with.configuration = Some(ConfigurationId("long".into()));
        with.display_state = Some(DisplayStateId("exploded".into()));
        let back: DocRef = serde_json::from_value(serde_json::to_value(&with).unwrap()).unwrap();
        assert_eq!(back, with, "reserved values survive a round trip");
    }

    #[test]
    fn legacy_model_references_become_unbound() {
        let r = DocRef::from_legacy_model(&json!({
            "document_id": "bracket", "path": "bracket.json", "digest_at_update": "00ff", "revision": 3
        }))
        .unwrap();
        assert!(!r.is_bound());
        assert_eq!(r.path, "bracket.json");
        assert_eq!(r.pinned, ContentHash::foreign("fnv64", "00ff"));
        assert!(DocRef::from_legacy_model(&json!({})).is_none());
    }

    #[test]
    fn relative_paths() {
        assert_eq!(relative_path(Path::new("/a/b"), Path::new("/a/b/c.dwprt")), "c.dwprt");
        assert_eq!(relative_path(Path::new("/a/b"), Path::new("/a/parts/c.dwprt")), "../parts/c.dwprt");
        assert_eq!(relative_path(Path::new("/a/b/"), Path::new("/a/b/./x/../c.dwprt")), "c.dwprt");
        assert_eq!(relative_path(Path::new(""), Path::new("c.dwprt")), "c.dwprt");
        assert_eq!(relative_path(Path::new(""), Path::new("parts/c.dwprt")), "parts/c.dwprt");
        let mut r = DocRef::unbound(DocKind::Part, "", ContentHash::of_bytes(b""));
        r.set_path(Path::new("/w/drawings/d.dwdrw"), Path::new("/w/parts/p.dwprt"));
        assert_eq!(r.path, "../parts/p.dwprt");
        assert_eq!(r.stored_path(Path::new("/w/drawings/d.dwdrw")), Path::new("/w/parts/p.dwprt"));
        assert_eq!(normalize(Path::new("../a/./b/../c")), Path::new("../a/c"));
    }
}
