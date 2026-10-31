//! An open assembly: its truth, what it shows, the edits made to it and their undo.
//!
//! [`AssemblyDocument`] is what the interface and the command line hold. Opening reads
//! the structure at once and loads display meshes in the background
//! ([`AssemblyDocument::poll`] takes in what has arrived), so the assembly can be shown
//! and edited from the first frame. Edits change the root's truth and the occurrence
//! table in place:
//!
//! - an occurrence that is one of the root's components is changed on the component;
//! - anything deeper, or a pattern member, is changed through an override in the root
//!   (an assembly-level change, like SolidWorks's), and its own file is not touched.
//!
//! Mates and values change through the commands in [`crate::mates`], which solve a
//! candidate and commit here. Every edit is one undo step. What changed for the renderer since it last looked is
//! collected in [`Changes`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use soliddraft_document::resolve::{Resolution, Resolver, repin};
use soliddraft_document::{ContentHash, DocKind, DocRef, DocumentId, OpenDocument};
use soliddraft_materials::{Appearance, Snapshots};
use soliddraft_model::{Document as PartDocument, ValueTable};
use soliddraft_tessellation::{Level, Quality, Settings};

use crate::file::{AssemblyPackage, FileError, RefCopy, display_entry, level_entry};
use crate::load::{self, LevelJob, LevelWorker, LoadError, LoadEvent, LoadHandle, Lookup, OpenOptions, Structure};
use crate::mates::bodies::{self, Scope};
use crate::mates::{MateEntry, MateRefusal, Proofs, SolverStatus};
use crate::occurrence::{Flags, OccurrenceIndex, OccurrenceTable, label_of};
use crate::path::{ComponentId, OccurrencePath, PatternId, SourceId, Step};
use crate::placement::Placement;
use crate::scene::Scene;
use crate::schema::{Assembly, Component, OccurrenceAppearance, Override, Pattern, PatternRule, Source};
use crate::source::KernelSource;
use crate::store::{DefinitionIndex, DefinitionKey, DefinitionStore, LoadState};

/// What changed since the renderer last asked ([`AssemblyDocument::take_changes`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Changes {
    /// Definitions whose display mesh arrived or changed.
    pub meshes: Vec<DefinitionIndex>,
    /// Rows whose world placement changed.
    pub moved: Vec<OccurrenceIndex>,
    /// Rows whose hidden, suppressed or removed flag changed.
    pub flags: Vec<OccurrenceIndex>,
    /// Rows whose assembly-level colour changed.
    pub colours: Vec<OccurrenceIndex>,
    /// Coarse or fine meshes that arrived.
    pub levels: Vec<(DefinitionIndex, Level)>,
    /// Rows were added, or the table was rebuilt: everything should be re-read.
    pub structure: bool,
}

impl Changes {
    /// True when nothing changed.
    pub fn is_empty(&self) -> bool {
        self.meshes.is_empty()
            && self.moved.is_empty()
            && self.flags.is_empty()
            && self.colours.is_empty()
            && self.levels.is_empty()
            && !self.structure
    }
}

/// Why an edit was refused.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    /// No occurrence has that path.
    #[error("no occurrence at {0}")]
    NoOccurrence(OccurrencePath),
    /// No component has that id.
    #[error("the assembly has no component {0}")]
    NoComponent(ComponentId),
    /// The root cannot be changed this way.
    #[error("the root assembly cannot be moved, hidden or suppressed")]
    Root,
    /// The edit is not consistent.
    #[error("{0}")]
    Invalid(String),
}

/// One row's editable state, for undo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RowState {
    row: OccurrenceIndex,
    local: Placement,
    hidden: bool,
    suppressed: bool,
}

/// One undo step: the truth before and after, and what to do to the table.
#[derive(Clone, Debug)]
struct Record {
    label: String,
    before: TruthImage,
    after: TruthImage,
    rows_before: Vec<RowState>,
    rows_after: Vec<RowState>,
    /// Rows the edit added (undo removes them) or removed (undo restores them).
    added: Vec<OccurrenceIndex>,
    removed: Vec<OccurrenceIndex>,
}

/// The mates and values, whole: a change to one can touch what reads it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Design {
    mates: Vec<MateEntry>,
    values: ValueTable,
}

impl Design {
    pub(crate) fn of(truth: &Assembly) -> Design {
        Design { mates: truth.mates.clone(), values: truth.values.clone() }
    }

    pub(crate) fn restore(&self, truth: &mut Assembly) {
        truth.mates = self.mates.clone();
        truth.values = self.values.clone();
    }
}

/// The records of the truth an edit touches.
#[derive(Clone, Debug, Default)]
pub(crate) struct TruthImage {
    components: Vec<(ComponentId, Option<Component>)>,
    overrides: Vec<(OccurrencePath, Option<Override>)>,
    sources: Vec<(SourceId, Option<Source>)>,
    patterns: Vec<(PatternId, Option<Pattern>)>,
    /// The colour assignments and appearance copies, when the edit changed colours,
    /// with the paths whose rows to repaint.
    colours: Option<(Vec<OccurrenceAppearance>, Snapshots, Vec<OccurrencePath>)>,
    /// The display settings, when the edit changed a quality.
    settings: Option<Settings>,
    /// The mates and values, when the edit changed them.
    pub(crate) design: Option<Design>,
    next_id: u32,
}

/// An open assembly.
pub struct AssemblyDocument {
    path: Option<PathBuf>,
    id: DocumentId,
    pub(crate) assembly: Assembly,
    /// Files whose display copies the document can carry into its own on save: the
    /// opened file first, then sub-assembly files the loader opened.
    packages: Vec<AssemblyPackage>,
    pub(crate) store: DefinitionStore,
    pub(crate) table: OccurrenceTable,
    root: DefinitionIndex,
    refs: HashMap<DefinitionIndex, DocRef>,
    /// The assembly whose file holds each definition's reference.
    parents: HashMap<DefinitionIndex, DefinitionIndex>,
    pub(crate) options: OpenOptions,
    loading: Option<LoadHandle>,
    /// Loads coarse and fine meshes on request, for the current generation.
    levels: Option<LevelWorker>,
    exact: HashMap<DefinitionIndex, Arc<Mutex<PartDocument>>>,
    undo: Vec<Record>,
    redo: Vec<Record>,
    pub(crate) changes: Changes,
    saved: ContentHash,
    /// The display settings as last saved or opened.
    saved_settings: Settings,
    /// The hierarchy, with the row count it was built for.
    bvh: Option<(usize, crate::bvh::Bvh)>,
    /// Rows moved, or whose meshes arrived, since the hierarchy was last refitted.
    pub(crate) bvh_moved: Vec<OccurrenceIndex>,
    /// A hierarchy being built in the background, for the structure generation it was
    /// started at.
    bvh_job: Option<(u64, std::sync::mpsc::Receiver<crate::bvh::Bvh>)>,
    /// Bumped whenever the table is rebuilt.
    generation: u64,
    /// Plane proofs and the parts rebuilt for them. Derived.
    pub(crate) proofs: Proofs,
    /// The mates' status, until the truth changes. Derived.
    pub(crate) solver_status: Option<SolverStatus>,
}

/// An assembly being opened on another thread ([`AssemblyDocument::open_in_background`]).
pub struct Opening {
    rx: std::sync::mpsc::Receiver<Result<(AssemblyDocument, Scene), LoadError>>,
}

impl Opening {
    /// The opened document and its scene once the structure is read, without waiting.
    pub fn try_take(&self) -> Option<Result<(AssemblyDocument, Scene), LoadError>> {
        self.rx.try_recv().ok()
    }

    /// The opened document and its scene, waiting for them.
    pub fn wait(self) -> Result<(AssemblyDocument, Scene), LoadError> {
        self.rx.recv().expect("the open thread sends its result")
    }
}

/// A selection in an assembly that outlives the session: the occurrence's path and the
/// persistent name of the picked face.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    /// The occurrence.
    pub path: OccurrencePath,
    /// The face's persistent name, when a face was picked.
    pub face: Option<String>,
}

impl AssemblyDocument {
    /// A new, empty, unsaved assembly.
    pub fn new(name: impl Into<String>) -> AssemblyDocument {
        AssemblyDocument::new_with(name, OpenOptions::default())
    }

    /// A new, empty, unsaved assembly that loads what is inserted into it as an opened
    /// one would, with `options`: before its first save it has no folder to resolve
    /// references from, so its parts come from `options.revisions` and `options.display`.
    pub fn new_with(name: impl Into<String>, options: OpenOptions) -> AssemblyDocument {
        let assembly = Assembly::new(name);
        let id = DocumentId::new();
        let mut doc = AssemblyDocument::from_parts(None, id, assembly, Vec::new(), options);
        let mut lookup = Lookup::new_empty(&doc.options);
        doc.restructure(&mut lookup).expect("an empty assembly flattens");
        doc
    }

    fn from_parts(
        path: Option<PathBuf>,
        id: DocumentId,
        assembly: Assembly,
        packages: Vec<AssemblyPackage>,
        options: OpenOptions,
    ) -> AssemblyDocument {
        let assembly_settings = assembly.settings.clone();
        let saved = assembly.semantic_hash();
        AssemblyDocument {
            path,
            id,
            assembly,
            packages,
            store: DefinitionStore::new(),
            table: OccurrenceTable::default(),
            root: DefinitionIndex(0),
            refs: HashMap::new(),
            parents: HashMap::new(),
            options,
            loading: None,
            levels: None,
            exact: HashMap::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            changes: Changes::default(),
            saved_settings: assembly_settings,
            saved,
            bvh: None,
            bvh_moved: Vec::new(),
            bvh_job: None,
            generation: 0,
            proofs: Proofs::default(),
            solver_status: None,
        }
    }

    /// Opens an assembly file: reads its structure now and starts loading its display
    /// meshes in the background.
    pub fn open(path: impl AsRef<Path>, options: OpenOptions) -> Result<AssemblyDocument, LoadError> {
        let path = path.as_ref();
        let package = AssemblyPackage::open(path)?;
        AssemblyDocument::open_package(package, Some(path), options)
    }

    /// Opens an assembly file on a thread of its own, with its [`Scene`], so the
    /// interface keeps drawing while the structure is read (one to two seconds for a
    /// million occurrences). The display meshes then load in the background as with
    /// [`AssemblyDocument::open`].
    pub fn open_in_background(path: impl AsRef<Path>, options: OpenOptions) -> Opening {
        let path = path.as_ref().to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("assembly open".into())
            .spawn(move || {
                let opened = AssemblyDocument::open(&path, options).map(|mut doc| {
                    let scene = Scene::new(&mut doc);
                    (doc, scene)
                });
                let _ = tx.send(opened);
            })
            .expect("the open thread starts");
        Opening { rx }
    }

    /// Opens an assembly from an opened file.
    pub fn open_package(
        package: AssemblyPackage,
        path: Option<&Path>,
        options: OpenOptions,
    ) -> Result<AssemblyDocument, LoadError> {
        let assembly = (**package.assembly()).clone();
        let mut doc =
            AssemblyDocument::from_parts(path.map(Path::to_path_buf), package.id(), assembly, Vec::new(), options);
        let mut lookup = Lookup::new(&package, path, &doc.options);
        doc.restructure(&mut lookup)?;
        doc.start_loading(lookup);
        Ok(doc)
    }

    /// Reads the structure for the current truth and replaces the store and table.
    fn restructure(&mut self, lookup: &mut Lookup) -> Result<(), LoadError> {
        let s: Structure =
            load::structure(self.id, self.assembly.semantic_hash(), Arc::new(self.assembly.clone()), lookup)?;
        // Keep the meshes already loaded for definitions that are still shown.
        let old = std::mem::take(&mut self.store);
        let mut store = s.store;
        for (i, d) in old.iter() {
            if let (Some(j), Some(mesh)) = (store.find(&d.key), &d.mesh)
                && store.get(j).state == LoadState::Unresolved
            {
                store.set_mesh(j, mesh.clone(), d.problem.clone().filter(|_| d.state == LoadState::Stale));
                store.get_mut(j).freshness = d.freshness.clone();
                let _ = i;
            }
        }
        self.store = store;
        self.table = s.table;
        self.table.refresh_bounds(&self.store);
        // Built now, on whatever thread opens the document, rather than on the first
        // mesh to arrive.
        self.table.rows_of_definitions(self.store.len());
        self.root = s.root;
        self.refs = s.refs;
        self.parents = s.parents;
        self.packages = lookup.packages.clone();
        self.exact.clear();
        self.changes = Changes { structure: true, ..Changes::default() };
        self.bvh = None;
        self.bvh_job = None;
        self.generation += 1;
        Ok(())
    }

    fn start_loading(&mut self, lookup: Lookup) {
        let parts: Vec<(DefinitionIndex, DocRef, Option<Quality>)> = self
            .store
            .iter()
            .filter(|(_, d)| d.kind == DocKind::Part && d.state == LoadState::Unresolved)
            .filter_map(|(i, d)| Some((i, self.refs.get(&i)?.clone(), d.quality)))
            .collect();
        // An unsaved assembly has no folder to resolve its references from, so its parts
        // stay unchecked rather than missing until it is saved.
        let mut refs: Vec<(DefinitionIndex, DocRef, DefinitionIndex)> = if self.options.check_freshness
            && self.path.is_some()
        {
            self.refs.iter().map(|(i, r)| (*i, r.clone(), self.parents.get(i).copied().unwrap_or(self.root))).collect()
        } else {
            Vec::new()
        };
        refs.sort_by_key(|(i, _, _)| *i);
        let freshness = load::Freshen { refs, root: self.root };
        self.loading = Some(load::spawn(lookup, parts, freshness, self.options.threads));
    }

    /// Takes in what the background loader has found so far, one batch at a time,
    /// until `budget` is spent (at least one batch); the rest waits for the next call.
    /// Returns true while loading goes on.
    pub fn poll(&mut self, budget: std::time::Duration) -> bool {
        let started = std::time::Instant::now();
        self.take_levels();
        loop {
            let Some(handle) = self.loading.as_mut() else { return false };
            let Some(event) = handle.try_event() else { break };
            self.apply_event(event);
            if started.elapsed() >= budget {
                break;
            }
        }
        let done = self.loading.as_ref().is_some_and(LoadHandle::is_done);
        if done {
            self.loading = None;
            if self.bvh.is_none() && self.bvh_job.is_none() {
                self.start_bvh();
            }
        }
        !done
    }

    /// Waits for the background loader to finish. Returns how long display and
    /// freshness took, when they ran.
    pub fn finish_loading(&mut self) -> Option<(std::time::Duration, std::time::Duration)> {
        let mut times = None;
        while let Some(handle) = self.loading.as_mut() {
            match handle.next_event() {
                Some(LoadEvent::Finished { display, freshness }) => times = Some((display, freshness)),
                Some(e) => self.apply_event(e),
                None => self.loading = None,
            }
        }
        times
    }

    /// Asks for the coarse or fine meshes of part definitions, loaded in the
    /// background: from the file's copies, from the part file's display data, or, for
    /// the fine level, rebuilt on the display source's kernel. They arrive through
    /// [`AssemblyDocument::poll`] and [`Changes::levels`]. Levels already loaded or
    /// asked for, standard levels, and definitions without a mesh are skipped.
    pub fn request_levels(&mut self, wanted: &[(DefinitionIndex, Level)]) {
        if wanted.is_empty() {
            return;
        }
        if self.levels.as_ref().is_none_or(|w| w.generation != self.generation) {
            let lookup = Lookup::with_packages(self.packages.clone(), self.path.as_deref(), &self.options);
            self.levels = Some(LevelWorker::spawn(lookup, self.generation));
        }
        let worker = self.levels.as_mut().expect("made above");
        for &(def, level) in wanted {
            if def.0 as usize >= self.store.len() || level == Level::Standard {
                continue;
            }
            let d = self.store.get(def);
            if d.kind != DocKind::Part || d.mesh.is_none() || d.level_mesh(level).is_some() {
                continue;
            }
            let Some(reference) = self.refs.get(&def).cloned() else { continue };
            worker.request(LevelJob { def, reference, quality: d.quality, level });
        }
    }

    /// How many requested levels have not arrived.
    pub fn levels_pending(&self) -> usize {
        self.levels.as_ref().map_or(0, |w| w.pending.len())
    }

    /// Waits until every requested level has arrived or failed. For tools and tests;
    /// the interface polls.
    pub fn finish_levels(&mut self) {
        while self.levels_pending() > 0 {
            if !self.take_levels() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }

    /// Takes in the levels that arrived. Returns true when any did.
    fn take_levels(&mut self) -> bool {
        let Some(worker) = self.levels.as_mut() else { return false };
        if worker.generation != self.generation {
            self.levels = None;
            return false;
        }
        let done = worker.take();
        let any = !done.is_empty();
        // A level that could not be made stays out; the occurrence keeps the level it has.
        for (def, level, mesh) in done {
            if let Ok(m) = mesh {
                self.store.set_level(def, level, m);
                self.changes.levels.push((def, level));
            }
        }
        any
    }

    /// True while meshes or freshness are still arriving.
    pub fn is_loading(&self) -> bool {
        self.loading.is_some()
    }

    fn apply_event(&mut self, e: LoadEvent) {
        match e {
            LoadEvent::Meshes(batch) => {
                for (i, result) in batch {
                    match result {
                        Ok((mesh, stale)) => {
                            self.store.set_mesh(i, mesh, stale);
                            self.table.refresh_definition_bounds(i, &self.store);
                            self.changes.meshes.push(i);
                            // Boxes grew from nothing: the hierarchy is rebuilt when next asked.
                            self.bvh = None;
                        }
                        Err(problem) => self.store.fail(i, problem),
                    }
                }
            }
            LoadEvent::Freshness(batch) => {
                for (i, f) in batch {
                    self.store.get_mut(i).freshness = f;
                }
            }
            LoadEvent::Finished { .. } => {}
        }
    }

    /// The changes since the last call.
    pub fn take_changes(&mut self) -> Changes {
        std::mem::take(&mut self.changes)
    }

    /// The document's id.
    pub fn id(&self) -> DocumentId {
        self.id
    }

    /// The file, once saved or opened.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The root's truth.
    pub fn assembly(&self) -> &Assembly {
        &self.assembly
    }

    /// The definitions.
    pub fn store(&self) -> &DefinitionStore {
        &self.store
    }

    /// The occurrences.
    pub fn table(&self) -> &OccurrenceTable {
        &self.table
    }

    /// The root assembly's definition.
    pub fn root(&self) -> DefinitionIndex {
        self.root
    }

    /// The reference each definition was reached by.
    pub fn reference(&self, def: DefinitionIndex) -> Option<&DocRef> {
        self.refs.get(&def)
    }

    /// The revision now, unsaved edits included.
    pub fn revision(&self) -> ContentHash {
        self.assembly.semantic_hash()
    }

    /// True when there are edits since the last save or open, display settings
    /// included.
    pub fn is_dirty(&self) -> bool {
        self.revision() != self.saved || self.assembly.settings != self.saved_settings
    }

    /// The display settings: a quality for every part, and components shown at their
    /// own. Saved in the file, outside the revision.
    pub fn settings(&self) -> &Settings {
        &self.assembly.settings
    }

    /// The quality the occurrence at `path` is shown at when the assembly overrides
    /// its parts' own: its own override, the nearest above it, or the assembly's
    /// quality for every part. `None` when its parts show at their own.
    pub fn quality_at(&self, path: &OccurrencePath) -> Option<&Quality> {
        self.table.find(path).and_then(|row| self.table.quality(row))
    }

    /// This document as the resolver sees an open one, so a drawing or a parent
    /// assembly resolves to it.
    pub fn as_open(&self) -> OpenDocument {
        OpenDocument { id: self.id, kind: DocKind::Assembly, path: self.path.clone(), revision: self.revision() }
    }

    /// The label the tree shows for an occurrence: the component's name, or its
    /// source's file name with its id ("bolt<12>"); a pattern member adds its index.
    pub fn label(&self, i: OccurrenceIndex) -> String {
        if i == OccurrenceIndex::ROOT {
            return self.assembly.name.clone();
        }
        let step = self.table.step(i);
        let def = self.store.get(self.table.definition(i));
        let named = if self.table.parent(i) == Some(OccurrenceIndex::ROOT) {
            self.assembly.component(step.component).and_then(|c| c.name.clone())
        } else {
            let parent = self.table.parent(i).map(|p| self.table.definition(p));
            parent
                .and_then(|p| self.store.get(p).assembly.clone())
                .and_then(|a| a.component(step.component).and_then(|c| c.name.clone()))
        };
        let base = named.unwrap_or_else(|| format!("{}<{}>", def.label, step.component));
        match step.member {
            None => base,
            Some((_, index)) => format!("{base} #{index}"),
        }
    }

    /// What a pick names, as a selection that outlives this session: the occurrence's
    /// path and, when a face was picked, the part's persistent name for it (empty when
    /// the display copy has none).
    pub fn selection(&self, picked: &crate::scene::Picked) -> Selection {
        let def = self.store.get(self.table.definition(picked.occurrence));
        let face =
            picked.face.and_then(|f| def.mesh.as_ref().and_then(|m| m.faces.get(f as usize)).map(|f| f.name.clone()));
        Selection { path: self.table.path(picked.occurrence), face }
    }

    /// The hierarchy of occurrence boxes, refitted after moves. Once loading is done
    /// it is built on a background thread ([`AssemblyDocument::poll`] starts it); this
    /// waits for that build, or builds it here when none is under way.
    pub fn bvh(&mut self) -> &crate::bvh::Bvh {
        if let Some((_, rx)) = self.bvh_job.take()
            && let Ok(b) = rx.recv()
        {
            self.install_bvh(b);
        }
        let stale = self.bvh.as_ref().is_none_or(|(n, _)| *n != self.table.len());
        if stale {
            self.bvh = Some((self.table.len(), crate::bvh::Bvh::build(&self.table)));
            self.bvh_moved.clear();
        }
        self.refit_bvh();
        &self.bvh.as_ref().expect("built above").1
    }

    /// The hierarchy if it is ready, without waiting: for the interface, which asks
    /// every frame and must never stall on a build.
    pub fn bvh_ready(&mut self) -> Option<&crate::bvh::Bvh> {
        if let Some((generation, rx)) = &self.bvh_job
            && let Ok(b) = rx.try_recv()
        {
            let current = *generation == self.generation;
            self.bvh_job = None;
            if current {
                self.install_bvh(b);
            }
        }
        if self.bvh.as_ref().is_some_and(|(n, _)| *n == self.table.len()) {
            self.refit_bvh();
            return self.bvh.as_ref().map(|(_, b)| b);
        }
        None
    }

    fn install_bvh(&mut self, b: crate::bvh::Bvh) {
        // Rows moved while it was built are refitted next.
        self.bvh = Some((self.table.len(), b));
    }

    fn refit_bvh(&mut self) {
        if self.bvh_moved.is_empty() {
            return;
        }
        let moved = std::mem::take(&mut self.bvh_moved);
        if let Some((_, b)) = self.bvh.as_mut() {
            b.refit(&self.table, &moved);
        }
    }

    /// Starts building the hierarchy on another thread from a copy of the boxes.
    fn start_bvh(&mut self) {
        let (bounds, flags) = (self.table.all_bounds().to_vec(), self.table.all_flags().to_vec());
        let origin = self.table.bounds(OccurrenceIndex::ROOT).center();
        let (tx, rx) = std::sync::mpsc::channel();
        let started = std::thread::Builder::new().name("assembly bvh".into()).spawn(move || {
            let _ = tx.send(crate::bvh::Bvh::build_from(&bounds, &flags, origin));
        });
        if started.is_ok() {
            self.bvh_moved.clear();
            self.bvh_job = Some((self.generation, rx));
        }
    }

    // ----- edits -----

    fn row(&self, path: &OccurrencePath) -> Result<OccurrenceIndex, EditError> {
        if path.is_root() {
            return Err(EditError::Root);
        }
        self.table.find(path).ok_or_else(|| EditError::NoOccurrence(path.clone()))
    }

    pub(crate) fn row_state(&self, row: OccurrenceIndex) -> RowState {
        let f = self.table.flags(row);
        RowState {
            row,
            local: self.table.local(row),
            hidden: f.has(Flags::HIDDEN_SELF),
            suppressed: f.has(Flags::SUPPRESSED_SELF),
        }
    }

    pub(crate) fn image(&self, components: &[ComponentId], overrides: &[OccurrencePath]) -> TruthImage {
        TruthImage {
            components: components.iter().map(|c| (*c, self.assembly.component(*c).cloned())).collect(),
            overrides: overrides.iter().map(|p| (p.clone(), self.assembly.override_at(p).cloned())).collect(),
            sources: Vec::new(),
            patterns: Vec::new(),
            colours: None,
            settings: None,
            design: None,
            next_id: self.assembly.next_id,
        }
    }

    /// Where an occurrence's state lives in the root's truth: its component when it is
    /// one of the root's own, otherwise an override at its path.
    pub(crate) fn owner(path: &OccurrencePath) -> Result<ComponentId, OccurrencePath> {
        match path.steps() {
            [s] if s.member.is_none() => Ok(s.component),
            _ => Err(path.clone()),
        }
    }

    pub(crate) fn record(&mut self, label: &str, before: TruthImage, rows_before: Vec<RowState>) {
        self.solver_status = None;
        let after = self.reimage(&before);
        let rows_after = rows_before.iter().map(|r| self.row_state(r.row)).collect();
        self.undo.push(Record {
            label: label.to_owned(),
            before,
            after,
            rows_before,
            rows_after,
            added: Vec::new(),
            removed: Vec::new(),
        });
        self.redo.clear();
    }

    fn reimage(&self, like: &TruthImage) -> TruthImage {
        TruthImage {
            components: like.components.iter().map(|(c, _)| (*c, self.assembly.component(*c).cloned())).collect(),
            overrides: like.overrides.iter().map(|(p, _)| (p.clone(), self.assembly.override_at(p).cloned())).collect(),
            sources: like.sources.iter().map(|(s, _)| (*s, self.assembly.source(*s).cloned())).collect(),
            patterns: like.patterns.iter().map(|(p, _)| (*p, self.assembly.pattern(*p).cloned())).collect(),
            colours: like.colours.as_ref().map(|(_, _, paths)| {
                (self.assembly.appearances.clone(), self.assembly.snapshots.clone(), paths.clone())
            }),
            settings: like.settings.as_ref().map(|_| self.assembly.settings.clone()),
            design: like.design.as_ref().map(|_| Design::of(&self.assembly)),
            next_id: self.assembly.next_id,
        }
    }

    /// Moves occurrences to new placements relative to the assembly that contains
    /// each. One undo step.
    pub fn move_to(&mut self, moves: &[(OccurrencePath, Placement)]) -> Result<(), EditError> {
        let rows: Vec<OccurrenceIndex> = moves.iter().map(|(p, _)| self.row(p)).collect::<Result<_, _>>()?;
        if moves.iter().any(|(p, _)| self.is_mated(p)) {
            return self.move_mated(moves);
        }
        let (comps, ovs) = split_owners(moves.iter().map(|(p, _)| p));
        let before = self.image(&comps, &ovs);
        let rows_before = rows.iter().map(|r| self.row_state(*r)).collect();
        for ((path, placement), row) in moves.iter().zip(&rows) {
            match Self::owner(path) {
                Ok(c) => self.assembly.component_mut(c).ok_or(EditError::NoComponent(c))?.placement = *placement,
                Err(p) => self.assembly.override_mut(&p).placement = Some(*placement),
            }
            let moved = self.table.set_local(*row, *placement, &self.store);
            self.bvh_moved.extend(moved.iter().copied());
            self.changes.moved.extend(moved);
        }
        self.record("Move", before, rows_before);
        Ok(())
    }

    /// Moves occurrences by `delta`, applied in the root's frame after their current
    /// placement. One undo step.
    pub fn move_by(&mut self, paths: &[OccurrencePath], delta: &Placement) -> Result<(), EditError> {
        let mut moves = Vec::with_capacity(paths.len());
        for p in paths {
            let row = self.row(p)?;
            let parent_world = self.table.parent(row).map(|r| self.table.world(r)).unwrap_or_default();
            let world = delta.compose(&self.table.world(row));
            moves.push((p.clone(), parent_world.inverse().compose(&world)));
        }
        self.move_to(&moves)
    }

    /// Hides or shows occurrences. One undo step.
    pub fn set_hidden(&mut self, paths: &[OccurrencePath], hidden: bool) -> Result<(), EditError> {
        let rows: Vec<OccurrenceIndex> = paths.iter().map(|p| self.row(p)).collect::<Result<_, _>>()?;
        let (comps, ovs) = split_owners(paths.iter());
        let before = self.image(&comps, &ovs);
        let rows_before = rows.iter().map(|r| self.row_state(*r)).collect();
        for (path, row) in paths.iter().zip(&rows) {
            match Self::owner(path) {
                Ok(c) => self.assembly.component_mut(c).ok_or(EditError::NoComponent(c))?.hidden = hidden,
                Err(p) => self.assembly.override_mut(&p).hidden = Some(hidden),
            }
            let changed = self.table.set_hidden(*row, hidden);
            self.changes.flags.extend(changed);
        }
        self.record(if hidden { "Hide" } else { "Show" }, before, rows_before);
        Ok(())
    }

    /// Suppresses or unsuppresses occurrences. One undo step. A sub-assembly that was
    /// suppressed when the document opened has no rows below it; unsuppressing it
    /// rereads the structure.
    pub fn set_suppressed(&mut self, paths: &[OccurrencePath], suppressed: bool) -> Result<(), EditError> {
        let rows: Vec<OccurrenceIndex> = paths.iter().map(|p| self.row(p)).collect::<Result<_, _>>()?;
        let (comps, ovs) = split_owners(paths.iter());
        let before = self.image(&comps, &ovs);
        let rows_before = rows.iter().map(|r| self.row_state(*r)).collect();
        let mut regrow = false;
        for (path, row) in paths.iter().zip(&rows) {
            match Self::owner(path) {
                Ok(c) => self.assembly.component_mut(c).ok_or(EditError::NoComponent(c))?.suppressed = suppressed,
                Err(p) => self.assembly.override_mut(&p).suppressed = Some(suppressed),
            }
            let f = self.table.flags(*row);
            regrow |= !suppressed && f.has(Flags::ASSEMBLY) && self.table.children(*row).next().is_none();
            let changed = self.table.set_suppressed(*row, suppressed);
            self.changes.flags.extend(changed);
        }
        self.record(if suppressed { "Suppress" } else { "Unsuppress" }, before, rows_before);
        if regrow {
            self.reflatten();
        }
        Ok(())
    }

    /// Gives occurrences an assembly-level colour, or takes it away (`None`). In this
    /// assembly every face of every part under each occurrence then shows the
    /// appearance, replacing the parts' own colours; the part files keep theirs. One
    /// undo step.
    pub fn set_colour(&mut self, paths: &[OccurrencePath], appearance: Option<Appearance>) -> Result<(), EditError> {
        let rows: Vec<OccurrenceIndex> = paths.iter().map(|p| self.row(p)).collect::<Result<_, _>>()?;
        if let Some(a) = &appearance
            && let Some(problem) = a.problems().first()
        {
            return Err(EditError::Invalid(format!("the appearance's {} {}", problem.field, problem.message)));
        }
        let mut before = self.image(&[], &[]);
        before.colours = Some((self.assembly.appearances.clone(), self.assembly.snapshots.clone(), paths.to_vec()));
        for (path, row) in paths.iter().zip(&rows) {
            self.assembly.set_colour(path, appearance.clone());
            let changed = self.table.set_colour(*row, appearance.as_ref());
            self.changes.colours.extend(changed);
        }
        self.record(if appearance.is_some() { "Colour" } else { "Remove colour" }, before, Vec::new());
        Ok(())
    }

    /// Shows occurrences, and everything under them, at `quality` instead of their
    /// parts' own, or takes the override away (`None`). The parts are re-tessellated
    /// for these occurrences only: other placements of the same parts keep their
    /// meshes. The assembly's revision does not change. One undo step.
    pub fn set_quality(&mut self, paths: &[OccurrencePath], quality: Option<Quality>) -> Result<(), EditError> {
        for p in paths {
            self.row(p)?;
        }
        if let Some(q) = &quality {
            q.check().map_err(EditError::Invalid)?;
        }
        let mut next = self.assembly.settings.clone();
        for p in paths {
            next.set_override(&p.to_string(), quality);
        }
        self.change_settings(if quality.is_some() { "Quality" } else { "Remove quality" }, next);
        Ok(())
    }

    /// Shows every part of the assembly at `quality` instead of at its own, or at its
    /// own again with [`Quality::Auto`]. Overrides on components still win. One undo
    /// step; the revision does not change.
    pub fn set_viewport_quality(&mut self, quality: Quality) -> Result<(), EditError> {
        quality.check().map_err(EditError::Invalid)?;
        let next = Settings { viewport: quality, ..self.assembly.settings.clone() };
        self.change_settings("Assembly quality", next);
        Ok(())
    }

    fn change_settings(&mut self, label: &str, next: Settings) {
        if next == self.assembly.settings {
            return;
        }
        let mut before = self.image(&[], &[]);
        before.settings = Some(self.assembly.settings.clone());
        self.assembly.settings = next;
        self.reflatten();
        let after = self.reimage(&before);
        self.undo.push(Record {
            label: label.into(),
            before,
            after,
            rows_before: Vec::new(),
            rows_after: Vec::new(),
            added: Vec::new(),
            removed: Vec::new(),
        });
        self.redo.clear();
    }

    /// Renames one of the root's components; `None` goes back to the source's name.
    pub fn rename(&mut self, component: ComponentId, name: Option<String>) -> Result<(), EditError> {
        let before = self.image(&[component], &[]);
        self.assembly.component_mut(component).ok_or(EditError::NoComponent(component))?.name =
            name.filter(|n| !n.trim().is_empty());
        self.record("Rename", before, Vec::new());
        Ok(())
    }

    /// Places `document` (a part or an assembly, pinned at the revision it carries) in
    /// the root at `placement`. Returns the new component. Its display mesh loads like
    /// any other: call [`AssemblyDocument::poll`].
    pub fn insert(&mut self, document: DocRef, placement: Placement) -> Result<ComponentId, EditError> {
        if !matches!(document.kind, DocKind::Part | DocKind::Assembly) || !document.is_bound() {
            return Err(EditError::Invalid("only a part or an assembly with an id can be inserted".into()));
        }
        if document.id == Some(self.id) {
            return Err(EditError::Invalid("an assembly cannot contain itself".into()));
        }
        let mut before = self.image(&[], &[]);
        let source = self.assembly.add_source(document.clone());
        before.sources.push((source, None));
        let id = self.assembly.add_component(source, placement);
        before.components.push((id, None));
        self.reflatten();
        self.solver_status = None;
        let after = self.reimage(&before);
        self.undo.push(Record {
            label: "Insert component".into(),
            before,
            after,
            rows_before: Vec::new(),
            rows_after: Vec::new(),
            added: Vec::new(),
            removed: Vec::new(),
        });
        self.redo.clear();
        Ok(id)
    }

    /// Adds a pattern of the root's components. One undo step.
    pub fn add_pattern(&mut self, seeds: Vec<ComponentId>, rule: PatternRule) -> Result<PatternId, EditError> {
        if let Some(s) = seeds.iter().find(|s| self.assembly.component(**s).is_none()) {
            return Err(EditError::NoComponent(*s));
        }
        let mut before = self.image(&[], &[]);
        let id = self.assembly.add_pattern(seeds, rule);
        if let Err(e) = self.assembly.validate() {
            self.assembly.patterns.retain(|p| p.id != id);
            self.assembly.next_id = before.next_id;
            return Err(EditError::Invalid(e.0));
        }
        before.patterns.push((id, None));
        self.reflatten();
        let after = self.reimage(&before);
        self.undo.push(Record {
            label: "Pattern".into(),
            before,
            after,
            rows_before: Vec::new(),
            rows_after: Vec::new(),
            added: Vec::new(),
            removed: Vec::new(),
        });
        self.redo.clear();
        Ok(id)
    }

    /// Deletes components of the root, with the patterns that only repeat them and
    /// the overrides below them. One undo step.
    pub fn delete(&mut self, components: &[ComponentId]) -> Result<(), EditError> {
        if let Some(c) = components.iter().find(|c| self.assembly.component(**c).is_none()) {
            return Err(EditError::NoComponent(*c));
        }
        let gone: HashSet<ComponentId> = components.iter().copied().collect();
        let held: Vec<&str> = self
            .assembly
            .known_mates()
            .filter(|m| m.components().iter().any(|c| gone.contains(c)))
            .map(|m| m.name())
            .collect();
        if !held.is_empty() {
            return Err(EditError::Invalid(format!("delete its mates first: {}", held.join(", "))));
        }
        let mut before = self.image(components, &[]);
        let overrides: Vec<OccurrencePath> = self
            .assembly
            .overrides
            .iter()
            .filter(|o| o.path.steps().first().is_some_and(|s| gone.contains(&s.component)))
            .map(|o| o.path.clone())
            .collect();
        before.overrides = overrides.iter().map(|p| (p.clone(), self.assembly.override_at(p).cloned())).collect();
        let patterns: Vec<PatternId> =
            self.assembly.patterns.iter().filter(|p| p.seeds.iter().any(|s| gone.contains(s))).map(|p| p.id).collect();
        before.patterns = patterns.iter().map(|p| (*p, self.assembly.pattern(*p).cloned())).collect();
        self.assembly.components.retain(|c| !gone.contains(&c.id));
        self.assembly.overrides.retain(|o| !overrides.contains(&o.path));
        for p in &mut self.assembly.patterns {
            p.seeds.retain(|s| !gone.contains(s));
        }
        self.assembly.patterns.retain(|p| !p.seeds.is_empty());
        let mut removed = Vec::new();
        for row in self.table.children(OccurrenceIndex::ROOT).collect::<Vec<_>>() {
            if gone.contains(&self.table.step(row).component) {
                removed.extend(self.table.remove(row));
            }
        }
        self.changes.flags.extend(removed.iter().copied());
        self.solver_status = None;
        let after = self.reimage(&before);
        self.undo.push(Record {
            label: "Delete".into(),
            before,
            after,
            rows_before: Vec::new(),
            rows_after: Vec::new(),
            added: Vec::new(),
            removed,
        });
        self.redo.clear();
        Ok(())
    }

    /// Rereads the structure from the truth, keeping loaded meshes, and loads what is
    /// new in the background.
    pub(crate) fn reflatten(&mut self) {
        self.finish_loading();
        let mut lookup = Lookup::with_packages(self.packages.clone(), self.path.as_deref(), &self.options);
        if let Err(e) = self.restructure(&mut lookup) {
            // The truth was valid before; an edit that breaks flattening is undone by
            // the caller's undo record.
            log_problem(&e.to_string());
        }
        self.start_loading(lookup);
    }

    /// How many accepted edits this assembly can undo.
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// How many undone edits this assembly can redo.
    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    /// The label of the step undo would revert.
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|r| r.label.as_str())
    }

    /// The label of the step redo would repeat.
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|r| r.label.as_str())
    }

    /// Reverts the last edit.
    pub fn undo(&mut self) -> bool {
        let Some(r) = self.undo.pop() else { return false };
        self.apply_record(&r, true);
        self.redo.push(r);
        true
    }

    /// Repeats the last undone edit.
    pub fn redo(&mut self) -> bool {
        let Some(r) = self.redo.pop() else { return false };
        self.apply_record(&r, false);
        self.undo.push(r);
        true
    }

    fn apply_record(&mut self, r: &Record, undo: bool) {
        let (image, rows) = if undo { (&r.before, &r.rows_before) } else { (&r.after, &r.rows_after) };
        for (id, c) in &image.components {
            self.assembly.components.retain(|x| x.id != *id);
            if let Some(c) = c {
                self.assembly.components.push(c.clone());
            }
        }
        for (path, o) in &image.overrides {
            self.assembly.overrides.retain(|x| x.path != *path);
            if let Some(o) = o {
                self.assembly.overrides.push(o.clone());
            }
        }
        for (id, s) in &image.sources {
            self.assembly.sources.retain(|x| x.id != *id);
            if let Some(s) = s {
                self.assembly.sources.push(s.clone());
            }
        }
        for (id, p) in &image.patterns {
            self.assembly.patterns.retain(|x| x.id != *id);
            if let Some(p) = p {
                self.assembly.patterns.push(p.clone());
            }
        }
        self.assembly.next_id = image.next_id;
        if let Some((appearances, snapshots, paths)) = &image.colours {
            self.assembly.appearances = appearances.clone();
            self.assembly.snapshots = snapshots.clone();
            for p in paths {
                if let Some(row) = self.table.find(p) {
                    let a = self.assembly.colour_at(p, None).cloned();
                    let changed = self.table.set_colour(row, a.as_ref());
                    self.changes.colours.extend(changed);
                }
            }
        }
        if let Some(settings) = &image.settings {
            self.assembly.settings = settings.clone();
        }
        if let Some(design) = &image.design {
            design.restore(&mut self.assembly);
        }
        self.assembly.normalize();
        self.solver_status = None;
        let seed_moved = image.design.is_some()
            && self
                .assembly
                .patterns
                .iter()
                .any(|p| p.seeds.iter().any(|s| image.components.iter().any(|(c, _)| c == s)));
        let structural = !image.sources.is_empty()
            || !image.patterns.is_empty()
            || !r.removed.is_empty()
            || !r.added.is_empty()
            || image.settings.is_some()
            || seed_moved;
        if structural {
            self.reflatten();
            return;
        }
        for s in rows {
            if self.table.local(s.row) != s.local {
                let moved = self.table.set_local(s.row, s.local, &self.store);
                self.bvh_moved.extend(moved.iter().copied());
                self.changes.moved.extend(moved);
            }
            let f = self.table.flags(s.row);
            if f.has(Flags::HIDDEN_SELF) != s.hidden {
                let changed = self.table.set_hidden(s.row, s.hidden);
                self.changes.flags.extend(changed);
            }
            if f.has(Flags::SUPPRESSED_SELF) != s.suppressed {
                let changed = self.table.set_suppressed(s.row, s.suppressed);
                self.changes.flags.extend(changed);
            }
        }
    }

    // ----- saving -----

    /// The display copies to write: every definition a live occurrence shows, from
    /// the files it was loaded from, or encoded from what is loaded. Stale and failed
    /// definitions have no copy of their pinned revision to write.
    pub fn copies(&self) -> Vec<RefCopy> {
        let mut used: HashSet<DefinitionIndex> = HashSet::new();
        for (row, f) in self.table.all_flags().iter().enumerate().skip(1) {
            if !f.has(Flags::REMOVED) {
                used.insert(self.table.definitions()[row]);
            }
        }
        let mut readers: Vec<_> = self.packages.iter().map(AssemblyPackage::reader).collect();
        let mut sorted: Vec<DefinitionIndex> = used.into_iter().collect();
        sorted.sort();
        // One copy per document and revision; a part shown at several qualities has
        // one mesh entry per quality in it.
        let mut out: Vec<RefCopy> = Vec::with_capacity(sorted.len());
        let mut at: HashMap<(DocumentId, ContentHash), usize> = HashMap::new();
        for i in sorted {
            let d = self.store.get(i);
            if i == self.root || matches!(d.state, LoadState::Stale | LoadState::Failed) {
                continue;
            }
            let (id, rev) = (d.key.document, &d.key.revision);
            if let Some(a) = &d.assembly {
                if let Some(c) = readers.iter_mut().find_map(|r| r.copy(id, rev).ok().flatten()) {
                    out.push(c);
                } else {
                    out.push(RefCopy::assembly(id, rev.clone(), &a.truth_entries()));
                }
                continue;
            }
            let name = display_entry(d.quality.as_ref());
            let bytes: Option<Arc<[u8]>> = readers
                .iter_mut()
                .find_map(|r| r.entry(id, rev, &name).ok().flatten())
                .map(Into::into)
                .or_else(|| d.mesh.as_ref().map(|m| m.encode("soliddraft").into()));
            let Some(bytes) = bytes else { continue };
            let slot = *at.entry((id, rev.clone())).or_insert_with(|| {
                out.push(RefCopy { document: id, revision: rev.clone(), entries: Vec::new() });
                out.len() - 1
            });
            out[slot].entries.push((name, bytes));
            // The coarse level too, when it was loaded or the file already has it, so a
            // reopened assembly can show far parts coarse without asking for them.
            let coarse = level_entry(d.quality.as_ref(), Level::Coarse);
            let coarse_bytes: Option<Arc<[u8]>> =
                d.coarse.as_ref().map(|m| m.encode("soliddraft coarse").into()).or_else(|| {
                    readers.iter_mut().find_map(|r| r.entry(id, rev, &coarse).ok().flatten()).map(Into::into)
                });
            if let Some(b) = coarse_bytes {
                out[slot].entries.push((coarse, b));
            }
        }
        for c in &mut out {
            c.entries.sort_by(|a, b| a.0.cmp(&b.0));
        }
        out
    }

    /// Saves to `path` (the document's own file when `None`), atomically, with the
    /// display copies of everything it shows.
    pub fn save(&mut self, path: Option<&Path>) -> Result<PathBuf, FileError> {
        let path = match path.or(self.path.as_deref()) {
            Some(p) => soliddraft_document::kind::saved_path(p, DocKind::Assembly),
            None => return Err(FileError::Io(std::io::Error::other("the assembly has no file yet; name one"))),
        };
        // References store paths relative to the file, so a new folder rewrites them. An
        // assembly saved for the first time holds them as they were given: absolute, or
        // relative to the working folder.
        let old = self.path.clone().unwrap_or_default();
        if self.path.is_none() || old.parent() != path.parent() {
            for s in self.assembly.sources.iter_mut().filter(|s| !s.document.path.is_empty()) {
                let target = s.document.stored_path(&old);
                s.document.set_path(&path, &target);
            }
        }
        self.finish_loading();
        let copies = self.copies();
        self.assembly.save(&path, self.id, None, &copies)?;
        self.saved = self.revision();
        self.saved_settings = self.assembly.settings.clone();
        self.path = Some(path.clone());
        // The saved file now holds every copy shown: read them from it first. The files
        // read before stay behind it, so undoing a Rebuild still finds the old copies.
        if let Ok(p) = AssemblyPackage::open(&path) {
            self.packages.insert(0, p);
        }
        Ok(path)
    }

    // ----- exact work -----

    /// The part a definition shows, rebuilt on `source`'s kernel at the pinned
    /// revision: what measure, mass and drawing projection read. The definition is
    /// exact-ready until [`AssemblyDocument::release_exact`]. Fails when the pinned
    /// revision's bytes are not available (the document moved on and no revision store
    /// has the old one): Rebuild to use the current version.
    pub fn exact(&mut self, def: DefinitionIndex, source: &KernelSource) -> Result<Arc<Mutex<PartDocument>>, String> {
        if let Some(e) = self.exact.get(&def) {
            return Ok(e.clone());
        }
        let d = self.store.get(def);
        if d.kind != DocKind::Part {
            return Err("only parts have an exact body".into());
        }
        let r = self.refs.get(&def).ok_or("the definition has no reference")?.clone();
        let bytes = self.revision_bytes(&r)?;
        let part = Arc::new(Mutex::new(source.exact(&bytes)?));
        self.exact.insert(def, part.clone());
        self.store.get_mut(def).state = LoadState::ExactReady;
        Ok(part)
    }

    /// The file of the revision `r` pins: the referenced file while it is at that
    /// revision, otherwise the revision store's copy. What is found is handed to the
    /// store, so a pin stays provable after the file moves on.
    pub(crate) fn revision_bytes(&self, r: &DocRef) -> Result<Vec<u8>, String> {
        let id = r.id.ok_or("the reference has no document id")?;
        let on_disk = match self.resolver().resolve(r) {
            Resolution::Found(t) if t.revision() == Some(&r.pinned) => t.path().and_then(|p| std::fs::read(p).ok()),
            _ => None,
        };
        // An open document's revision can be one its file does not hold yet.
        let matches =
            |b: &Vec<u8>| soliddraft_document::identify(std::io::Cursor::new(b)).is_ok_and(|i| i.revision == r.pinned);
        if let Some(bytes) = on_disk.filter(matches) {
            if let Some(store) = &self.options.revisions {
                store.record(id, &r.pinned, &bytes);
            }
            return Ok(bytes);
        }
        match &self.options.revisions {
            Some(store) => store.fetch(id, &r.pinned).map_err(|e| e.to_string()),
            None => Err(format!(
                "revision {} of {} is not available; Rebuild to use the current version",
                r.pinned.short(),
                label_of(r)
            )),
        }
    }

    /// Drops a definition's exact body.
    pub fn release_exact(&mut self, def: DefinitionIndex) {
        if self.exact.remove(&def).is_some() {
            let d = self.store.get_mut(def);
            d.state = if d.mesh.is_some() { LoadState::DisplayReady } else { LoadState::MetadataOnly };
        }
    }

    // ----- references -----

    /// A resolver for this document's references.
    pub fn resolver(&self) -> Resolver {
        Resolver::new(self.path.as_deref())
            .with_search_folders(self.options.search.iter())
            .with_open(self.options.open.iter().cloned())
    }

    /// Refreshes root source status without changing pins, truth or undo history.
    /// Before the first save, only matching open documents can be checked.
    pub fn refresh_references(&mut self, open: Vec<OpenDocument>) {
        self.options.open = open;
        let resolver = self.resolver();
        let freshness: HashMap<_, _> = self
            .assembly
            .sources
            .iter()
            .filter_map(|source| {
                let r = &source.document;
                let key = key_of(r)?;
                let state = if self.path.is_none()
                    && !self.options.open.iter().any(|d| Some(d.id) == r.id && d.kind == r.kind)
                {
                    crate::Freshness::Unchecked
                } else {
                    match resolver.resolve(r) {
                        Resolution::Found(t) => match t.revision() {
                            Some(now) if *now != r.pinned => crate::Freshness::NewerAvailable { now: now.clone() },
                            _ => crate::Freshness::Current,
                        },
                        Resolution::Missing(_) => crate::Freshness::Missing,
                    }
                };
                Some((key, state))
            })
            .collect();
        let definitions: Vec<_> = self
            .store
            .iter()
            .filter_map(|(i, d)| freshness.get(&d.key.at(None)).map(|state| (i, state.clone())))
            .collect();
        for (i, state) in definitions {
            self.store.get_mut(i).freshness = state;
        }
    }

    /// A candidate of this document with `truth` in its place, at `path` under `id`, its
    /// structure read and its meshes loaded, so a change that restructures the tree can be
    /// shown and solved before it is accepted. `extra` files are looked in first.
    pub(crate) fn candidate(
        &self,
        path: Option<PathBuf>,
        id: DocumentId,
        truth: Assembly,
        extra: Vec<AssemblyPackage>,
    ) -> Result<AssemblyDocument, LoadError> {
        let mut packages = extra;
        packages.extend(self.packages.iter().cloned());
        let mut candidate = AssemblyDocument::from_parts(path, id, truth, packages.clone(), self.options.clone());
        // Meshes already loaded are kept for the definitions the candidate still shows.
        candidate.store = self.store.clone();
        let mut lookup = Lookup::with_packages(packages, candidate.path.as_deref(), &self.options);
        candidate.restructure(&mut lookup)?;
        candidate.start_loading(lookup);
        candidate.finish_loading();
        Ok(candidate)
    }

    /// The truth an edit that restructures the tree touches: these components and
    /// sources, the mates and values whole, and the id counter.
    pub(crate) fn structural_image(&self, components: &[ComponentId], sources: &[SourceId]) -> TruthImage {
        let mut image = self.image(components, &[]);
        image.sources = sources.iter().map(|s| (*s, self.assembly.source(*s).cloned())).collect();
        image.design = Some(Design::of(&self.assembly));
        image
    }

    /// Makes a candidate's truth and structure this document's, as one undo step.
    pub(crate) fn adopt(&mut self, label: &str, candidate: AssemblyDocument, before: TruthImage) {
        self.assembly = candidate.assembly;
        self.solver_status = None;
        self.store = candidate.store;
        self.table = candidate.table;
        self.root = candidate.root;
        self.refs = candidate.refs;
        self.parents = candidate.parents;
        self.packages = candidate.packages;
        self.levels = None;
        self.exact.clear();
        self.bvh = None;
        self.bvh_job = None;
        self.bvh_moved.clear();
        self.generation += 1;
        self.changes = Changes { structure: true, ..Changes::default() };
        let after = self.reimage(&before);
        self.undo.push(Record {
            label: label.into(),
            before,
            after,
            rows_before: Vec::new(),
            rows_after: Vec::new(),
            added: Vec::new(),
            removed: Vec::new(),
        });
        self.redo.clear();
    }

    /// True when moving the occurrence at `path` moves a part a mate reads: it is one, or
    /// holds one.
    fn is_mated(&self, path: &OccurrencePath) -> bool {
        let scope = Scope::new(&self.assembly, &self.table, &self.store);
        self.gather(&scope).is_ok_and(|p| p.bodies.keys().any(|b| b.steps().starts_with(path.steps())))
            || Self::owner(path).is_ok_and(|c| self.assembly.known_mates().any(|m| m.components().contains(&c)))
    }

    /// Solves a loaded candidate's mates and moves its parts. Returns the components and
    /// overrides it changed.
    fn solve_candidate(
        &mut self,
        candidate: &mut AssemblyDocument,
    ) -> Result<(Vec<ComponentId>, Vec<OccurrencePath>), MateRefusal> {
        let problem = self.gather(&Scope::new(&candidate.assembly, &candidate.table, &candidate.store))?;
        let solved = self.solve_problem(problem)?;
        let kept =
            bodies::locals(&Scope::new(&candidate.assembly, &candidate.table, &candidate.store), &solved.placements);
        let (mut components, mut overrides) = (Vec::new(), Vec::new());
        for k in &kept {
            match Self::owner(&k.path) {
                Ok(c) => components.push(c),
                Err(p) => overrides.push(p),
            }
        }
        let placed: Vec<(OccurrencePath, Placement)> = kept.iter().map(|k| (k.path.clone(), k.local)).collect();
        bodies::keep(&mut candidate.assembly, kept);
        for (path, placement) in placed {
            if let Some(row) = candidate.table.find(&path) {
                candidate.table.set_local(row, placement, &candidate.store);
            }
        }
        Ok((components, overrides))
    }

    /// Accepts every root source together, only after its proposed revision can be
    /// shown and its mates solve on it. A refusal keeps the accepted truth, display and
    /// undo history.
    pub fn rebuild(&mut self) -> RebuildReport {
        let started = std::time::Instant::now();
        self.finish_loading();
        let resolver = self.resolver();
        let mut proposed = self.assembly.clone();
        let mut report = RebuildReport::default();
        let mut problems = Vec::new();
        for source in &mut proposed.sources {
            let r = &mut source.document;
            let before = r.pinned.clone();
            let label = label_of(r);
            let (status, moved) = match resolver.resolve(r) {
                Resolution::Found(t) => {
                    let moved = t.new_path.clone();
                    repin(r, &t);
                    (if r.pinned == before { RefChange::Unchanged } else { RefChange::Updated }, moved)
                }
                Resolution::Missing(_) => {
                    problems.push(format!("{}: missing", r.path));
                    (RefChange::Missing, None)
                }
            };
            report.sources.push(SourceReport {
                source: source.id,
                label,
                before,
                after: r.pinned.clone(),
                status,
                moved,
            });
        }
        if problems.is_empty() {
            let mut candidate = AssemblyDocument::from_parts(
                self.path.clone(),
                self.id,
                proposed,
                self.packages.clone(),
                self.options.clone(),
            );
            candidate.store = self.store.clone();
            let mut lookup = Lookup::with_packages(self.packages.clone(), self.path.as_deref(), &self.options);
            match candidate.restructure(&mut lookup) {
                Err(error) => problems.push(format!(
                    "{}: {error}",
                    self.assembly.sources.iter().map(|s| s.document.path.as_str()).collect::<Vec<_>>().join(", ")
                )),
                Ok(()) => {
                    candidate.start_loading(lookup);
                    candidate.finish_loading();
                    for (i, definition) in candidate.store.iter() {
                        if i == candidate.root {
                            continue;
                        }
                        let showable = match definition.kind {
                            DocKind::Part => {
                                matches!(definition.state, LoadState::DisplayReady | LoadState::ExactReady)
                            }
                            _ => !matches!(definition.state, LoadState::Failed | LoadState::Stale),
                        };
                        if !showable {
                            let label = candidate.refs.get(&i).map(|r| r.path.as_str()).unwrap_or(&definition.label);
                            problems.push(format!("{label}: cannot be shown at the proposed revision"));
                        }
                    }
                    let changes = report.sources.iter().any(|s| s.status == RefChange::Updated || s.moved.is_some());
                    let mut moved = (Vec::new(), Vec::new());
                    if problems.is_empty() && changes {
                        match self.solve_candidate(&mut candidate) {
                            Ok(placed) => moved = placed,
                            Err(refusal) => {
                                problems.push(refusal.to_string());
                                report.mate_refusal = Some(refusal);
                            }
                        }
                    }
                    if problems.is_empty() && changes {
                        let mut before = self.image(&moved.0, &moved.1);
                        before.sources = self.assembly.sources.iter().map(|s| (s.id, Some(s.clone()))).collect();
                        self.adopt("Update", candidate, before);
                    }
                }
            }
        }
        if !problems.is_empty() {
            report.refusal = Some(format!("Update refused: {}; the accepted revisions are kept", problems.join("; ")));
            for source in &mut report.sources {
                source.after = source.before.clone();
                source.moved = None;
                if source.status == RefChange::Updated {
                    source.status = RefChange::Unchanged;
                }
            }
        }
        let open = self.options.open.clone();
        self.refresh_references(open);
        report.elapsed = started.elapsed();
        report
    }
}

/// What a Rebuild did to one source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefChange {
    /// The accepted pin did not change.
    Unchanged,
    /// Re-pinned at a newer revision.
    Updated,
    /// The document could not be found; the pin is kept.
    Missing,
}

/// One source in a Rebuild report.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceReport {
    /// The source.
    pub source: SourceId,
    /// Its file name without the extension.
    pub label: String,
    /// The revision pinned before.
    pub before: ContentHash,
    /// The revision pinned now.
    pub after: ContentHash,
    /// What happened.
    pub status: RefChange,
    /// The new stored path, when the document was found elsewhere.
    pub moved: Option<String>,
}

/// What a Rebuild did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RebuildReport {
    /// Every source of the root.
    pub sources: Vec<SourceReport>,
    /// Why Update was refused, with the blocking source paths.
    pub refusal: Option<String>,
    /// The mates' refusal, when they were why.
    pub mate_refusal: Option<MateRefusal>,
    /// How long resolution and loading took.
    pub elapsed: std::time::Duration,
}

impl RebuildReport {
    /// How many sources were re-pinned at a newer revision.
    pub fn updated(&self) -> usize {
        self.sources.iter().filter(|s| s.status == RefChange::Updated).count()
    }

    /// How many sources could not be found.
    pub fn missing(&self) -> usize {
        self.sources.iter().filter(|s| s.status == RefChange::Missing).count()
    }
}

/// The root's components and the override paths that hold the state of `paths`.
fn split_owners<'a>(paths: impl Iterator<Item = &'a OccurrencePath>) -> (Vec<ComponentId>, Vec<OccurrencePath>) {
    let (mut comps, mut ovs) = (Vec::new(), Vec::new());
    for p in paths {
        match AssemblyDocument::owner(p) {
            Ok(c) => comps.push(c),
            Err(o) => ovs.push(o),
        }
    }
    comps.sort();
    comps.dedup();
    ovs.sort();
    ovs.dedup();
    (comps, ovs)
}

fn log_problem(message: &str) {
    eprintln!("soliddraft-assembly: {message}");
}

impl Lookup {
    /// A lookup with no files: for a new assembly.
    pub(crate) fn new_empty(options: &OpenOptions) -> Lookup {
        Lookup::with_packages(Vec::new(), None, options)
    }
}

/// A definition key for a reference.
pub fn key_of(r: &DocRef) -> Option<DefinitionKey> {
    Some(DefinitionKey::new(r.id?, r.configuration.clone(), r.pinned.clone()))
}

/// The occurrences of a step under a row, for callers that build paths.
pub fn child_path(table: &OccurrenceTable, parent: OccurrenceIndex, step: Step) -> OccurrencePath {
    table.path(parent).child(step)
}

#[allow(dead_code)]
fn _store_type_check(_: &DefinitionStore) {}
