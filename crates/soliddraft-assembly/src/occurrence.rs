//! The flat occurrence table: one row per placed part or sub-assembly, anywhere in the
//! tree.
//!
//! The renderer, the picker and the bounding volume hierarchy read columns of this
//! table; nothing walks a nested tree per frame. Row 0 is the root assembly. Every
//! other row has a parent row (the sub-assembly occurrence that contains it), the
//! [`Step`] that leads to it from there, its definition, its placement relative to the
//! parent and in the world (both `f64`), its world bounds and its flags. Children are a
//! linked list per row, so inserting a component appends rows without moving others.
//!
//! Row numbers ([`OccurrenceIndex`]) are stable while the table lives; they are not
//! written to files. Paths ([`OccurrencePath`]) are what persists.
//!
//! Edits work in place: moving an occurrence updates the world placements and bounds of
//! its subtree; hiding one updates the effective hidden flag of its subtree. Neither
//! touches any other row.

use std::collections::HashMap;
use std::sync::Arc;

use soliddraft_document::{DocKind, DocRef};

use crate::path::{OccurrencePath, Step};
use crate::placement::{Aabb, Placement};
use crate::schema::{Assembly, Override};
use crate::store::{DefinitionIndex, DefinitionKey, DefinitionStore};
use soliddraft_materials::Appearance;
use soliddraft_tessellation::Quality;

/// A row of the occurrence table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OccurrenceIndex(pub u32);

impl OccurrenceIndex {
    /// The root assembly's row.
    pub const ROOT: OccurrenceIndex = OccurrenceIndex(0);
}

const NONE: u32 = u32::MAX;

/// Flags of an occurrence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Flags(pub u8);

impl Flags {
    /// Hidden by its own component or an override.
    pub const HIDDEN_SELF: u8 = 1;
    /// Hidden, itself or through an ancestor: not drawn.
    pub const HIDDEN: u8 = 2;
    /// Suppressed, itself or through an ancestor: not drawn, not loaded, not counted.
    pub const SUPPRESSED: u8 = 4;
    /// A sub-assembly (the root included): drawn through its children.
    pub const ASSEMBLY: u8 = 8;
    /// Deleted since the table was built; kept so row numbers stay put.
    pub const REMOVED: u8 = 16;
    /// Suppressed by its own component or an override.
    pub const SUPPRESSED_SELF: u8 = 32;
    /// Given a colour of its own by an assembly above it.
    pub const COLOURED_SELF: u8 = 64;

    /// True when every bit of `bits` is set.
    pub fn has(self, bits: u8) -> bool {
        self.0 & bits == bits
    }

    /// True when the occurrence is a part that is drawn.
    pub fn drawn_part(self) -> bool {
        self.0 & (Flags::HIDDEN | Flags::SUPPRESSED | Flags::ASSEMBLY | Flags::REMOVED) == 0
    }

    fn set(&mut self, bits: u8, on: bool) {
        if on {
            self.0 |= bits;
        } else {
            self.0 &= !bits;
        }
    }
}

/// Why the tree could not be flattened.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum FlattenError {
    /// An assembly contains itself.
    #[error("the assembly contains itself through {0}")]
    Cycle(OccurrencePath),
    /// A sub-assembly's structure is not in the store.
    #[error("the structure of the sub-assembly at {0} is not loaded")]
    MissingStructure(OccurrencePath),
}

/// The flat occurrence table.
#[derive(Clone, Debug, Default)]
pub struct OccurrenceTable {
    parent: Vec<u32>,
    step: Vec<Step>,
    definition: Vec<DefinitionIndex>,
    local: Vec<Placement>,
    world: Vec<Placement>,
    bounds: Vec<Aabb>,
    flags: Vec<Flags>,
    /// Each row's assembly-level colour: 0 for none (the parts' own colours), otherwise
    /// one more than its index in `palette`.
    colour: Vec<u32>,
    palette: Vec<Appearance>,
    /// Each row's display quality when the assembly overrides the parts' own: 0 for
    /// none, otherwise one more than its index in `qualities`.
    quality: Vec<u32>,
    qualities: Vec<Quality>,
    first_child: Vec<u32>,
    last_child: Vec<u32>,
    next_sibling: Vec<u32>,
    by_step: HashMap<(u32, Step), u32, BuildFast>,
    /// The rows of each definition, built on first need.
    rows_of: Option<Arc<Vec<Vec<u32>>>>,
}

/// The source of a step in its assembly: the definition key and kind it places.
fn source_key(assembly: &Assembly, source: crate::path::SourceId) -> Option<(&DocRef, DefinitionKey)> {
    let s = assembly.source(source)?;
    let id = s.document.id?;
    Some((&s.document, DefinitionKey::new(id, s.document.configuration.clone(), s.document.pinned.clone())))
}

/// A tree label for a reference: its file name without the extension.
pub fn label_of(r: &DocRef) -> String {
    let name = r.path.rsplit('/').next().unwrap_or(&r.path);
    name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(name).to_owned()
}

/// The definition each source of `assembly` places, interned into `store`.
pub fn intern_sources(
    assembly: &Assembly,
    store: &mut DefinitionStore,
) -> HashMap<crate::path::SourceId, DefinitionIndex> {
    assembly
        .sources
        .iter()
        .filter_map(|s| {
            let (r, key) = source_key(assembly, s.id)?;
            Some((s.id, store.intern(key, r.kind, label_of(r))))
        })
        .collect()
}

impl OccurrenceTable {
    /// Flattens the tree under `root` (a definition in `store` whose structure is
    /// loaded). Every sub-assembly's structure must be in the store; parts need not be
    /// loaded (their bounds stay empty until they are, see
    /// [`OccurrenceTable::refresh_bounds`]). Definitions for every source are interned.
    pub fn flatten(root: DefinitionIndex, store: &mut DefinitionStore) -> Result<OccurrenceTable, FlattenError> {
        let mut t = OccurrenceTable::default();
        t.reserve(rows_under(root, store, &mut HashMap::new()) as usize + 1);
        t.push(NONE, Step::component(crate::path::ComponentId(0)), root, Placement::IDENTITY, Flags(Flags::ASSEMBLY));
        // Each frame: the assembly row, its structure, and overrides that reach below it,
        // as (the rest of the path, the override), outer assemblies' first.
        struct Frame {
            row: u32,
            assembly: Arc<Assembly>,
            pending: Vec<Pending>,
        }
        let root_assembly =
            store.get(root).assembly.clone().ok_or(FlattenError::MissingStructure(OccurrencePath::root()))?;
        let mut pending = t.own_effects(&root_assembly);
        // The open assembly's display settings: a quality for every part, then
        // components at their own, by path.
        let s = &root_assembly.settings;
        if s.viewport != Quality::Auto {
            t.quality[0] = t.quality_index(&s.viewport);
        }
        for o in &s.overrides {
            if let Ok(path) = o.path.parse::<OccurrencePath>()
                && !path.is_root()
            {
                pending.push((path.steps().to_vec(), Effect::Quality(t.quality_index(&o.quality))));
            }
        }
        let mut stack = vec![Frame { row: 0, pending, assembly: root_assembly }];
        while let Some(frame) = stack.pop() {
            t.expand(frame.row, &frame.assembly, &frame.pending, store, &mut |row, assembly, pending| {
                stack.push(Frame { row, assembly, pending })
            })?;
        }
        t.refresh_bounds(store);
        Ok(t)
    }

    /// Adds the rows one level under `row`, which places `assembly`, applying the
    /// overrides that reach them. Each child that is an unsuppressed sub-assembly is
    /// handed to `descend` with the overrides that reach below it.
    fn expand(
        &mut self,
        row: u32,
        assembly: &Assembly,
        pending: &[Pending],
        store: &mut DefinitionStore,
        descend: &mut dyn FnMut(u32, Arc<Assembly>, Vec<Pending>),
    ) -> Result<(), FlattenError> {
        let defs = intern_sources(assembly, store);
        let parent_world = self.world[row as usize];
        let parent_flags = self.flags[row as usize];
        // A pattern member starts visible and unsuppressed whatever its seed is; an
        // override hides or suppresses it.
        let components =
            assembly.components.iter().map(|c| (Step::component(c.id), c.placement, c.source, c.hidden, c.suppressed));
        let members = assembly.patterns.iter().flat_map(|p| {
            p.seeds.iter().filter_map(|s| assembly.component(*s)).flat_map(move |seed| {
                (1..p.rule.len()).map(move |i| {
                    (
                        Step::member(seed.id, p.id, i),
                        p.rule.member(i).compose(&seed.placement),
                        seed.source,
                        false,
                        false,
                    )
                })
            })
        });
        for (step, mut local, source, mut hidden, mut suppressed) in components.chain(members) {
            let Some(&def) = defs.get(&source) else { continue };
            // Overrides that land here; outer assemblies' come later in the list and win.
            let mut below = Vec::new();
            let mut colour = self.colour[row as usize];
            let mut coloured = false;
            let mut quality = self.quality[row as usize];
            for (rest, effect) in pending {
                if rest[0] != step {
                    continue;
                }
                if rest.len() > 1 {
                    below.push((rest[1..].to_vec(), effect.clone()));
                    continue;
                }
                match effect {
                    Effect::Override(o) => {
                        hidden = o.hidden.unwrap_or(hidden);
                        suppressed = o.suppressed.unwrap_or(suppressed);
                        local = o.placement.unwrap_or(local);
                    }
                    Effect::Colour(c) => {
                        colour = *c;
                        coloured = true;
                    }
                    Effect::Quality(q) => quality = *q,
                }
            }
            let is_assembly = store.get(def).kind == DocKind::Assembly;
            let def = if is_assembly { def } else { store.intern_at(def, self.quality_of(quality)) };
            let mut f = Flags::default();
            f.set(Flags::HIDDEN_SELF, hidden);
            f.set(Flags::HIDDEN, hidden || parent_flags.has(Flags::HIDDEN));
            f.set(Flags::SUPPRESSED_SELF, suppressed);
            f.set(Flags::SUPPRESSED, suppressed || parent_flags.has(Flags::SUPPRESSED));
            f.set(Flags::ASSEMBLY, is_assembly);
            f.set(Flags::COLOURED_SELF, coloured);
            let child = self.push(row, step, def, local, f);
            self.world[child as usize] = parent_world.compose(&local);
            self.colour[child as usize] = colour;
            self.quality[child as usize] = quality;
            if is_assembly && !f.has(Flags::SUPPRESSED) {
                let doc = store.get(def).key.document;
                let mut a = row;
                while a != NONE {
                    if store.get(self.definition[a as usize]).key.document == doc {
                        return Err(FlattenError::Cycle(self.path(OccurrenceIndex(child))));
                    }
                    a = self.parent[a as usize];
                }
                // A sub-assembly whose structure could not be read is a row with nothing
                // below it; the store says why.
                let Some(sub) = store.get(def).assembly.clone() else { continue };
                // This assembly's own overrides apply first; the outer ones after them win.
                let mut reach = self.own_effects(&sub);
                reach.extend(below);
                descend(child, sub, reach);
            }
        }
        Ok(())
    }

    /// What `assembly` does to the occurrences below it: its overrides, then its
    /// colours, as pending paths. Colours go into the palette.
    fn own_effects(&mut self, assembly: &Assembly) -> Vec<Pending> {
        let mut out: Vec<Pending> = assembly
            .overrides
            .iter()
            .map(|o| (o.path.steps().to_vec(), Effect::Override(Arc::new(o.clone()))))
            .collect();
        // Shown in the default configuration until configurations exist.
        for c in &assembly.appearances {
            if !soliddraft_materials::scope::applies(&c.scope, None) {
                continue;
            }
            if let Some(a) = assembly.appearance(&c.appearance) {
                out.push((c.path.steps().to_vec(), Effect::Colour(self.palette_index(a))));
            }
        }
        out
    }

    /// The palette entry for an appearance, plus one, added if new.
    fn palette_index(&mut self, a: &Appearance) -> u32 {
        match self.palette.iter().position(|p| p == a) {
            Some(i) => i as u32 + 1,
            None => {
                self.palette.push(a.clone());
                self.palette.len() as u32
            }
        }
    }

    /// The entry for a quality, plus one, added if new.
    fn quality_index(&mut self, q: &Quality) -> u32 {
        match self.qualities.iter().position(|p| p == q) {
            Some(i) => i as u32 + 1,
            None => {
                self.qualities.push(*q);
                self.qualities.len() as u32
            }
        }
    }

    fn quality_of(&self, index: u32) -> Option<&Quality> {
        (index != 0).then(|| &self.qualities[index as usize - 1])
    }

    /// The quality an occurrence is shown at when the assembly overrides its parts'
    /// own: the nearest override above it (or on it), or the assembly's quality for
    /// every part. `None` when its parts show at their own.
    pub fn quality(&self, i: OccurrenceIndex) -> Option<&Quality> {
        self.quality_of(self.quality[i.0 as usize])
    }

    /// The assembly-level colour an occurrence shows, if any: the nearest occurrence
    /// above it (or itself) that has one. Every face of the part shows it, replacing
    /// the part's own colours.
    pub fn colour(&self, i: OccurrenceIndex) -> Option<&Appearance> {
        let c = self.colour[i.0 as usize];
        (c != 0).then(|| &self.palette[c as usize - 1])
    }

    /// Every row's colour: 0 for none, otherwise one more than an index into
    /// [`OccurrenceTable::palette`].
    pub fn colours(&self) -> &[u32] {
        &self.colour
    }

    /// The appearances rows are coloured with.
    pub fn palette(&self) -> &[Appearance] {
        &self.palette
    }

    /// Colours `i` and everything under it that has no colour of its own nearer to it,
    /// or takes its colour away (`None`), after which it shows its parent's. Returns the
    /// rows whose colour changed.
    pub fn set_colour(&mut self, i: OccurrenceIndex, appearance: Option<&Appearance>) -> Vec<OccurrenceIndex> {
        self.flags[i.0 as usize].set(Flags::COLOURED_SELF, appearance.is_some());
        let parent = self.parent[i.0 as usize];
        let new = match appearance {
            Some(a) => self.palette_index(a),
            None if parent == NONE => 0,
            None => self.colour[parent as usize],
        };
        let old = self.colour[i.0 as usize];
        let mut changed = Vec::new();
        let mut stack = vec![i.0];
        while let Some(r) = stack.pop() {
            // A row below with a colour of its own keeps it, and so does its subtree.
            if r != i.0 && self.flags[r as usize].has(Flags::COLOURED_SELF) {
                continue;
            }
            if self.colour[r as usize] == old || r == i.0 {
                if self.colour[r as usize] != new {
                    self.colour[r as usize] = new;
                    changed.push(OccurrenceIndex(r));
                }
                let mut c = self.first_child[r as usize];
                while c != NONE {
                    stack.push(c);
                    c = self.next_sibling[c as usize];
                }
            }
        }
        changed
    }

    fn reserve(&mut self, n: usize) {
        self.parent.reserve(n);
        self.step.reserve(n);
        self.definition.reserve(n);
        self.local.reserve(n);
        self.world.reserve(n);
        self.bounds.reserve(n);
        self.flags.reserve(n);
        self.colour.reserve(n);
        self.quality.reserve(n);
        self.first_child.reserve(n);
        self.last_child.reserve(n);
        self.next_sibling.reserve(n);
        self.by_step.reserve(n);
    }

    fn push(&mut self, parent: u32, step: Step, def: DefinitionIndex, local: Placement, flags: Flags) -> u32 {
        let row = self.parent.len() as u32;
        self.parent.push(parent);
        self.step.push(step);
        self.definition.push(def);
        self.local.push(local);
        self.world.push(local);
        self.bounds.push(Aabb::EMPTY);
        self.flags.push(flags);
        self.colour.push(0);
        self.quality.push(0);
        self.first_child.push(NONE);
        self.last_child.push(NONE);
        self.next_sibling.push(NONE);
        if parent != NONE {
            let p = parent as usize;
            if self.last_child[p] == NONE {
                self.first_child[p] = row;
            } else {
                let last = self.last_child[p] as usize;
                self.next_sibling[last] = row;
            }
            self.last_child[p] = row;
            self.by_step.insert((parent, step), row);
        }
        self.rows_of = None;
        row
    }

    /// How many rows, removed ones included.
    pub fn len(&self) -> usize {
        self.parent.len()
    }

    /// True when the table has no rows (it always has the root once built).
    pub fn is_empty(&self) -> bool {
        self.parent.is_empty()
    }

    /// The containing row; `None` for the root.
    pub fn parent(&self, i: OccurrenceIndex) -> Option<OccurrenceIndex> {
        let p = self.parent[i.0 as usize];
        (p != NONE).then_some(OccurrenceIndex(p))
    }

    /// The step from the parent.
    pub fn step(&self, i: OccurrenceIndex) -> Step {
        self.step[i.0 as usize]
    }

    /// The definition placed.
    pub fn definition(&self, i: OccurrenceIndex) -> DefinitionIndex {
        self.definition[i.0 as usize]
    }

    /// The placement relative to the parent.
    pub fn local(&self, i: OccurrenceIndex) -> Placement {
        self.local[i.0 as usize]
    }

    /// The placement in the root's frame.
    pub fn world(&self, i: OccurrenceIndex) -> Placement {
        self.world[i.0 as usize]
    }

    /// The box around the occurrence in the root's frame; empty until its parts'
    /// meshes are loaded.
    pub fn bounds(&self, i: OccurrenceIndex) -> Aabb {
        self.bounds[i.0 as usize]
    }

    /// The flags.
    pub fn flags(&self, i: OccurrenceIndex) -> Flags {
        self.flags[i.0 as usize]
    }

    /// Every world placement, by row: what the renderer uploads.
    pub fn worlds(&self) -> &[Placement] {
        &self.world
    }

    /// Every definition, by row.
    pub fn definitions(&self) -> &[DefinitionIndex] {
        &self.definition
    }

    /// Every row's flags.
    pub fn all_flags(&self) -> &[Flags] {
        &self.flags
    }

    /// Every row's world bounds.
    pub fn all_bounds(&self) -> &[Aabb] {
        &self.bounds
    }

    /// The rows directly under `i`, in order.
    pub fn children(&self, i: OccurrenceIndex) -> Children<'_> {
        Children { table: self, next: self.first_child[i.0 as usize] }
    }

    /// The rows under `i` at any depth, and `i` itself first.
    pub fn subtree(&self, i: OccurrenceIndex) -> Vec<OccurrenceIndex> {
        let mut out = Vec::new();
        let mut stack = vec![i.0];
        while let Some(r) = stack.pop() {
            out.push(OccurrenceIndex(r));
            let mut c = self.first_child[r as usize];
            while c != NONE {
                stack.push(c);
                c = self.next_sibling[c as usize];
            }
        }
        out
    }

    /// The path of a row.
    pub fn path(&self, i: OccurrenceIndex) -> OccurrencePath {
        let mut steps = Vec::new();
        let mut r = i.0;
        while r != 0 && r != NONE {
            steps.push(self.step[r as usize]);
            r = self.parent[r as usize];
        }
        steps.reverse();
        OccurrencePath(steps)
    }

    /// The row at a path, if the path names one that is not removed.
    pub fn find(&self, path: &OccurrencePath) -> Option<OccurrenceIndex> {
        let mut r = 0u32;
        for s in path.steps() {
            r = *self.by_step.get(&(r, *s))?;
        }
        (!self.flags[r as usize].has(Flags::REMOVED)).then_some(OccurrenceIndex(r))
    }

    /// The rows placing each definition, indexed by definition.
    pub fn rows_of_definitions(&mut self, definitions: usize) -> Arc<Vec<Vec<u32>>> {
        if let Some(r) = &self.rows_of
            && r.len() >= definitions
        {
            return r.clone();
        }
        let mut out =
            vec![Vec::new(); definitions.max(self.definition.iter().map(|d| d.0 as usize + 1).max().unwrap_or(0))];
        for (row, d) in self.definition.iter().enumerate() {
            out[d.0 as usize].push(row as u32);
        }
        let out = Arc::new(out);
        self.rows_of = Some(out.clone());
        out
    }

    /// Recomputes every row's bounds from its definition's bounds: parts from their
    /// meshes, assemblies from their children.
    pub fn refresh_bounds(&mut self, store: &DefinitionStore) {
        // Children come after their parents, so one backward pass sees children first.
        self.bounds.fill(Aabb::EMPTY);
        for r in (0..self.len()).rev() {
            if self.flags[r].has(Flags::ASSEMBLY) {
                continue;
            }
            self.bounds[r] = store.get(self.definition[r]).bounds.placed(&self.world[r]);
        }
        for r in (1..self.len()).rev() {
            let f = self.flags[r];
            if f.has(Flags::SUPPRESSED) || f.has(Flags::REMOVED) {
                continue;
            }
            let p = self.parent[r] as usize;
            let b = self.bounds[r];
            self.bounds[p].add(&b);
        }
    }

    /// Updates the bounds of the rows placing `def`, after its mesh loaded, and of
    /// their ancestors.
    pub fn refresh_definition_bounds(&mut self, def: DefinitionIndex, store: &DefinitionStore) {
        let rows = self.rows_of_definitions(store.len());
        let local = store.get(def).bounds;
        for &r in rows.get(def.0 as usize).map(Vec::as_slice).unwrap_or(&[]) {
            let b = local.placed(&self.world[r as usize]);
            self.bounds[r as usize] = b;
            self.grow_ancestors(r, &b);
        }
    }

    fn grow_ancestors(&mut self, row: u32, b: &Aabb) {
        if b.is_empty() || self.flags[row as usize].has(Flags::SUPPRESSED) {
            return;
        }
        let mut p = self.parent[row as usize];
        while p != NONE {
            self.bounds[p as usize].add(b);
            p = self.parent[p as usize];
        }
    }

    /// Moves `i` to `local` (relative to its parent) and updates its subtree's world
    /// placements and bounds, and its ancestors' bounds. Returns the rows whose world
    /// placement changed.
    pub fn set_local(&mut self, i: OccurrenceIndex, local: Placement, store: &DefinitionStore) -> Vec<OccurrenceIndex> {
        self.local[i.0 as usize] = local;
        let rows = self.subtree(i);
        for &r in &rows {
            let parent = self.parent[r.0 as usize];
            let parent_world = if parent == NONE { Placement::IDENTITY } else { self.world[parent as usize] };
            self.world[r.0 as usize] = parent_world.compose(&self.local[r.0 as usize]);
        }
        // Bounds: parts first, then assemblies from their children, deepest first.
        for &r in rows.iter().rev() {
            let ri = r.0 as usize;
            if self.flags[ri].has(Flags::ASSEMBLY) {
                self.bounds[ri] = self.children_bounds(ri as u32);
            } else {
                self.bounds[ri] = store.get(self.definition[ri]).bounds.placed(&self.world[ri]);
            }
        }
        let mut p = self.parent[i.0 as usize];
        while p != NONE {
            self.bounds[p as usize] = self.children_bounds(p);
            p = self.parent[p as usize];
        }
        rows
    }

    fn children_bounds(&self, row: u32) -> Aabb {
        let mut b = Aabb::EMPTY;
        let mut c = self.first_child[row as usize];
        while c != NONE {
            let f = self.flags[c as usize];
            if !f.has(Flags::SUPPRESSED) && !f.has(Flags::REMOVED) {
                b.add(&self.bounds[c as usize]);
            }
            c = self.next_sibling[c as usize];
        }
        b
    }

    /// Hides or shows `i` and updates the effective hidden flag of its subtree.
    /// Returns the rows whose effective flag changed.
    pub fn set_hidden(&mut self, i: OccurrenceIndex, hidden: bool) -> Vec<OccurrenceIndex> {
        self.flags[i.0 as usize].set(Flags::HIDDEN_SELF, hidden);
        self.propagate(i, Flags::HIDDEN_SELF, Flags::HIDDEN)
    }

    /// Marks `i` suppressed or not. Its subtree's effective flag follows. A
    /// sub-assembly suppressed when the table was built has no rows below it; to show
    /// what is under it, flatten again.
    pub fn set_suppressed(&mut self, i: OccurrenceIndex, suppressed: bool) -> Vec<OccurrenceIndex> {
        self.flags[i.0 as usize].set(Flags::SUPPRESSED_SELF, suppressed);
        let changed = self.propagate(i, Flags::SUPPRESSED_SELF, Flags::SUPPRESSED);
        let mut p = self.parent[i.0 as usize];
        while p != NONE {
            self.bounds[p as usize] = self.children_bounds(p);
            p = self.parent[p as usize];
        }
        changed
    }

    fn propagate(&mut self, i: OccurrenceIndex, own: u8, effective: u8) -> Vec<OccurrenceIndex> {
        let mut changed = Vec::new();
        let mut stack = vec![i.0];
        while let Some(r) = stack.pop() {
            let ri = r as usize;
            let parent = self.parent[ri];
            let inherited = parent != NONE && self.flags[parent as usize].has(effective);
            let now = inherited || self.flags[ri].has(own);
            if self.flags[ri].has(effective) != now {
                self.flags[ri].set(effective, now);
                changed.push(OccurrenceIndex(r));
            }
            let mut c = self.first_child[ri];
            while c != NONE {
                stack.push(c);
                c = self.next_sibling[c as usize];
            }
        }
        changed
    }

    /// Adds a component under the root, with its subtree when it places a
    /// sub-assembly whose structure is loaded. Returns its row.
    pub fn insert(
        &mut self,
        parent: OccurrenceIndex,
        step: Step,
        def: DefinitionIndex,
        local: Placement,
        store: &mut DefinitionStore,
    ) -> Result<OccurrenceIndex, FlattenError> {
        let parent_flags = self.flags[parent.0 as usize];
        let is_assembly = store.get(def).kind == DocKind::Assembly;
        let quality = self.quality[parent.0 as usize];
        let def = if is_assembly { def } else { store.intern_at(def, self.quality_of(quality)) };
        let mut f = Flags::default();
        f.set(Flags::ASSEMBLY, is_assembly);
        f.set(Flags::HIDDEN, parent_flags.has(Flags::HIDDEN));
        f.set(Flags::SUPPRESSED, parent_flags.has(Flags::SUPPRESSED));
        let row = self.push(parent.0, step, def, local, f);
        self.world[row as usize] = self.world[parent.0 as usize].compose(&local);
        self.colour[row as usize] = self.colour[parent.0 as usize];
        self.quality[row as usize] = quality;
        if is_assembly {
            // Flatten the sub-assembly on its own and graft its rows.
            let part = OccurrenceTable::flatten(def, store)?;
            let base = row;
            let mut map = vec![NONE; part.len()];
            map[0] = base;
            for r in 1..part.len() {
                let p = map[part.parent[r] as usize];
                let mut fl = part.flags[r];
                fl.set(Flags::HIDDEN, fl.has(Flags::HIDDEN) || f.has(Flags::HIDDEN));
                fl.set(Flags::SUPPRESSED, fl.has(Flags::SUPPRESSED) || f.has(Flags::SUPPRESSED));
                let mut d = part.definition[r];
                if quality != 0 && !fl.has(Flags::ASSEMBLY) && part.quality[r] == 0 {
                    d = store.intern_at(d, self.quality_of(quality));
                }
                let nr = self.push(p, part.step[r], d, part.local[r], fl);
                self.quality[nr as usize] = if part.quality[r] == 0 {
                    quality
                } else {
                    self.quality_index(&part.qualities[part.quality[r] as usize - 1])
                };
                self.world[nr as usize] = self.world[p as usize].compose(&part.local[r]);
                self.colour[nr as usize] = match part.colour[r] {
                    0 => self.colour[p as usize],
                    c => self.palette_index(&part.palette[c as usize - 1]),
                };
                map[r] = nr;
            }
        }
        self.set_local(OccurrenceIndex(row), local, store);
        Ok(OccurrenceIndex(row))
    }

    /// Removes `i` and its subtree. Their rows stay, flagged removed, so other row
    /// numbers do not move. Returns the removed rows.
    pub fn remove(&mut self, i: OccurrenceIndex) -> Vec<OccurrenceIndex> {
        let rows = self.subtree(i);
        for r in &rows {
            self.flags[r.0 as usize].set(Flags::REMOVED, true);
        }
        let parent = self.parent[i.0 as usize];
        if parent != NONE {
            self.by_step.remove(&(parent, self.step[i.0 as usize]));
            // Unlink from the parent's children.
            let mut prev = NONE;
            let mut c = self.first_child[parent as usize];
            while c != NONE && c != i.0 {
                prev = c;
                c = self.next_sibling[c as usize];
            }
            let next = self.next_sibling[i.0 as usize];
            if prev == NONE {
                self.first_child[parent as usize] = next;
            } else {
                self.next_sibling[prev as usize] = next;
            }
            if self.last_child[parent as usize] == i.0 {
                self.last_child[parent as usize] = prev;
            }
            let mut p = parent;
            while p != NONE {
                self.bounds[p as usize] = self.children_bounds(p);
                p = self.parent[p as usize];
            }
        }
        rows
    }

    /// Counts: rows that are parts and drawn, parts in all, and sub-assemblies.
    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for f in &self.flags {
            if f.has(Flags::REMOVED) {
                continue;
            }
            if f.has(Flags::ASSEMBLY) {
                c.assemblies += 1;
            } else {
                c.parts += 1;
                if f.drawn_part() {
                    c.drawn += 1;
                }
            }
            if f.has(Flags::SUPPRESSED) {
                c.suppressed += 1;
            }
            if f.has(Flags::HIDDEN) {
                c.hidden += 1;
            }
        }
        c
    }

    /// Roughly how many bytes the table holds.
    pub fn heap_bytes(&self) -> usize {
        let per_row = 4
            + std::mem::size_of::<Step>()
            + 4
            + 2 * std::mem::size_of::<Placement>()
            + std::mem::size_of::<Aabb>()
            + 1
            + 16;
        self.len() * per_row + self.by_step.capacity() * (std::mem::size_of::<((u32, Step), u32)>() + 1)
    }
}

/// How many rows the tree under `def` flattens to, suppressed subtrees included (an
/// upper bound, to reserve space once).
fn rows_under(def: DefinitionIndex, store: &DefinitionStore, memo: &mut HashMap<DefinitionIndex, u64>) -> u64 {
    if let Some(n) = memo.get(&def) {
        return *n;
    }
    memo.insert(def, 0); // a cycle counts nothing; flattening reports it
    let Some(a) = store.get(def).assembly.clone() else { return 0 };
    // Rows one source places: itself, and everything under it when it is an assembly.
    let mut per_source = |source: crate::path::SourceId| -> u64 {
        let below = a
            .source(source)
            .filter(|s| s.document.kind == DocKind::Assembly)
            .and_then(|s| {
                let id = s.document.id?;
                store.find(&DefinitionKey::new(id, s.document.configuration.clone(), s.document.pinned.clone()))
            })
            .map(|sub| rows_under(sub, store, memo))
            .unwrap_or(0);
        1 + below
    };
    let mut n: u64 = a.components.iter().map(|c| per_source(c.source)).sum();
    for p in &a.patterns {
        let members = u64::from(p.rule.len().saturating_sub(1));
        for seed in p.seeds.iter().filter_map(|s| a.component(*s)) {
            n += members * per_source(seed.source);
        }
    }
    memo.insert(def, n);
    n
}

/// A fast hasher for the table's own integer keys (FxHash's multiply and rotate). The
/// keys are row numbers and ids, not input an attacker chooses, so SipHash's flood
/// resistance buys nothing and costs a third of flattening a million rows.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct BuildFast;

impl std::hash::BuildHasher for BuildFast {
    type Hasher = FastHasher;

    fn build_hasher(&self) -> FastHasher {
        FastHasher(0)
    }
}

/// The hasher [`BuildFast`] builds.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FastHasher(u64);

impl std::hash::Hasher for FastHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut b = [0u8; 8];
            b[..chunk.len()].copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(b));
        }
    }

    fn write_u32(&mut self, n: u32) {
        self.write_u64(u64::from(n));
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

/// What an assembly does to an occurrence below it.
#[derive(Clone, Debug)]
enum Effect {
    /// Hide, suppress or move it.
    Override(Arc<Override>),
    /// Colour it and everything under it: an index into the palette, plus one.
    Colour(u32),
    /// Show it and everything under it at a quality: an index into the qualities,
    /// plus one.
    Quality(u32),
}

/// An effect on its way down the tree: the rest of its path, and the effect.
type Pending = (Vec<Step>, Effect);

/// How many occurrences of each kind a table has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Part occurrences, suppressed and hidden ones included.
    pub parts: usize,
    /// Part occurrences that are drawn.
    pub drawn: usize,
    /// Sub-assembly occurrences, the root included.
    pub assemblies: usize,
    /// Suppressed occurrences.
    pub suppressed: usize,
    /// Hidden occurrences.
    pub hidden: usize,
}

/// The children of a row.
pub struct Children<'a> {
    table: &'a OccurrenceTable,
    next: u32,
}

impl Iterator for Children<'_> {
    type Item = OccurrenceIndex;

    fn next(&mut self) -> Option<OccurrenceIndex> {
        if self.next == NONE {
            return None;
        }
        let r = self.next;
        self.next = self.table.next_sibling[r as usize];
        Some(OccurrenceIndex(r))
    }
}
