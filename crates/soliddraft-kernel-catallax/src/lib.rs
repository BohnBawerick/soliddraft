//! Catallax's application entry behind SolidDraft's existing kernel interface.
//!
//! Profiles are deferred recipe fragments, not sheet geometry. Each solid owns an
//! immutable Catallax application build. Legacy history is derived from Catallax's
//! origin sets; display estimates never decide topology or operation success.

mod assembly_measure;
mod construction;
mod convert;
mod diagnostics;
mod drawing_queries;
mod edge_control;
mod frames;
mod identity;
mod import;
mod lattice;
mod measure;
mod native;
mod shell;
mod step;

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use catallax_app::{Error, EvaluatedApplicationBody};
use catallax_mesh::{MeshOptions, PreviewMesh, PreviewOptions, preview_mesh_body};
use catallax_numbers::{Decider, Rational};
use catallax_recipe::StepId;
use serde_json::{Value, json};
use soliddraft_kernel_api::*;

pub use diagnostics::{RequestWork, with_request_allowance};

/// The pinned Catallax application entry revision.
pub const CATALLAX_REVISION: &str = "e139919ae1d6b3718e49ff4519f956cc429c36ac";

/// An in-process Catallax backend. There is no global kernel state or kernel thread.
#[derive(Default)]
pub struct CatallaxKernel {
    state: Mutex<State>,
    diagnostics: Option<diagnostics::Diagnostics>,
}

#[derive(Default)]
struct State {
    next: u64,
    bodies: BTreeMap<BodyId, Stored>,
    fields: BTreeMap<BodyId, Arc<lattice::Field>>,
    capture: bool,
    questions: Vec<Value>,
}

struct Stored {
    steps: BTreeMap<u64, Value>,
    result: String,
    history: Vec<HistoryRecord>,
    solid: Option<Arc<Solid>>,
}

struct Solid {
    app: EvaluatedApplicationBody,
    ids: convert::Ids,
    display: MeshCache<catallax_mesh::Mesh>,
    /// `None` where Catallax refused to sample a preview, so it is not asked again.
    previews: MeshCache<Option<PreviewMesh>>,
    topology: OnceLock<KernelResult<Topology>>,
    measured: OnceLock<KernelResult<Topology>>,
    mass: OnceLock<KernelResult<MassProperties>>,
}

/// Every display control of a mesh request.
type DisplayKey = (Rational, Option<Rational>, Option<Rational>, Option<Rational>);

fn display_key(options: &MeshOptions) -> DisplayKey {
    (
        options.chord_tolerance.clone(),
        options.angular_deviation.clone(),
        options.max_edge_length.clone(),
        options.max_aspect_ratio.clone(),
    )
}

/// One body's meshes of one kind by their controls, the most recently asked last.
struct MeshCache<T>(Mutex<VecDeque<Arc<CachedMesh<T>>>>);

struct CachedMesh<T> {
    key: DisplayKey,
    value: Mutex<Option<Arc<T>>>,
}

impl<T> MeshCache<T> {
    fn new() -> Self {
        Self(Mutex::new(VecDeque::new()))
    }

    fn get(&self, key: DisplayKey, make: impl FnOnce() -> KernelResult<T>) -> KernelResult<Arc<T>> {
        let entry = {
            let mut cached =
                self.0.lock().map_err(|_| KernelError::Unavailable("Catallax display cache lock poisoned".into()))?;
            let entry = if let Some(i) = cached.iter().position(|entry| entry.key == key) {
                cached.remove(i).expect("the cached controls exist")
            } else {
                // Three viewport levels, the default mesh and a drawing tolerance coexist.
                const MESHES: usize = 5;
                if cached.len() == MESHES {
                    cached.pop_front();
                }
                Arc::new(CachedMesh { key, value: Mutex::new(None) })
            };
            cached.push_back(entry.clone());
            entry
        };
        // Only requests for these controls wait for their build. Background refinement
        // must not lock out a viewport or file query for an already available mesh.
        let mut value =
            entry.value.lock().map_err(|_| KernelError::Unavailable("Catallax mesh cache lock poisoned".into()))?;
        if let Some(mesh) = value.as_ref() {
            return Ok(mesh.clone());
        }
        let mesh = Arc::new(make()?);
        *value = Some(mesh.clone());
        Ok(mesh)
    }

    /// Visits every finished mesh, skipping any still being made.
    fn each(&self, mut visit: impl FnMut(&T)) {
        if let Ok(cached) = self.0.lock() {
            for entry in cached.iter() {
                if let Ok(value) = entry.value.try_lock()
                    && let Some(mesh) = value.as_ref()
                {
                    visit(mesh);
                }
            }
        }
    }
}

impl Solid {
    /// The certified mesh for these controls, for exports and proven results.
    fn display(&self, options: &MeshOptions, query: &str) -> KernelResult<Arc<catallax_mesh::Mesh>> {
        let key = display_key(options);
        let default = key == display_key(&MeshOptions::default());
        self.display.get(key, || {
            if default {
                return self.app.display(options, &mut Decider::default()).map(|answer| answer.value).map_err(
                    |error| match error {
                        Error::Display(refusal) => query_error(query, refusal),
                        other => KernelError::QueryFailed {
                            query: query.into(),
                            code: 0,
                            message: format!("{other} (Catallax stage 1)"),
                        },
                    },
                );
            }
            // The default mesh spans the body; pad it by both chord bounds so the
            // rounding allowance for a requested edge length covers every vertex.
            let pad =
                MeshOptions::default().chord_tolerance.to_f64_nearest() + options.chord_tolerance.to_f64_nearest();
            let default_mesh = self.display(&MeshOptions::default(), query)?;
            let extent = default_mesh.positions.iter().flatten().fold(0.0_f64, |m, v| m.max(v.abs())) + pad;
            let native = edge_control::native_options(options, extent)?;
            let mesh = numerical(query, |dec| {
                self.app.display(&native, dec).map_err(|error| match error {
                    Error::Display(refusal) => refusal,
                    other => catallax_numbers::Refusal::new(other.code(), other.to_string()),
                })
            })
            .map_err(|e| query_error(query, e))?
            .value;
            edge_control::check(options, &mesh)?;
            Ok(mesh)
        })
    }

    /// An unproved mesh for the screen, sampled to the same targets with no claimed
    /// bounds. Catallax keeps it apart from the certified mesh and never exports it.
    /// `None` when Catallax refuses to sample one; the operation log keeps the refusal.
    fn preview(&self, options: &MeshOptions, query: &str) -> KernelResult<Arc<Option<PreviewMesh>>> {
        self.previews.get(display_key(options), || {
            let built = self.app.checkpoint.built();
            let mut decider = Decider::default();
            let span = catallax_numbers::OperationSpan::begin("brep", &query, query, &decider);
            let result = preview_mesh_body(&built.model, built.result, &PreviewOptions::from(options), &mut decider);
            span.finish(&decider, result.as_ref().err(), None);
            Ok(result.ok())
        })
    }
}

impl State {
    fn id(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    fn get(&self, body: BodyId) -> KernelResult<&Stored> {
        self.bodies.get(&body).ok_or(KernelError::UnknownBody(body))
    }

    fn solid(&self, body: BodyId) -> KernelResult<Arc<Solid>> {
        if self.fields.contains_key(&body) {
            return Err(terminal_field(body));
        }
        self.get(body)?.solid.clone().ok_or_else(|| gap("profile sheet queries", "1"))
    }

    fn step(&self, body: BodyId) -> KernelResult<StepId> {
        Ok(StepId::new(self.get(body)?.result.clone()).expect("generated step ids are valid"))
    }
}

impl CatallaxKernel {
    /// Starts an empty backend with its own body arena.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enables synchronous per-call evidence and deterministic work accounting.
    pub fn with_diagnostics(max_work: u64) -> Self {
        Self::capture(Some(max_work))
    }

    /// Captures evidence without imposing a work allowance on normal application commands.
    pub fn with_operation_log() -> Self {
        Self::capture(None)
    }

    fn capture(max_work: Option<u64>) -> Self {
        Self {
            state: Mutex::new(State { capture: true, ..State::default() }),
            diagnostics: Some(diagnostics::Diagnostics::new(max_work)),
        }
    }

    /// Takes completed kernel calls and their portable recipe questions.
    pub fn take_diagnostics(&self) -> KernelResult<Value> {
        let mut state = self.state()?;
        let questions = std::mem::take(&mut state.questions);
        Ok(json!({"calls": self.diagnostics.as_ref().map(|d| d.take()).unwrap_or_default(),
            "questions":questions}))
    }

    /// The retained definition: a native Catallax recipe for fields, a legacy recipe for B-reps.
    pub fn body_recipe(&self, body: BodyId) -> KernelResult<Value> {
        if let Some(field) = self.field(body)? {
            return Ok(field.recipe());
        }
        let state = self.state()?;
        let stored = state.get(body)?;
        let format = if stored.steps.values().any(|s| s["op"] == "import_step") {
            "catallax-recipe/3"
        } else {
            "soliddraft-recipe/1"
        };
        Ok(json!({"format":format, "name":"SolidDraft",
            "steps":stored.steps.values().collect::<Vec<_>>(), "result":stored.result}))
    }

    fn observe<T>(&self, operation: &str, run: impl FnOnce() -> KernelResult<T>) -> KernelResult<T> {
        match &self.diagnostics {
            Some(d) => d.observe(operation, run),
            None => run(),
        }
    }

    /// Builds a body from `inputs` and one more step. The Catallax build runs without the
    /// body arena lock, so other threads keep reading and building bodies meanwhile.
    fn operation(
        &self,
        state: MutexGuard<'_, State>,
        tag: &OpTag,
        op: OpKind,
        inputs: &[BodyId],
        step: Value,
    ) -> KernelResult<OpResult> {
        self.operation_reading(state, tag, op, inputs, &[], step)
    }

    /// Like [`CatallaxKernel::operation`], also reading the steps of `context` bodies
    /// whose faces the operation only looks at, so their history stays out of the result's.
    fn operation_reading(
        &self,
        mut state: MutexGuard<'_, State>,
        tag: &OpTag,
        op: OpKind,
        inputs: &[BodyId],
        context: &[BodyId],
        mut step: Value,
    ) -> KernelResult<OpResult> {
        if let Some(field) = inputs.iter().chain(context).find(|body| state.fields.contains_key(body)) {
            return Err(terminal_field(*field));
        }
        let mut steps = BTreeMap::new();
        let mut history = BTreeMap::new();
        let mut parents = Vec::new();
        for body in context {
            steps.extend(state.get(*body)?.steps.clone());
        }
        for input in inputs {
            let stored = state.get(*input)?;
            steps.extend(stored.steps.clone());
            history.extend(stored.history.iter().map(|r| (r.seq, r.clone())));
            if let Some(solid) = &stored.solid {
                parents.push(solid.clone());
            }
        }
        let seq = state.id();
        let result = format!("op_{seq}");
        step["id"] = result.clone().into();
        steps.insert(seq, step);
        // Read-only targets can supply geometry checkpoints without becoming history
        // parents. Independent tools still require an exact matching leading prefix.
        let mut prefix: Option<&Stored> = None;
        for body in inputs.iter().chain(context) {
            let stored = state.get(*body)?;
            if stored.solid.is_some()
                && prefix.is_none_or(|best| stored.steps.len() > best.steps.len())
                && stored.steps.iter().eq(steps.iter().take(stored.steps.len()))
            {
                prefix = Some(stored);
            }
        }
        let prefix = prefix.and_then(|stored| stored.solid.clone());
        let input = json!({"format":"soliddraft-recipe/1", "name":"SolidDraft", "steps": steps.values().collect::<Vec<_>>(), "result": result});
        let capture = state.capture;
        drop(state);
        let mut questions = Vec::new();
        let built = build(&input, op, prefix.as_ref().map(|solid| &solid.app), capture.then_some(&mut questions));
        let mut state = self.state()?;
        // A refused build keeps its question too.
        state.questions.extend(questions);
        let (app, names) = built?;
        // Catallax names faces by recipe step; the interface uses the caller's tag.
        let tags: BTreeMap<String, OpTag> = names
            .into_iter()
            .filter_map(|(name, id)| {
                let n: u64 = name.strip_prefix("op_")?.parse().ok()?;
                let tag = if n == seq { tag.clone() } else { history.get(&n)?.tag.clone() };
                Some((id.as_str().to_owned(), tag))
            })
            .collect();
        let exact = app.checkpoint.built();
        if exact.model.body(exact.result).kind == catallax_topology::BodyKind::Empty {
            return Err(gap("empty results have no BodyKind or centroid in the existing application interface", "1"));
        }
        let ids = convert::Ids::new(&app, &parents.iter().map(|p| &p.ids).collect::<Vec<_>>(), &tags, &mut state.next);
        let record = ids.record(seq, tag, op, &result, &parents.iter().map(|p| &p.ids).collect::<Vec<_>>());
        history.insert(seq, record.clone());
        let body = BodyId::from_raw(state.id());
        let display = MeshCache::new();
        state.bodies.insert(
            body,
            Stored {
                steps,
                result,
                history: history.into_values().collect(),
                solid: Some(Arc::new(Solid {
                    app,
                    ids,
                    display,
                    previews: MeshCache::new(),
                    topology: OnceLock::new(),
                    measured: OnceLock::new(),
                    mass: OnceLock::new(),
                })),
            },
        );
        Ok(OpResult { body, record })
    }

    /// Stores a Catallax-native step that reads the solid `base` and builds the chain.
    #[allow(clippy::too_many_arguments)]
    fn native(
        &self,
        state: MutexGuard<'_, State>,
        tag: &OpTag,
        op: OpKind,
        base: BodyId,
        inputs: &[BodyId],
        native: catallax_recipe::Operation,
        sweep: Option<Value>,
    ) -> KernelResult<OpResult> {
        let base = state.get(base)?.result.clone();
        let mut step = json!({"op":"native","base":base,"native":native});
        if let Some(sweep) = sweep {
            step["sweep"] = sweep;
        }
        self.operation(state, tag, op, inputs, step)
    }

    fn state(&self) -> KernelResult<MutexGuard<'_, State>> {
        self.state.lock().map_err(|_| KernelError::Unavailable("Catallax body arena lock poisoned".into()))
    }

    fn solid(&self, body: BodyId) -> KernelResult<Arc<Solid>> {
        self.state()?.solid(body)
    }
}

/// A terminal field is last: no CAD step or B-rep query reads it.
fn terminal_field(body: BodyId) -> KernelError {
    KernelError::InvalidInput(format!(
        "lattice.cad-after-lattice: {body} is a terminal field; return before it to edit (Catallax stage 1)"
    ))
}

fn gap(what: &str, stage: &str) -> KernelError {
    KernelError::Unsupported(format!(
        "{what} is not available in the Catallax application entry (Catallax stage {stage})"
    ))
}

/// Converts retained application definitions, preserving native steps and frames.
fn converted_recipe(input: &Value, op: OpKind) -> KernelResult<catallax_app::soliddraft::Converted> {
    // The converter reads `soliddraft-recipe/1` only. A native step stands in as an
    // identity union of the body it reads, then takes its own operation back.
    let mut legacy = input.clone();
    let mut natives = Vec::new();
    for step in legacy["steps"].as_array_mut().expect("generated recipe steps") {
        if step["op"] != "native" {
            continue;
        }
        let tag = step["id"].as_str().expect("generated step id").to_owned();
        let native: catallax_recipe::Operation = serde_json::from_value(step["native"].clone())
            .map_err(|e| KernelError::InvalidInput(format!("stored Catallax step: {e}")))?;
        natives.push((tag.clone(), native, step["sweep"].clone()));
        let base = step["base"].clone();
        *step = json!({"id":tag, "op":"boolean", "kind":"union", "target":base, "tool":base});
    }
    let mut converted =
        catallax_app::soliddraft::convert(&legacy.to_string()).map_err(|e| build_error(op, Error::Conversion(e)))?;
    // Start in the end-condition format so adding a limited sweep does not discard
    // an otherwise identical target checkpoint merely to upgrade its recipe version.
    catallax_recipe::FORMAT_V4.clone_into(&mut converted.recipe.format);
    let sources = input["steps"].as_array().expect("generated recipe steps");
    let mut planes = BTreeMap::new();
    for source in sources {
        if source["op"] != "profile" {
            continue;
        }
        let plane: Plane = serde_json::from_value(source["plane"].clone())
            .map_err(|e| KernelError::InvalidInput(format!("stored profile frame: {e}")))?;
        let name = source["id"].as_str().expect("generated step id");
        let id = &converted.steps[name];
        let step = converted.recipe.steps.iter_mut().find(|s| &s.id == id).expect("converted profile step");
        if let catallax_recipe::Operation::Sketch { plane: pose, .. } = &mut step.op {
            if let Some(rotation) = frames::rotation(plane) {
                pose.rotation = rotation;
            }
            planes.insert(name.to_owned(), (plane, pose.rotation.clone()));
        }
    }
    // A sweep along the sketch normal uses the normal of the same certified frame.
    let sweep = |profile: &str, direction: &Value| -> KernelResult<Option<catallax_recipe::Vector3>> {
        let direction: Vec3 = serde_json::from_value(direction.clone())
            .map_err(|e| KernelError::InvalidInput(format!("stored sweep direction: {e}")))?;
        let (plane, rotation) = &planes[profile];
        Ok(frames::normal_sweep(*plane, rotation, direction))
    };
    for source in sources {
        if source["op"] != "extrude" || source["direction"].is_null() {
            continue;
        }
        let profile = source["profile"].as_str().expect("generated profile id");
        if let Some(direction) = sweep(profile, &source["direction"])? {
            let id = &converted.steps[source["id"].as_str().expect("generated step id")];
            let step = converted.recipe.steps.iter_mut().find(|s| &s.id == id).expect("converted extrusion step");
            if let catallax_recipe::Operation::Extrude { direction: d, .. } = &mut step.op {
                *d = Some(direction);
            }
        }
    }
    for (tag, mut native, stored_sweep) in natives {
        let id = converted.steps[&tag].clone();
        // Generated tags are already valid step ids, so native inputs name converted steps.
        debug_assert_eq!(id.as_str(), tag);
        if let Some(profile) = stored_sweep["profile"].as_str()
            && !stored_sweep["direction"].is_null()
            && let Some(direction) = sweep(profile, &stored_sweep["direction"])?
            && let catallax_recipe::Operation::ExtrudeTo { direction: d, .. }
            | catallax_recipe::Operation::ExtrudeNext { direction: d, .. } = &mut native
        {
            *d = Some(direction);
        }
        let step = converted.recipe.steps.iter_mut().find(|s| s.id == id).expect("converted native step");
        step.op = native;
    }
    Ok(converted)
}

/// Builds a legacy recipe, returning the body and the legacy step names' Catallax step ids.
fn build(
    input: &Value,
    op: OpKind,
    prefix: Option<&EvaluatedApplicationBody>,
    questions: Option<&mut Vec<Value>>,
) -> KernelResult<(EvaluatedApplicationBody, BTreeMap<String, catallax_recipe::StepId>)> {
    let converted = converted_recipe(input, op)?;
    if let Some(questions) = questions {
        let problem = serde_json::to_value(&converted.recipe).expect("Catallax recipes serialize");
        questions.push(json!({"format":"catallax-question/1", "source":"recipe", "problem":problem}));
    }
    // Earlier bodies remain immutable for undo, branches and concurrent queries.
    // Clone only the selected checkpoint, after releasing the body arena lock.
    let prefix = prefix.map(|app| app.checkpoint.clone());
    let app = catallax_app::build_incremental_without_display(&converted.recipe, prefix, &mut Decider::default())
        .map_err(|e| build_error(op, e))?;
    Ok((app, converted.steps))
}

/// The Catallax stage that brought an operation.
fn stage(op: OpKind) -> &'static str {
    match op {
        OpKind::Fillet => "3 for fillets between planes and cylinders; other fillets wait for Catallax stage 4",
        OpKind::Chamfer => "3 for chamfers between planes and cylinders; other chamfers wait for chamfer continuation",
        OpKind::ExtrudeTo | OpKind::Shell | OpKind::Hole | OpKind::Pattern | OpKind::Mirror => "3",
        OpKind::Draft => "4",
        _ => "1",
    }
}

fn build_error(op: OpKind, error: Error) -> KernelError {
    let code = error.code();
    if matches!(error, Error::Conversion(catallax_app::soliddraft::ConversionError::NotYet { .. })) {
        return gap(&format!("{code}: {error}"), stage(op));
    }
    KernelError::Failed { op, code: 0, message: format!("{code}: {error} (Catallax stage {})", stage(op)) }
}

fn query_error(query: &str, error: catallax_numbers::Refusal) -> KernelError {
    KernelError::QueryFailed {
        query: query.into(),
        code: 0,
        message: format!("{}: {} (Catallax stage 1)", error.code, error.detail),
    }
}

fn numerical<T>(
    operation: &str,
    run: impl FnOnce(&mut Decider) -> Result<catallax_numbers::Answer<T>, catallax_numbers::Refusal>,
) -> Result<catallax_numbers::Answer<T>, catallax_numbers::Refusal> {
    let mut decider = Decider::default();
    let span = catallax_numbers::OperationSpan::begin("brep", &operation, operation, &decider);
    let result = run(&mut decider);
    span.finish(&decider, result.as_ref().err(), result.as_ref().ok().map(|a| &a.marks));
    result
}

/// A rational lower bound of pi, so a requested angle in degrees becomes a bound in
/// radians that is never looser than asked.
const PI_BELOW: (i64, i64) = (314_159_265, 100_000_000);

fn options(params: &TessellationParams) -> KernelResult<MeshOptions> {
    let values = [
        params.chord_height,
        params.curve_angle_deg,
        params.surface_angle_deg,
        params.max_edge_length,
        params.max_aspect_ratio,
    ];
    if values.iter().any(|v| !v.is_finite() || *v < 0.0) {
        return Err(KernelError::InvalidInput("tessellation controls must be finite and nonnegative".into()));
    }
    let exact = |v: f64| Rational::parse(&v.to_string()).map_err(|e| KernelError::InvalidInput(e.to_string()));
    let mut options = MeshOptions::default();
    if params.chord_height > 0.0 {
        // Only an explicit bound needs room for the trait's f32 positions.
        // Zero disables this control; keep the already-certified native default.
        options.chord_tolerance = &exact(params.chord_height)? / &Rational::from_i64(2);
    }
    if params.surface_angle_deg > 0.0 {
        let pi = Rational::from_ratio(PI_BELOW.0, PI_BELOW.1).expect("pi bound");
        options.angular_deviation = Some(&(&exact(params.surface_angle_deg)? * &pi) / &Rational::from_i64(180));
    }
    if params.max_edge_length > 0.0 {
        options.max_edge_length = Some(exact(params.max_edge_length)?);
    }
    if params.max_aspect_ratio > 0.0 {
        options.max_aspect_ratio = Some(exact(params.max_aspect_ratio)?);
    }
    Ok(options)
}

impl Kernel for CatallaxKernel {
    fn propose_import(&self, source: &[u8], options: &ImportOptions) -> KernelResult<ProposalSet> {
        self.observe("propose_import", || import::propose(source, options))
    }

    fn import_step(&self, tag: &OpTag, request: &ImportStepRequest<'_>) -> KernelResult<ImportStepResult> {
        self.observe("import_step", || self.import_body(tag, request))
    }

    fn backend(&self) -> BackendInfo {
        BackendInfo { name: "catallax".into(), version: CATALLAX_REVISION.into() }
    }

    fn create_body_from_profile(&self, tag: &OpTag, profile: &Profile) -> KernelResult<OpResult> {
        self.observe("create_body_from_profile", || {
            profile.check()?;
            let mut state = self.state()?;
            let seq = state.id();
            let result = format!("op_{seq}");
            let step = json!({
                "id":result, "op":"profile", "plane":profile.plane,
                "loops":{"outer":profile.outer,"holes":profile.holes}
            });
            // Conversion validates the exact frame and joined profile without manufacturing a sheet.
            // The converter requires a body result, so validate with a structural extrusion only.
            let input = json!({
                "format":"soliddraft-recipe/1","name":"profile",
                "steps":[step, {"id":"check_body","op":"extrude","profile":result,"distance":1}],
                "result":"check_body"
            });
            catallax_app::soliddraft::convert(&input.to_string())
                .map_err(|e| build_error(OpKind::Profile, Error::Conversion(e)))?;
            let record = HistoryRecord {
                seq,
                tag: tag.clone(),
                op: OpKind::Profile,
                faces: EntityChanges::default(),
                edges: EntityChanges::default(),
                vertices: EntityChanges::default(),
                face_roles: vec![],
            };
            let body = BodyId::from_raw(state.id());
            state.bodies.insert(
                body,
                Stored { steps: BTreeMap::from([(seq, step)]), result, history: vec![record.clone()], solid: None },
            );
            Ok(OpResult { body, record })
        })
    }

    fn extrude(&self, tag: &OpTag, profile: BodyId, params: &Extrude) -> KernelResult<OpResult> {
        self.observe("extrude", || {
            let state = self.state()?;
            let p = state.get(profile)?;
            if p.solid.is_some() {
                return Err(KernelError::InvalidInput("extrude needs a profile".into()));
            }
            let step =
                json!({"op":"extrude","profile":p.result,"distance":params.distance,"direction":params.direction});
            self.operation(state, tag, OpKind::Extrude, &[profile], step)
        })
    }

    fn revolve(&self, tag: &OpTag, profile: BodyId, params: &Revolve) -> KernelResult<OpResult> {
        self.observe("revolve", || {
            let state = self.state()?;
            let p = state.get(profile)?;
            if p.solid.is_some() {
                return Err(KernelError::InvalidInput("revolve needs a profile".into()));
            }
            let step = json!({
                "op":"revolve","profile":p.result,"axis_origin":params.axis_origin,
                "axis_direction":params.axis_direction,"angle_deg":params.angle_deg
            });
            self.operation(state, tag, OpKind::Revolve, &[profile], step)
        })
    }

    fn boolean(&self, tag: &OpTag, op: BooleanOp, target: BodyId, tool: BodyId) -> KernelResult<OpResult> {
        self.observe("boolean", || {
            let state = self.state()?;
            state.solid(target)?;
            state.solid(tool)?;
            let step =
                json!({"op":"boolean","kind":op,"target":state.get(target)?.result,"tool":state.get(tool)?.result});
            self.operation(state, tag, op.kind(), &[target, tool], step)
        })
    }

    fn fillet(&self, tag: &OpTag, body: BodyId, edges: &[EdgeId], radius: f64) -> KernelResult<OpResult> {
        self.observe("fillet", || {
            let state = self.state()?;
            let solid = state.solid(body)?;
            if edges.is_empty() {
                return Err(KernelError::InvalidInput("a fillet needs at least one edge".into()));
            }
            let refs = edges.iter().map(|e| solid.edge_ref(*e, body)).collect::<KernelResult<Vec<_>>>()?;
            let native = catallax_recipe::Operation::Fillet {
                body: state.step(body)?,
                edges: catallax_recipe::EdgeSelection::Refs(refs),
                radius: native::number(radius, "fillet radius")?,
            };
            self.native(state, tag, OpKind::Fillet, body, &[body], native, None)
        })
    }

    fn chamfer(&self, tag: &OpTag, body: BodyId, edges: &[EdgeId], distance: f64) -> KernelResult<OpResult> {
        self.observe("chamfer", || {
            let state = self.state()?;
            let solid = state.solid(body)?;
            if edges.is_empty() {
                return Err(KernelError::InvalidInput("a chamfer needs at least one edge".into()));
            }
            let refs = edges.iter().map(|e| solid.edge_ref(*e, body)).collect::<KernelResult<Vec<_>>>()?;
            let native = catallax_recipe::Operation::Chamfer {
                body: state.step(body)?,
                edges: catallax_recipe::EdgeSelection::Refs(refs),
                distance: native::number(distance, "chamfer distance")?,
                other_distance: None,
            };
            self.native(state, tag, OpKind::Chamfer, body, &[body], native, None)
        })
    }

    fn shell(&self, tag: &OpTag, body: BodyId, open: &[FaceId], thickness: f64) -> KernelResult<OpResult> {
        self.observe("shell", || {
            let state = self.state()?;
            let solid = state.solid(body)?;
            let operation = shell::operation(state.step(body)?, open, thickness, |face| solid.face_ref(face, body))?;
            self.native(state, tag, OpKind::Shell, body, &[body], operation, None)
        })
    }

    fn extrude_to(
        &self,
        tag: &OpTag,
        profile: BodyId,
        target: BodyId,
        direction: Option<Vec3>,
        limit: &ExtrudeLimit,
    ) -> KernelResult<OpResult> {
        self.observe("extrude_to", || {
            let state = self.state()?;
            if state.get(profile)?.solid.is_some() {
                return Err(KernelError::InvalidInput("extrude_to needs a profile".into()));
            }
            let solid = state.solid(target)?;
            let (target_step, profile_step) = (state.step(target)?, state.step(profile)?);
            let exact = direction.map(|d| native::vector(d, "sweep direction")).transpose()?;
            let native = match limit {
                ExtrudeLimit::UpToNext => catallax_recipe::Operation::ExtrudeNext {
                    target: target_step,
                    profile: profile_step,
                    direction: exact,
                },
                ExtrudeLimit::UpToFace { face } => catallax_recipe::Operation::ExtrudeTo {
                    target: target_step,
                    profile: profile_step,
                    direction: exact,
                    limit: solid.face_ref(*face, target)?,
                },
            };
            let sweep = json!({"profile": state.get(profile)?.result, "direction": direction});
            let base = state.get(target)?.result.clone();
            let step = json!({"op":"native","base":base,"native":native,"sweep":sweep});
            // The swept solid's history starts at the profile: the target is only read.
            self.operation_reading(state, tag, OpKind::ExtrudeTo, &[profile], &[target], step)
        })
    }

    fn hole(&self, tag: &OpTag, body: BodyId, params: &Hole) -> KernelResult<OpResult> {
        self.observe("hole", || {
            let state = self.state()?;
            let solid = state.solid(body)?;
            let native = native::hole(state.step(body)?, &solid, body, params)?;
            self.native(state, tag, OpKind::Hole, body, &[body], native, None)
        })
    }

    fn pattern(&self, tag: &OpTag, body: BodyId, layout: &PatternLayout) -> KernelResult<OpResult> {
        self.observe("pattern", || {
            let state = self.state()?;
            state.solid(body)?;
            let native = catallax_recipe::Operation::Pattern {
                body: state.step(body)?,
                layout: native::pattern_layout(layout)?,
            };
            self.native(state, tag, OpKind::Pattern, body, &[body], native, None)
        })
    }

    fn mirror(&self, tag: &OpTag, body: BodyId, params: &Mirror) -> KernelResult<OpResult> {
        self.observe("mirror", || {
            let state = self.state()?;
            state.solid(body)?;
            let native = catallax_recipe::Operation::Mirror {
                body: state.step(body)?,
                plane: native::plane(params.plane_origin, params.plane_normal, "mirror plane")?,
                keep_original: params.keep_original,
            };
            self.native(state, tag, OpKind::Mirror, body, &[body], native, None)
        })
    }

    fn draft(&self, tag: &OpTag, body: BodyId, faces: &[FaceId], params: &Draft) -> KernelResult<OpResult> {
        self.observe("draft", || {
            let state = self.state()?;
            let solid = state.solid(body)?;
            if faces.is_empty() {
                return Err(KernelError::InvalidInput("a draft needs at least one face".into()));
            }
            let native = catallax_recipe::Operation::Draft {
                body: state.step(body)?,
                faces: faces.iter().map(|f| solid.face_ref(*f, body)).collect::<KernelResult<Vec<_>>>()?,
                pull: native::vector(params.pull, "pull direction")?,
                neutral: native::plane(params.neutral_origin, params.neutral_normal, "neutral plane")?,
                angle: native::number(params.angle_deg, "draft angle")?,
                keep_regions: None,
            };
            self.native(state, tag, OpKind::Draft, body, &[body], native, None)
        })
    }

    fn lattice(&self, tag: &OpTag, body: BodyId, params: &Lattice) -> KernelResult<OpResult> {
        self.observe("lattice", || self.build_lattice(tag, body, params))
    }

    fn gyroid(&self, tag: &OpTag, body: BodyId, params: &Gyroid) -> KernelResult<OpResult> {
        self.observe("gyroid", || self.build_gyroid(tag, body, params))
    }

    fn export_field_stl(&self, body: BodyId) -> KernelResult<FieldExport> {
        self.observe("export_field_stl", || self.field(body)?.ok_or(KernelError::UnknownBody(body))?.export())
    }

    fn field_mass(&self, body: BodyId, accuracy: &FieldMassAccuracy) -> KernelResult<FieldMass> {
        self.observe("field_mass", || self.field(body)?.ok_or(KernelError::UnknownBody(body))?.mass(accuracy))
    }

    fn export_field_stand_in(&self, body: BodyId, accuracy: &FieldMassAccuracy) -> KernelResult<StandInExport> {
        self.observe("export_field_stand_in", || {
            self.field(body)?.ok_or(KernelError::UnknownBody(body))?.stand_in(accuracy)
        })
    }

    fn tessellate(&self, body: BodyId, params: &TessellationParams) -> KernelResult<Mesh> {
        self.observe("tessellate", || {
            if self.field(body)?.is_some() {
                return Err(KernelError::Unsupported("field meshes require labelled certified export".into()));
            }
            let solid = self.solid(body)?;
            let options = options(params)?;
            let mesh = solid.display(&options, "tessellate")?;
            let rounding_budget = options.chord_tolerance.to_f64_nearest();
            if mesh.positions.iter().any(|p| {
                let rounded = p.map(|v| f64::from(v as f32));
                !rounded.iter().all(|v| v.is_finite()) || Vec3::from(*p).distance(rounded.into()) > rounding_budget
            }) {
                return Err(gap("requested chord tolerance cannot survive f32 display rounding", "1"));
            }
            Ok(convert::mesh(&mesh, &solid.ids, params))
        })
    }

    fn tessellate_edges(&self, body: BodyId, params: &TessellationParams) -> KernelResult<Vec<EdgePolyline>> {
        self.observe("tessellate_edges", || {
            if self.field(body)?.is_some() {
                return Ok(vec![]);
            }
            let solid = self.solid(body)?;
            let mesh = solid.display(&options(params)?, "tessellate_edges")?;
            Ok(mesh
                .edges
                .iter()
                .map(|edge| EdgePolyline {
                    edge: solid.ids.edges[&edge.edge],
                    points: edge.vertices.iter().map(|v| mesh.positions[*v].into()).collect(),
                })
                .collect())
        })
    }

    fn reserve_proof_work(&self) -> KernelResult<Box<dyn ProofWork>> {
        construction::reserve_proof_work()
    }

    fn construction_edit(
        &self,
        body: BodyId,
        edits: &[ScalarEdit],
        params: &TessellationParams,
    ) -> KernelResult<ConstructionEdit> {
        self.observe("construction_edit", || construction::prepare(self, body, edits, params))
    }

    fn preview(&self, body: BodyId, params: &TessellationParams) -> KernelResult<Mesh> {
        self.observe("preview", || {
            if let Some(field) = self.field(body)? {
                return Ok(field.preview());
            }
            let solid = self.solid(body)?;
            // Where Catallax cannot sample a preview, the screen shows the certified mesh.
            let preview = solid.preview(&options(params)?, "preview")?;
            let Some(mesh) = preview.as_ref() else {
                return self.tessellate(body, params);
            };
            if mesh.positions.iter().flatten().any(|v| !(*v as f32).is_finite()) {
                return Err(gap("preview coordinates beyond f32 display range", "1"));
            }
            Ok(convert::preview(mesh, &solid.ids))
        })
    }

    fn preview_edges(&self, body: BodyId, params: &TessellationParams) -> KernelResult<Vec<EdgePolyline>> {
        self.observe("preview_edges", || {
            if self.field(body)?.is_some() {
                return Ok(vec![]);
            }
            let solid = self.solid(body)?;
            let preview = solid.preview(&options(params)?, "preview_edges")?;
            let Some(mesh) = preview.as_ref() else {
                return self.tessellate_edges(body, params);
            };
            Ok(mesh
                .edges
                .iter()
                .map(|edge| EdgePolyline {
                    edge: solid.ids.edges[&edge.edge],
                    points: edge.vertices.iter().map(|v| mesh.positions[*v].into()).collect(),
                })
                .collect())
        })
    }

    fn mass_properties(&self, body: BodyId) -> KernelResult<MassProperties> {
        self.observe("mass_properties", || {
            if self.field(body)?.is_some() {
                return Err(KernelError::Unsupported("a field's moments need an explicit field mass request".into()));
            }
            let solid = self.solid(body)?;
            solid
                .mass
                .get_or_init(|| {
                    let built = solid.app.checkpoint.built();
                    let accuracy = catallax_measure::Accuracy::default();
                    let mass = numerical("mass_properties", |dec| {
                        // A fitted blend has no second moments, so it reports no inertia.
                        if built.model.blend_base(built.result).is_some() {
                            let measured =
                                catallax_measure::volume_area_centroid(&built.model, built.result, &accuracy, dec)?;
                            return Ok(catallax_numbers::Answer::with_marks(
                                catallax_measure::MassProperties {
                                    volume: measured.value.volume,
                                    area: measured.value.area,
                                    centroid: measured.value.centroid,
                                    inertia: None,
                                },
                                measured.marks,
                            ));
                        }
                        catallax_measure::mass_properties(&built.model, built.result, &accuracy, dec)
                    })
                    .map_err(|e| query_error("mass_properties", e))?
                    .value;
                    Ok(MassProperties {
                        volume: convert::midpoint(&mass.volume),
                        area: convert::midpoint(&mass.area),
                        centroid: mass
                            .centroid
                            .map(|c| Vec3::from(c.each_ref().map(convert::midpoint)))
                            .ok_or_else(|| gap("centroid of an empty body", "1"))?,
                        inertia: mass.inertia.map(|i| i.map(|row| row.each_ref().map(convert::midpoint))),
                    })
                })
                .clone()
        })
    }

    fn cached_mass_properties(&self, body: BodyId) -> Option<MassProperties> {
        self.solid(body).ok()?.mass.get()?.as_ref().ok().copied()
    }

    fn validate(&self, body: BodyId) -> KernelResult<Validation> {
        self.observe("validate", || {
            let solid = self.solid(body)?;
            let built = solid.app.checkpoint.built();
            let faults = built.model.check(built.result);
            Ok(Validation {
                valid: faults.is_empty(),
                kind: BodyKind::Solid,
                manifold_solid: faults.is_empty(),
                issues: faults.iter().map(|f| format!("{f:?}")).collect(),
            })
        })
    }

    fn topology(&self, body: BodyId) -> KernelResult<Topology> {
        self.observe("topology", || {
            if let Some(field) = self.field(body)? {
                return Ok(field.topology());
            }
            let solid = self.solid(body)?;
            // Topology uses the original certified display, independent of later mesh requests.
            if let Some(value) = solid.topology.get() {
                return value.clone();
            }
            let mesh = solid.display(&MeshOptions::default(), "topology")?;
            solid.topology.get_or_init(|| convert::topology(&solid.app, &solid.ids, &mesh, false)).clone()
        })
    }

    fn measured_topology(&self, body: BodyId) -> KernelResult<Topology> {
        self.observe("measured_topology", || {
            let solid = self.solid(body)?;
            if let Some(value) = solid.measured.get() {
                return value.clone();
            }
            let mesh = solid.display(&MeshOptions::default(), "measured_topology")?;
            solid.measured.get_or_init(|| convert::topology(&solid.app, &solid.ids, &mesh, true)).clone()
        })
    }

    fn edge_length(&self, body: BodyId, edge: EdgeId) -> KernelResult<Measurement> {
        self.observe("edge_length", || measure::edge_length(self, body, edge))
    }

    fn face_area(&self, body: BodyId, face: FaceId) -> KernelResult<Measurement> {
        self.observe("face_area", || measure::face_area(self, body, face))
    }

    fn edge_midpoint(&self, body: BodyId, edge: EdgeId) -> KernelResult<PointProof> {
        self.observe("edge_midpoint", || measure::edge_midpoint(self, body, edge))
    }

    fn measure_pair(
        &self,
        body: BodyId,
        a: PairEndpoint,
        b: PairEndpoint,
        quantity: PairQuantity,
    ) -> KernelResult<Measurement> {
        self.observe("measure_pair", || measure::measure_pair(self, body, a, b, quantity))
    }

    fn face_support_plane(&self, body: BodyId, face: FaceId) -> KernelResult<ExactAxisPlane> {
        self.observe("face_support_plane", || assembly_measure::face_support_plane(self, body, face))
    }

    fn history(&self, body: BodyId) -> KernelResult<Vec<HistoryRecord>> {
        self.observe("history", || Ok(self.state()?.get(body)?.history.clone()))
    }

    fn release(&self, body: BodyId) -> KernelResult<()> {
        self.observe("release", || {
            let mut state = self.state()?;
            state.fields.remove(&body);
            state.bodies.remove(&body).map(|_| ()).ok_or(KernelError::UnknownBody(body))
        })
    }

    /// The recipe text, the history, cached certified meshes and previews, plus a
    /// fixed allowance per B-rep face and edge of every body the build kept.
    /// Catallax does not report its model's size, so the allowance is an estimate.
    fn retained_bytes(&self, body: BodyId) -> KernelResult<u64> {
        if let Some(field) = self.field(body)? {
            return Ok(field.retained_bytes());
        }
        const FACE: usize = 2048;
        const EDGE: usize = 512;
        let mesh_bytes = |m: &catallax_mesh::Mesh| {
            m.positions.len() * 24
                + m.triangles.len() * 32
                + m.faces.len() * 64
                + m.edges.iter().map(|e| 64 + e.vertices.len() * 8).sum::<usize>()
        };
        let state = self.state()?;
        let stored = state.get(body)?;
        let recipe: usize = stored.steps.values().map(|s| s.to_string().len()).sum();
        let history: usize = stored.history.iter().map(|r| 256 + r.face_roles.len() * 32).sum();
        let mut bytes = recipe + history;
        if let Some(solid) = &stored.solid {
            let brep = solid.app.checkpoint.built().bodies.len().max(1)
                * (solid.ids.faces.len() * FACE + solid.ids.edges.len() * EDGE);
            bytes += brep;
            solid.display.each(|mesh| bytes += mesh_bytes(mesh));
            solid.previews.each(|m| {
                let Some(m) = m else { return };
                bytes += m.positions.len() * 24
                    + m.triangles.len() * 32
                    + m.faces.len() * 64
                    + m.edges.iter().map(|e| 64 + e.vertices.len() * 8).sum::<usize>()
            });
        }
        Ok(bytes as u64)
    }

    fn view_curves(&self, body: BodyId, spec: &drawing::ViewSpec) -> KernelResult<drawing::ViewCurves> {
        self.observe("view_curves", || drawing_queries::view_curves(self, body, spec))
    }
    /// A section for the screen is cut from the preview mesh, with no boolean; a
    /// certified one subtracts a half-space, as every backend's default does. Either is
    /// observed as one drawing query: its mesh or scratch boolean and the cut body's
    /// view are nested in this call's log and work scope, so a drawing's section costs
    /// what it costs on the display side.
    fn section_view(&self, body: BodyId, spec: &drawing::SectionSpec) -> KernelResult<drawing::SectionView> {
        self.observe("section_view", || {
            if spec.view.preview {
                let view = drawing_queries::resolved_view(self, body, &spec.view);
                drawing::section::by_mesh_slice(self, body, &drawing::SectionSpec { view, ..*spec })
            } else {
                drawing::section::by_half_space(self, body, spec)
            }
        })
    }
    fn silhouette_curves(&self, _: BodyId, _: Vec3) -> KernelResult<Vec<drawing::ViewCurve>> {
        self.observe("silhouette_curves", || Err(gap("exact silhouettes", "1, drawing queries (roadmap section 7)")))
    }
    fn section_curves(&self, _: BodyId, _: &Plane) -> KernelResult<Vec<drawing::ViewCurve>> {
        self.observe("section_curves", || Err(gap("section curves", "1, drawing queries (roadmap section 7)")))
    }
    fn edge_curve(&self, body: BodyId, edge: EdgeId) -> KernelResult<drawing::Curve3> {
        self.observe("edge_curve", || drawing_queries::edge_curve(self.solid(body)?.as_ref(), body, edge))
    }
    fn face_loops(&self, body: BodyId, face: FaceId) -> KernelResult<Vec<Vec<drawing::Curve3>>> {
        self.observe("face_loops", || drawing_queries::face_loops(self.solid(body)?.as_ref(), body, face))
    }
    fn lines_and_arcs(&self, _: &drawing::Curve3, _: f64) -> KernelResult<Vec<drawing::Curve3>> {
        self.observe("lines_and_arcs", || {
            Err(gap("curve approximation for drawings", "1, drawing queries (roadmap section 7)"))
        })
    }

    fn export_step(&self, body: BodyId, options: &StepOptions) -> KernelResult<StepExport> {
        self.observe("export_step", || step::export(&self.solid(body)?.app, options))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn certified_display_is_cached_after_a_refused_request_retries() {
        let kernel = CatallaxKernel::new();
        let body = contract::cylinder(&kernel, Vec3::ZERO, 5.0, 10.0);
        let solid = kernel.solid(body).unwrap();
        let retained = kernel.retained_bytes(body).unwrap();
        let mut meshes = 0;
        solid.display.each(|_| meshes += 1);
        assert_eq!(meshes, 0, "building and accounting must not request a mesh");
        let options = MeshOptions::default();
        let (refused, work) = catallax_numbers::with_operation_budget(0, || kernel.topology(body));
        assert!(work.exhausted);
        assert!(matches!(refused, Err(KernelError::QueryFailed { message, .. })
            if message.contains("numbers.operation-budget")));
        let (mesh, work) = catallax_numbers::with_operation_budget(100_000_000, || solid.display(&options, "test"));
        assert!(!work.exhausted);
        assert!(work.used > 0);
        let mesh = mesh.unwrap();
        let (cached, work) = catallax_numbers::with_operation_budget(0, || solid.display(&options, "test"));
        assert!(!work.exhausted);
        assert_eq!(work.used, 0);
        assert!(Arc::ptr_eq(&mesh, &cached.unwrap()));
        assert!(kernel.retained_bytes(body).unwrap() > retained);
        assert!(kernel.topology(body).is_ok(), "a mesh refusal must not poison the topology cache");
    }

    #[test]
    fn retained_prefixes_branch_and_refuse_without_replaying_or_changing_earlier_bodies() {
        let kernel = CatallaxKernel::with_operation_log();
        let base = contract::block(&kernel);
        let base_recipe = kernel.body_recipe(base).unwrap();
        let options = StepOptions::default();
        let original = kernel.export_step(base, &options).unwrap();
        for x in [-8.0, 8.0] {
            let profile = kernel
                .create_body_from_profile(
                    &OpTag::new("tool_sketch"),
                    &Profile::rectangle(Plane::XY.with_origin(Vec3::new(x, 0.0, 0.0)), Point2::new(0.0, 0.0), 4.0, 6.0),
                )
                .unwrap();
            let tool = kernel
                .extrude(&OpTag::new("tool"), profile.body, &Extrude { distance: 10.0, direction: None })
                .unwrap()
                .body;
            kernel.take_diagnostics().unwrap();
            let cut = kernel.boolean(&OpTag::new("cut"), BooleanOp::Subtract, base, tool).unwrap().body;
            let evidence = kernel.take_diagnostics().unwrap();
            let recipe = kernel.body_recipe(cut).unwrap();
            let (replayed, steps) = build(&recipe, OpKind::Subtract, None, None).unwrap();
            let operations = evidence["calls"][0]["operation_log"]["operations"].as_array().unwrap();
            let result = steps[recipe["result"].as_str().unwrap()].as_str();
            assert!(operations.iter().any(|op| op["id"] == result), "{evidence}");
            for step in base_recipe["steps"].as_array().unwrap() {
                let old = steps[step["id"].as_str().unwrap()].as_str();
                assert!(!operations.iter().any(|op| op["id"] == old), "replayed {old}: {evidence}");
            }
            let solid = kernel.solid(cut).unwrap();
            assert_eq!(solid.app.names, replayed.names);
            assert_eq!(kernel.export_step(cut, &options).unwrap(), step::export(&replayed, &options).unwrap());
            assert!((kernel.mass_properties(cut).unwrap().volume - 11760.0).abs() < 1e-6);

            // The later-created target is not the leading operand. This refusal
            // must neither consume nor mutate the earlier body's checkpoint.
            let error = kernel.boolean(&OpTag::new("empty"), BooleanOp::Subtract, tool, base).unwrap_err();
            assert!(matches!(error, KernelError::Unsupported(_)), "{error}");
            assert_eq!(kernel.export_step(base, &options).unwrap(), original);
        }
        assert_eq!(kernel.mass_properties(base).unwrap().volume, 12000.0);
    }

    #[test]
    fn limited_sweeps_reuse_read_only_targets_without_claiming_their_history() {
        let kernel = CatallaxKernel::with_operation_log();
        let target = contract::block(&kernel);
        let original = kernel.export_step(target, &StepOptions::default()).unwrap();
        let target_recipe = kernel.body_recipe(target).unwrap();
        let bottom =
            kernel.topology(target).unwrap().faces.iter().find(|face| face.normal == Some(-Vec3::Z)).unwrap().id;
        let profile = kernel
            .create_body_from_profile(
                &OpTag::new("post"),
                &Profile::circle(Plane::XY.with_origin(Vec3::new(0.0, 0.0, -5.0)), Point2::new(0.0, 0.0), 3.0),
            )
            .unwrap()
            .body;

        for limit in [ExtrudeLimit::UpToNext, ExtrudeLimit::UpToFace { face: bottom }] {
            kernel.take_diagnostics().unwrap();
            let swept = kernel.extrude_to(&OpTag::new("sweep"), profile, target, None, &limit).unwrap();
            let evidence = kernel.take_diagnostics().unwrap();
            let operations = evidence["calls"][0]["operation_log"]["operations"].as_array().unwrap();
            for step in target_recipe["steps"].as_array().unwrap() {
                let id = &step["id"];
                assert!(!operations.iter().any(|op| &op["id"] == id), "rebuilt read-only target {id}: {evidence}");
            }
            let recipe = kernel.body_recipe(swept.body).unwrap();
            let (fresh, _) = build(&recipe, OpKind::ExtrudeTo, None, None).unwrap();
            assert_eq!(kernel.solid(swept.body).unwrap().app.names, fresh.names);
            assert_eq!(
                kernel.export_step(swept.body, &StepOptions::default()).unwrap(),
                step::export(&fresh, &StepOptions::default()).unwrap()
            );
            assert!((kernel.mass_properties(swept.body).unwrap().volume - 45.0 * std::f64::consts::PI).abs() < 1e-6);
            let history = kernel.history(swept.body).unwrap();
            assert_eq!(history.iter().map(|record| record.tag.as_str()).collect::<Vec<_>>(), ["post", "sweep"]);
            assert!(swept.record.faces.deleted.is_empty());
            assert_eq!(kernel.export_step(target, &StepOptions::default()).unwrap(), original);
        }
    }
}
