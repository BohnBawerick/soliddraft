//! The definition store: one entry per document, configuration and revision an
//! assembly shows, however many times it is placed.
//!
//! A thousand placements of one bolt are one definition with one display mesh. The
//! occurrence table (one row per placement) points at definitions by
//! [`DefinitionIndex`], a dense index that is stable while the store lives and is never
//! written to a file.
//!
//! Each definition has a [`LoadState`]:
//!
//! - `Unresolved`: known by reference only; nothing read yet.
//! - `MetadataOnly`: its kind, name and (for a sub-assembly) structure are read; no
//!   display data. It is drawn as nothing and picked by its bounds, if known.
//! - `DisplayReady`: its display mesh at the pinned revision is loaded. Enough to draw,
//!   select, hide, show and move it. Never enough for an engineering answer.
//! - `ExactReady`: its kernel body is built too. Measure, mass, drawing projection and
//!   editing promote a definition to this state; the display mesh never answers them.
//! - `Stale`: drawn from display data of a revision other than the pinned one, because
//!   the pinned revision's copy was missing and the current file had to stand in. A
//!   Rebuild fixes it.
//! - `Failed`: nothing could be loaded; the problem says why.
//!
//! Separately, [`Freshness`] says how the pinned revision compares with the document as
//! it is now, once the reference has been resolved.

use std::collections::HashMap;
use std::sync::Arc;

use soliddraft_document::{ConfigurationId, ContentHash, DocKind, DocumentId};

use soliddraft_tessellation::{Level, Levels, Quality};

use crate::mesh::{MeshBounds, SharedMesh};
use crate::placement::Aabb;
use crate::schema::Assembly;

/// What a definition is: a document at a revision, in a configuration, and for a part
/// shown at a quality of its own, that quality.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefinitionKey {
    /// The document.
    pub document: DocumentId,
    /// Its configuration; `None` for the default.
    pub configuration: Option<ConfigurationId>,
    /// The revision shown: the one the referencing assembly pins.
    pub revision: ContentHash,
    /// The quality the part is shown at, as [`Quality::key`], when a component or the
    /// assembly overrides the part's own; `None` for the part's own. Two placements of
    /// one part at two qualities are two definitions, each with its own mesh, so the
    /// scene, culling and picking draw them like any other definitions.
    pub quality: Option<String>,
}

impl DefinitionKey {
    /// A document at a revision, in a configuration, at its own quality.
    pub fn new(document: DocumentId, configuration: Option<ConfigurationId>, revision: ContentHash) -> DefinitionKey {
        DefinitionKey { document, configuration, revision, quality: None }
    }

    /// The same document, revision and configuration at `quality` (`None` for its own).
    pub fn at(&self, quality: Option<&Quality>) -> DefinitionKey {
        DefinitionKey { quality: quality.map(Quality::key), ..self.clone() }
    }
}

/// A definition's place in its store.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefinitionIndex(pub u32);

/// How far a definition is loaded. Ordered from least to most loaded, with the two
/// problem states last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LoadState {
    /// Known by reference only.
    Unresolved,
    /// Kind, name and structure read; no display data.
    MetadataOnly,
    /// The display mesh at the pinned revision is loaded.
    DisplayReady,
    /// The kernel body is built too.
    ExactReady,
    /// Drawn from another revision's display data; needs a Rebuild.
    Stale,
    /// Nothing could be loaded.
    Failed,
}

impl LoadState {
    /// Every state, in order.
    pub const ALL: [LoadState; 6] = [
        LoadState::Unresolved,
        LoadState::MetadataOnly,
        LoadState::DisplayReady,
        LoadState::ExactReady,
        LoadState::Stale,
        LoadState::Failed,
    ];

    /// The state as one word: "unresolved", "metadata", "display", "exact", "stale",
    /// "failed".
    pub fn as_str(self) -> &'static str {
        match self {
            LoadState::Unresolved => "unresolved",
            LoadState::MetadataOnly => "metadata",
            LoadState::DisplayReady => "display",
            LoadState::ExactReady => "exact",
            LoadState::Stale => "stale",
            LoadState::Failed => "failed",
        }
    }

    /// True when there is something to draw.
    pub fn drawable(self) -> bool {
        matches!(self, LoadState::DisplayReady | LoadState::ExactReady | LoadState::Stale)
    }
}

/// How the pinned revision compares with the referenced document as it is now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Freshness {
    /// The reference has not been resolved yet. Opening never waits for it.
    #[default]
    Unchecked,
    /// The document is at the pinned revision.
    Current,
    /// The document moved on; the assembly shows the pinned revision until a Rebuild.
    NewerAvailable {
        /// The document's revision now.
        now: ContentHash,
    },
    /// The document cannot be found; the assembly shows its copy and needs a repair.
    Missing,
}

/// One definition.
#[derive(Clone, Debug)]
pub struct Definition {
    /// What it is.
    pub key: DefinitionKey,
    /// Part or assembly.
    pub kind: DocKind,
    /// A name for the tree: the referenced file's name without its extension.
    pub label: String,
    /// How far it is loaded.
    pub state: LoadState,
    /// Why it is stale or failed.
    pub problem: Option<String>,
    /// How the pinned revision compares with the document now.
    pub freshness: Freshness,
    /// The display mesh, for a part once display-ready.
    pub mesh: Option<SharedMesh>,
    /// The box around the part in its own frame; empty until known.
    pub bounds: Aabb,
    /// The sub-assembly's structure at the pinned revision, once read.
    pub assembly: Option<Arc<Assembly>>,
    /// The quality a part is shown at instead of its own, as in its key.
    pub quality: Option<Quality>,
    /// The part's coarse and fine meshes, once loaded; `mesh` is its standard one. The
    /// renderer picks one per occurrence from how large its error is on screen.
    pub coarse: Option<SharedMesh>,
    /// See `coarse`.
    pub fine: Option<SharedMesh>,
}

impl Definition {
    /// The mesh of `level`, if loaded.
    pub fn level_mesh(&self, level: Level) -> Option<&SharedMesh> {
        match level {
            Level::Coarse => self.coarse.as_ref(),
            Level::Standard => self.mesh.as_ref(),
            Level::Fine => self.fine.as_ref(),
        }
    }

    /// The largest distance from each level's mesh to the exact surface, in
    /// millimetres, coarsest first: what the renderer compares with a pixel. It follows
    /// the definition's quality, and a part's own is taken as automatic; zero when no
    /// chord height bounds it.
    pub fn deviations(&self) -> [f32; 3] {
        let levels = Levels::new(&self.quality.unwrap_or_default(), self.bounds.diagonal());
        Level::ALL.map(|l| levels.deviation(l).unwrap_or(0.0) as f32)
    }
}

impl Definition {
    fn new(key: DefinitionKey, kind: DocKind, label: String) -> Definition {
        Definition {
            key,
            kind,
            label,
            state: LoadState::Unresolved,
            problem: None,
            freshness: Freshness::Unchecked,
            mesh: None,
            bounds: Aabb::EMPTY,
            assembly: None,
            quality: None,
            coarse: None,
            fine: None,
        }
    }
}

/// Every definition an open assembly shows.
#[derive(Clone, Debug, Default)]
pub struct DefinitionStore {
    defs: Vec<Definition>,
    index: HashMap<DefinitionKey, DefinitionIndex>,
}

impl DefinitionStore {
    /// An empty store.
    pub fn new() -> DefinitionStore {
        DefinitionStore::default()
    }

    /// The definition for `key`, added as unresolved if it is new.
    pub fn intern(&mut self, key: DefinitionKey, kind: DocKind, label: impl Into<String>) -> DefinitionIndex {
        if let Some(i) = self.index.get(&key) {
            return *i;
        }
        let i = DefinitionIndex(self.defs.len() as u32);
        self.index.insert(key.clone(), i);
        self.defs.push(Definition::new(key, kind, label.into()));
        i
    }

    /// The definition of part `base` shown at `quality` instead of its own (or the base
    /// itself for `None`), added as unresolved if it is new.
    pub fn intern_at(&mut self, base: DefinitionIndex, quality: Option<&Quality>) -> DefinitionIndex {
        let Some(q) = quality else { return base };
        let (key, kind, label) = {
            let d = self.get(base);
            (d.key.at(Some(q)), d.kind, d.label.clone())
        };
        let i = self.intern(key, kind, label);
        self.get_mut(i).quality = Some(*q);
        i
    }

    /// The definition for `key`, if the store has it.
    pub fn find(&self, key: &DefinitionKey) -> Option<DefinitionIndex> {
        self.index.get(key).copied()
    }

    /// A definition.
    pub fn get(&self, i: DefinitionIndex) -> &Definition {
        &self.defs[i.0 as usize]
    }

    /// A definition, to change.
    pub fn get_mut(&mut self, i: DefinitionIndex) -> &mut Definition {
        &mut self.defs[i.0 as usize]
    }

    /// How many definitions.
    pub fn len(&self) -> usize {
        self.defs.len()
    }

    /// True when there are none.
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    /// Every definition with its index.
    pub fn iter(&self) -> impl Iterator<Item = (DefinitionIndex, &Definition)> {
        self.defs.iter().enumerate().map(|(i, d)| (DefinitionIndex(i as u32), d))
    }

    /// Makes a part display-ready with `mesh`, or stale when the mesh is of another
    /// revision (`problem` then says which).
    pub fn set_mesh(&mut self, i: DefinitionIndex, mesh: SharedMesh, stale: Option<String>) {
        let d = self.get_mut(i);
        d.bounds = mesh.bounds();
        d.mesh = Some(mesh);
        d.state = if stale.is_some() { LoadState::Stale } else { LoadState::DisplayReady };
        d.problem = stale;
    }

    /// Records the coarse or fine mesh of a part; the standard one goes through
    /// [`DefinitionStore::set_mesh`].
    pub fn set_level(&mut self, i: DefinitionIndex, level: Level, mesh: SharedMesh) {
        let d = self.get_mut(i);
        match level {
            Level::Coarse => d.coarse = Some(mesh),
            Level::Standard => d.mesh = Some(mesh),
            Level::Fine => d.fine = Some(mesh),
        }
    }

    /// Records a sub-assembly's structure.
    pub fn set_assembly(&mut self, i: DefinitionIndex, assembly: Arc<Assembly>) {
        let d = self.get_mut(i);
        d.assembly = Some(assembly);
        if d.state == LoadState::Unresolved {
            d.state = LoadState::MetadataOnly;
        }
    }

    /// Marks a definition failed.
    pub fn fail(&mut self, i: DefinitionIndex, problem: impl Into<String>) {
        let d = self.get_mut(i);
        d.state = LoadState::Failed;
        d.problem = Some(problem.into());
    }

    /// How many definitions are in each state, in [`LoadState::ALL`] order.
    pub fn counts(&self) -> [usize; 6] {
        let mut out = [0; 6];
        for d in &self.defs {
            out[LoadState::ALL.iter().position(|s| *s == d.state).expect("every state is listed")] += 1;
        }
        out
    }

    /// Roughly how many bytes the store holds, meshes included.
    pub fn heap_bytes(&self) -> usize {
        let per_def = std::mem::size_of::<Definition>() + std::mem::size_of::<(DefinitionKey, DefinitionIndex)>() + 160;
        self.defs.len() * per_def
            + self.defs.iter().filter_map(|d| d.mesh.as_ref()).map(|m| m.heap_bytes()).sum::<usize>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::DisplayMesh;
    use glam::DVec3;

    fn key(n: u8) -> DefinitionKey {
        DefinitionKey::new(DocumentId::new(), None, ContentHash::of_bytes(&[n]))
    }

    #[test]
    fn definitions_are_shared_by_key() {
        let mut s = DefinitionStore::new();
        let k = key(1);
        let a = s.intern(k.clone(), DocKind::Part, "bolt");
        assert_eq!(s.intern(k.clone(), DocKind::Part, "bolt"), a);
        let other_revision = DefinitionKey { revision: ContentHash::of_bytes(b"2"), ..k.clone() };
        assert_ne!(s.intern(other_revision, DocKind::Part, "bolt"), a, "two revisions are two definitions");
        assert_eq!(s.len(), 2);
        assert_eq!(s.find(&k), Some(a));
        assert_eq!(s.get(a).state, LoadState::Unresolved);
    }

    #[test]
    fn states_follow_what_is_loaded() {
        let mut s = DefinitionStore::new();
        let a = s.intern(key(1), DocKind::Part, "a");
        let b = s.intern(key(2), DocKind::Part, "b");
        let c = s.intern(key(3), DocKind::Assembly, "c");
        let mesh = Arc::new(DisplayMesh::cuboid(DVec3::ZERO, DVec3::splat(2.0)));
        s.set_mesh(a, mesh.clone(), None);
        s.set_mesh(b, mesh, Some("shown at the current revision".into()));
        s.set_assembly(c, Arc::new(Assembly::new("sub")));
        assert_eq!(s.get(a).state, LoadState::DisplayReady);
        assert_eq!(s.get(a).bounds.max, DVec3::splat(2.0));
        assert_eq!(s.get(b).state, LoadState::Stale);
        assert_eq!(s.get(c).state, LoadState::MetadataOnly);
        s.fail(c, "unreadable");
        assert_eq!(s.counts(), [0, 0, 1, 0, 1, 1]);
        assert!(LoadState::Stale.drawable() && !LoadState::MetadataOnly.drawable());
        assert!(s.heap_bytes() > 0);
    }
}
