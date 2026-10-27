//! Drag previews: while the user drags a feature's dimension, intermediate results come
//! from a worker off the interface thread, and the exact result of the final value is
//! applied as one command, so one drag is one undo step.
//!
//! The worker holds a fork of the part, which shares the part's feature builds on the
//! same kernel. For each value it edits the dragged feature in the fork and lets the
//! model rebuild from that feature down, so an intermediate result is the whole part as the edit would leave it. It
//! answers only the newest value it has been asked for, so a fast drag gets results at
//! the worker's pace and none for values the drag has already passed. Results arrive as
//! display meshes without picks: preview state stays in the feature controls and tree, never saved, exported,
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
    CandidateDisplay, Command, DisplayMesh, Document, EndKind, FeatureId, FeatureKind, FeatureStatus, ReadyDisplay,
    Selection, ViewToken, construction_view, fmt_mm, ready_display,
};
use soliddraft_kernel_api::{KernelError, ProofWork, TessellationParams};
use soliddraft_model as model;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
    mpsc,
};

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
    document: soliddraft_document::DocumentId,
    basis: Option<soliddraft_kernel_api::BodyId>,
    pub revision: u64,
    pub selection: Option<Selection>,
    pub feature: FeatureId,
    pub dimension: Dimension,
}

impl Setup {
    /// The setup of a drag of `dimension` on `feature` in `doc` as it is now.
    pub fn of(doc: &Document, feature: FeatureId, dimension: Dimension) -> Setup {
        Setup {
            document: doc.model().id(),
            basis: doc.display_body(),
            revision: doc.revision(),
            selection: doc.selection,
            feature,
            dimension,
        }
    }
}

/// One displayed value. Its geometry is not a saved or exportable body.
#[derive(Clone, Debug)]
pub struct Intermediate {
    pub value: f64,
    pub mesh: Arc<DisplayMesh>,
    pub proved: bool,
    candidate: Option<CandidateDisplay>,
}

#[derive(Clone, Debug)]
pub enum Shown {
    Waiting,
    Result(Intermediate),
    Failed { value: f64, message: String },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Finish {
    Applied { value: f64 },
    Pending { value: f64 },
    Failed { value: f64, message: String },
    Refused { value: f64, message: String },
    Unchanged,
    Obsolete,
}

const CACHED: usize = 16;

#[derive(Clone)]
struct Cached {
    shown: Intermediate,
    proof: Option<ProvedBuild>,
}

/// The existing drag lifecycle owns the document's worker until commit/cancel.
pub struct DragPreview {
    setup: Setup,
    base: FeatureKind,
    source: Arc<model::Document>,
    start: f64,
    value: f64,
    latest: u64,
    shown_request: u64,
    proof_request: u64,
    shown: Shown,
    proof: Option<Result<ProvedBuild, String>>,
    notice: Option<String>,
    cache: Vec<Cached>,
    worker: Worker,
}

impl DragPreview {
    pub fn start(doc: &mut Document, feature: FeatureId, dimension: Dimension) -> Result<Self, String> {
        let item = doc.feature(feature).ok_or("no such feature")?;
        let start = dimension.get(&item.kind).ok_or_else(|| format!("{} has no {}", item.name, dimension.label()))?;
        if item.status == FeatureStatus::Inactive {
            return Err(format!("{} is not built; unsuppress it or move the rollback bar below it", item.name));
        }
        let base = item.kind.clone();
        let setup = Setup::of(doc, feature, dimension);
        let mut cache = Vec::new();
        if !doc.proof_pending()
            && item.status == FeatureStatus::Ok
            && let Some(mesh) = doc.display_body().and_then(|body| doc.meshes.get(&body))
        {
            cache.push(Cached {
                shown: Intermediate { value: start, mesh: mesh.clone(), proved: true, candidate: None },
                proof: None,
            });
        }
        let worker = doc.take_worker()?;
        let source = Arc::new(doc.model().fork());
        Ok(Self {
            setup,
            base,
            source,
            start,
            value: start,
            latest: 0,
            shown_request: 0,
            proof_request: 0,
            shown: Shown::Waiting,
            proof: None,
            notice: None,
            cache,
            worker,
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
    pub fn value(&self) -> f64 {
        self.value
    }
    pub fn start_value(&self) -> f64 {
        self.start
    }
    pub fn shown(&self) -> &Shown {
        &self.shown
    }
    pub fn mesh(&self) -> Option<Arc<DisplayMesh>> {
        match &self.shown {
            Shown::Result(value) => Some(value.mesh.clone()),
            Shown::Waiting | Shown::Failed { .. } => None,
        }
    }
    pub fn pending(&self) -> bool {
        self.proof_request < self.latest
    }
    pub fn current_request(&self) -> Option<u64> {
        (self.latest != 0).then_some(self.latest)
    }
    pub fn displayed_request(&self) -> Option<u64> {
        matches!(self.shown, Shown::Result(_)).then_some(self.shown_request)
    }
    pub fn proved_request(&self) -> Option<u64> {
        let proved = match &self.proof {
            Some(Ok(proof)) => proof.failure.is_none() && !proof.model.is_dirty(),
            _ => matches!(&self.shown, Shown::Result(shown) if shown.proved),
        };
        (proved && self.proof_request == self.latest).then_some(self.latest)
    }
    pub fn is_current(&self, doc: &Document) -> bool {
        Setup::of(doc, self.setup.feature, self.setup.dimension) == self.setup
    }
    fn token(&self) -> ViewToken {
        ViewToken { revision: self.setup.revision, request: self.latest, transient: true }
    }

    pub fn request(&mut self, value: f64, doc: &mut Document) {
        if !value.is_finite() || value == self.value {
            return;
        }
        doc.discard_provisional(self.token());
        self.value = value;
        self.latest = self.worker.invalidate();
        self.proof = None;
        self.notice = None;
        if let Some(hit) = self.cache.iter().find(|entry| entry.shown.value == value) {
            self.shown = Shown::Result(hit.shown.clone());
            self.shown_request = self.latest;
            self.proof_request = self.latest;
            self.proof = hit.proof.clone().map(Ok);
            if let Some(candidate) = &hit.shown.candidate {
                doc.show_provisional(self.token(), candidate.clone());
            }
            return;
        }
        self.shown = Shown::Waiting;
        let edit = ScalarDraft { feature: model::FeatureId(self.feature() as u32), dimension: self.dimension(), value };
        let result = self.worker.enqueue(self.latest, doc.build_job(self.source.clone(), Some(edit)));
        if let Err(message) = result {
            self.shown = Shown::Failed { value, message: message.clone() };
            self.proof = Some(Err(message));
            self.proof_request = self.latest;
        }
    }

    pub fn poll(&mut self, doc: &mut Document) -> bool {
        let mut changed = false;
        for answer in self.worker.answers() {
            if answer.request != self.latest {
                doc.hold_proof_answer(answer);
                continue;
            }
            match answer.kind {
                AnswerKind::Construction(Ok(candidate)) => {
                    self.shown = Shown::Result(Intermediate {
                        value: self.value,
                        mesh: candidate.mesh(),
                        proved: false,
                        candidate: Some(candidate.clone()),
                    });
                    self.shown_request = answer.request;
                    doc.show_provisional(self.token(), candidate);
                    changed = true;
                }
                AnswerKind::Construction(Err(message)) => self.notice = Some(message),
                AnswerKind::Proved(result) => {
                    self.proof_request = answer.request;
                    match &result {
                        Ok(proof) if proof.failure.is_none() => {
                            if let Some(display) = &proof.display {
                                let candidate = CandidateDisplay::Proved(display.clone());
                                let shown = Intermediate {
                                    value: self.value,
                                    mesh: candidate.mesh(),
                                    proved: true,
                                    candidate: Some(candidate.clone()),
                                };
                                self.shown = Shown::Result(shown.clone());
                                self.shown_request = self.latest;
                                if self.cache.len() == CACHED {
                                    self.cache.remove(0);
                                }
                                self.cache.push(Cached { shown, proof: Some(proof.clone()) });
                                doc.show_provisional(self.token(), candidate);
                            }
                            self.notice = proof.display_error.clone();
                        }
                        Ok(proof) => {
                            self.shown = Shown::Failed {
                                value: self.value,
                                message: proof.failure.clone().expect("matched failed proof"),
                            };
                            doc.discard_provisional(self.token());
                        }
                        Err(message) => {
                            self.shown = Shown::Failed { value: self.value, message: message.clone() };
                            doc.discard_provisional(self.token());
                        }
                    }
                    self.proof = Some(result);
                    changed = true;
                }
            }
        }
        changed
    }

    pub fn label(&self) -> (String, bool) {
        if let Shown::Failed { value, message } = &self.shown {
            return (format!("Edit failed · {} {} mm: {message}", self.dimension().label(), fmt_mm(*value)), true);
        }
        let phase = if self.pending() { "Proof pending" } else { "Preview" };
        let mut label = format!("{phase} · {} {} mm", self.dimension().label(), fmt_mm(self.value));
        if let Some(notice) = &self.notice {
            label.push_str(&format!(" · {notice}"));
        }
        (label, false)
    }

    pub fn cancel(self, doc: &mut Document) {
        if self.setup.document != doc.model().id()
            || self.setup.revision != doc.revision()
            || self.setup.basis != doc.display_body()
        {
            return;
        }
        doc.discard_provisional(self.token());
        doc.return_worker(self.worker);
    }

    pub fn finish(self, doc: &mut Document) -> Finish {
        if !self.is_current(doc) {
            self.cancel(doc);
            return Finish::Obsolete;
        }
        let value = self.value;
        if value == self.start {
            self.cancel(doc);
            return Finish::Unchanged;
        }
        let Some(command) = edit_command(self.feature(), self.dimension().with(&self.base, value)) else {
            self.cancel(doc);
            return Finish::Unchanged;
        };
        let feature = self.feature();
        let token = self.token();
        let candidate = if self.proof.is_none() {
            match &self.shown {
                Shown::Result(value) => value.candidate.clone(),
                _ => None,
            }
        } else {
            None
        };
        let prepared = PreparedBuild { worker: self.worker, request: self.latest, completed: self.proof, candidate };
        doc.discard_provisional(token);
        drop(self.cache);
        drop(self.shown);
        match doc.apply_prepared(command, prepared) {
            Err(message) => Finish::Refused { value, message },
            Ok(false) => Finish::Pending { value },
            Ok(true) => match doc.feature(feature).map(|feature| &feature.status) {
                Some(FeatureStatus::Error(message)) => Finish::Failed { value, message: message.clone() },
                _ => Finish::Applied { value },
            },
        }
    }
}

#[derive(Clone)]
pub(crate) struct ProvedBuild {
    pub model: Arc<model::Document>,
    pub display: Option<Arc<ReadyDisplay>>,
    pub failure: Option<String>,
    pub display_error: Option<String>,
}

pub(crate) struct PreparedBuild {
    pub worker: Worker,
    pub request: u64,
    pub completed: Option<Result<ProvedBuild, String>>,
    pub candidate: Option<CandidateDisplay>,
}

#[derive(Clone, Copy)]
pub(crate) struct ScalarDraft {
    pub feature: model::FeatureId,
    pub dimension: Dimension,
    pub value: f64,
}

pub(crate) struct Job {
    pub source: Arc<model::Document>,
    pub edit: Option<ScalarDraft>,
    pub baseline: Option<Arc<model::Document>>,
    pub params: TessellationParams,
    pub level: soliddraft_tessellation::Level,
}

pub(crate) enum AnswerKind {
    Construction(Result<CandidateDisplay, String>),
    Proved(Result<ProvedBuild, String>),
}

pub(crate) struct Answer {
    pub request: u64,
    pub kind: AnswerKind,
}

struct Queued {
    request: u64,
    job: Job,
}

struct ProofTask {
    request: u64,
    model: model::Document,
    level: soliddraft_tessellation::Level,
    allowance: Box<dyn ProofWork>,
}

fn prepare(queued: Queued, answered: &mpsc::Sender<Answer>) -> Result<Option<ProofTask>, ()> {
    let Queued { request, job: Job { source, edit, baseline, params, level } } = queued;
    let mut model = Arc::try_unwrap(source).unwrap_or_else(|source| source.fork());
    if let Some(edit) = edit {
        let changed = crate::document::scalar_draft_command(&model, edit)
            .and_then(|command| model.execute_deferred(command).map(|_| ()).map_err(|error| error.to_string()));
        if let Err(message) = changed {
            answered.send(Answer { request, kind: AnswerKind::Proved(Err(message)) }).map_err(|_| ())?;
            return Ok(None);
        }
    }
    let kernel = model.kernel();
    let body = baseline.as_ref().and_then(|baseline| baseline.body());
    let edits = baseline.as_ref().and_then(|baseline| crate::document::scalar_edits(baseline, &model));
    let allowance = match (edits.as_deref(), body) {
        (Some(edits), Some(body)) if !edits.is_empty() => match kernel.construction_edit(body, edits, &params) {
            Ok(preview) => {
                let candidate = preview
                    .display
                    .map_err(|error| error.to_string())
                    .and_then(|display| construction_view(display, &model))
                    .map(|view| CandidateDisplay::Construction(Arc::new(view)));
                answered.send(Answer { request, kind: AnswerKind::Construction(candidate) }).map_err(|_| ())?;
                Ok(preview.proof)
            }
            Err(KernelError::Unsupported(message)) => {
                answered.send(Answer { request, kind: AnswerKind::Construction(Err(message)) }).map_err(|_| ())?;
                kernel.reserve_proof_work()
            }
            Err(error) => Err(error),
        },
        _ => kernel.reserve_proof_work(),
    };
    match allowance {
        Ok(allowance) => Ok(Some(ProofTask { request, model, level, allowance })),
        Err(error) => {
            answered.send(Answer { request, kind: AnswerKind::Proved(Err(error.to_string())) }).map_err(|_| ())?;
            Ok(None)
        }
    }
}

/// A feature that shapes the body did not build. A sketch that did not build blocks
/// each active feature that uses it, so it is not a failure of the body itself.
fn model_failure(model: &model::Document) -> Option<String> {
    let mut shaping = model.features().iter().filter(|feature| !matches!(feature.kind, model::FeatureKind::Sketch(_)));
    shaping.find_map(|feature| match model.status(feature.id) {
        Some(model::FeatureStatus::Failed(message)) => Some(format!("{}: {message}", feature.name)),
        Some(status @ (model::FeatureStatus::RepairNeeded(_) | model::FeatureStatus::Blocked(_))) => {
            Some(format!("{}: {status:?}", feature.name))
        }
        _ => None,
    })
}

fn prove(
    task: ProofTask,
    newest: &AtomicU64,
    protected: &AtomicU64,
    previous: Option<&model::Document>,
) -> ProvedBuild {
    let ProofTask { request, mut model, level, allowance } = task;
    if let Some(previous) = previous {
        model.reuse_builds_from(previous);
    }
    let mut display = None;
    let mut display_error = None;
    let mut failure = None;
    let result = allowance.run(&mut || {
        model.rebuild();
        failure = model_failure(&model);
        if failure.is_none() && wanted(request, newest, protected) {
            match ready_display(&model, level) {
                Ok(view) => display = view.map(Arc::new),
                Err(error) => display_error = Some(error),
            }
        }
        Ok(())
    });
    if let Err(error) = result
        && failure.is_none()
    {
        display_error = Some(error.to_string());
    }
    ProvedBuild { model: Arc::new(model), display, failure, display_error }
}

fn wanted(request: u64, newest: &AtomicU64, protected: &AtomicU64) -> bool {
    newest.load(Ordering::Acquire) == request || protected.load(Ordering::Acquire) == request
}

fn next_work<T>(pending: &mut Vec<T>, newest: u64, protected: u64, key: impl Fn(&T) -> u64) -> Option<T> {
    pending.retain(|work| {
        let request = key(work);
        request == newest || request == protected
    });
    let index = pending.iter().position(|work| key(work) == newest).or_else(|| (!pending.is_empty()).then_some(0))?;
    Some(pending.swap_remove(index))
}

/// One scheduler, with independent construction and exact jobs. A long proof
/// cannot hold the next edit's construction preview behind it.
pub(crate) struct Worker {
    answers: mpsc::Receiver<Answer>,
    newest: Arc<AtomicU64>,
    protected: Arc<AtomicU64>,
    request: u64,
    #[cfg(not(target_arch = "wasm32"))]
    jobs: mpsc::Sender<Queued>,
    #[cfg(target_arch = "wasm32")]
    answered: mpsc::Sender<Answer>,
    #[cfg(target_arch = "wasm32")]
    queued: Vec<Queued>,
    #[cfg(target_arch = "wasm32")]
    proof: Vec<ProofTask>,
    #[cfg(target_arch = "wasm32")]
    previous: Option<Arc<model::Document>>,
}

impl Worker {
    pub(crate) fn new() -> Result<Self, String> {
        let (answered, answers) = mpsc::channel();
        let newest = Arc::new(AtomicU64::new(0));
        let protected = Arc::new(AtomicU64::new(0));
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (jobs, queue) = mpsc::channel::<Queued>();
            let (proofs, proof_queue) = mpsc::channel::<ProofTask>();
            let current = newest.clone();
            let retained = protected.clone();
            let finished = answered.clone();
            std::thread::Builder::new()
                .name("edit-proof".into())
                .spawn(move || {
                    let mut previous: Option<Arc<model::Document>> = None;
                    let mut pending = Vec::new();
                    loop {
                        if pending.is_empty() {
                            match proof_queue.recv() {
                                Ok(task) => pending.push(task),
                                Err(_) => break,
                            }
                        }
                        pending.extend(proof_queue.try_iter());
                        let Some(task) = next_work(
                            &mut pending,
                            current.load(Ordering::Acquire),
                            retained.load(Ordering::Acquire),
                            |task| task.request,
                        ) else {
                            continue;
                        };
                        let request = task.request;
                        if !wanted(request, &current, &retained) {
                            continue;
                        }
                        let result = prove(task, &current, &retained, previous.as_deref());
                        if result.failure.is_none() && !result.model.is_dirty() {
                            previous = Some(result.model.clone());
                        }
                        if wanted(request, &current, &retained)
                            && finished.send(Answer { request, kind: AnswerKind::Proved(Ok(result)) }).is_err()
                        {
                            break;
                        }
                    }
                })
                .map_err(|error| format!("cannot start edit proof: {error}"))?;
            let current = newest.clone();
            let retained = protected.clone();
            std::thread::Builder::new()
                .name("edit-construction".into())
                .spawn(move || {
                    let mut pending = Vec::new();
                    loop {
                        if pending.is_empty() {
                            match queue.recv() {
                                Ok(job) => pending.push(job),
                                Err(_) => break,
                            }
                        }
                        pending.extend(queue.try_iter());
                        let Some(queued) = next_work(
                            &mut pending,
                            current.load(Ordering::Acquire),
                            retained.load(Ordering::Acquire),
                            |job| job.request,
                        ) else {
                            continue;
                        };
                        if !wanted(queued.request, &current, &retained) {
                            continue;
                        }
                        match prepare(queued, &answered) {
                            Ok(Some(task)) => {
                                if proofs.send(task).is_err() {
                                    break;
                                }
                            }
                            Ok(None) => {}
                            Err(()) => break,
                        }
                    }
                })
                .map_err(|error| format!("cannot start edit construction: {error}"))?;
            Ok(Self { answers, newest, protected, request: 0, jobs })
        }
        #[cfg(target_arch = "wasm32")]
        Ok(Self {
            answers,
            newest,
            protected,
            request: 0,
            answered,
            queued: Vec::new(),
            proof: Vec::new(),
            previous: None,
        })
    }

    pub(crate) fn protect(&mut self, request: Option<u64>) {
        self.protected.store(request.unwrap_or(0), Ordering::Release);
    }

    pub(crate) fn activate(&mut self, request: u64) {
        self.newest.store(request, Ordering::Release);
    }

    pub(crate) fn invalidate(&mut self) -> u64 {
        self.request =
            self.request.checked_add(1).filter(|request| *request != u64::MAX).expect("edit request space exhausted");
        self.newest.store(self.request, Ordering::Release);
        self.request
    }

    pub(crate) fn enqueue(&mut self, request: u64, job: Job) -> Result<(), String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.jobs.send(Queued { request, job }).map_err(|_| "the edit proof worker stopped".into())
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.queued.push(Queued { request, job });
            Ok(())
        }
    }

    pub(crate) fn answers(&mut self) -> Vec<Answer> {
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(queued) = next_work(
                &mut self.queued,
                self.newest.load(Ordering::Acquire),
                self.protected.load(Ordering::Acquire),
                |job| job.request,
            ) {
                if let Ok(Some(task)) = prepare(queued, &self.answered) {
                    self.proof.push(task);
                }
            } else if let Some(task) = next_work(
                &mut self.proof,
                self.newest.load(Ordering::Acquire),
                self.protected.load(Ordering::Acquire),
                |task| task.request,
            ) {
                let request = task.request;
                let result = prove(task, &self.newest, &self.protected, self.previous.as_deref());
                if result.failure.is_none() && !result.model.is_dirty() {
                    self.previous = Some(result.model.clone());
                }
                if wanted(request, &self.newest, &self.protected) {
                    let _ = self.answered.send(Answer { request, kind: AnswerKind::Proved(Ok(result)) });
                }
            }
        }
        self.answers.try_iter().collect()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.protected.store(u64::MAX, Ordering::Release);
        self.newest.store(u64::MAX, Ordering::Release);
    }
}
