//! The display cache: meshes keyed by definition and level.
//!
//! A definition is whatever the caller keys a body by, such as an assembly's
//! definition key (document, configuration, revision) or an open part's geometry hash.
//! Every placement of a definition draws from the same entries, so a million placements
//! of a bolt hold one bolt's meshes. Each entry is keyed by the exact tessellation
//! parameters that made it ([`ParamsKey`]), so a component that overrides the quality
//! gets its own entries beside the document's, and a quality change never serves a
//! mesh made for other parameters.
//!
//! Two levels whose meshes come out identical (a small part whose curves the angle
//! bounds rather than the chord) share one mesh and are counted once.
//!
//! The cache trims itself to the [`DeviceBudget`] by dropping the least recently used
//! standard and fine meshes. Coarse meshes stay, so every loaded definition can always
//! be drawn, and nothing used since the last [`tick`](DisplayCache::tick) is dropped.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;

use soliddraft_kernel_api::Mesh;

use crate::budget::{DeviceBudget, Usage};
use crate::level::Level;
use crate::policy::{Levels, ParamsKey};

/// How much display data a mesh is.
pub trait Weigh {
    /// Its triangles.
    fn triangles(&self) -> usize;
    /// Its bytes of vertex and index data.
    fn bytes(&self) -> usize;
}

impl Weigh for Mesh {
    fn triangles(&self) -> usize {
        self.triangle_count()
    }

    fn bytes(&self) -> usize {
        self.positions.len() * 24 + self.indices.len() * 4 + self.triangle_faces.len() * 8
    }
}

struct Entry<M> {
    key: ParamsKey,
    level: Level,
    mesh: Arc<M>,
    used: u64,
}

/// Meshes by definition and level, within a device budget.
pub struct DisplayCache<K, M> {
    budget: DeviceBudget,
    definitions: HashMap<K, Vec<Entry<M>>>,
    usage: Usage,
    clock: u64,
}

impl<K: Clone + Eq + Hash, M: Weigh + PartialEq> DisplayCache<K, M> {
    /// An empty cache within `budget`.
    pub fn new(budget: DeviceBudget) -> DisplayCache<K, M> {
        DisplayCache { budget, definitions: HashMap::new(), usage: Usage::default(), clock: 1 }
    }

    /// The budget.
    pub fn budget(&self) -> DeviceBudget {
        self.budget
    }

    /// Changes the budget. Call [`trim`](Self::trim) to apply a smaller one.
    pub fn set_budget(&mut self, budget: DeviceBudget) {
        self.budget = budget;
    }

    /// What the cache holds, each distinct mesh counted once.
    pub fn usage(&self) -> Usage {
        self.usage
    }

    /// The finest level the budget lets occurrences promote to now.
    pub fn ceiling(&self) -> Level {
        self.budget.ceiling(self.usage)
    }

    /// Starts a new frame: what is used from now on counts as in use.
    pub fn tick(&mut self) {
        self.clock += 1;
    }

    /// The number of definitions with at least one mesh.
    pub fn definition_count(&self) -> usize {
        self.definitions.len()
    }

    /// The number of distinct meshes held.
    pub fn mesh_count(&self) -> usize {
        self.definitions.values().map(|entries| distinct(entries).count()).sum()
    }

    /// The mesh made for `definition` with `key`, marked as used.
    pub fn get(&mut self, definition: &K, key: ParamsKey) -> Option<Arc<M>> {
        let clock = self.clock;
        let entry = self.definitions.get_mut(definition)?.iter_mut().find(|e| e.key == key)?;
        entry.used = clock;
        Some(entry.mesh.clone())
    }

    /// True when the cache holds the mesh made for `definition` with `key`.
    pub fn contains(&self, definition: &K, key: ParamsKey) -> bool {
        self.definitions.get(definition).is_some_and(|entries| entries.iter().any(|e| e.key == key))
    }

    /// The levels of `levels` the cache does not hold for `definition`, coarsest first.
    pub fn missing(&self, definition: &K, levels: &Levels) -> Vec<Level> {
        Level::ALL.into_iter().filter(|l| !self.contains(definition, levels.key(*l))).collect()
    }

    /// The mesh to draw for `definition` at `wanted`: that level if held, otherwise the
    /// nearest held level of the same settings, coarser ones first. Marked as used.
    pub fn best(&mut self, definition: &K, levels: &Levels, wanted: Level) -> Option<(Level, Arc<M>)> {
        let mut order = vec![wanted];
        let (mut down, mut up) = (wanted.coarser(), wanted.finer());
        while down.is_some() || up.is_some() {
            order.extend(down);
            order.extend(up);
            down = down.and_then(Level::coarser);
            up = up.and_then(Level::finer);
        }
        order.into_iter().find_map(|l| self.get(definition, levels.key(l)).map(|m| (l, m)))
    }

    /// Adds `mesh`, made for `definition` at `level` with `key`. A mesh equal to one the
    /// definition already holds shares it. Replaces an entry with the same key.
    pub fn insert(&mut self, definition: K, level: Level, key: ParamsKey, mesh: M) -> Arc<M> {
        self.remove_entry(&definition, key);
        let clock = self.clock;
        let entries = self.definitions.entry(definition).or_default();
        let mesh = match entries.iter().find(|e| *e.mesh == mesh) {
            Some(same) => same.mesh.clone(),
            None => {
                self.usage.triangles += mesh.triangles() as u64;
                self.usage.bytes += mesh.bytes() as u64;
                Arc::new(mesh)
            }
        };
        entries.push(Entry { key, level, mesh: mesh.clone(), used: clock });
        mesh
    }

    /// Adds the three meshes of `levels`, coarsest first, as
    /// [`Kernel::tessellate_levels`](soliddraft_kernel_api::Kernel::tessellate_levels)
    /// returns them.
    pub fn insert_levels(&mut self, definition: K, levels: &Levels, meshes: Vec<M>) -> Vec<Arc<M>> {
        Level::ALL
            .into_iter()
            .zip(meshes)
            .map(|(level, mesh)| self.insert(definition.clone(), level, levels.key(level), mesh))
            .collect()
    }

    /// Drops every mesh of `definition`, as when it changed or was unloaded.
    pub fn remove(&mut self, definition: &K) {
        if let Some(entries) = self.definitions.remove(definition) {
            for e in distinct(&entries) {
                self.forget(&e.mesh);
            }
        }
    }

    /// Drops the least recently used standard and fine meshes until the cache fits its
    /// budget, sparing coarse meshes and anything used this frame. Returns how many
    /// entries it dropped.
    pub fn trim(&mut self) -> usize {
        if self.budget.fits(self.usage) {
            return 0;
        }
        let mut candidates: Vec<(u64, K, ParamsKey)> = self
            .definitions
            .iter()
            .flat_map(|(k, entries)| {
                entries
                    .iter()
                    .filter(|e| e.level != Level::Coarse && e.used < self.clock)
                    .map(move |e| (e.used, k.clone(), e.key))
            })
            .collect();
        candidates.sort_by_key(|c| c.0);
        let mut dropped = 0;
        for (_, definition, key) in candidates {
            if self.budget.fits(self.usage) {
                break;
            }
            self.remove_entry(&definition, key);
            dropped += 1;
        }
        dropped
    }

    fn remove_entry(&mut self, definition: &K, key: ParamsKey) {
        let Some(entries) = self.definitions.get_mut(definition) else { return };
        let Some(i) = entries.iter().position(|e| e.key == key) else { return };
        let gone = entries.swap_remove(i);
        let shared = entries.iter().any(|e| Arc::ptr_eq(&e.mesh, &gone.mesh));
        if entries.is_empty() {
            self.definitions.remove(definition);
        }
        if !shared {
            self.forget(&gone.mesh);
        }
    }

    fn forget(&mut self, mesh: &M) {
        self.usage.triangles -= mesh.triangles() as u64;
        self.usage.bytes -= mesh.bytes() as u64;
    }
}

/// The entries with distinct meshes.
fn distinct<M>(entries: &[Entry<M>]) -> impl Iterator<Item = &Entry<M>> {
    entries
        .iter()
        .enumerate()
        .filter(|(i, e)| !entries[..*i].iter().any(|f| Arc::ptr_eq(&f.mesh, &e.mesh)))
        .map(|(_, e)| e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality::Quality;

    /// A stand-in mesh of `n` triangles and 100 bytes per triangle.
    #[derive(Debug, PartialEq)]
    struct Fake(usize, &'static str);

    impl Weigh for Fake {
        fn triangles(&self) -> usize {
            self.0
        }
        fn bytes(&self) -> usize {
            self.0 * 100
        }
    }

    fn levels() -> Levels {
        Levels::new(&Quality::Auto, 100.0)
    }

    #[test]
    fn levels_are_kept_per_definition_and_shared_when_equal() {
        let mut c: DisplayCache<&str, Fake> = DisplayCache::new(DeviceBudget::UNLIMITED);
        let l = levels();
        assert_eq!(c.missing(&"bolt", &l), Level::ALL);
        let got = c.insert_levels("bolt", &l, vec![Fake(10, "a"), Fake(10, "a"), Fake(40, "b")]);
        assert!(Arc::ptr_eq(&got[0], &got[1]), "coarse and standard came out the same");
        assert_eq!(c.usage(), Usage { triangles: 50, bytes: 5000 });
        assert_eq!(c.mesh_count(), 2);
        assert!(c.missing(&"bolt", &l).is_empty());
        assert_eq!(c.get(&"bolt", l.key(Level::Fine)).unwrap().0, 40);
        assert!(c.get(&"nut", l.key(Level::Fine)).is_none());

        // Another quality is another set of entries.
        let fine = Levels::new(&Quality::Slider { value: 8 }, 100.0);
        assert_eq!(c.missing(&"bolt", &fine), Level::ALL);
        c.insert("bolt", Level::Standard, fine.key(Level::Standard), Fake(80, "c"));
        assert_eq!(c.usage().triangles, 130);

        // Dropping one of two sharers keeps the mesh counted.
        c.remove_entry(&"bolt", l.key(Level::Coarse));
        assert_eq!(c.usage().triangles, 130);
        c.remove_entry(&"bolt", l.key(Level::Standard));
        assert_eq!(c.usage().triangles, 120);
        c.remove(&"bolt");
        assert_eq!(c.usage(), Usage::default());
        assert_eq!(c.definition_count(), 0);
    }

    #[test]
    fn the_best_held_level_stands_in() {
        let mut c: DisplayCache<u32, Fake> = DisplayCache::new(DeviceBudget::UNLIMITED);
        let l = levels();
        assert!(c.best(&1, &l, Level::Fine).is_none());
        c.insert(1, Level::Coarse, l.key(Level::Coarse), Fake(1, "coarse"));
        c.insert(1, Level::Fine, l.key(Level::Fine), Fake(16, "fine"));
        assert_eq!(c.best(&1, &l, Level::Fine).unwrap().0, Level::Fine);
        assert_eq!(c.best(&1, &l, Level::Standard).unwrap().0, Level::Coarse, "coarser first while loading");
        c.remove_entry(&1, l.key(Level::Coarse));
        assert_eq!(c.best(&1, &l, Level::Coarse).unwrap().0, Level::Fine);
    }

    #[test]
    fn trimming_spares_coarse_and_this_frame() {
        let mut c: DisplayCache<u32, Fake> = DisplayCache::new(DeviceBudget { triangles: 100, bytes: u64::MAX });
        let l = levels();
        for d in 0..4 {
            c.insert_levels(d, &l, vec![Fake(5, "c"), Fake(10, "s"), Fake(20, "f")]);
            c.tick();
        }
        assert_eq!(c.usage().triangles, 140);
        assert_eq!(c.ceiling(), Level::Coarse);
        // Definition 3 is in use this frame.
        c.get(&3, l.key(Level::Fine));
        let dropped = c.trim();
        assert!(c.budget().fits(c.usage()), "{:?}", c.usage());
        assert_eq!(dropped, 3, "the oldest went first");
        assert!(c.contains(&0, l.key(Level::Coarse)));
        assert!(!c.contains(&0, l.key(Level::Standard)) && !c.contains(&0, l.key(Level::Fine)));
        assert!(!c.contains(&1, l.key(Level::Standard)) && c.contains(&1, l.key(Level::Fine)));
        assert!(c.contains(&3, l.key(Level::Fine)));
        assert_eq!(c.usage().triangles, 100);
        assert_eq!(c.ceiling(), Level::Coarse);

        // Under a budget even coarse meshes exceed, coarse meshes still stay.
        c.set_budget(DeviceBudget { triangles: 1, bytes: 1 });
        c.tick();
        c.trim();
        assert_eq!(c.usage().triangles, 20);
        assert_eq!(c.definition_count(), 4);
    }
}
