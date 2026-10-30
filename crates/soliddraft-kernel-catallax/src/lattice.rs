//! Terminal fields keep their native stage separate from B-rep bodies and exports.

use catallax_field::{
    lattice_stage::{self, GyroidSpec, LatticeSpec, Output, Settings, Stage, TerminalField},
    printing,
};
use catallax_numbers::{Decider, OperationSpan, Rational, Refusal};
use catallax_recipe::{Grade, Pose, Recipe};
use sha2::{Digest, Sha256};

use crate::*;

/// The stage calls an active field needs, whichever field it holds.
trait FieldStage: Send + Sync {
    fn recipe(&self) -> &Recipe;
    fn accuracy(&self) -> &Rational;
    fn export(&self, dec: &mut Decider) -> Result<Output, Refusal>;
}

impl<S: TerminalField + Send + Sync> FieldStage for Stage<S> {
    fn recipe(&self) -> &Recipe {
        Stage::recipe(self)
    }
    fn accuracy(&self) -> &Rational {
        &self.settings().accuracy
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
            radius: Grade::Constant(native::number(params.radius_mm, "density")?),
        };
        self.build_field(tag, body, OpKind::Lattice, spec, settings)
    }

    pub(super) fn build_gyroid(&self, tag: &OpTag, body: BodyId, params: &Gyroid) -> KernelResult<OpResult> {
        let settings = settings("gyroid", params.accuracy_mm, params.min_detail_mm)?;
        let spec = GyroidSpec {
            period: native::number(params.period_mm, "gyroid period")?,
            level: Grade::Constant(native::number(params.level, "gyroid level")?),
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
        let (bytes, output) = run("export_field_stl", |dec| self.stage.export(dec)?.labelled_stl())?;
        // Read the encoded bound so the JSON report describes the file exactly.
        let bound = f64::from_le_bytes(bytes[68..76].try_into().expect("labelled header"));
        let report = FieldReport {
            approximate: true,
            route: "field".into(),
            requested_accuracy_mm: self.stage.accuracy().to_string(),
            deviation_bound_mm: bound,
            closed: output.verdict().closed,
            connected: output.verdict().connected,
            components: output.verdict().components,
            triangles: output.mesh().triangles.len(),
            vertices: output.mesh().positions.len(),
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        };
        Ok(FieldExport { bytes, report })
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
