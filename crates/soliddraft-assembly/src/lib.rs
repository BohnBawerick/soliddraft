//! SolidDraft assemblies.
//!
//! An assembly places parts and other assemblies. They stay separate files: the
//! assembly references each by id with a pinned revision, and keeps display copies of
//! what it shows so it opens without them. The crate holds:
//!
//! - **The schema** ([`schema`]): sources, components with stable ids and rigid f64
//!   placements, patterns stored as generators, sparse overrides by occurrence path,
//!   and reserved configurations, display states and mates.
//! - **Paths** ([`path`]): component, pattern and source ids, and occurrence paths made
//!   of them, never of names.
//! - **The file** ([`file`]): `.dwasm` in the document envelope, `placements.bin` for
//!   large assemblies ([`bulk`]), and `cache/refs` display copies ([`mesh`]).
//! - **The definition store** ([`store`]): one entry per document, configuration and
//!   revision, with its loading state.
//! - **The occurrence table** ([`occurrence`]): one row per placed part or
//!   sub-assembly, flat, with f64 world placements, bounds and flags.
//! - **Loading** ([`load`]): structure first, display meshes and reference freshness
//!   in the background, with fallbacks when a copy is missing ([`source`]).
//! - **The open document** ([`document`]): edits with undo, Rebuild of pinned
//!   references, saving with display copies, and exact bodies on demand.
//! - **The scene** ([`scene`]): instances grouped by definition for instanced draws,
//!   and what an id-buffer texel names.
//! - **The hierarchy** ([`bvh`]): boxes over occurrences for rays, regions and the
//!   camera's view.
//!
//! The schema is documented in `docs/assemblies.md`.

mod binary;
pub mod bulk;
pub mod bvh;
pub mod document;
pub mod file;
pub mod load;
pub mod mesh;
pub mod occurrence;
pub mod path;
pub mod placement;
pub mod scene;
pub mod schema;
pub mod source;
pub mod store;

pub use binary::BinaryError;
pub use document::{AssemblyDocument, Changes, EditError, Opening, RebuildReport, RefChange, Selection, SourceReport};
pub use file::{AssemblyPackage, CopyReader, FileError, RefCopy, TruthEntries};
pub use load::{LoadError, LoadEvent, OpenOptions};
pub use mesh::{DisplayMesh, MeshBounds, MeshFace, SharedMesh};
pub use occurrence::{Counts, Flags, OccurrenceIndex, OccurrenceTable};
pub use path::{ComponentId, OccurrencePath, PatternId, SourceId, Step};
pub use placement::{Aabb, Placement};
pub use scene::{Draw, Instance, Picked, Scene};
pub use schema::{Assembly, Component, Direction, OccurrenceAppearance, Override, Pattern, PatternRule, Source, Units};
pub use source::{DisplaySource, FileSource, KernelSource};
pub use store::{Definition, DefinitionIndex, DefinitionKey, DefinitionStore, Freshness, LoadState};
