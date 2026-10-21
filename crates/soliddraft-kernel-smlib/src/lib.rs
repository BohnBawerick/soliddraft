//! The SMLib backend of the SolidDraft kernel trait.
//!
//! SMLib is the C++ B-rep kernel in NVIDIA's USD BRep (Apache-2.0), built by
//! `soliddraft-smlib-sys` from the pinned submodule in `third_party/usd-brep`. The backend reaches it through
//! SM_API, the kernel's C-style layer, over a [cxx] bridge.
//!
//! SM_API keeps one process-global context that is not synchronised, so every kernel
//! call runs on one dedicated thread. The thread creates the context once with
//! `SmApiCreateContext()` and then serves commands sent over a channel. Every
//! [`SmlibKernel`] in a process talks to that one thread and shares its bodies.
//!
//! A kernel crash still takes the process down; moving the thread into a child process
//! would not change the trait.

mod convert;
mod drawing;
mod ffi;
mod history;
mod roles;

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::Arc;
#[cfg(not(target_os = "emscripten"))]
use std::sync::{OnceLock, mpsc};

use soliddraft_kernel_api::drawing::{Curve3, ViewCurve, ViewCurves, ViewSpec};
use soliddraft_kernel_api::{
    BackendInfo, BodyId, BodyKind, BooleanOp, EdgeId, EdgePolyline, Extrude, FaceId, HistoryRecord, Kernel,
    KernelError, KernelResult, MassProperties, Mesh, OpKind, OpResult, OpTag, Plane, Profile, Revolve,
    TessellationParams, Topology, Validation, Vec3, VertexId,
};

use crate::convert::invalid;
use crate::ffi::{CurveIn, Engine};
use crate::history::Shot;
use crate::roles::Roles;

pub use soliddraft_smlib_sys::{USD_BREP_COMMIT, USD_BREP_VERSION};

/// Relative tolerance for the volume checks that back up the kernel's status codes.
const VOLUME_RTOL: f64 = 1e-3;

#[cfg(not(target_os = "emscripten"))]
type Job = Box<dyn FnOnce(&mut State) + Send>;

/// The SolidDraft kernel on SMLib. Cheap to create and clone: all instances share the
/// process's one kernel thread.
#[derive(Clone)]
pub struct SmlibKernel {
    #[cfg(not(target_os = "emscripten"))]
    jobs: mpsc::Sender<Job>,
}

#[cfg(not(target_os = "emscripten"))]
static KERNEL_THREAD: OnceLock<mpsc::Sender<Job>> = OnceLock::new();

// A WebAssembly build has one thread, so the kernel's state lives on it and calls run
// in place.
#[cfg(target_os = "emscripten")]
thread_local! {
    static KERNEL_STATE: std::cell::RefCell<Option<State>> = const { std::cell::RefCell::new(None) };
}

impl SmlibKernel {
    /// Connects to the kernel thread, starting it on first use.
    #[cfg(not(target_os = "emscripten"))]
    pub fn new() -> Self {
        let jobs = KERNEL_THREAD
            .get_or_init(|| {
                let (tx, rx) = mpsc::channel::<Job>();
                std::thread::Builder::new()
                    .name("smlib-kernel".into())
                    .spawn(move || {
                        let mut state = State::new();
                        for job in rx {
                            job(&mut state);
                        }
                    })
                    .expect("spawning the SMLib kernel thread");
                tx
            })
            .clone();
        SmlibKernel { jobs }
    }

    /// Connects to the kernel, which runs on the calling thread.
    #[cfg(target_os = "emscripten")]
    pub fn new() -> Self {
        SmlibKernel {}
    }

    /// Number of bodies the kernel thread currently holds, across all instances.
    pub fn live_bodies(&self) -> KernelResult<usize> {
        self.call(|s| Ok(s.engine.body_count() as usize))
    }

    /// Runs `f` on the kernel thread and waits for its answer.
    #[cfg(not(target_os = "emscripten"))]
    fn call<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut State) -> KernelResult<R> + Send + 'static,
    ) -> KernelResult<R> {
        let (tx, rx) = mpsc::sync_channel(1);
        let job: Job = Box::new(move |state| {
            let result = catch_unwind(AssertUnwindSafe(|| f(state)))
                .unwrap_or_else(|_| Err(KernelError::Unavailable("a kernel command panicked".into())));
            let _ = tx.send(result);
        });
        self.jobs.send(job).map_err(|_| KernelError::Unavailable("the kernel thread has stopped".into()))?;
        rx.recv().map_err(|_| KernelError::Unavailable("the kernel thread has stopped".into()))?
    }

    /// Runs `f` on the kernel's state, here on the one thread.
    #[cfg(target_os = "emscripten")]
    fn call<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut State) -> KernelResult<R> + Send + 'static,
    ) -> KernelResult<R> {
        KERNEL_STATE.with(|cell| {
            let mut slot = cell
                .try_borrow_mut()
                .map_err(|_| KernelError::Unavailable("the kernel was called from inside a kernel call".into()))?;
            let state = slot.get_or_insert_with(State::new);
            catch_unwind(AssertUnwindSafe(|| f(state)))
                .unwrap_or_else(|_| Err(KernelError::Unavailable("a kernel command panicked".into())))
        })
    }
}

impl Default for SmlibKernel {
    fn default() -> Self {
        Self::new()
    }
}

/// The profile a sheet body was made from, kept so it can be swept.
struct ProfileData {
    plane: Plane,
    curves: Vec<CurveIn>,
    loop_sizes: Vec<u32>,
}

struct BodyMeta {
    snapshot: Shot,
    profile: Option<Arc<ProfileData>>,
    /// Records of every operation that led to this body, oldest first.
    lineage: Vec<Arc<HistoryRecord>>,
}

/// Everything that lives on the kernel thread.
struct State {
    engine: cxx::UniquePtr<Engine>,
    bodies: HashMap<BodyId, BodyMeta>,
    next_tag: u64,
    next_seq: u64,
}

impl State {
    fn new() -> Self {
        State { engine: ffi::new_engine(), bodies: HashMap::new(), next_tag: 1, next_seq: 1 }
    }

    fn engine(&mut self) -> Pin<&mut Engine> {
        self.engine.pin_mut()
    }

    fn meta(&self, body: BodyId) -> KernelResult<&BodyMeta> {
        self.bodies.get(&body).ok_or(KernelError::UnknownBody(body))
    }

    fn solid(&self, body: BodyId, role: &str) -> KernelResult<&BodyMeta> {
        let meta = self.meta(body)?;
        if !meta.snapshot.manifold_solid {
            return Err(invalid(format!("{role} {body} is not a solid")));
        }
        Ok(meta)
    }

    /// Records a kernel result: tags and snapshots it, checks it, and computes its history.
    /// On any failure the new kernel body is freed, so nothing changes.
    fn finish(
        &mut self,
        op: OpKind,
        tag: &OpTag,
        inputs: &[BodyId],
        handle: u64,
        expect: Expect,
        roles: Roles,
    ) -> KernelResult<OpResult> {
        let body = BodyId::from_raw(handle);
        match self.finish_inner(op, tag, inputs, body, expect, roles) {
            Ok(r) => Ok(r),
            Err(e) => {
                self.engine().release(handle);
                Err(e)
            }
        }
    }

    fn finish_inner(
        &mut self,
        op: OpKind,
        tag: &OpTag,
        inputs: &[BodyId],
        body: BodyId,
        expect: Expect,
        roles: Roles,
    ) -> KernelResult<OpResult> {
        let raw = self.engine.snapshot(body.raw()).map_err(|e| failed(op, &e))?;

        // Profiles, extrusions and revolutions build new geometry; their inputs' entities
        // are not carried into the result, so everything in it counts as created.
        let tracked: Vec<&BodyMeta> = match op {
            OpKind::Profile | OpKind::Extrude | OpKind::Revolve => Vec::new(),
            _ => inputs.iter().map(|b| self.meta(*b)).collect::<KernelResult<_>>()?,
        };
        let before: Vec<&Shot> = tracked.iter().map(|m| &m.snapshot).collect();
        let diag = convert::vec3(raw.bbox_min).distance(convert::vec3(raw.bbox_max));
        let tol = 1e-9 * (1.0 + diag);
        let mut next_tag = self.next_tag;
        let (shot, retagged) = history::tag_snapshot(&raw, &before, &mut next_tag, tol);
        let before_faces: Vec<_> = before.iter().flat_map(|s| s.faces.iter()).collect();
        let before_edges: Vec<_> = before.iter().flat_map(|s| s.edges.iter()).collect();
        let before_vertices: Vec<_> = before.iter().flat_map(|s| s.vertices.iter()).collect();
        let faces = history::diff(&before_faces, &shot.faces, tol, FaceId::from_raw);
        // Blend inputs are read from the input body even though its entities are tracked.
        let role_inputs: Vec<&Shot> = match op {
            OpKind::Fillet | OpKind::Chamfer => {
                inputs.iter().map(|b| self.meta(*b).map(|m| &m.snapshot)).collect::<KernelResult<_>>()?
            }
            _ => before.clone(),
        };
        let face_roles = roles::assign(&roles, &faces.created, &shot, &role_inputs, 1e-6 * (1.0 + diag));
        let record = HistoryRecord {
            seq: self.next_seq,
            tag: tag.clone(),
            op,
            faces,
            edges: history::diff(&before_edges, &shot.edges, tol, EdgeId::from_raw),
            vertices: history::diff(&before_vertices, &shot.vertices, tol, VertexId::from_raw),
            face_roles,
        };

        let mut issues = self.result_issues(op, body, &shot, &expect)?;
        if matches!(op, OpKind::Fillet | OpKind::Chamfer) && record.faces.created.is_empty() {
            issues.push(format!("{op} reported success but created no faces"));
        }
        if !issues.is_empty() {
            return Err(KernelError::InvalidResult { op, issues });
        }
        if retagged {
            let faces: Vec<u64> = shot.faces.iter().map(|e| e.tag).collect();
            let edges: Vec<u64> = shot.edges.iter().map(|e| e.tag).collect();
            let vertices: Vec<u64> = shot.vertices.iter().map(|e| e.tag).collect();
            self.engine().set_tags(body.raw(), &faces, &edges, &vertices).map_err(|e| failed(op, &e))?;
        }
        self.next_tag = next_tag;
        self.next_seq += 1;

        let mut lineage: Vec<Arc<HistoryRecord>> = Vec::new();
        for input in inputs {
            for r in &self.meta(*input)?.lineage {
                if !lineage.iter().any(|x| x.seq == r.seq) {
                    lineage.push(r.clone());
                }
            }
        }
        lineage.sort_by_key(|r| r.seq);
        lineage.push(Arc::new(record.clone()));
        let profile = match &expect {
            Expect::Sheet(p) => Some(p.clone()),
            _ => None,
        };
        self.bodies.insert(body, BodyMeta { snapshot: shot, profile, lineage });
        Ok(OpResult { body, record })
    }

    /// The checks that decide whether a result is accepted, independent of the status the
    /// kernel returned.
    fn result_issues(
        &mut self,
        op: OpKind,
        body: BodyId,
        snapshot: &Shot,
        expect: &Expect,
    ) -> KernelResult<Vec<String>> {
        let mut issues = Vec::new();
        let check = self.engine.check(body.raw()).map_err(|e| failed(op, &e))?;
        issues.extend(check.issues);
        if snapshot.faces.is_empty() {
            issues.push("the result has no faces".into());
        }
        if let Expect::Sheet(_) = expect {
            if snapshot.manifold_solid {
                issues.push("a profile produced a solid".into());
            }
            if snapshot.faces.iter().map(|f| f.geom.size).sum::<f64>() <= 0.0 {
                issues.push("the profile has no area".into());
            }
            return Ok(issues);
        }
        if !snapshot.manifold_solid {
            issues.push("the result is not a closed manifold solid".into());
            return Ok(issues);
        }
        let volume = self.engine.volume(body.raw()).map_err(|e| failed(op, &e))?;
        if !(volume.is_finite() && volume > 0.0) {
            issues.push(format!("the result has volume {volume}"));
            return Ok(issues);
        }
        let tol = |v: f64| VOLUME_RTOL * v.abs().max(volume);
        match *expect {
            Expect::Volume(v) if (volume - v).abs() > tol(v) => {
                issues.push(format!("volume {volume} differs from the expected {v}"));
            }
            Expect::Union { a, b } if volume < a.max(b) - tol(a.max(b)) || volume > a + b + tol(a + b) => {
                issues.push(format!("union volume {volume} is outside [{}, {}]", a.max(b), a + b));
            }
            Expect::AtMost(v) if volume > v + tol(v) => {
                issues.push(format!("volume {volume} exceeds the input's {v}"));
            }
            Expect::Changed(v) if (volume - v).abs() <= 1e-12 * v => {
                issues.push(format!("volume {volume} is unchanged; the {op} did nothing"));
            }
            _ => {}
        }
        Ok(issues)
    }

    fn volume_of(&self, body: BodyId) -> KernelResult<f64> {
        self.engine.volume(body.raw()).map_err(|e| query_failed("volume", &e))
    }

    fn profile_of(&self, body: BodyId) -> KernelResult<Arc<ProfileData>> {
        self.meta(body)?.profile.clone().ok_or_else(|| invalid(format!("{body} is not a profile body")))
    }

    fn profile_area(&self, body: BodyId) -> KernelResult<(f64, Vec3)> {
        let s = &self.meta(body)?.snapshot;
        let area: f64 = s.faces.iter().map(|f| f.geom.size).sum();
        let mut c = Vec3::ZERO;
        for f in &s.faces {
            c = c + convert::vec3(f.geom.point) * (f.geom.size / area);
        }
        Ok((area, c))
    }
}

/// What a result must satisfy beyond being valid.
enum Expect {
    /// A planar sheet made from this profile.
    Sheet(Arc<ProfileData>),
    /// A solid of about this volume.
    Volume(f64),
    /// A union of solids with these volumes.
    Union { a: f64, b: f64 },
    /// A solid no larger than this.
    AtMost(f64),
    /// A solid whose volume differs from this.
    Changed(f64),
    /// Any valid solid.
    Solid,
}

fn failed(op: OpKind, e: &cxx::Exception) -> KernelError {
    let (code, message) = convert::exception(e);
    KernelError::Failed { op, code, message }
}

fn query_failed(query: &str, e: &cxx::Exception) -> KernelError {
    let (code, message) = convert::exception(e);
    KernelError::QueryFailed { query: query.into(), code, message }
}

fn positive(name: &str, v: f64) -> KernelResult<()> {
    if v.is_finite() && v > 0.0 { Ok(()) } else { Err(invalid(format!("{name} must be a positive number, got {v}"))) }
}

impl Kernel for SmlibKernel {
    fn backend(&self) -> BackendInfo {
        BackendInfo {
            name: "smlib".into(),
            version: format!("usd-brep {USD_BREP_VERSION} ({})", &USD_BREP_COMMIT[..12]),
        }
    }

    fn create_body_from_profile(&self, tag: &OpTag, profile: &Profile) -> KernelResult<OpResult> {
        profile.check()?;
        let tag = tag.clone();
        let (curves, loop_sizes) = convert::profile_curves(profile);
        let data = Arc::new(ProfileData { plane: profile.plane, curves, loop_sizes });
        self.call(move |s| {
            let frame = convert::frame(&data.plane);
            let handle = s
                .engine()
                .make_profile(&data.curves, &data.loop_sizes, &frame)
                .map_err(|e| failed(OpKind::Profile, &e))?;
            s.finish(OpKind::Profile, &tag, &[], handle, Expect::Sheet(data), Roles::Region)
        })
    }

    fn extrude(&self, tag: &OpTag, profile: BodyId, params: &Extrude) -> KernelResult<OpResult> {
        positive("extrude distance", params.distance)?;
        let (tag, params) = (tag.clone(), *params);
        self.call(move |s| {
            let data = s.profile_of(profile)?;
            let plane = data.plane;
            let dir = match params.direction {
                Some(d) => d.normalized().ok_or_else(|| invalid("extrude direction is zero"))?,
                None => plane.normal,
            };
            let local = convert::to_local_dir(&plane, dir);
            if local.z.abs() < 1e-9 {
                return Err(invalid("extrude direction lies in the profile plane"));
            }
            let (area, _) = s.profile_area(profile)?;
            let expected = area * params.distance * local.z.abs();
            let frame = convert::frame(&plane);
            let handle = s
                .engine()
                .extrude(&data.curves, &data.loop_sizes, &frame, &convert::v3(local), params.distance)
                .map_err(|e| failed(OpKind::Extrude, &e))?;
            let roles = Roles::Extrude {
                plane,
                curves: data.curves.clone(),
                loop_sizes: data.loop_sizes.clone(),
                local_dir: local,
                distance: params.distance,
            };
            s.finish(OpKind::Extrude, &tag, &[profile], handle, Expect::Volume(expected), roles)
        })
    }

    fn revolve(&self, tag: &OpTag, profile: BodyId, params: &Revolve) -> KernelResult<OpResult> {
        positive("revolve angle", params.angle_deg)?;
        if params.angle_deg > 360.0 {
            return Err(invalid("revolve angle must be at most 360 degrees"));
        }
        let (tag, params) = (tag.clone(), *params);
        self.call(move |s| {
            let data = s.profile_of(profile)?;
            let plane = data.plane;
            let axis = params.axis_direction.normalized().ok_or_else(|| invalid("revolve axis is zero"))?;
            if axis.dot(plane.normal).abs() > 1e-9 {
                return Err(invalid("revolve axis must lie in the profile plane"));
            }
            // Pappus: volume = area x distance the centroid travels.
            let (area, centroid) = s.profile_area(profile)?;
            let rel = centroid - params.axis_origin;
            let radius = (rel - axis * rel.dot(axis)).length();
            let expected = area * radius * params.angle_deg.to_radians();
            let frame = convert::frame(&plane);
            let origin = convert::v3(convert::to_local_point(&plane, params.axis_origin));
            let local_axis = convert::v3(convert::to_local_dir(&plane, axis));
            let handle = s
                .engine()
                .revolve(&data.curves, &data.loop_sizes, &frame, &origin, &local_axis, params.angle_deg)
                .map_err(|e| failed(OpKind::Revolve, &e))?;
            let roles = Roles::Revolve {
                plane,
                curves: data.curves.clone(),
                loop_sizes: data.loop_sizes.clone(),
                origin: params.axis_origin,
                axis,
                angle_deg: params.angle_deg,
            };
            s.finish(OpKind::Revolve, &tag, &[profile], handle, Expect::Volume(expected), roles)
        })
    }

    fn boolean(&self, tag: &OpTag, op: BooleanOp, target: BodyId, tool: BodyId) -> KernelResult<OpResult> {
        let tag = tag.clone();
        self.call(move |s| {
            s.solid(target, "boolean target")?;
            s.solid(tool, "boolean tool")?;
            if target == tool {
                return Err(invalid("boolean target and tool are the same body"));
            }
            let (va, vb) = (s.volume_of(target)?, s.volume_of(tool)?);
            let (code, expect) = match op {
                BooleanOp::Union => (0, Expect::Union { a: va, b: vb }),
                BooleanOp::Subtract => (1, Expect::AtMost(va)),
                BooleanOp::Intersect => (2, Expect::AtMost(va.min(vb))),
            };
            let handle = s.engine().boolean_op(code, target.raw(), tool.raw()).map_err(|e| failed(op.kind(), &e))?;
            s.finish(op.kind(), &tag, &[target, tool], handle, expect, Roles::Other)
        })
    }

    fn fillet(&self, tag: &OpTag, body: BodyId, edges: &[EdgeId], radius: f64) -> KernelResult<OpResult> {
        blend(self, tag, body, edges, radius, false)
    }

    fn chamfer(&self, tag: &OpTag, body: BodyId, edges: &[EdgeId], distance: f64) -> KernelResult<OpResult> {
        blend(self, tag, body, edges, distance, true)
    }

    fn shell(&self, tag: &OpTag, body: BodyId, open_faces: &[FaceId], thickness: f64) -> KernelResult<OpResult> {
        positive("shell thickness", thickness)?;
        let (tag, faces) = (tag.clone(), open_faces.to_vec());
        self.call(move |s| {
            let meta = s.solid(body, "shell input")?;
            for f in &faces {
                if !meta.snapshot.faces.iter().any(|r| r.tag == f.raw()) {
                    return Err(KernelError::UnknownFace { body, face: *f });
                }
            }
            let before = s.volume_of(body)?;
            let tags: Vec<u64> = faces.iter().map(|f| f.raw()).collect();
            let handle = s.engine().shell(body.raw(), &tags, thickness).map_err(|e| failed(OpKind::Shell, &e))?;
            let expect = if before > 0.0 { Expect::AtMost(before) } else { Expect::Solid };
            s.finish(OpKind::Shell, &tag, &[body], handle, expect, Roles::Offset)
        })
    }

    fn tessellate(&self, body: BodyId, params: &TessellationParams) -> KernelResult<Mesh> {
        let p = convert::tess_params(params);
        self.call(move |s| {
            s.meta(body)?;
            s.engine.tessellate(body.raw(), &p).map(convert::mesh).map_err(|e| query_failed("tessellate", &e))
        })
    }

    fn tessellate_levels(&self, body: BodyId, levels: &[TessellationParams]) -> KernelResult<Vec<Mesh>> {
        let p: Vec<_> = levels.iter().map(convert::tess_params).collect();
        self.call(move |s| {
            s.meta(body)?;
            let meshes = s.engine.tessellate_levels(body.raw(), &p).map_err(|e| query_failed("tessellate", &e))?;
            Ok(meshes.into_iter().map(convert::mesh).collect())
        })
    }

    fn tessellate_edges(&self, body: BodyId, params: &TessellationParams) -> KernelResult<Vec<EdgePolyline>> {
        let (angle, chord) = (params.curve_angle_deg, params.chord_height);
        self.call(move |s| {
            s.meta(body)?;
            s.engine
                .sample_edges(body.raw(), angle, chord)
                .map(convert::polylines)
                .map_err(|e| query_failed("tessellate_edges", &e))
        })
    }

    fn mass_properties(&self, body: BodyId) -> KernelResult<MassProperties> {
        self.call(move |s| {
            s.meta(body)?;
            s.engine.mass(body.raw()).map(convert::mass).map_err(|e| query_failed("mass_properties", &e))
        })
    }

    fn validate(&self, body: BodyId) -> KernelResult<Validation> {
        self.call(move |s| {
            s.meta(body)?;
            let check = s.engine.check(body.raw()).map_err(|e| query_failed("validate", &e))?;
            let kind = if check.manifold_solid { BodyKind::Solid } else { BodyKind::Sheet };
            let mut issues = check.issues;
            if check.face_count == 0 {
                issues.push("the body has no faces".into());
            }
            if kind == BodyKind::Solid {
                let v = s.volume_of(body)?;
                if !(v.is_finite() && v > 0.0) {
                    issues.push(format!("the solid has volume {v}"));
                }
            }
            Ok(Validation {
                valid: check.assert_valid && issues.is_empty(),
                kind,
                manifold_solid: check.manifold_solid,
                issues,
            })
        })
    }

    fn topology(&self, body: BodyId) -> KernelResult<Topology> {
        self.call(move |s| Ok(convert::topology(&s.meta(body)?.snapshot)))
    }

    fn history(&self, body: BodyId) -> KernelResult<Vec<HistoryRecord>> {
        self.call(move |s| Ok(s.meta(body)?.lineage.iter().map(|r| (**r).clone()).collect()))
    }

    fn release(&self, body: BodyId) -> KernelResult<()> {
        self.call(move |s| {
            s.bodies.remove(&body).ok_or(KernelError::UnknownBody(body))?;
            s.engine().release(body.raw());
            Ok(())
        })
    }

    fn view_curves(&self, body: BodyId, spec: &ViewSpec) -> KernelResult<ViewCurves> {
        drawing::view_curves(self, body, spec)
    }

    fn silhouette_curves(&self, body: BodyId, direction: Vec3) -> KernelResult<Vec<ViewCurve>> {
        drawing::silhouette_curves(self, body, direction)
    }

    fn section_curves(&self, body: BodyId, plane: &Plane) -> KernelResult<Vec<ViewCurve>> {
        drawing::section_curves(self, body, plane)
    }

    fn edge_curve(&self, body: BodyId, edge: EdgeId) -> KernelResult<Curve3> {
        drawing::edge_curve(self, body, edge)
    }

    fn face_loops(&self, body: BodyId, face: FaceId) -> KernelResult<Vec<Vec<Curve3>>> {
        drawing::face_loops(self, body, face)
    }

    fn lines_and_arcs(&self, curve: &Curve3, tolerance: f64) -> KernelResult<Vec<Curve3>> {
        drawing::lines_and_arcs(self, curve, tolerance)
    }
}

fn blend(
    k: &SmlibKernel,
    tag: &OpTag,
    body: BodyId,
    edges: &[EdgeId],
    size: f64,
    chamfer: bool,
) -> KernelResult<OpResult> {
    let op = if chamfer { OpKind::Chamfer } else { OpKind::Fillet };
    positive(if chamfer { "chamfer distance" } else { "fillet radius" }, size)?;
    if edges.is_empty() {
        return Err(invalid(format!("{op} needs at least one edge")));
    }
    let (tag, edges) = (tag.clone(), edges.to_vec());
    k.call(move |s| {
        let meta = s.solid(body, "blend input")?;
        for e in &edges {
            if !meta.snapshot.edges.iter().any(|r| r.tag == e.raw()) {
                return Err(KernelError::UnknownEdge { body, edge: *e });
            }
        }
        let before = s.volume_of(body)?;
        let tags: Vec<u64> = edges.iter().map(|e| e.raw()).collect();
        let handle = s.engine().blend(body.raw(), &tags, size, chamfer).map_err(|e| failed(op, &e))?;
        s.finish(op, &tag, &[body], handle, Expect::Changed(before), Roles::Blend { edges: tags })
    })
}
