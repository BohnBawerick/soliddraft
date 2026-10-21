//! The kernel trait's contract, checked through the protocol on the SMLib backend: every
//! call and reply goes through JSON and a session.

use soliddraft_kernel_api::contract;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use soliddraft_kernel_api::{BodyId, Kernel, KernelError, OpTag, Plane, Point2, Profile, TessellationParams, Vec3};
use soliddraft_kernel_remote::{LocalTransport, RemoteKernel, Request, Session, Transport};
use soliddraft_kernel_smlib::SmlibKernel;

fn remote() -> RemoteKernel {
    RemoteKernel::new(Box::new(LocalTransport::new(Box::new(SmlibKernel::new()))))
}

#[test]
fn operations_build_what_they_say() {
    contract::operations_build_what_they_say(&remote());
}

#[test]
fn operations_leave_inputs_untouched() {
    contract::operations_leave_inputs_untouched(&remote());
}

#[test]
fn failed_operations_return_errors() {
    contract::failed_operations_return_errors(&remote());
}

#[test]
fn history_tracks_entities() {
    contract::history_tracks_entities(&remote());
}

#[test]
fn tessellation_is_labelled() {
    contract::tessellation_is_labelled(&remote());
}

#[test]
fn tessellation_controls_refine() {
    contract::tessellation_controls_refine(&remote());
}

#[test]
fn tessellation_levels_match_single_calls() {
    contract::tessellation_levels_match_single_calls(&remote());
}

/// Counts the requests that cross the transport.
struct Counting {
    inner: LocalTransport,
    calls: Arc<AtomicUsize>,
}

impl Transport for Counting {
    fn call(&self, request: &str) -> Result<String, String> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.inner.call(request)
    }
}

#[test]
fn tessellation_levels_are_one_round_trip() {
    let calls = Arc::new(AtomicUsize::new(0));
    let inner = LocalTransport::new(Box::new(SmlibKernel::new()));
    let k = RemoteKernel::new(Box::new(Counting { inner, calls: calls.clone() }));
    let c = contract::cylinder(&k, Vec3::new(0.0, 0.0, 0.0), 5.0, 10.0);
    let levels = [0.2, 0.05, 0.0125].map(|chord_height| TessellationParams { chord_height, ..Default::default() });
    let before = calls.load(Ordering::Relaxed);
    assert_eq!(k.tessellate_levels(c, &levels).unwrap().len(), 3);
    assert_eq!(calls.load(Ordering::Relaxed), before + 1);
}

#[test]
fn mass_properties_match_closed_form() {
    contract::mass_properties_match_closed_form(&remote());
}

#[test]
fn vertices_have_ids_that_survive() {
    contract::vertices_have_ids_that_survive(&remote());
}

#[test]
fn edges_sample_along_their_curves() {
    contract::edges_sample_along_their_curves(&remote());
}

#[test]
fn created_faces_have_roles() {
    contract::created_faces_have_roles(&remote());
}

#[test]
fn view_curves_are_tagged() {
    contract::view_curves_are_tagged(&remote());
}

#[test]
fn edge_curves_are_exact() {
    contract::edge_curves_are_exact(&remote());
}

#[test]
fn sessions_only_see_their_own_bodies() {
    let kernel = SmlibKernel::new();
    let mut alice = Session::new();
    let mut bob = Session::new();
    let ask = |session: &mut Session, request: Request| -> Result<serde_json::Value, KernelError> {
        serde_json::from_str(&session.handle(&kernel, &serde_json::to_string(&request).unwrap())).unwrap()
    };
    let profile = Profile::circle(Plane::XY, Point2::new(0.0, 0.0), 5.0);
    let made = ask(&mut alice, Request::CreateBodyFromProfile { tag: OpTag::new("s"), profile }).expect("profile");
    let body = BodyId::from_raw(made["body"].as_u64().unwrap());
    assert_eq!(alice.body_count(), 1);

    assert_eq!(ask(&mut bob, Request::Topology { body }), Err(KernelError::UnknownBody(body)));
    assert!(ask(&mut alice, Request::Topology { body }).is_ok());

    alice.release_all(&kernel);
    assert_eq!(alice.body_count(), 0);
    assert!(matches!(kernel.topology(body), Err(KernelError::UnknownBody(_))));
}
