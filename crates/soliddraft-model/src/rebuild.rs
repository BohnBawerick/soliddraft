//! The rebuild cache: feature builds kept by the exact inputs they read.
//!
//! Kernel bodies are immutable, so a feature built once from the same definition and the
//! same input bodies gives the same result. The cache keeps each build under a key of
//! its exact inputs: the feature, the exact bytes of its definition, the body it starts from,
//! and, for an extrusion, the build of the sketch it sweeps. A body handle names one
//! immutable body and is never reused, so it stands for everything that built the body.
//! Nothing is keyed by rounded coordinates.
//!
//! A rebuild walks the tree from the first changed feature and asks the cache before
//! building. An edit misses from the edited feature down. Undo, redo and a value put
//! back find the earlier builds and reuse their bodies.
//!
//! The cache owns the bodies its builds made. A copy of the part shares them, and the
//! last cache to drop a build releases its body. It keeps the retained bytes of its builds within a budget by dropping the least
//! recently used builds the current tree does not use, and with them every build that
//! started from a body it released.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use soliddraft_kernel_api::{BodyId, Kernel};

use crate::evaluate::Built;
use crate::feature::{Feature, FeatureId};

/// The default budget for kept builds, in bytes.
pub const DEFAULT_BUDGET: u64 = 256 << 20;

/// What a backend that cannot estimate a body's size is charged for one.
const UNKNOWN_BODY: u64 = 1 << 20;

/// The exact inputs of one feature build.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct StepKey {
    feature: FeatureId,
    definition: Definition,
    /// The part body the feature starts from.
    input: Option<BodyId>,
    /// The version of the sketch build an extrusion sweeps.
    profile: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Definition {
    /// The exact bytes of the feature's definition, without its name.
    Exact(Vec<u8>),
    /// A definition serde cannot write out: a key no later build matches, so the build
    /// is kept only for its body's sake.
    Unmatched(u64),
}

/// A body a kept build made, released when the last cache holding it lets go.
struct Kept {
    kernel: Arc<dyn Kernel>,
    body: BodyId,
}

impl Drop for Kept {
    fn drop(&mut self) {
        let _ = self.kernel.release(self.body);
    }
}

#[derive(Clone)]
struct Entry {
    built: Built,
    /// Holds `built.owned` alive.
    _body: Option<Arc<Kept>>,
    bytes: u64,
    used: u64,
}

/// Feature builds by exact inputs, within a byte budget. A clone shares the bodies.
#[derive(Clone)]
pub(crate) struct BuildCache {
    entries: HashMap<StepKey, Entry>,
    budget: u64,
    retained: u64,
    clock: u64,
    next_version: u64,
}

impl Default for BuildCache {
    fn default() -> Self {
        BuildCache { entries: HashMap::new(), budget: DEFAULT_BUDGET, retained: 0, clock: 0, next_version: 1 }
    }
}

impl BuildCache {
    /// The key of `feature` built on `input`, from the sketch build `profile` for an
    /// extrusion of a sketch.
    pub(crate) fn key(&mut self, feature: &Feature, input: Option<BodyId>, profile: Option<u64>) -> StepKey {
        let definition = match crate::exact::bytes(&feature.kind) {
            Some(bytes) => Definition::Exact(bytes),
            None => {
                self.next_version += 1;
                Definition::Unmatched(self.next_version)
            }
        };
        StepKey { feature: feature.id, definition, input, profile }
    }

    /// The budget, in bytes.
    pub(crate) fn budget(&self) -> u64 {
        self.budget
    }

    /// Changes the budget. The next [`trim`](Self::trim) applies a smaller one.
    pub(crate) fn set_budget(&mut self, bytes: u64) {
        self.budget = bytes;
    }

    /// The bytes the kept builds retain.
    pub(crate) fn retained(&self) -> u64 {
        self.retained
    }

    /// True when a kept build made `body`.
    pub(crate) fn holds(&self, body: BodyId) -> bool {
        self.entries.values().any(|e| e.built.owned == Some(body))
    }

    /// The build kept under `key`, marked as used.
    pub(crate) fn get(&mut self, key: &StepKey) -> Option<Built> {
        self.clock += 1;
        let entry = self.entries.get_mut(key)?;
        entry.used = self.clock;
        Some(entry.built.clone())
    }

    /// Keeps `built` under `key`, charging the size of the body it made, and returns it
    /// with its version.
    pub(crate) fn insert(&mut self, key: StepKey, mut built: Built, kernel: &Arc<dyn Kernel>) -> Built {
        self.clock += 1;
        built.version = self.next_version;
        self.next_version += 1;
        let body = built.owned.map_or(0, |b| kernel.retained_bytes(b).unwrap_or(UNKNOWN_BODY));
        let bytes = body + weight(&key, &built);
        self.retained += bytes;
        let kept = built.owned.map(|body| Arc::new(Kept { kernel: kernel.clone(), body }));
        let entry = Entry { built: built.clone(), _body: kept, bytes, used: self.clock };
        if let Some(old) = self.entries.insert(key, entry) {
            // A key is only inserted after it missed, so this is not expected; keep the
            // books right if it happens.
            self.retained -= old.bytes;
        }
        built
    }

    /// Imports exact builds from a fork and returns its version translation. Sketch
    /// versions are local to each cache, so dependencies are translated before reuse.
    pub(crate) fn import(&mut self, source: &BuildCache) -> HashMap<u64, u64> {
        let mut entries: Vec<_> = source.entries.iter().collect();
        entries.sort_unstable_by_key(|(_, entry)| entry.built.version);
        let mut versions = HashMap::new();
        let mut bodies: HashSet<_> = self.entries.values().filter_map(|e| e.built.owned).collect();
        for (source_key, source_entry) in entries {
            if matches!(source_key.definition, Definition::Unmatched(_))
                || source_key.input.is_some_and(|body| !bodies.contains(&body))
            {
                continue;
            }
            let mut key = source_key.clone();
            if let Some(profile) = key.profile {
                let Some(&version) = versions.get(&profile) else { continue };
                key.profile = Some(version);
            }
            if let Some(existing) = self.entries.get(&key) {
                // Never replace a live body's owner with a separately rebuilt body.
                if existing.built.output == source_entry.built.output {
                    versions.insert(source_entry.built.version, existing.built.version);
                }
                continue;
            }
            let mut entry = source_entry.clone();
            entry.built.version = self.next_version;
            self.next_version += 1;
            self.clock += 1;
            entry.used = self.clock;
            versions.insert(source_entry.built.version, entry.built.version);
            bodies.extend(entry.built.owned);
            self.retained += entry.bytes;
            self.entries.insert(key, entry);
        }
        versions
    }

    /// Drops the least recently used builds outside `live` until the cache fits its
    /// budget, then every build that started from a body or sketch build no longer
    /// kept. Lets go of the bodies of what it drops. Returns how many builds it dropped.
    pub(crate) fn trim(&mut self, live: &[Built]) -> usize {
        if self.retained <= self.budget {
            return 0;
        }
        let pinned: HashSet<u64> = live.iter().map(|b| b.version).filter(|v| *v != 0).collect();
        let mut candidates: Vec<(u64, StepKey)> = self
            .entries
            .iter()
            .filter(|(_, e)| !pinned.contains(&e.built.version))
            .map(|(k, e)| (e.used, k.clone()))
            .collect();
        candidates.sort_by_key(|c| c.0);
        let mut dropped = 0;
        for (_, key) in candidates {
            if self.retained <= self.budget {
                break;
            }
            self.remove(&key);
            dropped += 1;
        }
        // Builds that read a dropped body or sketch build can never be asked for again.
        loop {
            let bodies: HashSet<BodyId> = self.entries.values().filter_map(|e| e.built.owned).collect();
            let versions: HashSet<u64> = self.entries.values().map(|e| e.built.version).collect();
            let orphans: Vec<StepKey> = self
                .entries
                .iter()
                .filter(|(k, e)| {
                    !pinned.contains(&e.built.version)
                        && (k.input.is_some_and(|b| !bodies.contains(&b))
                            || k.profile.is_some_and(|v| !versions.contains(&v)))
                })
                .map(|(k, _)| k.clone())
                .collect();
            if orphans.is_empty() {
                break;
            }
            for key in orphans {
                self.remove(&key);
                dropped += 1;
            }
        }
        dropped
    }

    fn remove(&mut self, key: &StepKey) {
        if let Some(e) = self.entries.remove(key) {
            self.retained -= e.bytes;
        }
    }
}

/// The bytes a build keeps besides its body: its key, its names and its regions.
fn weight(key: &StepKey, built: &Built) -> u64 {
    let names = built.names.len() * std::mem::size_of::<crate::reference::NamedFace>();
    let definition = match &key.definition {
        Definition::Exact(bytes) => bytes.len(),
        Definition::Unmatched(_) => 0,
    };
    // Regions hold about as many points as the sketch they come from.
    let regions = if built.sketch.is_some() { definition } else { 0 };
    (std::mem::size_of::<Entry>() + std::mem::size_of::<StepKey>() + definition + names + regions) as u64
}
