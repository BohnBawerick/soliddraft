//! A kernel that writes down the work asked of it, for tests that check how much a
//! change rebuilds: the tag of every operation and the body of every tessellation.
//! It wraps another backend and answers exactly as that backend does.
//!
//! Enabled by the `counting` feature. The counting lives in the tests and bundle writers
//! that use it; the code that builds a part has none.

use std::sync::{Arc, Mutex};

use crate::{
    BackendInfo, BodyId, BooleanOp, Draft, EdgeId, EdgePolyline, Extrude, ExtrudeLimit, FaceId, HistoryRecord, Hole,
    Kernel, KernelResult, MassProperties, Mesh, Mirror, OpResult, OpTag, PatternLayout, Plane, Profile, Revolve,
    TessellationParams, Topology, Validation, Vec3, drawing,
};

/// One piece of work a kernel was asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Work {
    /// An operation that makes a body, by its tag.
    Operation(String),
    /// A triangulation of a body: `tessellate`, `tessellate_levels` or
    /// `tessellate_edges`.
    Mesh(BodyId),
}

/// `inner`, with the work asked of it written down.
pub struct Counting {
    inner: Arc<dyn Kernel>,
    work: Mutex<Vec<Work>>,
}

impl Counting {
    /// Wraps `inner`.
    pub fn new(inner: Arc<dyn Kernel>) -> Counting {
        Counting { inner, work: Mutex::default() }
    }

    /// The work asked for since the last call, oldest first.
    pub fn take(&self) -> Vec<Work> {
        std::mem::take(&mut *self.work.lock().expect("work log lock"))
    }

    fn note(&self, work: Work) {
        self.work.lock().expect("work log lock").push(work);
    }

    fn op(&self, tag: &OpTag) {
        self.note(Work::Operation(tag.as_str().to_owned()));
    }
}

impl Kernel for Counting {
    fn backend(&self) -> BackendInfo {
        self.inner.backend()
    }

    fn create_body_from_profile(&self, tag: &OpTag, profile: &Profile) -> KernelResult<OpResult> {
        self.op(tag);
        self.inner.create_body_from_profile(tag, profile)
    }

    fn extrude(&self, tag: &OpTag, profile: BodyId, params: &Extrude) -> KernelResult<OpResult> {
        self.op(tag);
        self.inner.extrude(tag, profile, params)
    }

    fn revolve(&self, tag: &OpTag, profile: BodyId, params: &Revolve) -> KernelResult<OpResult> {
        self.op(tag);
        self.inner.revolve(tag, profile, params)
    }

    fn boolean(&self, tag: &OpTag, op: BooleanOp, target: BodyId, tool: BodyId) -> KernelResult<OpResult> {
        self.op(tag);
        self.inner.boolean(tag, op, target, tool)
    }

    fn fillet(&self, tag: &OpTag, body: BodyId, edges: &[EdgeId], radius: f64) -> KernelResult<OpResult> {
        self.op(tag);
        self.inner.fillet(tag, body, edges, radius)
    }

    fn chamfer(&self, tag: &OpTag, body: BodyId, edges: &[EdgeId], distance: f64) -> KernelResult<OpResult> {
        self.op(tag);
        self.inner.chamfer(tag, body, edges, distance)
    }

    fn shell(&self, tag: &OpTag, body: BodyId, open_faces: &[FaceId], thickness: f64) -> KernelResult<OpResult> {
        self.op(tag);
        self.inner.shell(tag, body, open_faces, thickness)
    }

    fn extrude_to(
        &self,
        tag: &OpTag,
        profile: BodyId,
        target: BodyId,
        direction: Option<Vec3>,
        limit: &ExtrudeLimit,
    ) -> KernelResult<OpResult> {
        self.op(tag);
        self.inner.extrude_to(tag, profile, target, direction, limit)
    }

    fn hole(&self, tag: &OpTag, body: BodyId, params: &Hole) -> KernelResult<OpResult> {
        self.op(tag);
        self.inner.hole(tag, body, params)
    }

    fn pattern(&self, tag: &OpTag, body: BodyId, layout: &PatternLayout) -> KernelResult<OpResult> {
        self.op(tag);
        self.inner.pattern(tag, body, layout)
    }

    fn mirror(&self, tag: &OpTag, body: BodyId, params: &Mirror) -> KernelResult<OpResult> {
        self.op(tag);
        self.inner.mirror(tag, body, params)
    }

    fn draft(&self, tag: &OpTag, body: BodyId, faces: &[FaceId], params: &Draft) -> KernelResult<OpResult> {
        self.op(tag);
        self.inner.draft(tag, body, faces, params)
    }

    fn tessellate(&self, body: BodyId, params: &TessellationParams) -> KernelResult<Mesh> {
        self.note(Work::Mesh(body));
        self.inner.tessellate(body, params)
    }

    fn tessellate_levels(&self, body: BodyId, levels: &[TessellationParams]) -> KernelResult<Vec<Mesh>> {
        self.note(Work::Mesh(body));
        self.inner.tessellate_levels(body, levels)
    }

    fn tessellate_edges(&self, body: BodyId, params: &TessellationParams) -> KernelResult<Vec<EdgePolyline>> {
        self.note(Work::Mesh(body));
        self.inner.tessellate_edges(body, params)
    }

    fn preview(&self, body: BodyId, params: &TessellationParams) -> KernelResult<Mesh> {
        self.note(Work::Mesh(body));
        self.inner.preview(body, params)
    }

    fn preview_levels(&self, body: BodyId, levels: &[TessellationParams]) -> KernelResult<Vec<Mesh>> {
        self.note(Work::Mesh(body));
        self.inner.preview_levels(body, levels)
    }

    fn preview_edges(&self, body: BodyId, params: &TessellationParams) -> KernelResult<Vec<EdgePolyline>> {
        self.note(Work::Mesh(body));
        self.inner.preview_edges(body, params)
    }

    fn mass_properties(&self, body: BodyId) -> KernelResult<MassProperties> {
        self.inner.mass_properties(body)
    }

    fn cached_mass_properties(&self, body: BodyId) -> Option<MassProperties> {
        self.inner.cached_mass_properties(body)
    }

    fn validate(&self, body: BodyId) -> KernelResult<Validation> {
        self.inner.validate(body)
    }

    fn topology(&self, body: BodyId) -> KernelResult<Topology> {
        self.inner.topology(body)
    }

    fn measured_topology(&self, body: BodyId) -> KernelResult<Topology> {
        self.inner.measured_topology(body)
    }

    fn history(&self, body: BodyId) -> KernelResult<Vec<HistoryRecord>> {
        self.inner.history(body)
    }

    fn release(&self, body: BodyId) -> KernelResult<()> {
        self.inner.release(body)
    }

    fn retained_bytes(&self, body: BodyId) -> KernelResult<u64> {
        self.inner.retained_bytes(body)
    }

    fn view_curves(&self, body: BodyId, spec: &drawing::ViewSpec) -> KernelResult<drawing::ViewCurves> {
        self.inner.view_curves(body, spec)
    }

    fn silhouette_curves(&self, body: BodyId, direction: Vec3) -> KernelResult<Vec<drawing::ViewCurve>> {
        self.inner.silhouette_curves(body, direction)
    }

    fn section_curves(&self, body: BodyId, plane: &Plane) -> KernelResult<Vec<drawing::ViewCurve>> {
        self.inner.section_curves(body, plane)
    }

    fn section_view(&self, body: BodyId, spec: &drawing::SectionSpec) -> KernelResult<drawing::SectionView> {
        self.inner.section_view(body, spec)
    }

    fn edge_curve(&self, body: BodyId, edge: EdgeId) -> KernelResult<drawing::Curve3> {
        self.inner.edge_curve(body, edge)
    }

    fn face_loops(&self, body: BodyId, face: FaceId) -> KernelResult<Vec<Vec<drawing::Curve3>>> {
        self.inner.face_loops(body, face)
    }

    fn lines_and_arcs(&self, curve: &drawing::Curve3, tolerance: f64) -> KernelResult<Vec<drawing::Curve3>> {
        self.inner.lines_and_arcs(curve, tolerance)
    }
}
