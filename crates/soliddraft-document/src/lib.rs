//! The SolidDraft document envelope.
//!
//! Parts, assemblies and drawings are separate files that reference each other. This
//! crate holds what every kind shares, so the references between them mean the same
//! thing everywhere:
//!
//! - **Identity.** A [`DocumentId`] per document, a random UUID minted at creation and
//!   kept through renames and moves ([`id`]).
//! - **Kinds and names.** Parts `.dwprt` (`.dwpart` still opens), assemblies `.dwasm`,
//!   drawings `.dwdrw` (`.dwdrawing` still opens) ([`kind`]).
//! - **The manifest.** `manifest.json`, the first entry of every document file: id,
//!   kind, format, writer, hashes, references, and the file's sections with which are
//!   required ([`manifest`]).
//! - **Canonical hashes.** BLAKE3 over canonical JSON of what the user authored: the
//!   semantic hash is the revision references pin; the geometry hash leaves out names,
//!   colours and materials ([`hash`]).
//! - **References.** A [`DocRef`] names its target by id and kind, keeps a path
//!   relative to the referencing file, and pins the target's revision until a Rebuild
//!   ([`reference`]). Configuration and display-state fields are reserved.
//! - **Resolution.** The [`Resolver`] finds a reference's target: open documents, the
//!   stored path, then the [`FolderIndex`]; a missing reference lists repair
//!   candidates ([`resolve`], [`index`]).
//! - **Revisions.** The [`RevisionStore`] interface serves a pinned revision's bytes;
//!   phase 0 has the [`CacheOnlyStore`] ([`store`]).
//! - **File locations.** Where reference files are found: libraries, templates, sheet
//!   formats, styles, configuration tables, search folders ([`locations`]).
//! - **Atomic saves.** [`save_atomic`] for every writer ([`save`]).
//!
//! The schema is documented in `docs/documents.md`.

pub mod hash;
pub mod id;
pub mod index;
pub mod kind;
pub mod locations;
pub mod manifest;
pub mod reference;
pub mod resolve;
pub mod save;
pub mod store;

pub use hash::ContentHash;
pub use id::{ConfigScope, ConfigurationId, DisplayStateId, DocumentId};
pub use index::{FolderIndex, IndexEntry};
pub use kind::{DRAWING_EXTENSION, DocKind, LEGACY_DRAWING_EXTENSION, LEGACY_PART_EXTENSION, PART_EXTENSION};
pub use locations::{FileLocations, LocationKind};
pub use manifest::{EnvelopeError, Identity, Manifest, Package, Section, identify, identify_file};
pub use reference::DocRef;
pub use resolve::{Location, OpenDocument, RefStatus, Resolution, Resolver, Target, Via};
pub use save::save_atomic;
pub use store::{CacheOnlyStore, RevisionStore, StoreError};
