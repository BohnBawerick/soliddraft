//! The kernel trait over a message transport.
//!
//! The web build of the app runs in a browser, where the C++ kernel cannot run, so it
//! reaches a kernel in another place: a server on another machine, or a kernel compiled
//! to its own WebAssembly module next to the app. Both speak the protocol here.
//!
//! - [`Request`] is one kernel call as JSON. The reply is the call's
//!   `Result<T, KernelError>` as JSON, with serde's `{"Ok": ...}` or `{"Err": ...}`.
//! - [`RemoteKernel`] implements [`Kernel`] by sending each call through a [`Transport`]
//!   and waiting for the reply.
//! - [`Session`] answers requests on the kernel's side and keeps track of the bodies one
//!   client made, so a client can only use its own bodies and they can all be released
//!   when the client goes away.

use std::collections::HashSet;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use soliddraft_kernel_api::drawing::{Curve3, ViewCurve, ViewCurves, ViewSpec};
use soliddraft_kernel_api::{
    BackendInfo, BodyId, BooleanOp, EdgeId, EdgePolyline, Extrude, FaceId, HistoryRecord, Kernel, KernelError,
    KernelResult, MassProperties, Mesh, OpResult, OpTag, Plane, Profile, Revolve, TessellationParams, Topology,
    Validation, Vec3,
};

/// One kernel call. Each variant carries the arguments of the [`Kernel`] method of the
/// same name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "call", rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum Request {
    Backend,
    CreateBodyFromProfile { tag: OpTag, profile: Profile },
    Extrude { tag: OpTag, profile: BodyId, params: Extrude },
    Revolve { tag: OpTag, profile: BodyId, params: Revolve },
    Boolean { tag: OpTag, op: BooleanOp, target: BodyId, tool: BodyId },
    Fillet { tag: OpTag, body: BodyId, edges: Vec<EdgeId>, radius: f64 },
    Chamfer { tag: OpTag, body: BodyId, edges: Vec<EdgeId>, distance: f64 },
    Shell { tag: OpTag, body: BodyId, open_faces: Vec<FaceId>, thickness: f64 },
    Tessellate { body: BodyId, params: TessellationParams },
    TessellateLevels { body: BodyId, levels: Vec<TessellationParams> },
    TessellateEdges { body: BodyId, params: TessellationParams },
    MassProperties { body: BodyId },
    Validate { body: BodyId },
    Topology { body: BodyId },
    History { body: BodyId },
    Release { body: BodyId },
    ViewCurves { body: BodyId, spec: ViewSpec },
    SilhouetteCurves { body: BodyId, direction: Vec3 },
    SectionCurves { body: BodyId, plane: Plane },
    EdgeCurve { body: BodyId, edge: EdgeId },
    FaceLoops { body: BodyId, face: FaceId },
    LinesAndArcs { curve: Curve3, tolerance: f64 },
}

impl Request {
    /// Whether the call makes a new body, returned in an [`OpResult`].
    pub fn creates_body(&self) -> bool {
        use Request::*;
        matches!(
            self,
            CreateBodyFromProfile { .. }
                | Extrude { .. }
                | Revolve { .. }
                | Boolean { .. }
                | Fillet { .. }
                | Chamfer { .. }
                | Shell { .. }
        )
    }

    /// The bodies the call reads.
    pub fn bodies(&self) -> Vec<BodyId> {
        use Request::*;
        match self {
            Backend | CreateBodyFromProfile { .. } | LinesAndArcs { .. } => Vec::new(),
            Boolean { target, tool, .. } => vec![*target, *tool],
            Extrude { profile, .. } | Revolve { profile, .. } => vec![*profile],
            Fillet { body, .. }
            | Chamfer { body, .. }
            | Shell { body, .. }
            | Tessellate { body, .. }
            | TessellateLevels { body, .. }
            | TessellateEdges { body, .. }
            | MassProperties { body }
            | Validate { body }
            | Topology { body }
            | History { body }
            | Release { body }
            | ViewCurves { body, .. }
            | SilhouetteCurves { body, .. }
            | SectionCurves { body, .. }
            | EdgeCurve { body, .. }
            | FaceLoops { body, .. } => vec![*body],
        }
    }
}

/// Runs one request on `kernel` and returns its result as JSON.
pub fn dispatch(kernel: &dyn Kernel, request: Request) -> KernelResult<Value> {
    use Request::*;
    fn json<T: Serialize>(value: T) -> KernelResult<Value> {
        serde_json::to_value(value).map_err(|e| KernelError::Unavailable(format!("encoding a reply: {e}")))
    }
    match request {
        Backend => json(kernel.backend()),
        CreateBodyFromProfile { tag, profile } => json(kernel.create_body_from_profile(&tag, &profile)?),
        Extrude { tag, profile, params } => json(kernel.extrude(&tag, profile, &params)?),
        Revolve { tag, profile, params } => json(kernel.revolve(&tag, profile, &params)?),
        Boolean { tag, op, target, tool } => json(kernel.boolean(&tag, op, target, tool)?),
        Fillet { tag, body, edges, radius } => json(kernel.fillet(&tag, body, &edges, radius)?),
        Chamfer { tag, body, edges, distance } => json(kernel.chamfer(&tag, body, &edges, distance)?),
        Shell { tag, body, open_faces, thickness } => json(kernel.shell(&tag, body, &open_faces, thickness)?),
        Tessellate { body, params } => json(kernel.tessellate(body, &params)?),
        TessellateLevels { body, levels } => json(kernel.tessellate_levels(body, &levels)?),
        TessellateEdges { body, params } => json(kernel.tessellate_edges(body, &params)?),
        MassProperties { body } => json(kernel.mass_properties(body)?),
        Validate { body } => json(kernel.validate(body)?),
        Topology { body } => json(kernel.topology(body)?),
        History { body } => json(kernel.history(body)?),
        Release { body } => json(kernel.release(body)?),
        ViewCurves { body, spec } => json(kernel.view_curves(body, &spec)?),
        SilhouetteCurves { body, direction } => json(kernel.silhouette_curves(body, direction)?),
        SectionCurves { body, plane } => json(kernel.section_curves(body, &plane)?),
        EdgeCurve { body, edge } => json(kernel.edge_curve(body, edge)?),
        FaceLoops { body, face } => json(kernel.face_loops(body, face)?),
        LinesAndArcs { curve, tolerance } => json(kernel.lines_and_arcs(&curve, tolerance)?),
    }
}

/// Encodes a reply.
fn reply(result: KernelResult<Value>) -> String {
    serde_json::to_string(&result).unwrap_or_else(|e| {
        serde_json::to_string(&KernelResult::<Value>::Err(KernelError::Unavailable(format!("encoding a reply: {e}"))))
            .expect("an error reply encodes")
    })
}

/// Answers one client's requests and remembers which bodies it owns.
///
/// A request that names a body the client did not make is refused with
/// [`KernelError::UnknownBody`], as if the body did not exist, so clients sharing one
/// kernel cannot see or free each other's bodies.
#[derive(Debug, Default)]
pub struct Session {
    owned: HashSet<BodyId>,
}

impl Session {
    /// A session that owns no bodies yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of bodies the client holds.
    pub fn body_count(&self) -> usize {
        self.owned.len()
    }

    /// Answers one request, given and returned as JSON text.
    pub fn handle(&mut self, kernel: &dyn Kernel, request: &str) -> String {
        let request: Request = match serde_json::from_str(request) {
            Ok(r) => r,
            Err(e) => return reply(Err(KernelError::InvalidInput(format!("bad kernel request: {e}")))),
        };
        reply(self.run(kernel, request))
    }

    fn run(&mut self, kernel: &dyn Kernel, request: Request) -> KernelResult<Value> {
        if let Some(body) = request.bodies().into_iter().find(|b| !self.owned.contains(b)) {
            return Err(KernelError::UnknownBody(body));
        }
        let released = match &request {
            Request::Release { body } => Some(*body),
            _ => None,
        };
        let creates = request.creates_body();
        let value = dispatch(kernel, request)?;
        if let Some(body) = released {
            self.owned.remove(&body);
        } else if creates {
            let made: OpResult = serde_json::from_value(value.clone())
                .map_err(|e| KernelError::Unavailable(format!("reading an operation's result: {e}")))?;
            self.owned.insert(made.body);
        }
        Ok(value)
    }

    /// Releases every body the client still holds.
    pub fn release_all(&mut self, kernel: &dyn Kernel) {
        for body in self.owned.drain() {
            let _ = kernel.release(body);
        }
    }
}

/// Carries one request to a kernel and brings its reply back.
pub trait Transport: Send + Sync {
    /// Sends a request and waits for the reply, both JSON text. An `Err` means the
    /// request did not get an answer, such as a broken connection.
    fn call(&self, request: &str) -> Result<String, String>;
}

/// A transport that answers in the same process, through a [`Session`] on a local
/// kernel. Tests use it to run the trait's contract through the protocol.
pub struct LocalTransport {
    kernel: Box<dyn Kernel>,
    session: std::sync::Mutex<Session>,
}

impl LocalTransport {
    /// A transport onto `kernel`.
    pub fn new(kernel: Box<dyn Kernel>) -> Self {
        LocalTransport { kernel, session: std::sync::Mutex::new(Session::new()) }
    }
}

impl Transport for LocalTransport {
    fn call(&self, request: &str) -> Result<String, String> {
        let mut session = self.session.lock().map_err(|_| "the session lock is poisoned".to_owned())?;
        Ok(session.handle(self.kernel.as_ref(), request))
    }
}

/// A kernel reached through a [`Transport`].
pub struct RemoteKernel {
    transport: Box<dyn Transport>,
}

impl RemoteKernel {
    /// A kernel that sends its calls through `transport`.
    pub fn new(transport: Box<dyn Transport>) -> Self {
        RemoteKernel { transport }
    }

    fn call<T: DeserializeOwned>(&self, request: Request) -> KernelResult<T> {
        let text = serde_json::to_string(&request)
            .map_err(|e| KernelError::InvalidInput(format!("encoding a kernel request: {e}")))?;
        let answer = self.transport.call(&text).map_err(KernelError::Unavailable)?;
        serde_json::from_str::<KernelResult<T>>(&answer)
            .map_err(|e| KernelError::Unavailable(format!("bad reply from the kernel: {e}")))?
    }
}

impl Kernel for RemoteKernel {
    fn backend(&self) -> BackendInfo {
        self.call(Request::Backend)
            .unwrap_or_else(|e| BackendInfo { name: "remote".into(), version: format!("unavailable: {e}") })
    }

    fn create_body_from_profile(&self, tag: &OpTag, profile: &Profile) -> KernelResult<OpResult> {
        self.call(Request::CreateBodyFromProfile { tag: tag.clone(), profile: profile.clone() })
    }

    fn extrude(&self, tag: &OpTag, profile: BodyId, params: &Extrude) -> KernelResult<OpResult> {
        self.call(Request::Extrude { tag: tag.clone(), profile, params: *params })
    }

    fn revolve(&self, tag: &OpTag, profile: BodyId, params: &Revolve) -> KernelResult<OpResult> {
        self.call(Request::Revolve { tag: tag.clone(), profile, params: *params })
    }

    fn boolean(&self, tag: &OpTag, op: BooleanOp, target: BodyId, tool: BodyId) -> KernelResult<OpResult> {
        self.call(Request::Boolean { tag: tag.clone(), op, target, tool })
    }

    fn fillet(&self, tag: &OpTag, body: BodyId, edges: &[EdgeId], radius: f64) -> KernelResult<OpResult> {
        self.call(Request::Fillet { tag: tag.clone(), body, edges: edges.to_vec(), radius })
    }

    fn chamfer(&self, tag: &OpTag, body: BodyId, edges: &[EdgeId], distance: f64) -> KernelResult<OpResult> {
        self.call(Request::Chamfer { tag: tag.clone(), body, edges: edges.to_vec(), distance })
    }

    fn shell(&self, tag: &OpTag, body: BodyId, open_faces: &[FaceId], thickness: f64) -> KernelResult<OpResult> {
        self.call(Request::Shell { tag: tag.clone(), body, open_faces: open_faces.to_vec(), thickness })
    }

    fn tessellate(&self, body: BodyId, params: &TessellationParams) -> KernelResult<Mesh> {
        self.call(Request::Tessellate { body, params: *params })
    }

    fn tessellate_levels(&self, body: BodyId, levels: &[TessellationParams]) -> KernelResult<Vec<Mesh>> {
        self.call(Request::TessellateLevels { body, levels: levels.to_vec() })
    }

    fn tessellate_edges(&self, body: BodyId, params: &TessellationParams) -> KernelResult<Vec<EdgePolyline>> {
        self.call(Request::TessellateEdges { body, params: *params })
    }

    fn mass_properties(&self, body: BodyId) -> KernelResult<MassProperties> {
        self.call(Request::MassProperties { body })
    }

    fn validate(&self, body: BodyId) -> KernelResult<Validation> {
        self.call(Request::Validate { body })
    }

    fn topology(&self, body: BodyId) -> KernelResult<Topology> {
        self.call(Request::Topology { body })
    }

    fn history(&self, body: BodyId) -> KernelResult<Vec<HistoryRecord>> {
        self.call(Request::History { body })
    }

    fn release(&self, body: BodyId) -> KernelResult<()> {
        self.call(Request::Release { body })
    }

    fn view_curves(&self, body: BodyId, spec: &ViewSpec) -> KernelResult<ViewCurves> {
        self.call(Request::ViewCurves { body, spec: *spec })
    }

    fn silhouette_curves(&self, body: BodyId, direction: Vec3) -> KernelResult<Vec<ViewCurve>> {
        self.call(Request::SilhouetteCurves { body, direction })
    }

    fn section_curves(&self, body: BodyId, plane: &Plane) -> KernelResult<Vec<ViewCurve>> {
        self.call(Request::SectionCurves { body, plane: *plane })
    }

    fn edge_curve(&self, body: BodyId, edge: EdgeId) -> KernelResult<Curve3> {
        self.call(Request::EdgeCurve { body, edge })
    }

    fn face_loops(&self, body: BodyId, face: FaceId) -> KernelResult<Vec<Vec<Curve3>>> {
        self.call(Request::FaceLoops { body, face })
    }

    fn lines_and_arcs(&self, curve: &Curve3, tolerance: f64) -> KernelResult<Vec<Curve3>> {
        self.call(Request::LinesAndArcs { curve: curve.clone(), tolerance })
    }
}
