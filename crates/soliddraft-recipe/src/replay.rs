//! Replays a recipe on any [`Kernel`] and reports what each step produced.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use soliddraft_kernel_api::{
    BackendInfo, BodyId, BodyKind, EdgeId, EntityChanges, Extrude, FaceId, HistoryRecord, Kernel, KernelError, OpKind,
    OpResult, OpTag, Profile, Revolve, Vec3,
};

use crate::digest::digest;
use crate::format::{Command, FORMAT, Outcome, Recipe, Selector, Shape};

/// Why a recipe could not be replayed.
#[derive(Debug, thiserror::Error)]
pub enum RecipeError {
    /// The recipe text is not valid JSON for the format.
    #[error("cannot parse recipe: {0}")]
    Parse(#[from] serde_json::Error),
    /// The recipe is well formed JSON but inconsistent.
    #[error("invalid recipe: {0}")]
    Invalid(String),
    /// A step that should succeed failed.
    #[error("step {step} failed: {error}")]
    StepFailed {
        /// Step id.
        step: String,
        /// The kernel's error.
        error: KernelError,
    },
    /// A step marked `"expect": "error"` succeeded.
    #[error("step {0} was expected to fail but succeeded")]
    UnexpectedSuccess(String),
    /// A step that failed changed one of its inputs.
    #[error("step {step} failed but changed its input {input}")]
    InputChanged {
        /// Step id.
        step: String,
        /// Id of the input step whose body changed.
        input: String,
    },
    /// A selector matched no entity.
    #[error("step {step}: selector matched nothing: {selector}")]
    EmptySelection {
        /// Step id.
        step: String,
        /// The selector, as JSON.
        selector: String,
    },
    /// A query on a body failed.
    #[error("query on step {step} failed: {error}")]
    Query {
        /// Step id.
        step: String,
        /// The kernel's error.
        error: KernelError,
    },
}

/// Counts, measures and digest of one body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BodySummary {
    /// Solid or sheet.
    pub kind: BodyKind,
    /// Number of faces.
    pub faces: usize,
    /// Number of edges.
    pub edges: usize,
    /// Number of vertices.
    pub vertices: usize,
    /// Volume (zero for a sheet).
    pub volume: f64,
    /// Surface area.
    pub area: f64,
    /// Result of the kernel's validation.
    pub valid: bool,
    /// Geometric digest, see [`crate::digest`].
    pub digest: String,
}

/// Application-side facts about a deferred profile. No kernel body has been built.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProfileSummary {
    /// Number of boundary loops, including holes.
    pub loops: usize,
    /// Whether every loop passes the application's closure check.
    pub closed: bool,
    /// Tags retained for the operation that will consume this profile.
    pub tags: Vec<OpTag>,
}

/// What a step did.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum StepOutcome {
    /// A profile is stored until an extrusion or revolution consumes it.
    Profile {
        /// Application-side profile facts.
        profile: ProfileSummary,
    },
    /// The step produced a body.
    Ok {
        /// The body.
        body: BodySummary,
        /// Faces created, modified, deleted and split by this step.
        faces_changed: [usize; 4],
        /// Edges created, modified, deleted and split by this step.
        edges_changed: [usize; 4],
    },
    /// The step failed as expected and its inputs kept their digests.
    ExpectedError {
        /// The kernel's error.
        error: String,
    },
}

/// One step of a replay.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StepReport {
    /// Step id.
    pub id: String,
    /// Operation.
    pub op: OpKind,
    /// What happened.
    #[serde(flatten)]
    pub outcome: StepOutcome,
}

/// The outcome of a replay.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplayReport {
    /// Recipe name.
    pub recipe: String,
    /// The backend that ran it.
    pub backend: BackendInfo,
    /// Every step, in order.
    pub steps: Vec<StepReport>,
    /// Id of the result step.
    pub result_step: String,
    /// The result body.
    pub result: BodySummary,
}

/// Parses a recipe and checks its structure.
pub fn parse(text: &str) -> Result<Recipe, RecipeError> {
    let mut recipe: Recipe = serde_json::from_str(text)?;
    check(&recipe)?;
    recipe.format = FORMAT.into();
    Ok(recipe)
}

/// Checks the format tag, unique ids, that every input names an earlier successful step,
/// and that the result names a successful step.
pub fn check(recipe: &Recipe) -> Result<(), RecipeError> {
    if !matches!(recipe.format.as_str(), FORMAT | "draftwell-recipe/1") {
        return Err(RecipeError::Invalid(format!("format is {:?}, expected {FORMAT:?}", recipe.format)));
    }
    let mut ok: HashSet<&str> = HashSet::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for step in &recipe.steps {
        if !seen.insert(&step.id) {
            return Err(RecipeError::Invalid(format!("duplicate step id {:?}", step.id)));
        }
        for input in step.command.inputs() {
            if !ok.contains(input) {
                return Err(RecipeError::Invalid(format!(
                    "step {:?} uses {input:?}, which is not an earlier step that succeeds",
                    step.id
                )));
            }
        }
        if step.expect == Outcome::Ok {
            ok.insert(&step.id);
        }
    }
    if !ok.contains(recipe.result.as_str()) {
        return Err(RecipeError::Invalid(format!("result {:?} is not a step that succeeds", recipe.result)));
    }
    Ok(())
}

/// Summarises a body: counts, volume, area, validity and digest.
pub fn summarize(kernel: &dyn Kernel, body: BodyId) -> Result<BodySummary, KernelError> {
    let topology = kernel.topology(body)?;
    let mass = kernel.mass_properties(body)?;
    let validation = kernel.validate(body)?;
    Ok(BodySummary {
        kind: topology.kind,
        faces: topology.faces.len(),
        edges: topology.edges.len(),
        vertices: topology.vertex_count,
        volume: mass.volume,
        area: mass.area,
        valid: validation.valid,
        digest: digest(&topology, &mass),
    })
}

/// Replays a recipe from scratch. Every body the replay creates is released before it
/// returns, whether it succeeds or not.
pub fn replay(kernel: &dyn Kernel, recipe: &Recipe) -> Result<ReplayReport, RecipeError> {
    let (report, body) = build(kernel, recipe)?;
    let _ = kernel.release(body);
    Ok(report)
}

/// Replays a recipe like [`replay`] but keeps the result body, which the caller then
/// owns and releases. Every other body the replay creates is released.
pub fn build(kernel: &dyn Kernel, recipe: &Recipe) -> Result<(ReplayReport, BodyId), RecipeError> {
    check(recipe)?;
    let mut run = Run { kernel, bodies: BTreeMap::new(), profiles: BTreeMap::new() };
    let result = run.all(recipe).map(|report| (report, run.body(&recipe.result)));
    let keep = result.as_ref().ok().map(|(_, body)| *body);
    for body in run.bodies.values() {
        if Some(*body) != keep {
            let _ = kernel.release(*body);
        }
    }
    result
}

struct Run<'k> {
    kernel: &'k dyn Kernel,
    bodies: BTreeMap<String, BodyId>,
    profiles: BTreeMap<String, (OpTag, Profile)>,
}

impl Run<'_> {
    fn all(&mut self, recipe: &Recipe) -> Result<ReplayReport, RecipeError> {
        if matches!(
            recipe.steps.iter().find(|s| s.id == recipe.result).map(|s| &s.command),
            Some(Command::Profile { .. })
        ) {
            return Err(RecipeError::Invalid(
                "the result must be a body-producing step, not a deferred profile".into(),
            ));
        }
        let mut steps = Vec::new();
        for step in &recipe.steps {
            let tag = OpTag::new(&step.id);
            if let Command::Profile { plane, shape } = &step.command {
                let profile = match shape {
                    Shape::Rectangle { center, width, height } => {
                        Profile::rectangle(plane.plane(), *center, *width, *height)
                    }
                    Shape::Circle { center, radius } => Profile::circle(plane.plane(), *center, *radius),
                    Shape::Loops { outer, holes } => {
                        Profile { plane: plane.plane(), outer: outer.clone(), holes: holes.clone() }
                    }
                };
                let checked = profile.check();
                let outcome = match (step.expect, checked) {
                    (Outcome::Ok, Ok(())) => {
                        let summary =
                            ProfileSummary { loops: profile.loops().count(), closed: true, tags: vec![tag.clone()] };
                        self.profiles.insert(step.id.clone(), (tag, profile));
                        StepOutcome::Profile { profile: summary }
                    }
                    (Outcome::Error, Err(error)) => StepOutcome::ExpectedError { error: error.to_string() },
                    (Outcome::Error, Ok(())) => return Err(RecipeError::UnexpectedSuccess(step.id.clone())),
                    (Outcome::Ok, Err(error)) => {
                        return Err(RecipeError::StepFailed { step: step.id.clone(), error: error.into() });
                    }
                };
                steps.push(StepReport { id: step.id.clone(), op: OpKind::Profile, outcome });
                continue;
            }
            let before: Vec<(String, String)> = if step.expect == Outcome::Error {
                step.command
                    .inputs()
                    .into_iter()
                    .map(|input| Ok((input.to_string(), self.snapshot(input)?)))
                    .collect::<Result<_, RecipeError>>()?
            } else {
                Vec::new()
            };
            let outcome = self.run(&step.id, &tag, &step.command);
            let report = match (step.expect, outcome) {
                (Outcome::Ok, Ok((op, result))) => {
                    self.bodies.insert(step.id.clone(), result.body);
                    let body = self.summary(&step.id)?;
                    StepReport {
                        id: step.id.clone(),
                        op,
                        outcome: StepOutcome::Ok {
                            body,
                            faces_changed: counts(&result.record.faces),
                            edges_changed: counts(&result.record.edges),
                        },
                    }
                }
                (Outcome::Ok, Err(e)) => return Err(e),
                (Outcome::Error, Ok((_, result))) => {
                    let _ = self.kernel.release(result.body);
                    return Err(RecipeError::UnexpectedSuccess(step.id.clone()));
                }
                (Outcome::Error, Err(e)) => {
                    let error = match e {
                        RecipeError::StepFailed { error, .. } => error.to_string(),
                        RecipeError::EmptySelection { .. } => e.to_string(),
                        other => return Err(other),
                    };
                    for (input, digest) in before {
                        if self.snapshot(&input)? != digest {
                            return Err(RecipeError::InputChanged { step: step.id.clone(), input });
                        }
                    }
                    StepReport {
                        id: step.id.clone(),
                        op: op_kind(&step.command),
                        outcome: StepOutcome::ExpectedError { error },
                    }
                }
            };
            steps.push(report);
        }
        let result = self.summary(&recipe.result)?;
        Ok(ReplayReport {
            recipe: recipe.name.clone(),
            backend: self.kernel.backend(),
            steps,
            result_step: recipe.result.clone(),
            result,
        })
    }

    fn snapshot(&self, id: &str) -> Result<String, RecipeError> {
        if let Some(profile) = self.profiles.get(id) {
            return Ok(serde_json::to_string(profile)?);
        }
        Ok(self.summary(id)?.digest)
    }

    fn sweep(
        &self,
        id: &str,
        op: impl FnOnce(BodyId) -> Result<OpResult, KernelError>,
    ) -> Result<OpResult, KernelError> {
        let (tag, profile) = self
            .profiles
            .get(id)
            .ok_or_else(|| KernelError::InvalidInput(format!("{id} is not a deferred profile")))?;
        let sheet = self.kernel.create_body_from_profile(tag, profile)?;
        let result = op(sheet.body);
        let _ = self.kernel.release(sheet.body);
        result
    }

    fn body(&self, id: &str) -> BodyId {
        // `check` guarantees every input is an earlier successful step.
        self.bodies[id]
    }

    fn summary(&self, id: &str) -> Result<BodySummary, RecipeError> {
        summarize(self.kernel, self.body(id)).map_err(|error| RecipeError::Query { step: id.into(), error })
    }

    fn run(&self, id: &str, tag: &OpTag, command: &Command) -> Result<(OpKind, OpResult), RecipeError> {
        let k = self.kernel;
        let fail = |error| RecipeError::StepFailed { step: id.into(), error };
        let op = op_kind(command);
        if !matches!(command, Command::Extrude { .. } | Command::Revolve { .. })
            && command.inputs().iter().any(|id| self.profiles.contains_key(*id))
        {
            return Err(fail(KernelError::InvalidInput("this operation needs a solid, not a deferred profile".into())));
        }
        let result = match command {
            Command::Profile { .. } => unreachable!("profiles are deferred in all"),
            Command::Extrude { profile, distance, direction } => self
                .sweep(profile, |body| k.extrude(tag, body, &Extrude { distance: *distance, direction: *direction })),
            Command::Revolve { profile, axis_origin, axis_direction, angle_deg } => self.sweep(profile, |body| {
                k.revolve(
                    tag,
                    body,
                    &Revolve { axis_origin: *axis_origin, axis_direction: *axis_direction, angle_deg: *angle_deg },
                )
            }),
            Command::Boolean { kind, target, tool } => k.boolean(tag, *kind, self.body(target), self.body(tool)),
            Command::Fillet { body, edges, radius } => {
                let b = self.body(body);
                let edges = self.select_edges(id, b, edges)?;
                k.fillet(tag, b, &edges, *radius)
            }
            Command::Chamfer { body, edges, distance } => {
                let b = self.body(body);
                let edges = self.select_edges(id, b, edges)?;
                k.chamfer(tag, b, &edges, *distance)
            }
            Command::Shell { body, open_faces, thickness } => {
                let b = self.body(body);
                let faces = self.select_faces(id, b, open_faces)?;
                k.shell(tag, b, &faces, *thickness)
            }
        };
        result.map(|r| (op, r)).map_err(fail)
    }

    fn select_edges(&self, step: &str, body: BodyId, sel: &Selector) -> Result<Vec<EdgeId>, RecipeError> {
        let query = |error| RecipeError::Query { step: step.into(), error };
        let topo = self.kernel.topology(body).map_err(query)?;
        let history = self.kernel.history(body).map_err(query)?;
        let items: Vec<Item<EdgeId>> =
            topo.edges.iter().map(|e| Item { id: e.id, point: e.midpoint, min: e.bbox.min, max: e.bbox.max }).collect();
        let chosen = select(sel, &items, &history, |r| &r.edges);
        nonempty(step, sel, chosen)
    }

    fn select_faces(&self, step: &str, body: BodyId, sel: &Selector) -> Result<Vec<FaceId>, RecipeError> {
        let query = |error| RecipeError::Query { step: step.into(), error };
        let topo = self.kernel.topology(body).map_err(query)?;
        let history = self.kernel.history(body).map_err(query)?;
        let items: Vec<Item<FaceId>> =
            topo.faces.iter().map(|f| Item { id: f.id, point: f.centroid, min: f.bbox.min, max: f.bbox.max }).collect();
        let chosen = select(sel, &items, &history, |r| &r.faces);
        nonempty(step, sel, chosen)
    }
}

fn op_kind(command: &Command) -> OpKind {
    match command {
        Command::Profile { .. } => OpKind::Profile,
        Command::Extrude { .. } => OpKind::Extrude,
        Command::Revolve { .. } => OpKind::Revolve,
        Command::Boolean { kind, .. } => kind.kind(),
        Command::Fillet { .. } => OpKind::Fillet,
        Command::Chamfer { .. } => OpKind::Chamfer,
        Command::Shell { .. } => OpKind::Shell,
    }
}

fn counts<Id>(c: &EntityChanges<Id>) -> [usize; 4] {
    [c.created.len(), c.modified.len(), c.deleted.len(), c.split.len()]
}

fn nonempty<T>(step: &str, sel: &Selector, chosen: Vec<T>) -> Result<Vec<T>, RecipeError> {
    if chosen.is_empty() {
        let selector = serde_json::to_string(sel).unwrap_or_default();
        return Err(RecipeError::EmptySelection { step: step.into(), selector });
    }
    Ok(chosen)
}

/// What a selector sees of one entity.
struct Item<Id> {
    id: Id,
    point: Vec3,
    min: Vec3,
    max: Vec3,
}

/// Distance under which a bounding box counts as inside a `within` box.
const WITHIN_TOL: f64 = 1e-6;

/// Evaluates a selector. Results keep the body's entity order.
fn select<Id: Copy + Eq + std::hash::Hash>(
    sel: &Selector,
    items: &[Item<Id>],
    history: &[HistoryRecord],
    changes: impl Fn(&HistoryRecord) -> &EntityChanges<Id> + Copy,
) -> Vec<Id> {
    match sel {
        Selector::All => items.iter().map(|i| i.id).collect(),
        Selector::CreatedBy(step) => {
            let alive = descendants(step, history, changes);
            items.iter().filter(|i| alive.contains(&i.id)).map(|i| i.id).collect()
        }
        Selector::Nearest(p) => items
            .iter()
            .min_by(|a, b| a.point.distance(*p).total_cmp(&b.point.distance(*p)))
            .map(|i| vec![i.id])
            .unwrap_or_default(),
        Selector::Within { min, max } => items
            .iter()
            .filter(|i| {
                let inside = |lo: f64, v: f64, hi: f64| v >= lo - WITHIN_TOL && v <= hi + WITHIN_TOL;
                inside(min.x, i.min.x, max.x)
                    && inside(min.y, i.min.y, max.y)
                    && inside(min.z, i.min.z, max.z)
                    && inside(min.x, i.max.x, max.x)
                    && inside(min.y, i.max.y, max.y)
                    && inside(min.z, i.max.z, max.z)
            })
            .map(|i| i.id)
            .collect(),
        Selector::AllOf(list) => {
            let mut chosen: Vec<Id> = items.iter().map(|i| i.id).collect();
            for s in list {
                let these: HashSet<Id> = select(s, items, history, changes).into_iter().collect();
                chosen.retain(|id| these.contains(id));
            }
            chosen
        }
    }
}

/// The entities the step tagged `step` generated, followed through every later record:
/// split entities are replaced by their pieces and deleted ones dropped.
fn descendants<Id: Copy + Eq + std::hash::Hash>(
    step: &str,
    history: &[HistoryRecord],
    changes: impl Fn(&HistoryRecord) -> &EntityChanges<Id>,
) -> HashSet<Id> {
    let mut set: HashSet<Id> = HashSet::new();
    let mut started = false;
    for record in history {
        let c = changes(record);
        if record.tag.as_str() == step {
            set.extend(c.generated());
            started = true;
            continue;
        }
        if !started {
            continue;
        }
        for s in &c.split {
            if set.remove(&s.from) {
                set.extend(s.into.iter().copied());
            }
        }
        for d in &c.deleted {
            set.remove(d);
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use soliddraft_kernel_api::{OpKind, Split};

    fn record(seq: u64, tag: &str, changes: EntityChanges<u64>) -> HistoryRecord {
        let ids = |v: &Vec<u64>| v.iter().map(|&i| FaceId::from_raw(i)).collect();
        HistoryRecord {
            seq,
            tag: OpTag::new(tag),
            op: OpKind::Union,
            faces: EntityChanges {
                created: ids(&changes.created),
                modified: ids(&changes.modified),
                deleted: ids(&changes.deleted),
                split: changes
                    .split
                    .iter()
                    .map(|s| Split { from: FaceId::from_raw(s.from), into: ids(&s.into) })
                    .collect(),
            },
            edges: EntityChanges::default(),
            vertices: EntityChanges::default(),
            face_roles: Vec::new(),
        }
    }

    fn changes(created: &[u64], deleted: &[u64], split: &[(u64, &[u64])]) -> EntityChanges<u64> {
        EntityChanges {
            created: created.to_vec(),
            modified: vec![],
            deleted: deleted.to_vec(),
            split: split.iter().map(|(f, i)| Split { from: *f, into: i.to_vec() }).collect(),
        }
    }

    fn item(id: u64, p: [f64; 3]) -> Item<FaceId> {
        let p = Vec3::from(p);
        Item { id: FaceId::from_raw(id), point: p, min: p, max: p }
    }

    #[test]
    fn created_by_follows_splits_and_deletions() {
        let history = vec![
            record(1, "a", changes(&[1, 2, 3], &[], &[])),
            record(2, "b", changes(&[4], &[], &[])),
            record(3, "c", changes(&[], &[3], &[(2, &[5, 6])])),
        ];
        let got = descendants("a", &history, |r| &r.faces);
        let want: HashSet<FaceId> = [1, 5, 6].into_iter().map(FaceId::from_raw).collect();
        assert_eq!(got, want);
        assert!(descendants("missing", &history, |r| &r.faces).is_empty());
    }

    #[test]
    fn selectors_choose_by_position_and_combine() {
        let items = vec![item(1, [0.0, 0.0, 0.0]), item(2, [0.0, 0.0, 10.0]), item(3, [5.0, 0.0, 10.0])];
        let ids = |v: Vec<FaceId>| v.into_iter().map(|f| f.raw()).collect::<Vec<_>>();
        let pick = |s: &Selector| ids(select(s, &items, &[], |r| &r.faces));
        assert_eq!(pick(&Selector::All), [1, 2, 3]);
        assert_eq!(pick(&Selector::Nearest(Vec3::new(4.0, 0.0, 9.0))), [3]);
        let top = Selector::Within { min: Vec3::new(-1.0, -1.0, 10.0), max: Vec3::new(10.0, 1.0, 10.0) };
        assert_eq!(pick(&top), [2, 3]);
        let both = Selector::AllOf(vec![top, Selector::Nearest(Vec3::new(0.0, 0.0, 11.0))]);
        assert_eq!(pick(&both), [2]);
    }

    fn minimal(steps: &str, result: &str) -> String {
        format!(r#"{{"format": "soliddraft-recipe/1", "name": "t", "steps": [{steps}], "result": "{result}"}}"#)
    }

    const SKETCH: &str = r#"{"id": "s", "op": "profile", "plane": "XY", "circle": {"center": [0, 0], "radius": 1}}"#;

    #[test]
    fn check_rejects_inconsistent_recipes() {
        assert!(parse(&minimal(SKETCH, "s")).is_ok());
        let dup = format!("{SKETCH}, {SKETCH}");
        assert!(matches!(parse(&minimal(&dup, "s")), Err(RecipeError::Invalid(m)) if m.contains("duplicate")));
        let forward = r#"{"id": "e", "op": "extrude", "profile": "s", "distance": 1}"#;
        assert!(matches!(parse(&minimal(forward, "e")), Err(RecipeError::Invalid(_))));
        assert!(matches!(parse(&minimal(SKETCH, "nope")), Err(RecipeError::Invalid(_))));
        let failing = r#"{"id": "s", "op": "profile", "plane": "XY", "circle": {"center": [0, 0], "radius": 1}, "expect": "error"}"#;
        assert!(matches!(parse(&minimal(failing, "s")), Err(RecipeError::Invalid(_))), "result must succeed");
        let wrong = minimal(SKETCH, "s").replace("soliddraft-recipe/1", "soliddraft-recipe/9");
        let legacy = minimal(SKETCH, "s").replace(FORMAT, "draftwell-recipe/1");
        assert_eq!(parse(&legacy).unwrap().format, FORMAT);
        assert!(parse(&legacy.replace("draftwell-recipe/1", "draftwell-recipe/9")).is_err());
        assert!(matches!(parse(&wrong), Err(RecipeError::Invalid(_))));
        assert!(matches!(parse("{"), Err(RecipeError::Parse(_))));
    }
}
