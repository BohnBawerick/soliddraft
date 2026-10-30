use soliddraft_kernel_api::*;
use soliddraft_kernel_catallax::CatallaxKernel;
use std::sync::{Condvar, Mutex};
/// Catallax, with extrusions of one depth held back and others refused on request.
#[derive(Default)]
pub struct Gated {
    inner: CatallaxKernel,
    pub rules: Mutex<Rules>,
    pub changed: Condvar,
}

#[derive(Default)]
pub struct Rules {
    /// Extrusions this deep wait until the hold is lifted.
    pub hold: Option<f64>,
    pub hold_next: bool,
    /// True while an extrusion waits on the hold.
    pub holding: Option<f64>,
    /// Extrusions this deep fail.
    pub refuse: Vec<f64>,
}
impl Kernel for Gated {
    fn backend(&self) -> BackendInfo {
        self.inner.backend()
    }
    fn create_body_from_profile(&self, tag: &OpTag, profile: &Profile) -> KernelResult<OpResult> {
        self.inner.create_body_from_profile(tag, profile)
    }
    fn extrude(&self, tag: &OpTag, profile: BodyId, params: &Extrude) -> KernelResult<OpResult> {
        {
            let mut rules = self.rules.lock().unwrap();
            if rules.hold_next {
                rules.hold_next = false;
                rules.hold = Some(params.distance);
            }
            if rules.hold == Some(params.distance) {
                rules.holding = Some(params.distance);
                self.changed.notify_all();
                rules = self.changed.wait_while(rules, |r| r.hold == Some(params.distance)).unwrap();
                rules.holding = None;
            }
            if rules.refuse.contains(&params.distance) {
                return Err(KernelError::Failed {
                    op: OpKind::Extrude,
                    code: 0,
                    message: format!("the scenario refuses a depth of {}", params.distance),
                });
            }
        }
        self.inner.extrude(tag, profile, params)
    }
    fn revolve(&self, tag: &OpTag, profile: BodyId, params: &Revolve) -> KernelResult<OpResult> {
        self.inner.revolve(tag, profile, params)
    }
    fn boolean(&self, tag: &OpTag, op: BooleanOp, target: BodyId, tool: BodyId) -> KernelResult<OpResult> {
        self.inner.boolean(tag, op, target, tool)
    }
    fn fillet(&self, tag: &OpTag, body: BodyId, edges: &[EdgeId], radius: f64) -> KernelResult<OpResult> {
        self.inner.fillet(tag, body, edges, radius)
    }
    fn chamfer(&self, tag: &OpTag, body: BodyId, edges: &[EdgeId], distance: f64) -> KernelResult<OpResult> {
        self.inner.chamfer(tag, body, edges, distance)
    }
    fn shell(&self, tag: &OpTag, body: BodyId, open: &[FaceId], thickness: f64) -> KernelResult<OpResult> {
        self.inner.shell(tag, body, open, thickness)
    }
    fn construction_edit(
        &self,
        body: BodyId,
        edits: &[soliddraft_kernel_api::ScalarEdit],
        params: &TessellationParams,
    ) -> KernelResult<soliddraft_kernel_api::ConstructionEdit> {
        self.inner.construction_edit(body, edits, params)
    }
    fn reserve_proof_work(&self) -> KernelResult<Box<dyn soliddraft_kernel_api::ProofWork>> {
        self.inner.reserve_proof_work()
    }
    fn preview(&self, body: BodyId, params: &TessellationParams) -> KernelResult<Mesh> {
        self.inner.preview(body, params)
    }
    fn tessellate(&self, body: BodyId, params: &TessellationParams) -> KernelResult<Mesh> {
        self.inner.tessellate(body, params)
    }
    fn tessellate_edges(&self, body: BodyId, params: &TessellationParams) -> KernelResult<Vec<EdgePolyline>> {
        self.inner.tessellate_edges(body, params)
    }
    fn mass_properties(&self, body: BodyId) -> KernelResult<MassProperties> {
        self.inner.mass_properties(body)
    }
    fn validate(&self, body: BodyId) -> KernelResult<Validation> {
        self.inner.validate(body)
    }
    fn topology(&self, body: BodyId) -> KernelResult<Topology> {
        self.inner.topology(body)
    }
    fn edge_length(&self, body: BodyId, edge: EdgeId) -> KernelResult<Measurement> {
        self.inner.edge_length(body, edge)
    }
    fn face_area(&self, body: BodyId, face: FaceId) -> KernelResult<Measurement> {
        self.inner.face_area(body, face)
    }
    fn history(&self, body: BodyId) -> KernelResult<Vec<HistoryRecord>> {
        self.inner.history(body)
    }
    fn export_step(
        &self,
        body: BodyId,
        options: &soliddraft_kernel_api::StepOptions,
    ) -> KernelResult<soliddraft_kernel_api::StepExport> {
        self.inner.export_step(body, options)
    }
    fn release(&self, body: BodyId) -> KernelResult<()> {
        self.inner.release(body)
    }
}
