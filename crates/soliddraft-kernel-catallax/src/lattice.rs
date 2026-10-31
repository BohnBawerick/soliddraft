//! Terminal fields keep their native stage separate from B-rep bodies and exports.

use catallax_field::{
    Plan,
    lattice_stage::{self, GyroidSpec, LatticeSpec, Output, Settings, Stage, TerminalField},
    mass, printing,
};
use catallax_measure::{Accuracy as QuantityAccuracy, Enclosure};
use catallax_numbers::{
    Decider, OperationSpan, Rational, Refusal, current_operation_work, operation_budget_active, with_operation_budget,
};
use catallax_recipe::{Pose, Recipe};
use sha2::{Digest, Sha256};

use crate::*;

/// The deterministic work an explicit field moment or stand-in request may use when its
/// caller has set no allowance: just under the application's 20,000,000-unit limit.
pub(crate) const FIELD_ACTION_WORK: u64 = 19_999_999;

/// The label format Catallax writes into a certified field STL's header.
const STL_LABEL: &str = "catallax-print-stl/1";

/// The stage calls an active field needs, whichever field it holds.
trait FieldStage: Send + Sync {
    fn recipe(&self) -> &Recipe;
    fn plan(&self) -> &Plan;
    fn settings(&self) -> &Settings;
    fn export(&self, dec: &mut Decider) -> Result<Output, Refusal>;
}

impl<S: TerminalField + Send + Sync> FieldStage for Stage<S> {
    fn recipe(&self) -> &Recipe {
        Stage::recipe(self)
    }
    fn plan(&self) -> &Plan {
        Stage::plan(self)
    }
    fn settings(&self) -> &Settings {
        Stage::settings(self)
    }
    fn export(&self, dec: &mut Decider) -> Result<Output, Refusal> {
        Stage::export(self, dec)
    }
}

pub(super) struct Field {
    stage: Box<dyn FieldStage>,
    preview: Mesh,
    topology: Topology,
}

fn run<T>(operation: &str, action: impl FnOnce(&mut Decider) -> Result<T, Refusal>) -> KernelResult<T> {
    let mut dec = Decider::default();
    let span = OperationSpan::begin("field", &operation, operation, &dec);
    let result = action(&mut dec);
    span.finish(&dec, result.as_ref().err(), None);
    result.map_err(|e| query_error(operation, e))
}

/// Runs one field action on its caller's allowance, or on `limit` when the caller has
/// none, and returns the work it used. A nested action never gets a fresh allowance, and
/// a value computed after the allowance ran out is discarded.
fn allowance<T>(operation: &str, limit: u64, action: impl FnOnce() -> KernelResult<T>) -> KernelResult<(T, u64)> {
    let exhausted =
        || query_error(operation, Refusal::new("numbers.operation-budget", "operation work allowance exhausted"));
    if operation_budget_active() {
        let before = current_operation_work().used;
        let result = action();
        let after = current_operation_work();
        if after.exhausted {
            return Err(exhausted());
        }
        return result.map(|value| (value, after.used - before));
    }
    let (result, work) = with_operation_budget(limit, action);
    if work.exhausted {
        return Err(exhausted());
    }
    result.map(|value| (value, work.used))
}

/// The output settings of one field, at the default caps.
fn settings(name: &str, accuracy_mm: f64, min_detail_mm: f64) -> KernelResult<Settings> {
    let defaults = printing::Options::default();
    Ok(Settings {
        accuracy: native::number(accuracy_mm, &format!("{name} accuracy"))?.value().clone(),
        minimum_detail: native::number(min_detail_mm, &format!("{name} minimum detail"))?.value().clone(),
        max_cells: defaults.max_cells,
        max_vertices: defaults.max_vertices,
    })
}

/// The native spelling of a grade: a linear grade's anchors are part points on its axis.
fn grade(grade: &Grade, what: &str) -> KernelResult<catallax_recipe::Grade> {
    Ok(match grade {
        Grade::Constant(value) => catallax_recipe::Grade::Constant(native::number(*value, what)?),
        Grade::Linear(linear) => {
            let [from, to] = linear.anchors();
            let anchor = format!("{what} grade anchor");
            catallax_recipe::Grade::Linear {
                from: native::point(Vec3::from(from), &anchor)?,
                to: native::point(Vec3::from(to), &anchor)?,
                start: native::number(linear.start, what)?,
                end: native::number(linear.end, what)?,
            }
        }
    })
}

fn quantity(width: f64, what: &str) -> KernelResult<QuantityAccuracy> {
    if !(width.is_finite() && width >= 0.0) {
        return Err(KernelError::InvalidInput(format!("the {what} width must be finite and not negative")));
    }
    Ok(QuantityAccuracy { relative: Rational::zero(), absolute: native::number(width, what)?.value().clone() })
}

fn requested(accuracy: &FieldMassAccuracy) -> KernelResult<mass::Accuracy> {
    Ok(mass::Accuracy {
        volume: quantity(accuracy.volume_mm3, "volume")?,
        centroid: quantity(accuracy.centroid_mm, "centroid")?,
        inertia: quantity(accuracy.inertia_mm5, "inertia")?,
    })
}

fn range(e: &Enclosure) -> ExactRange {
    ExactRange { lo: e.lo.to_string(), hi: e.hi.to_string() }
}

fn report_mass(measured: &mass::Properties, requested: &mass::Accuracy, work: u64) -> FieldMass {
    FieldMass {
        scope: "original-field-definition".into(),
        requested_widths: [&requested.volume, &requested.centroid, &requested.inertia].map(|q| q.absolute.to_string()),
        volume_mm3: range(&measured.volume),
        centroid_mm: measured.centroid.as_ref().map(|c| c.each_ref().map(range)),
        inertia_mm5: measured.inertia.as_ref().map(|m| m.each_ref().map(|row| row.each_ref().map(range))),
        cells: measured.cells,
        work,
    }
}

/// What the stand-in request proved, before its report is written.
struct StandIn {
    bytes: Vec<u8>,
    measured: mass::Properties,
    surrogate: catallax_features::stand_in::StandInBodies,
    exchanged: mass::SurrogateProperties,
    step: catallax_step::StepReport,
}

impl CatallaxKernel {
    pub(super) fn field(&self, body: BodyId) -> KernelResult<Option<Arc<Field>>> {
        Ok(self.state()?.fields.get(&body).cloned())
    }

    pub(super) fn build_lattice(&self, tag: &OpTag, body: BodyId, params: &Lattice) -> KernelResult<OpResult> {
        let settings = settings("lattice", params.accuracy_mm, params.min_detail_mm)?;
        let spec = LatticeSpec {
            cell: match params.cell {
                LatticeCell::Cubic => catallax_recipe::LatticeCell::Cubic,
            },
            size: native::number(params.cell_size_mm, "cell size")?,
            frame: match params.frame {
                FieldFrame::Part => Pose::identity(),
            },
            radius: grade(&params.radius_mm, "density")?,
        };
        self.build_field(tag, body, OpKind::Lattice, spec, settings)
    }

    pub(super) fn build_gyroid(&self, tag: &OpTag, body: BodyId, params: &Gyroid) -> KernelResult<OpResult> {
        let settings = settings("gyroid", params.accuracy_mm, params.min_detail_mm)?;
        let spec = GyroidSpec {
            period: native::number(params.period_mm, "gyroid period")?,
            level: grade(&params.level, "gyroid level")?,
            kind: match params.kind {
                GyroidKind::Network => catallax_recipe::GyroidKind::Network,
            },
            frame: match params.frame {
                FieldFrame::Part => Pose::identity(),
            },
        };
        self.build_field(tag, body, OpKind::Gyroid, spec, settings)
    }

    fn build_field<S>(
        &self,
        tag: &OpTag,
        body: BodyId,
        op: OpKind,
        spec: S,
        settings: Settings,
    ) -> KernelResult<OpResult>
    where
        S: TerminalField + Send + Sync + 'static,
    {
        if let Some(field) = self.field(body)? {
            // Catallax refuses a stage over a body that already ends in a terminal field.
            return run(op.name(), |dec| Stage::new(field.stage.recipe(), spec, settings, dec))
                .and(Err(KernelError::InvalidInput(format!("the {} needs a CAD body", op.name()))));
        }
        let recipe = converted_recipe(&self.body_recipe(body)?, op)?.recipe;
        let solid = self.solid(body)?;
        let built = solid.app.checkpoint.built();
        let bounds = numerical("bounding_box", |dec| {
            catallax_measure::bounding_box(&built.model, built.result, &catallax_measure::Accuracy::default(), dec)
        })
        .map_err(|e| query_error("bounding_box", e))?
        .value
        .ok_or_else(|| KernelError::InvalidInput(format!("the {} needs a nonempty body", op.name())))?;
        let bbox = Aabb {
            min: bounds.min.each_ref().map(Rational::to_f64_nearest).into(),
            max: bounds.max.each_ref().map(Rational::to_f64_nearest).into(),
        };
        let stage = run(op.name(), |dec| Stage::new(&recipe, spec, settings, dec))?;
        // Keep the native question even when preview generation refuses.
        {
            let mut state = self.state()?;
            if state.capture {
                state.questions.push(json!({"format":"catallax-question/1", "source":"recipe",
                    "problem":stage.recipe()}));
            }
        }
        let mesh = run("preview", |dec| {
            let controls = lattice_stage::default_preview_options(stage.plan(), dec)?;
            Ok(stage.preview(&controls, dec)?.mesh)
        })?;
        let mut state = self.state()?;
        let id = BodyId::from_raw(state.id());
        let face = FaceId::from_raw(state.id());
        let preview = preview_mesh(&mesh, face);
        // One selectable display region, with no analytic face or mass claim.
        let topology = Topology {
            kind: BodyKind::Solid,
            faces: vec![FaceInfo {
                id: face,
                surface: SurfaceKind::Other,
                area: 0.0,
                centroid: (bbox.min + bbox.max) * 0.5,
                bbox,
                edges: vec![],
                normal: None,
                origins: vec![Origin { tag: tag.clone(), role: FaceRole::Other }],
            }],
            edges: vec![],
            vertex_count: 0,
            vertices: vec![],
            bbox,
            display_chord_height: Some(mesh.max_deviation.to_f64_nearest()),
        };
        let record = HistoryRecord {
            seq: state.id(),
            tag: tag.clone(),
            op,
            faces: EntityChanges { created: vec![face], ..Default::default() },
            edges: Default::default(),
            vertices: Default::default(),
            face_roles: vec![FaceOrigin { face, role: FaceRole::Other }],
        };
        let input = state.get(body)?;
        let mut history = input.history.clone();
        history.push(record.clone());
        let stored = Stored { steps: input.steps.clone(), result: input.result.clone(), history, solid: None };
        state.bodies.insert(id, stored);
        state.fields.insert(id, Arc::new(Field { stage: Box::new(stage), preview, topology }));
        Ok(OpResult { body: id, record })
    }
}

impl Field {
    pub(super) fn recipe(&self) -> Value {
        serde_json::to_value(self.stage.recipe()).expect("native field recipe serializes")
    }
    pub(super) fn preview(&self) -> Mesh {
        self.preview.clone()
    }
    pub(super) fn topology(&self) -> Topology {
        self.topology.clone()
    }
    pub(super) fn retained_bytes(&self) -> u64 {
        (self.preview.positions.len() * 24 + self.preview.indices.len() * 4 + self.preview.triangle_faces.len() * 8)
            as u64
    }
    pub(super) fn export(&self) -> KernelResult<FieldExport> {
        let operation = "export_field_stl";
        // Without a caller allowance the export keeps the unbounded run it always had; the
        // allowance only counts its work.
        let ((bytes, output), work) =
            allowance(operation, u64::MAX, || run(operation, |dec| self.stage.export(dec)?.labelled_stl()))?;
        // Read the encoded bound so the JSON report describes the file exactly.
        let bound = f64::from_le_bytes(bytes[68..76].try_into().expect("labelled header"));
        let settings = self.stage.settings();
        let verdict = output.verdict();
        let report = FieldReport {
            label: STL_LABEL.into(),
            approximate: true,
            route: "field".into(),
            requested_accuracy_mm: settings.accuracy.to_string(),
            minimum_detail_mm: settings.minimum_detail.to_string(),
            deviation_bound_mm: bound,
            closed: verdict.closed,
            non_self_intersecting: verdict.non_self_intersecting,
            connected: verdict.connected,
            components: verdict.components,
            triangles: output.mesh().triangles.len(),
            vertices: output.mesh().positions.len(),
            max_cells: settings.max_cells,
            max_vertices: settings.max_vertices,
            work,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        };
        Ok(FieldExport { bytes, report })
    }

    /// Certified unit-density moments of the field's definition, integrated from the
    /// compiled field alone.
    pub(super) fn mass(&self, accuracy: &FieldMassAccuracy) -> KernelResult<FieldMass> {
        let requested = requested(accuracy)?;
        let operation = "field_mass";
        let (measured, work) = allowance(operation, FIELD_ACTION_WORK, || {
            run(operation, |dec| mass::properties(self.stage.plan(), &requested, &mass::Options::default(), dec))
        })?;
        Ok(report_mass(&measured, &requested, work))
    }

    /// The explicit mass stand-in, checked against the field's enclosures before any
    /// byte is returned: the proxy's exact moments, then its moments after the file's
    /// rounding, must each lie inside the field's.
    pub(super) fn stand_in(&self, accuracy: &FieldMassAccuracy) -> KernelResult<StandInExport> {
        let requested = requested(accuracy)?;
        let operation = "export_field_stand_in";
        let recipe = self.stage.recipe();
        let (proof, work) = allowance(operation, FIELD_ACTION_WORK, || {
            run(operation, |dec| {
                let mut built = catallax_features::evaluate(recipe, dec).map_err(|e| match e {
                    catallax_features::BuildError::Geometry { refusal, .. } => refusal,
                    other => Refusal::new("field.recipe", other.to_string()),
                })?;
                mass::check_region_envelope(self.stage.plan())?;
                let measured = mass::properties(self.stage.plan(), &requested, &mass::Options::default(), dec)?;
                let Some(target) = measured.estimate.as_ref() else {
                    return Err(Refusal::new("field.stand-in-export", "the field has no material to stand in for"));
                };
                let surrogate = catallax_features::stand_in::equivalent_blocks(&mut built.model, target)?;
                if surrogate.inertia_residual >= Rational::from_ratio(1, 1_000_000_000).expect("nonzero") {
                    return Err(Refusal::new(
                        "stand-in.inertia-not-matched",
                        format!("surrogate inertia residual {} exceeds 1e-9 mm5", surrogate.inertia_residual),
                    ));
                }
                if !measured.contains(&surrogate.achieved) {
                    return Err(Refusal::new(
                        "stand-in.properties-not-matched",
                        "surrogate properties exceed the field enclosures",
                    ));
                }
                let options = catallax_step::StepOptions {
                    product_name: format!("{} field stand-in", recipe.name),
                    significant_digits: 32,
                    ..Default::default()
                };
                let mut bytes = Vec::new();
                let step = catallax_step::write_stand_in_step(
                    &built.model,
                    built.result,
                    &surrogate.bodies,
                    &options,
                    dec,
                    &mut bytes,
                )?
                .value;
                let exchanged = mass::surrogate_properties(&surrogate, &step.max_deviation)?;
                if !measured.encloses(&exchanged) {
                    return Err(Refusal::new(
                        "stand-in.exchange-not-enclosed",
                        "STEP-rounded surrogate property bounds exceed the field enclosures",
                    ));
                }
                Ok(StandIn { bytes, measured, surrogate, exchanged, step })
            })
        })?;
        let StandIn { bytes, measured, surrogate, exchanged, step } = proof;
        let strings = |values: &[Rational; 3]| values.each_ref().map(ToString::to_string);
        let report = StandInReport {
            label: "mass stand-in — not field boundary geometry".into(),
            representation: "massless-surface-envelope-plus-unit-density-solids".into(),
            field: report_mass(&measured, &requested, work),
            proxy: ProxyMoments {
                volume_mm3: surrogate.achieved.volume.to_string(),
                centroid_mm: strings(&surrogate.achieved.centroid),
                inertia_mm5: surrogate.achieved.inertia.each_ref().map(strings),
            },
            exchanged: ExchangedMoments {
                volume_mm3: range(&exchanged.volume),
                centroid_mm: exchanged.centroid.each_ref().map(range),
                inertia_mm5: exchanged.inertia.each_ref().map(|row| row.each_ref().map(range)),
            },
            inertia_residual: surrogate.inertia_residual.to_string(),
            mass_bodies: surrogate.bodies.len(),
            surface_envelopes: 1,
            max_deviation_mm: step.max_deviation.to_string(),
            density_metadata: false,
            work,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        };
        Ok(StandInExport { bytes, report })
    }
}

fn preview_mesh(mesh: &printing::Mesh, face: FaceId) -> Mesh {
    let positions = mesh.positions.clone();
    let mut normals = vec![[0.0_f32; 3]; positions.len()];
    for [a, b, c] in &mesh.triangles {
        let p = |i: usize| Vec3::from(positions[i].map(f64::from));
        let n = (p(*b) - p(*a)).cross(p(*c) - p(*a));
        for i in [a, b, c] {
            for (target, v) in normals[*i].iter_mut().zip([n.x, n.y, n.z]) {
                *target += v as f32;
            }
        }
    }
    for n in &mut normals {
        let unit = Vec3::from(n.map(f64::from)).normalized().unwrap_or(Vec3::Z);
        *n = [unit.x as f32, unit.y as f32, unit.z as f32];
    }
    Mesh {
        positions,
        normals,
        indices: mesh.triangles.iter().flatten().map(|i| *i as u32).collect(),
        triangle_faces: vec![face; mesh.triangles.len()],
        unavailable_controls: vec![],
        unproved: true,
    }
}
