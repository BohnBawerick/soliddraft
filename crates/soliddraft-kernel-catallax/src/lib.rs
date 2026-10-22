//! Catallax's application entry behind SolidDraft's existing kernel interface.
//!
//! Profiles are deferred recipe fragments, not sheet geometry. Each solid owns an
//! immutable Catallax application build. Legacy history is derived from Catallax's
//! origin sets; display estimates never decide topology or operation success.

mod convert;
mod diagnostics;
mod drawing_queries;
mod frames;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use catallax_app::{ApplicationBody, Error};
use catallax_mesh::{MeshOptions, mesh_body};
use catallax_numbers::{Decider, Rational};
use serde_json::{Value, json};
use soliddraft_kernel_api::*;

/// The pinned Catallax application entry revision.
pub const CATALLAX_REVISION: &str = "674d284db928c076b74eac3846670403c38bbad3";

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
    app: ApplicationBody,
    ids: convert::Ids,
    display: Mutex<Option<(Rational, Arc<catallax_mesh::Mesh>)>>,
    topology: OnceLock<KernelResult<Topology>>,
    mass: OnceLock<KernelResult<MassProperties>>,
}

impl Solid {
    fn display(&self, options: &MeshOptions, query: &str) -> KernelResult<Arc<catallax_mesh::Mesh>> {
        let mut cached =
            self.display.lock().map_err(|_| KernelError::Unavailable("Catallax display cache lock poisoned".into()))?;
        if let Some((chord, mesh)) = cached.as_ref()
            && chord == &options.chord_tolerance
        {
            return Ok(mesh.clone());
        }
        let mesh = Arc::new(
            numerical(query, |dec| mesh_body(&self.app.built.model, self.app.built.result, options, dec))
                .map_err(|e| query_error(query, e))?
                .value,
        );
        // Keep one tolerance per immutable body so triangles and edges share the same mesh.
        *cached = Some((options.chord_tolerance.clone(), mesh.clone()));
        Ok(mesh)
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
        self.get(body)?.solid.clone().ok_or_else(|| gap("profile sheet queries", "1"))
    }

    fn operation(&mut self, tag: &OpTag, op: OpKind, inputs: &[BodyId], mut step: Value) -> KernelResult<OpResult> {
        let mut steps = BTreeMap::new();
        let mut history = BTreeMap::new();
        let mut parents = Vec::new();
        for input in inputs {
            let stored = self.get(*input)?;
            steps.extend(stored.steps.clone());
            history.extend(stored.history.iter().map(|r| (r.seq, r.clone())));
            if let Some(solid) = &stored.solid {
                parents.push(solid.clone());
            }
        }
        let seq = self.id();
        let result = format!("op_{seq}");
        step["id"] = result.clone().into();
        steps.insert(seq, step);
        let input = json!({"format":"soliddraft-recipe/1", "name":"SolidDraft", "steps": steps.values().collect::<Vec<_>>(), "result": result});
        let questions = self.capture.then_some(&mut self.questions);
        let app = build(&input, op, questions)?;
        if app.built.model.body(app.built.result).kind == catallax_topology::BodyKind::Empty {
            return Err(gap("empty results have no BodyKind or centroid in the existing application interface", "1"));
        }
        let ids = convert::Ids::new(&app, &parents.iter().map(|p| &p.ids).collect::<Vec<_>>(), &mut self.next);
        let record = ids.record(seq, tag, op, &result, &parents.iter().map(|p| &p.ids).collect::<Vec<_>>());
        history.insert(seq, record.clone());
        let body = BodyId::from_raw(self.id());
        self.bodies.insert(
            body,
            Stored {
                steps,
                result,
                history: history.into_values().collect(),
                solid: Some(Arc::new(Solid {
                    app,
                    ids,
                    display: Mutex::new(None),
                    topology: OnceLock::new(),
                    mass: OnceLock::new(),
                })),
            },
        );
        Ok(OpResult { body, record })
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

    /// The legacy recipe that actually constructed a body's current geometry.
    pub fn body_recipe(&self, body: BodyId) -> KernelResult<Value> {
        let state = self.state()?;
        let stored = state.get(body)?;
        Ok(json!({"format":"soliddraft-recipe/1", "name":"SolidDraft",
            "steps":stored.steps.values().collect::<Vec<_>>(), "result":stored.result}))
    }

    fn observe<T>(&self, operation: &str, run: impl FnOnce() -> KernelResult<T>) -> KernelResult<T> {
        match &self.diagnostics {
            Some(d) => d.observe(operation, run),
            None => run(),
        }
    }

    fn state(&self) -> KernelResult<MutexGuard<'_, State>> {
        self.state.lock().map_err(|_| KernelError::Unavailable("Catallax body arena lock poisoned".into()))
    }

    fn solid(&self, body: BodyId) -> KernelResult<Arc<Solid>> {
        self.state()?.solid(body)
    }
}

fn gap(what: &str, stage: &str) -> KernelError {
    KernelError::Unsupported(format!(
        "{what} is not available in the Catallax application entry (Catallax stage {stage})"
    ))
}

fn build(input: &Value, op: OpKind, questions: Option<&mut Vec<Value>>) -> KernelResult<ApplicationBody> {
    let mut converted =
        catallax_app::soliddraft::convert(&input.to_string()).map_err(|e| build_error(op, Error::Conversion(e)))?;
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
            planes.insert(name, (plane, pose.rotation.clone()));
        }
    }
    for source in sources {
        if source["op"] != "extrude" || source["direction"].is_null() {
            continue;
        }
        let direction: Vec3 = serde_json::from_value(source["direction"].clone())
            .map_err(|e| KernelError::InvalidInput(format!("stored sweep direction: {e}")))?;
        let profile = source["profile"].as_str().expect("generated profile id");
        let (plane, rotation) = &planes[profile];
        if let Some(direction) = frames::normal_sweep(*plane, rotation, direction) {
            let id = &converted.steps[source["id"].as_str().expect("generated step id")];
            let step = converted.recipe.steps.iter_mut().find(|s| &s.id == id).expect("converted extrusion step");
            if let catallax_recipe::Operation::Extrude { direction: sweep, .. } = &mut step.op {
                *sweep = Some(direction);
            }
        }
    }
    if let Some(questions) = questions {
        let problem = serde_json::to_value(&converted.recipe).expect("Catallax recipes serialize");
        questions.push(json!({"format":"catallax-question/1", "source":"recipe", "problem":problem}));
    }
    catallax_app::build(&converted.recipe, &MeshOptions::default(), &mut Decider::default())
        .map_err(|e| build_error(op, e))
}

fn build_error(op: OpKind, error: Error) -> KernelError {
    let code = error.code();
    if matches!(error, Error::Conversion(catallax_app::soliddraft::ConversionError::NotYet { .. })) {
        return gap(&format!("{code}: {error}"), "1");
    }
    KernelError::Failed { op, code: 0, message: format!("{code}: {error} (Catallax stage 1)") }
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
    let mut options = MeshOptions::default();
    if params.chord_height > 0.0 {
        options.chord_tolerance =
            Rational::parse(&params.chord_height.to_string()).map_err(|e| KernelError::InvalidInput(e.to_string()))?;
    }
    // Reserve half the requested chord bound for the trait's f32 display positions.
    options.chord_tolerance = &options.chord_tolerance / &Rational::from_i64(2);
    Ok(options)
}

impl Kernel for CatallaxKernel {
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
            let mut state = self.state()?;
            let p = state.get(profile)?;
            if p.solid.is_some() {
                return Err(KernelError::InvalidInput("extrude needs a profile".into()));
            }
            let step =
                json!({"op":"extrude","profile":p.result,"distance":params.distance,"direction":params.direction});
            state.operation(tag, OpKind::Extrude, &[profile], step)
        })
    }

    fn revolve(&self, tag: &OpTag, profile: BodyId, params: &Revolve) -> KernelResult<OpResult> {
        self.observe("revolve", || {
            let mut state = self.state()?;
            let p = state.get(profile)?;
            if p.solid.is_some() {
                return Err(KernelError::InvalidInput("revolve needs a profile".into()));
            }
            let step = json!({
                "op":"revolve","profile":p.result,"axis_origin":params.axis_origin,
                "axis_direction":params.axis_direction,"angle_deg":params.angle_deg
            });
            state.operation(tag, OpKind::Revolve, &[profile], step)
        })
    }

    fn boolean(&self, tag: &OpTag, op: BooleanOp, target: BodyId, tool: BodyId) -> KernelResult<OpResult> {
        self.observe("boolean", || {
            let mut state = self.state()?;
            state.solid(target)?;
            state.solid(tool)?;
            let step =
                json!({"op":"boolean","kind":op,"target":state.get(target)?.result,"tool":state.get(tool)?.result});
            state.operation(tag, op.kind(), &[target, tool], step)
        })
    }

    fn fillet(&self, _: &OpTag, body: BodyId, _: &[EdgeId], _: f64) -> KernelResult<OpResult> {
        self.observe("fillet", || {
            self.solid(body)?;
            Err(gap("fillet", "3 for fillets between planes and cylinders; Catallax stage 4 for other fillets"))
        })
    }

    fn chamfer(&self, _: &OpTag, body: BodyId, _: &[EdgeId], _: f64) -> KernelResult<OpResult> {
        self.observe("chamfer", || {
            self.solid(body)?;
            Err(gap("chamfer", "3"))
        })
    }

    fn shell(&self, _: &OpTag, body: BodyId, _: &[FaceId], _: f64) -> KernelResult<OpResult> {
        self.observe("shell", || {
            self.solid(body)?;
            Err(gap("shell", "3"))
        })
    }

    fn tessellate(&self, body: BodyId, params: &TessellationParams) -> KernelResult<Mesh> {
        self.observe("tessellate", || {
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
            Ok(convert::mesh(&solid.app.built.model, &mesh, &solid.ids, params))
        })
    }

    fn tessellate_edges(&self, body: BodyId, params: &TessellationParams) -> KernelResult<Vec<EdgePolyline>> {
        self.observe("tessellate_edges", || {
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

    fn mass_properties(&self, body: BodyId) -> KernelResult<MassProperties> {
        self.observe("mass_properties", || {
            let solid = self.solid(body)?;
            solid
                .mass
                .get_or_init(|| {
                    let mass = numerical("mass_properties", |dec| {
                        catallax_measure::mass_properties(
                            &solid.app.built.model,
                            solid.app.built.result,
                            &catallax_measure::Accuracy::default(),
                            dec,
                        )
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
                        inertia: None,
                    })
                })
                .clone()
        })
    }

    fn validate(&self, body: BodyId) -> KernelResult<Validation> {
        self.observe("validate", || {
            let solid = self.solid(body)?;
            let faults = solid.app.built.model.check(solid.app.built.result);
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
            let solid = self.solid(body)?;
            solid.topology.get_or_init(|| convert::topology(&solid.app, &solid.ids)).clone()
        })
    }

    fn history(&self, body: BodyId) -> KernelResult<Vec<HistoryRecord>> {
        self.observe("history", || Ok(self.state()?.get(body)?.history.clone()))
    }

    fn release(&self, body: BodyId) -> KernelResult<()> {
        self.observe("release", || self.state()?.bodies.remove(&body).map(|_| ()).ok_or(KernelError::UnknownBody(body)))
    }

    fn view_curves(&self, body: BodyId, spec: &drawing::ViewSpec) -> KernelResult<drawing::ViewCurves> {
        self.observe("view_curves", || drawing_queries::view_curves(self, body, spec))
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
}
