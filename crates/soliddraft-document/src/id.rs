//! Document identity.
//!
//! A [`DocumentId`] is a random UUID minted when a document is created. It is not the
//! file name and not the display name: renaming or moving a file keeps it, and only Save
//! as copy mints a new one. Files written before documents had ids get an id derived
//! from their content (see [`DocumentId::legacy`]), so every open of the same old file
//! agrees on it until a save writes it down.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::hash::ContentHash;
use crate::kind::DocKind;

/// A document's identity, stable across renames, moves and edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DocumentId(Uuid);

impl DocumentId {
    /// A new random id (UUID version 4).
    pub fn new() -> DocumentId {
        DocumentId(Uuid::new_v4())
    }

    /// The id of a document from a file that predates document ids: derived from the
    /// kind and the hash of the file's content, so reopening the same unchanged file
    /// gives the same id. It is a version 8 UUID, so it never collides with a random
    /// one. Two byte-identical old files get the same id, as two copies of a new file
    /// do.
    pub fn legacy(kind: DocKind, content: &ContentHash) -> DocumentId {
        let mut hasher = blake3::Hasher::new();
        // Keep the original seed so pre-envelope files retain their document ids.
        hasher.update(b"draftwell legacy document id\0");
        hasher.update(kind.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(content.as_str().as_bytes());
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
        DocumentId(Uuid::new_v8(bytes))
    }

    /// True for an id made by [`DocumentId::legacy`].
    pub fn is_legacy(&self) -> bool {
        self.0.get_version_num() == 8
    }

    /// The first eight hex digits, for display.
    pub fn short(&self) -> String {
        self.0.simple().to_string()[..8].to_owned()
    }
}

impl Default for DocumentId {
    /// A new random id, as a new document gets.
    fn default() -> DocumentId {
        DocumentId::new()
    }
}

impl fmt::Display for DocumentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.hyphenated().fmt(f)
    }
}

impl FromStr for DocumentId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<DocumentId, uuid::Error> {
        Uuid::parse_str(s).map(DocumentId)
    }
}

/// A configuration of a part or an assembly: a named set of overrides (suppressed
/// features, dimension values, suppressed components, component configurations).
///
/// Reserved: phase 0 writes no configurations and every reference uses the document's
/// default. The id is opaque and stable within its document; the configuration's name
/// is a separate label.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConfigurationId(pub String);

/// A display state of a part or an assembly: visibility, colours and an exploded pose,
/// kept apart from configurations so a colour change never invalidates geometry.
///
/// Reserved like [`ConfigurationId`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DisplayStateId(pub String);

/// Which configurations an assignment applies to, such as an appearance on a face.
///
/// Reserved for material assignments: every appearance assignment, at part or assembly
/// level, carries one, so colours can vary by configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "scope", content = "configurations")]
pub enum ConfigScope {
    /// Every configuration, the default.
    #[default]
    All,
    /// Only these configurations.
    Only(Vec<ConfigurationId>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_random_and_parse_back() {
        let a = DocumentId::new();
        assert_ne!(a, DocumentId::new());
        assert_eq!(a.to_string().parse::<DocumentId>().unwrap(), a);
        assert_eq!(serde_json::to_string(&a).unwrap(), format!("\"{a}\""));
        assert!(!a.is_legacy());
        assert_eq!(a.short().len(), 8);
    }

    #[test]
    fn legacy_ids_follow_the_content() {
        let h = ContentHash::of_bytes(b"recipe");
        let a = DocumentId::legacy(DocKind::Part, &h);
        assert_eq!(a.to_string(), "97b59eff-e760-8586-afa6-d48d7d8f64ea", "the original Draftwell identity");
        assert_eq!(a, DocumentId::legacy(DocKind::Part, &h));
        assert_ne!(a, DocumentId::legacy(DocKind::Drawing, &h));
        assert_ne!(a, DocumentId::legacy(DocKind::Part, &ContentHash::of_bytes(b"other")));
        assert!(a.is_legacy());
    }

    #[test]
    fn scopes_serialise_plainly() {
        assert_eq!(serde_json::to_string(&ConfigScope::All).unwrap(), r#"{"scope":"all"}"#);
        let only = ConfigScope::Only(vec![ConfigurationId("c1".into())]);
        let json = serde_json::to_string(&only).unwrap();
        assert_eq!(json, r#"{"scope":"only","configurations":["c1"]}"#);
        assert_eq!(serde_json::from_str::<ConfigScope>(&json).unwrap(), only);
    }
}
