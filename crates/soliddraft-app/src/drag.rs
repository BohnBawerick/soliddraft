//! Drag previews: while the user drags a feature's dimension, intermediate results come
//! from a worker off the interface thread, and the exact result of the final value is
//! applied as one command, so one drag is one undo step.
//!
//! The worker holds a fork of the part, which shares the part's feature builds on the
//! same kernel. For each value it edits the dragged feature in the fork and lets the
//! model rebuild from that feature down, so an intermediate result is the whole part as the edit would leave it. It
//! answers only the newest value it has been asked for, so a fast drag gets results at
//! the worker's pace and none for values the drag has already passed. Results arrive as
//! display meshes without picks: a preview is shown and labelled, never saved, exported,
//! measured or used for a reference. Those all read the document, which a drag leaves
//! alone until its final value is applied.
//!
//! A drag's [`Setup`] is the document revision, the selection, the feature and the
//! dimension. When any of them changes the drag is obsolete: its results and errors are
//! dropped and nothing is applied. Within one drag, a result older than the one shown is
//! dropped, and an error is shown only for the newest value; an error for a value the
//! drag has passed is dropped.
//!
//! The meshes of values already built are kept for the rest of the drag, starting with
//! the document's own mesh at the starting value, so dragging back over them shows them
//! at once without asking the worker.

use crate::document::{
    Command, DisplayMesh, Document, EndKind, FeatureId, FeatureKind, FeatureStatus, Selection, fmt_mm, preview_mesh,
};
use soliddraft_kernel_api::{Kernel, TessellationParams};
use soliddraft_model as model;
use std::sync::{Arc, mpsc};

/// A feature dimension a drag can change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dimension {
    /// A boss's depth, or a blind cut's.
    Depth,
    Radius,
    Distance,
}

impl Dimension {
    /// The dimension a feature offers for dragging, if it has one.
    pub fn of(kind: &FeatureKind) -> Option<Dimension> {
        match kind {
            FeatureKind::Extrude(p) if p.end == EndKind::Blind => Some(Dimension::Depth),
            FeatureKind::Cut(p) if !p.through_all && p.up_to.is_none() => Some(Dimension::Depth),
            FeatureKind::Fillet(_) => Some(Dimension::Radius),
            FeatureKind::Chamfer(_) => Some(Dimension::Distance),
            FeatureKind::Extrude(_)
            | FeatureKind::Cut(_)
            | FeatureKind::Sketch(_)
            | FeatureKind::Hole(_)
            | FeatureKind::Pattern(_)
            | FeatureKind::Mirror(_)
            | FeatureKind::Draft(_)
            | FeatureKind::Shell(_)
            | FeatureKind::ImportStep(_)
            | FeatureKind::Lattice(_)
            | FeatureKind::Revolve(_) => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Dimension::Depth => "Depth",
            Dimension::Radius => "Radius",
            Dimension::Distance => "Distance",
        }
    }

    /// The dimension's value in `kind`, when `kind` has it.
    pub fn get(self, kind: &FeatureKind) -> Option<f64> {
        match (self, kind) {
            (Dimension::Depth, FeatureKind::Extrude(p)) => Some(p.depth),
            (Dimension::Depth, FeatureKind::Cut(p)) if !p.through_all => Some(p.depth),
            (Dimension::Radius, FeatureKind::Fillet(p)) => Some(p.radius),
            (Dimension::Distance, FeatureKind::Chamfer(p)) => Some(p.distance),
            _ => None,
        }
    }

    /// `kind` with the dimension set to `value`.
    pub fn with(self, kind: &FeatureKind, value: f64) -> FeatureKind {
        let mut kind = kind.clone();
        match (self, &mut kind) {
            (Dimension::Depth, FeatureKind::Extrude(p)) => p.depth = value,
            (Dimension::Depth, FeatureKind::Cut(p)) => p.depth = value,
            (Dimension::Radius, FeatureKind::Fillet(p)) => p.radius = value,
            (Dimension::Distance, FeatureKind::Chamfer(p)) => p.distance = value,
            _ => {}
        }
        kind
    }
}

/// The command that edits feature `id` to `kind`.
pub fn edit_command(id: FeatureId, kind: FeatureKind) -> Option<Command> {
    Some(match kind {
        FeatureKind::Extrude(params) => Command::EditExtrude { id, params },
        FeatureKind::Cut(params) => Command::EditCut { id, params },
        FeatureKind::Fillet(params) => Command::EditFillet { id, params },
        FeatureKind::Chamfer(params) => Command::EditChamfer { id, params },
        kind @ (FeatureKind::Hole(_)
        | FeatureKind::Pattern(_)
        | FeatureKind::Mirror(_)
        | FeatureKind::Draft(_)
        | FeatureKind::Shell(_)
        | FeatureKind::Revolve(_)) => Command::EditFeature { id, kind },
        FeatureKind::Sketch(_) | FeatureKind::ImportStep(_) | FeatureKind::Lattice(_) => return None,
    })
}

/// What a drag's results depend on. Any change makes them obsolete.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Setup {
    pub revision: u64,
    pub selection: Option<Selection>,
    pub feature: FeatureId,
    pub dimension: Dimension,
}

impl Setup {
    /// The setup of a drag of `dimension` on `feature` in `doc` as it is now.
    pub fn of(doc: &Document, feature: FeatureId, dimension: Dimension) -> Setup {
        Setup { revision: doc.revision(), selection: doc.selection, feature, dimension }
    }
}

/// The whole part rebuilt with the dragged dimension at `value`, for display only.
#[derive(Clone, Debug)]
pub struct Intermediate {
    pub value: f64,
    pub mesh: Arc<DisplayMesh>,
}

/// What a drag shows.
#[derive(Clone, Debug)]
pub enum Shown {
    /// Nothing built yet: the document's body stays on screen.
    Waiting,
    Result(Intermediate),
    /// The newest value did not build. The document's body stays on screen.
    Failed {
        value: f64,
        message: String,
    },
}

/// How a drag ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Finish {
    /// The final value was applied as one undo step and built.
    Applied { value: f64 },
    /// The final value was applied as one undo step and the feature reports `message`.
    /// The preview is gone, so the failure shows on the part as it is.
    Failed { value: f64, message: String },
    /// The edit was refused before it reached the part: nothing changed.
    Refused { value: f64, message: String },
    /// The drag ended where it started: nothing to apply.
    Unchanged,
    /// The part, the selection or the feature changed during the drag: nothing applied.
    Obsolete,
}

/// How many built values a drag keeps for dragging back over them.
const CACHED: usize = 16;

/// One drag of one dimension.
pub struct DragPreview {
    setup: Setup,
    /// The feature's definition when the drag started.
    base: FeatureKind,
    start: f64,
    value: f64,
    /// The newest request. Requests count from 1; 0 is the starting value.
    latest: u64,
    /// The request whose answer is shown.
    shown_request: u64,
    shown: Shown,
    cache: Vec<Intermediate>,
    worker: Worker,
}

impl DragPreview {
    /// Starts a drag of `dimension` on `feature`, with a worker over a copy of the part.
    pub fn start(doc: &Document, feature: FeatureId, dimension: Dimension) -> Result<DragPreview, String> {
        let f = doc.feature(feature).ok_or("no such feature")?;
        let start = dimension.get(&f.kind).ok_or_else(|| format!("{} has no {}", f.name, dimension.label()))?;
        if f.status == FeatureStatus::Inactive {
            return Err(format!("{} is not built; unsuppress it or move the rollback bar below it", f.name));
        }
        let mut cache = Vec::new();
        if f.status == FeatureStatus::Ok
            && let Some(mesh) = doc.display_body().and_then(|b| doc.meshes.get(&b))
        {
            cache.push(Intermediate { value: start, mesh: mesh.clone() });
        }
        let scratch = Scratch {
            model: doc.model().fork(),
            kernel: doc.kernel().clone(),
            params: doc.tess_params,
            feature: model::FeatureId(feature as u32),
            base: f.kind.clone(),
            dimension,
        };
        Ok(DragPreview {
            setup: Setup::of(doc, feature, dimension),
            base: f.kind.clone(),
            start,
            value: start,
            latest: 0,
            shown_request: 0,
            shown: Shown::Waiting,
            cache,
            worker: Worker::new(scratch),
        })
    }

    pub fn setup(&self) -> Setup {
        self.setup
    }

    pub fn feature(&self) -> FeatureId {
        self.setup.feature
    }

    pub fn dimension(&self) -> Dimension {
        self.setup.dimension
    }

    /// The value the drag is at.
    pub fn value(&self) -> f64 {
        self.value
    }

    pub fn start_value(&self) -> f64 {
        self.start
    }

    pub fn shown(&self) -> &Shown {
        &self.shown
    }

    /// The mesh to draw in place of the document's body, if a result is shown.
    pub fn mesh(&self) -> Option<Arc<DisplayMesh>> {
        match &self.shown {
            Shown::Result(r) => Some(r.mesh.clone()),
            Shown::Waiting | Shown::Failed { .. } => None,
        }
    }

    /// True while the newest value has no answer yet.
    pub fn pending(&self) -> bool {
        self.shown_request < self.latest
    }

    /// True while the document still has the setup the drag started from.
    pub fn is_current(&self, doc: &Document) -> bool {
        Setup::of(doc, self.setup.feature, self.setup.dimension) == self.setup
    }

    /// Moves the drag to `value`. A value built before is shown at once; any other goes
    /// to the worker.
    pub fn request(&mut self, value: f64) {
        if !value.is_finite() || value == self.value {
            return;
        }
        self.value = value;
        self.latest += 1;
        if let Some(hit) = self.cache.iter().find(|c| c.value == value) {
            self.shown = Shown::Result(hit.clone());
            self.shown_request = self.latest;
            return;
        }
        self.worker.ask(Job { request: self.latest, value });
    }

    /// Takes the worker's answers. Returns true when what the drag shows changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        for answer in self.worker.answers() {
            match answer.result {
                Ok(mesh) => {
                    let result = Intermediate { value: answer.value, mesh };
                    self.remember(result.clone());
                    if answer.request > self.shown_request {
                        self.shown = Shown::Result(result);
                        self.shown_request = answer.request;
                        changed = true;
                    }
                }
                // An error for a value the drag has passed is dropped.
                Err(message) if answer.request == self.latest && answer.request > self.shown_request => {
                    self.shown = Shown::Failed { value: answer.value, message };
                    self.shown_request = answer.request;
                    changed = true;
                }
                Err(_) => {}
            }
        }
        changed
    }

    fn remember(&mut self, result: Intermediate) {
        if self.cache.iter().any(|c| c.value == result.value) {
            return;
        }
        if self.cache.len() == CACHED {
            self.cache.remove(0);
        }
        self.cache.push(result);
    }

    /// What the viewport and the tree say about the drag, and whether it reports a failure.
    pub fn label(&self) -> (String, bool) {
        let what = self.dimension().label();
        let building = fmt_mm(self.value);
        match &self.shown {
            Shown::Result(r) if self.pending() => {
                (format!("Preview · {what} {} mm · building {building} mm", fmt_mm(r.value)), false)
            }
            _ if self.pending() => (format!("Preview · building {what} {building} mm"), false),
            Shown::Waiting => (format!("Preview · {what} {building} mm"), false),
            Shown::Result(r) => (format!("Preview · {what} {} mm", fmt_mm(r.value)), false),
            Shown::Failed { value, message } => {
                (format!("Preview failed · {what} {} mm: {message}", fmt_mm(*value)), true)
            }
        }
    }

    /// Ends the drag at its value. The worker stops after the value it is on; only the
    /// exact result of the final value reaches the document, as one command.
    pub fn finish(self, doc: &mut Document) -> Finish {
        if !self.is_current(doc) {
            return Finish::Obsolete;
        }
        let value = self.value;
        if value == self.start {
            return Finish::Unchanged;
        }
        let Some(command) = edit_command(self.setup.feature, self.setup.dimension.with(&self.base, value)) else {
            return Finish::Unchanged;
        };
        let feature = self.setup.feature;
        drop(self);
        if let Err(message) = doc.apply(command) {
            return Finish::Refused { value, message };
        }
        match doc.feature(feature).map(|f| f.status.clone()) {
            Some(FeatureStatus::Error(message)) => Finish::Failed { value, message },
            Some(FeatureStatus::NeedsRepair(items)) => Finish::Failed {
                value,
                message: format!(
                    "{} no longer resolve{}",
                    items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>().join(", "),
                    if items.len() == 1 { "s" } else { "" }
                ),
            },
            _ => Finish::Applied { value },
        }
    }
}

/// One value for the worker to build.
struct Job {
    request: u64,
    value: f64,
}

struct Answer {
    request: u64,
    value: f64,
    result: Result<Arc<DisplayMesh>, String>,
}

/// The worker's fork of the part.
struct Scratch {
    model: model::Document,
    kernel: Arc<dyn Kernel>,
    params: TessellationParams,
    feature: model::FeatureId,
    base: FeatureKind,
    dimension: Dimension,
}

impl Scratch {
    fn answer(&mut self, job: Job) -> Answer {
        Answer { request: job.request, value: job.value, result: self.build(job.value) }
    }

    fn build(&mut self, value: f64) -> Result<Arc<DisplayMesh>, String> {
        let model = &mut self.model;
        let kind = self.dimension.with(&self.base, value).to_model();
        model.execute(model::Command::EditFeature { id: self.feature, kind }).map_err(|e| e.to_string())?;
        match model.status(self.feature) {
            Some(model::FeatureStatus::Failed(message)) => return Err(message.clone()),
            Some(model::FeatureStatus::RepairNeeded(_)) => return Err("a reference no longer resolves".into()),
            Some(model::FeatureStatus::Blocked(_)) => return Err("an input feature needs repair".into()),
            _ => {}
        }
        let body = model.body().ok_or("the part has no body")?;
        preview_mesh(self.kernel.as_ref(), body, &self.params)
    }
}

/// Builds a drag's values on a thread of its own. Dropping it stops the thread after the
/// value it is on; nothing waits for it. The browser build has no threads, so there each
/// poll builds the newest value on the interface thread.
struct Worker {
    answers: mpsc::Receiver<Answer>,
    #[cfg(not(target_arch = "wasm32"))]
    jobs: mpsc::Sender<Job>,
    #[cfg(target_arch = "wasm32")]
    answered: mpsc::Sender<Answer>,
    #[cfg(target_arch = "wasm32")]
    scratch: Scratch,
    #[cfg(target_arch = "wasm32")]
    newest: Option<Job>,
}

impl Worker {
    fn new(scratch: Scratch) -> Worker {
        let (answered, answers) = mpsc::channel();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (jobs, queue) = mpsc::channel::<Job>();
            let mut scratch = scratch;
            std::thread::Builder::new()
                .name("drag-preview".into())
                .spawn(move || {
                    while let Ok(mut job) = queue.recv() {
                        // Only the newest value matters; skip the ones the drag has passed.
                        while let Ok(newer) = queue.try_recv() {
                            job = newer;
                        }
                        if answered.send(scratch.answer(job)).is_err() {
                            break;
                        }
                    }
                })
                .expect("starting the drag preview thread");
            Worker { answers, jobs }
        }
        #[cfg(target_arch = "wasm32")]
        Worker { answers, answered, scratch, newest: None }
    }

    fn ask(&mut self, job: Job) {
        #[cfg(not(target_arch = "wasm32"))]
        let _ = self.jobs.send(job);
        #[cfg(target_arch = "wasm32")]
        {
            self.newest = Some(job);
        }
    }

    fn answers(&mut self) -> Vec<Answer> {
        #[cfg(target_arch = "wasm32")]
        if let Some(job) = self.newest.take() {
            let _ = self.answered.send(self.scratch.answer(job));
        }
        self.answers.try_iter().collect()
    }
}
