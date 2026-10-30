//! Checks every backend must pass, written against `&dyn Kernel` so each backend's test
//! suite can run them. Each check panics with a message on failure.
//!
//! Enabled by the `contract` feature.

use crate::drawing::mesh_hlr::{self, visible_length_by_source};
use crate::drawing::{Curve3, CurveSource, SectionSpec, SectionView, StandardView, ViewCurves, ViewSpec, deviation};
use crate::{
    Axis, BodyId, BooleanOp, Certainty, CurveKind, EdgeId, ExactAxisPlane, ExactRational, Extrude, FaceId, FaceRole,
    Kernel, KernelError, Loop, Measurement, OpKind, OpTag, PairEndpoint, PairQuantity, Plane, Point2, Profile, Revolve,
    Segment, SurfaceKind, TessellationParams, Topology, Vec3, VertexId,
};

fn tag(s: &str) -> OpTag {
    OpTag::new(s)
}

/// A 40 x 30 x 10 block on XY from (-20, -15, 0) to (20, 15, 10).
pub fn block(k: &dyn Kernel) -> BodyId {
    let p = k
        .create_body_from_profile(
            &tag("block_sketch"),
            &Profile::rectangle(Plane::XY, Point2::new(0.0, 0.0), 40.0, 30.0),
        )
        .expect("block profile");
    k.extrude(&tag("block"), p.body, &Extrude { distance: 10.0, direction: None }).expect("block extrude").body
}

/// A cylinder of radius `r` from `base` along +Z.
pub fn cylinder(k: &dyn Kernel, base: Vec3, r: f64, h: f64) -> BodyId {
    let p = k
        .create_body_from_profile(
            &tag("cyl_sketch"),
            &Profile::circle(Plane::XY.with_origin(base), Point2::new(0.0, 0.0), r),
        )
        .expect("cylinder profile");
    k.extrude(&tag("cyl"), p.body, &Extrude { distance: h, direction: None }).expect("cylinder extrude").body
}

/// What must not change about a body: its topology, mass properties and history.
#[derive(Debug, PartialEq)]
struct Fixed {
    topology: Topology,
    volume: f64,
    area: f64,
    history_len: usize,
}

fn fixed(k: &dyn Kernel, b: BodyId) -> Fixed {
    let m = k.mass_properties(b).expect("mass properties");
    Fixed {
        topology: k.topology(b).expect("topology"),
        volume: m.volume,
        area: m.area,
        history_len: k.history(b).expect("history").len(),
    }
}

fn top_edges(k: &dyn Kernel, b: BodyId) -> Vec<EdgeId> {
    let t = k.topology(b).unwrap();
    t.edges.iter().filter(|e| e.bbox.min.z > 10.0 - 1e-6).map(|e| e.id).collect()
}

fn face_near(t: &Topology, p: Vec3) -> FaceId {
    t.faces.iter().min_by(|a, b| a.centroid.distance(p).total_cmp(&b.centroid.distance(p))).unwrap().id
}

fn close(a: f64, b: f64, rtol: f64) -> bool {
    (a - b).abs() <= rtol * b.abs().max(1.0)
}

/// Box, cylinder, booleans, fillet, chamfer, shell and revolve give the right volumes,
/// valid bodies and the expected face counts.
pub fn operations_build_what_they_say(k: &dyn Kernel) {
    let b = block(k);
    let t = k.topology(b).unwrap();
    assert_eq!((t.faces.len(), t.edges.len(), t.vertex_count), (6, 12, 8), "block counts");
    assert!(close(k.mass_properties(b).unwrap().volume, 12000.0, 1e-6), "block volume");
    assert!(k.validate(b).unwrap().valid, "block valid");

    let c = cylinder(k, Vec3::new(10.0, 0.0, 5.0), 6.0, 20.0);
    let cv = k.mass_properties(c).unwrap().volume;
    assert!(close(cv, std::f64::consts::PI * 36.0 * 20.0, 1e-3), "cylinder volume {cv}");

    let inside = std::f64::consts::PI * 36.0 * 5.0;
    let u = k.boolean(&tag("u"), BooleanOp::Union, b, c).expect("union");
    assert!(close(k.mass_properties(u.body).unwrap().volume, 12000.0 + cv - inside, 1e-3), "union volume");
    let s = k.boolean(&tag("s"), BooleanOp::Subtract, b, c).expect("subtract");
    assert!(close(k.mass_properties(s.body).unwrap().volume, 12000.0 - inside, 1e-3), "subtract volume");
    let i = k.boolean(&tag("i"), BooleanOp::Intersect, b, c).expect("intersect");
    assert!(close(k.mass_properties(i.body).unwrap().volume, inside, 1e-3), "intersect volume");

    let f = k.fillet(&tag("f"), b, &top_edges(k, b), 2.0).expect("fillet");
    assert_eq!(k.topology(f.body).unwrap().faces.len(), 10, "fillet adds four faces");
    let fv = k.mass_properties(f.body).unwrap().volume;
    // Each rounded edge removes (1 - pi/4) r^2 of cross-section along its length.
    let removed = (1.0 - std::f64::consts::FRAC_PI_4) * 4.0 * 140.0;
    assert!(close(fv, 12000.0 - removed, 1e-2), "fillet volume {fv}");

    let ch = k.chamfer(&tag("ch"), b, &top_edges(k, b), 2.0).expect("chamfer");
    assert_eq!(k.topology(ch.body).unwrap().faces.len(), 10, "chamfer adds four faces");

    let top = face_near(&t, Vec3::new(0.0, 0.0, 10.0));
    let sh = k.shell(&tag("sh"), b, &[top], 2.0).expect("shell");
    assert!(close(k.mass_properties(sh.body).unwrap().volume, 12000.0 - 36.0 * 26.0 * 8.0, 1e-3), "shell volume");

    let section = k
        .create_body_from_profile(&tag("section"), &Profile::rectangle(Plane::ZX, Point2::new(3.0, 25.0), 6.0, 10.0))
        .unwrap();
    let r = k
        .revolve(
            &tag("r"),
            section.body,
            &Revolve { axis_origin: Vec3::ZERO, axis_direction: Vec3::Z, angle_deg: 360.0 },
        )
        .expect("revolve");
    let rv = k.mass_properties(r.body).unwrap().volume;
    assert!(close(rv, std::f64::consts::PI * 500.0 * 6.0, 1e-3), "revolve volume {rv}");

    for body in [b, c, u.body, s.body, i.body, f.body, ch.body, sh.body, r.body, section.body] {
        let v = k.validate(body).unwrap();
        assert!(v.valid, "{body} invalid: {:?}", v.issues);
        k.release(body).unwrap();
    }
}

/// Operations never change their inputs, whether they succeed or fail.
pub fn operations_leave_inputs_untouched(k: &dyn Kernel) {
    let b = block(k);
    let c = cylinder(k, Vec3::new(10.0, 0.0, 5.0), 6.0, 20.0);
    let (fb, fc) = (fixed(k, b), fixed(k, c));
    let edges = top_edges(k, b);
    let top = face_near(&k.topology(b).unwrap(), Vec3::new(0.0, 0.0, 10.0));
    let results = [
        k.boolean(&tag("u"), BooleanOp::Union, b, c),
        k.boolean(&tag("s"), BooleanOp::Subtract, b, c),
        k.fillet(&tag("f"), b, &edges, 2.0),
        k.fillet(&tag("too_big"), b, &edges, 50.0),
        k.chamfer(&tag("ch"), b, &edges, 1.0),
        k.shell(&tag("sh"), b, &[top], 2.0),
    ];
    assert_eq!(fixed(k, b), fb, "an operation changed the block");
    assert_eq!(fixed(k, c), fc, "an operation changed the cylinder");
    for r in results.into_iter().flatten() {
        k.release(r.body).unwrap();
    }
    k.release(b).unwrap();
    k.release(c).unwrap();
}

/// A failing operation returns an error, leaves the previous body exactly as it was and
/// usable, and leaves no body behind.
pub fn failed_operations_return_errors(k: &dyn Kernel) {
    let b = block(k);
    let before = fixed(k, b);
    let edges = top_edges(k, b);

    let too_big = k.fillet(&tag("too_big"), b, &edges, 50.0);
    assert!(
        matches!(too_big, Err(KernelError::Failed { op: OpKind::Fillet, .. } | KernelError::InvalidResult { .. })),
        "{too_big:?}"
    );

    // Subtracting a body from an identical one leaves nothing.
    let twin = block(k);
    let empty = k.boolean(&tag("empty"), BooleanOp::Subtract, b, twin);
    assert!(empty.is_err(), "subtracting a body from its twin must fail, got {empty:?}");

    assert!(matches!(
        k.extrude(&tag("x"), b, &Extrude { distance: -1.0, direction: None }),
        Err(KernelError::InvalidInput(_))
    ));
    assert!(matches!(k.fillet(&tag("x"), b, &[], 1.0), Err(KernelError::InvalidInput(_))));
    assert!(matches!(k.fillet(&tag("x"), b, &[EdgeId::from_raw(u64::MAX)], 1.0), Err(KernelError::UnknownEdge { .. })));
    assert!(matches!(k.boolean(&tag("x"), BooleanOp::Union, b, b), Err(KernelError::InvalidInput(_))));
    let mut open = Profile::rectangle(Plane::XY, Point2::new(0.0, 0.0), 4.0, 4.0);
    open.outer.0.pop();
    assert!(matches!(k.create_body_from_profile(&tag("x"), &open), Err(KernelError::Profile(_))));

    assert_eq!(fixed(k, b), before, "a failed operation changed the block");
    assert!(k.validate(b).unwrap().valid);
    // The block is still usable after the failures.
    let ok = k.fillet(&tag("after"), b, &edges, 2.0).expect("fillet after failures");
    k.release(ok.body).unwrap();
    k.release(twin).unwrap();
    k.release(b).unwrap();
    assert!(matches!(k.topology(b), Err(KernelError::UnknownBody(_))));
    assert!(matches!(k.release(b), Err(KernelError::UnknownBody(_))));
}

/// History records which faces and edges each operation created, modified, deleted or
/// split, keyed by the caller's tags, and ids of untouched entities survive.
pub fn history_tracks_entities(k: &dyn Kernel) {
    let b = block(k);
    let h = k.history(b).unwrap();
    assert_eq!(h.iter().map(|r| r.tag.as_str()).collect::<Vec<_>>(), ["block_sketch", "block"]);
    let ext = h.last().unwrap();
    assert_eq!((ext.op, ext.faces.created.len(), ext.edges.created.len()), (OpKind::Extrude, 6, 12));

    let t = k.topology(b).unwrap();
    let bottom = face_near(&t, Vec3::new(0.0, 0.0, 0.0));
    let top = face_near(&t, Vec3::new(0.0, 0.0, 10.0));
    let edges = top_edges(k, b);
    let f = k.fillet(&tag("Fillet1"), b, &edges, 2.0).unwrap();
    let r = &f.record;
    assert_eq!(r.tag.as_str(), "Fillet1");
    assert_eq!(r.faces.created.len(), 4, "four fillet faces");
    assert!(r.faces.modified.contains(&top), "the top face shrinks");
    assert!(!r.faces.mentions(bottom), "the bottom face is untouched");
    for e in &edges {
        assert!(r.edges.deleted.contains(e), "filleted edge {e} is replaced");
    }
    let ft = k.topology(f.body).unwrap();
    assert!(ft.faces.iter().any(|x| x.id == bottom), "the bottom face keeps its id");
    for id in &r.faces.created {
        assert!(ft.faces.iter().any(|x| x.id == *id), "created face {id} is in the result");
    }
    let lineage = k.history(f.body).unwrap();
    assert_eq!(lineage.iter().map(|r| r.tag.as_str()).collect::<Vec<_>>(), ["block_sketch", "block", "Fillet1"]);
    assert!(lineage.windows(2).all(|w| w[0].seq < w[1].seq));

    // A union's history covers both inputs and records the new intersection edge.
    let c = cylinder(k, Vec3::new(10.0, 0.0, 5.0), 6.0, 20.0);
    let u = k.boolean(&tag("Boss"), BooleanOp::Union, b, c).unwrap();
    assert_eq!(u.record.edges.created.len(), 1, "one new edge where the boss meets the top");
    assert!(u.record.faces.modified.contains(&top), "the top face gets a hole");
    let tags: Vec<String> = k.history(u.body).unwrap().iter().map(|r| r.tag.0.clone()).collect();
    assert_eq!(tags, ["block_sketch", "block", "cyl_sketch", "cyl", "Boss"]);

    for body in [b, f.body, c, u.body] {
        k.release(body).unwrap();
    }
}

/// Tessellation covers every face, labels every triangle with a face of the body and
/// gives unit normals.
pub fn tessellation_is_labelled(k: &dyn Kernel) {
    let b = block(k);
    let f = k.fillet(&tag("f"), b, &top_edges(k, b), 2.0).unwrap();
    let t = k.topology(f.body).unwrap();
    let mesh = k.tessellate(f.body, &TessellationParams::default()).unwrap();
    assert!(mesh.triangle_count() > 0);
    assert_eq!(mesh.triangle_faces.len(), mesh.triangle_count());
    assert_eq!(mesh.positions.len(), mesh.normals.len());
    assert!(mesh.indices.iter().all(|&i| (i as usize) < mesh.positions.len()));
    for face in &t.faces {
        assert!(mesh.triangle_faces.contains(&face.id), "face {} has no triangles", face.id);
    }
    assert!(mesh.triangle_faces.iter().all(|id| t.faces.iter().any(|f| f.id == *id)));
    for n in &mesh.normals {
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        assert!((len - 1.0).abs() < 1e-3, "normal length {len}");
    }
    k.release(f.body).unwrap();
    k.release(b).unwrap();
}

/// The controls refine the mesh: a chord height adds triangles on a curved face and
/// keeps every triangle edge that close to the surface, and a maximum aspect ratio is
/// accepted and never coarsens the mesh.
pub fn tessellation_controls_refine(k: &dyn Kernel) {
    let (r, chord) = (10.0, 0.005);
    let c = cylinder(k, Vec3::new(0.0, 0.0, 0.0), r, 80.0);
    let base = k.tessellate(c, &TessellationParams::default()).unwrap();
    let fine = k.tessellate(c, &TessellationParams { chord_height: chord, ..Default::default() }).unwrap();
    assert!(fine.triangle_count() > base.triangle_count(), "{} vs {}", fine.triangle_count(), base.triangle_count());
    let side = k.topology(c).unwrap().faces.iter().find(|f| f.surface == SurfaceKind::Cylinder).unwrap().id;
    for (t, face) in fine.indices.chunks(3).zip(&fine.triangle_faces) {
        if *face != side {
            continue;
        }
        let v: Vec<[f32; 3]> = t.iter().map(|&i| fine.positions[i as usize]).collect();
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            let m = [(v[a][0] + v[b][0]) as f64 / 2.0, (v[a][1] + v[b][1]) as f64 / 2.0];
            let sag = r - m[0].hypot(m[1]);
            assert!(sag <= chord + 1e-4, "edge midpoint {sag} mm inside the cylinder");
        }
    }
    // A maximum aspect ratio may split long thin triangles; it never removes any, and
    // the mesh stays labelled.
    let squat = k.tessellate(c, &TessellationParams { max_aspect_ratio: 4.0, ..Default::default() }).unwrap();
    assert!(squat.triangle_count() >= base.triangle_count(), "{} vs {}", squat.triangle_count(), base.triangle_count());
    assert_eq!(squat.triangle_faces.len(), squat.triangle_count());
    k.release(c).unwrap();
}

/// One call tessellates a body at several settings, giving per entry and in order
/// what [`Kernel::tessellate`] gives for it.
pub fn tessellation_levels_match_single_calls(k: &dyn Kernel) {
    let b = block(k);
    let f = k.fillet(&tag("f"), b, &top_edges(k, b), 3.0).unwrap();
    // Below the 0.026 mm sagitta the 15 degree angle allows on the 3 mm fillets, so the
    // chord height decides every level.
    let levels: Vec<TessellationParams> = [0.02, 0.005, 0.00125]
        .into_iter()
        .map(|chord_height| TessellationParams { chord_height, ..Default::default() })
        .collect();
    let meshes = k.tessellate_levels(f.body, &levels).unwrap();
    assert_eq!(meshes.len(), levels.len());
    for (mesh, params) in meshes.iter().zip(&levels) {
        assert_eq!(*mesh, k.tessellate(f.body, params).unwrap(), "chord {}", params.chord_height);
    }
    let counts: Vec<usize> = meshes.iter().map(|m| m.triangle_count()).collect();
    assert!(counts.windows(2).all(|w| w[0] < w[1]), "finer levels add triangles: {counts:?}");
    assert!(k.tessellate_levels(f.body, &[]).unwrap().is_empty());
    k.release(f.body).unwrap();
    assert!(matches!(k.tessellate_levels(f.body, &levels), Err(KernelError::UnknownBody(_))));
    k.release(b).unwrap();
}

/// Mass properties of the block match the closed form.
pub fn mass_properties_match_closed_form(k: &dyn Kernel) {
    let b = block(k);
    let m = k.mass_properties(b).unwrap();
    assert!(close(m.volume, 12000.0, 1e-6));
    assert!(close(m.area, 2.0 * (1200.0 + 400.0 + 300.0), 1e-6));
    assert!(m.centroid.distance(Vec3::new(0.0, 0.0, 5.0)) < 1e-6);
    let mass = 12000.0;
    let expect = [mass * (900.0 + 100.0) / 12.0, mass * (1600.0 + 100.0) / 12.0, mass * (1600.0 + 900.0) / 12.0];
    if let Some(inertia) = m.inertia {
        for (i, e) in expect.iter().enumerate() {
            assert!(close(inertia[i][i], *e, 1e-4), "I{i}{i} = {} expected {e}", inertia[i][i]);
        }
    }
    k.release(b).unwrap();
}

/// The kernel can be called from several threads at once.
pub fn concurrent_calls_are_serialised(k: &(dyn Kernel + Sync)) {
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| {
                let b = block(k);
                assert!(close(k.mass_properties(b).unwrap().volume, 12000.0, 1e-6));
                k.release(b).unwrap();
            });
        }
    });
}

fn near(a: Vec3, b: Vec3, tol: f64) -> bool {
    a.distance(b) <= tol
}

/// Vertices have positions, know their edges, and keep their ids through operations
/// that leave them alone.
pub fn vertices_have_ids_that_survive(k: &dyn Kernel) {
    let b = block(k);
    let t = k.topology(b).unwrap();
    assert_eq!((t.vertices.len(), t.vertex_count), (8, 8), "a block has eight vertices");
    for v in &t.vertices {
        let p = v.position;
        assert!((p.x.abs() - 20.0).abs() < 1e-9 && (p.y.abs() - 15.0).abs() < 1e-9, "corner {p:?}");
        assert!(p.z.abs() < 1e-9 || (p.z - 10.0).abs() < 1e-9, "corner {p:?}");
        assert_eq!(v.edges.len(), 3, "three edges meet at a block corner");
    }
    for e in &t.edges {
        assert_eq!(e.vertices.len(), 2, "a straight edge has two ends");
        let (a, z) = (t.vertex(e.vertices[0]).unwrap(), t.vertex(e.vertices[1]).unwrap());
        assert!((a.position.distance(z.position) - e.length).abs() < 1e-9, "edge ends span its length");
        assert!(a.edges.contains(&e.id) && z.edges.contains(&e.id), "vertices list their edges");
    }
    let made = k.history(b).unwrap();
    assert_eq!(made.last().unwrap().vertices.created.len(), 8, "the extrusion creates every vertex");

    let bottom: Vec<(VertexId, Vec3)> =
        t.vertices.iter().filter(|v| v.position.z.abs() < 1e-9).map(|v| (v.id, v.position)).collect();
    let top: Vec<VertexId> = t.vertices.iter().filter(|v| v.position.z > 5.0).map(|v| v.id).collect();
    let f = k.fillet(&tag("f"), b, &top_edges(k, b), 2.0).unwrap();
    let ft = k.topology(f.body).unwrap();
    for (id, p) in &bottom {
        let v = ft.vertex(*id).unwrap_or_else(|| panic!("bottom vertex {id} lost its id"));
        assert!(near(v.position, *p, 1e-9), "bottom vertex {id} moved");
        assert!(!f.record.vertices.mentions(*id), "the fillet does not touch bottom vertex {id}");
    }
    for id in &top {
        assert!(f.record.vertices.mentions(*id), "the fillet changes top vertex {id}");
    }
    assert!(!f.record.vertices.created.is_empty(), "a fillet makes new vertices");

    let c = cylinder(k, Vec3::new(10.0, 0.0, 5.0), 6.0, 20.0);
    let u = k.boolean(&tag("u"), BooleanOp::Union, b, c).unwrap();
    let ut = k.topology(u.body).unwrap();
    for v in &t.vertices {
        let kept = ut.vertex(v.id).unwrap_or_else(|| panic!("block corner {} lost its id in a union", v.id));
        assert!(near(kept.position, v.position, 1e-9));
    }
    for body in [b, f.body, c, u.body] {
        k.release(body).unwrap();
    }
}

/// Edge polylines follow the exact curves, run from the start vertex to the end vertex,
/// and honour the angle and chord-height controls.
pub fn edges_sample_along_their_curves(k: &dyn Kernel) {
    let b = block(k);
    let f = k.fillet(&tag("f"), b, &top_edges(k, b), 2.0).unwrap();
    let c = cylinder(k, Vec3::new(10.0, 0.0, 5.0), 6.0, 20.0);
    for body in [f.body, c] {
        let t = k.topology(body).unwrap();
        let polys = k.tessellate_edges(body, &TessellationParams::default()).unwrap();
        assert_eq!(polys.iter().map(|p| p.edge).collect::<Vec<_>>(), t.edges.iter().map(|e| e.id).collect::<Vec<_>>());
        for (p, e) in polys.iter().zip(&t.edges) {
            assert!(p.points.len() >= 2, "edge {} has {} samples", e.id, p.points.len());
            let length: f64 = p.points.windows(2).map(|w| w[0].distance(w[1])).sum();
            assert!(length <= e.length + 1e-9 && length > 0.98 * e.length, "polyline {length} vs edge {}", e.length);
            let pos = |v: VertexId| t.vertex(v).unwrap().position;
            let (first, last) = (p.points[0], *p.points.last().unwrap());
            match e.vertices.as_slice() {
                [a, z] => assert!(near(first, pos(*a), 1e-6) && near(last, pos(*z), 1e-6), "edge {} ends", e.id),
                [a] => assert!(near(first, pos(*a), 1e-6) && near(last, pos(*a), 1e-6), "closed edge {}", e.id),
                other => panic!("edge {} has {} vertices", e.id, other.len()),
            }
            let (lo, hi) = (e.bbox.min, e.bbox.max);
            for q in &p.points {
                let inside = |l: f64, v: f64, h: f64| v >= l - 1e-6 && v <= h + 1e-6;
                assert!(inside(lo.x, q.x, hi.x) && inside(lo.y, q.y, hi.y) && inside(lo.z, q.z, hi.z));
            }
            if e.curve != CurveKind::Line {
                assert!(p.points.len() > 2, "curved edge {} is sampled", e.id);
            }
        }
    }
    // The cylinder's circles: every sample on the circle, turns of at most 15 degrees,
    // and a tighter chord height gives sagittas within it.
    let axis_distance = |q: Vec3| ((q.x - 10.0).powi(2) + q.y.powi(2)).sqrt();
    let t = k.topology(c).unwrap();
    let fine = TessellationParams { chord_height: 0.001, ..Default::default() };
    let coarse = k.tessellate_edges(c, &TessellationParams::default()).unwrap();
    let tight = k.tessellate_edges(c, &fine).unwrap();
    let circles: Vec<usize> = t
        .edges
        .iter()
        .enumerate()
        .filter(|(_, e)| (e.length - 12.0 * std::f64::consts::PI).abs() < 1e-6)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(circles.len(), 2, "a cylinder has two circular edges");
    for i in circles {
        for q in &coarse[i].points {
            assert!((axis_distance(*q) - 6.0).abs() < 1e-6, "sample off the circle");
        }
        let turns = coarse[i].points.windows(3).map(|w| {
            let (u, v) = (w[1] - w[0], w[2] - w[1]);
            (u.dot(v) / (u.length() * v.length())).clamp(-1.0, 1.0).acos().to_degrees()
        });
        assert!(turns.fold(0.0, f64::max) <= 15.0 + 1e-6, "turn above the angle control");
        assert!(tight[i].points.len() > coarse[i].points.len(), "a chord height adds samples");
        for w in tight[i].points.windows(2) {
            let sag = 6.0 - axis_distance((w[0] + w[1]) * 0.5);
            assert!(sag <= 0.001 + 1e-9, "sagitta {sag}");
        }
    }
    for body in [b, f.body, c] {
        k.release(body).unwrap();
    }
}

/// Planar faces carry the normal that points out of the material, also on the inside
/// faces of holes and shells; curved faces carry none.
pub fn planar_faces_have_outward_normals(k: &dyn Kernel) {
    let b = block(k);
    let t = k.topology(b).unwrap();
    for f in &t.faces {
        let n = f.normal.unwrap_or_else(|| panic!("planar face {} has no normal", f.id));
        let out = (f.centroid - Vec3::new(0.0, 0.0, 5.0)).normalized().unwrap();
        let axis = |v: f64| if v.abs() > 0.5 { v.signum() } else { 0.0 };
        assert!(near(n, Vec3::new(axis(out.x), axis(out.y), axis(out.z)), 1e-9), "face {} normal {n:?}", f.id);
    }
    // A blind hole from the top: its floor at z = 5 faces up, into the hole.
    let c = cylinder(k, Vec3::new(10.0, 0.0, 5.0), 6.0, 20.0);
    let s = k.boolean(&tag("s"), BooleanOp::Subtract, b, c).unwrap();
    let st = k.topology(s.body).unwrap();
    let floor = face_near(&st, Vec3::new(10.0, 0.0, 5.0));
    let floor = st.face(floor).unwrap();
    assert!(near(floor.centroid, Vec3::new(10.0, 0.0, 5.0), 1e-2), "hole floor at {:?}", floor.centroid);
    assert!(near(floor.normal.expect("planar floor"), Vec3::Z, 1e-9), "hole floor normal {:?}", floor.normal);
    let wall = st.faces.iter().find(|f| f.surface == SurfaceKind::Cylinder).expect("hole wall");
    assert_eq!(wall.normal, None, "curved faces have no normal");
    // A shell open at the top: the inner floor at z = 2 faces up.
    let top = face_near(&t, Vec3::new(0.0, 0.0, 10.0));
    let sh = k.shell(&tag("sh"), b, &[top], 2.0).unwrap();
    let sht = k.topology(sh.body).unwrap();
    let inner = sht.face(face_near(&sht, Vec3::new(0.0, 0.0, 2.0))).unwrap();
    assert!(near(inner.centroid, Vec3::new(0.0, 0.0, 2.0), 1e-2), "inner floor at {:?}", inner.centroid);
    assert!(near(inner.normal.unwrap(), Vec3::Z, 1e-9), "inner floor normal {:?}", inner.normal);
    // A sheet faces the side its profile plane's normal points to.
    let sheet = k.create_body_from_profile(&tag("p"), &Profile::rectangle(Plane::ZX, Point2::new(0.0, 0.0), 4.0, 4.0));
    let sheet = sheet.unwrap();
    let face = &k.topology(sheet.body).unwrap().faces[0];
    assert!(near(face.normal.unwrap(), Vec3::Y, 1e-9), "sheet normal {:?}", face.normal);
    for body in [b, c, s.body, sh.body, sheet.body] {
        k.release(body).unwrap();
    }
}

/// Every created face has one role: the region of a profile, the caps and sides of a
/// sweep (naming the profile segment), the edge a blend replaced, the face a shell wall
/// is offset from.
pub fn created_faces_have_roles(k: &dyn Kernel) {
    let covered = |r: &crate::HistoryRecord| {
        let mut named: Vec<FaceId> = r.face_roles.iter().map(|o| o.face).collect();
        named.sort();
        let mut created = r.faces.created.clone();
        created.sort();
        assert_eq!(named, created, "{} names exactly its created faces", r.tag);
    };
    let rect = Profile::rectangle(Plane::XY, Point2::new(0.0, 0.0), 40.0, 30.0);
    let p = k.create_body_from_profile(&tag("p"), &rect).unwrap();
    covered(&p.record);
    assert!(p.record.face_roles.iter().all(|o| o.role == FaceRole::Region));

    let b = k.extrude(&tag("b"), p.body, &Extrude { distance: 10.0, direction: None }).unwrap();
    covered(&b.record);
    let t = k.topology(b.body).unwrap();
    let face_of = |role: FaceRole| {
        let found: Vec<FaceId> = b.record.face_roles.iter().filter(|o| o.role == role).map(|o| o.face).collect();
        assert_eq!(found.len(), 1, "one face with role {role:?}");
        t.face(found[0]).unwrap().centroid
    };
    assert!(near(face_of(FaceRole::StartCap), Vec3::new(0.0, 0.0, 0.0), 1e-6));
    assert!(near(face_of(FaceRole::EndCap), Vec3::new(0.0, 0.0, 10.0), 1e-6));
    // Profile::rectangle runs (-20, -15), (20, -15), (20, 15), (-20, 15).
    let sides =
        [Vec3::new(0.0, -15.0, 5.0), Vec3::new(20.0, 0.0, 5.0), Vec3::new(0.0, 15.0, 5.0), Vec3::new(-20.0, 0.0, 5.0)];
    for (i, want) in sides.iter().enumerate() {
        let got = face_of(FaceRole::Side { loop_index: 0, segment: i as u32 });
        assert!(near(got, *want, 1e-6), "side {i} at {got:?}, expected {want:?}");
    }

    // A hole loop names its own sides, and a clockwise arc is a valid segment.
    let lp = |segs: Vec<Segment>| Loop(segs);
    let arc_hole = Profile {
        plane: Plane::XY,
        outer: lp(vec![
            Segment::Line { from: Point2::new(-20.0, -15.0), to: Point2::new(20.0, -15.0) },
            Segment::Line { from: Point2::new(20.0, -15.0), to: Point2::new(20.0, 15.0) },
            // A concave bite out of the top edge: clockwise in a counter-clockwise loop.
            Segment::Line { from: Point2::new(20.0, 15.0), to: Point2::new(5.0, 15.0) },
            Segment::Arc {
                center: Point2::new(0.0, 15.0),
                from: Point2::new(5.0, 15.0),
                to: Point2::new(-5.0, 15.0),
                clockwise: true,
            },
            Segment::Line { from: Point2::new(-5.0, 15.0), to: Point2::new(-20.0, 15.0) },
            Segment::Line { from: Point2::new(-20.0, 15.0), to: Point2::new(-20.0, -15.0) },
        ]),
        holes: vec![lp(vec![Segment::Circle { center: Point2::new(-10.0, -5.0), radius: 3.0 }])],
    };
    let hp = k.create_body_from_profile(&tag("hp"), &arc_hole).unwrap();
    let area = 1200.0 - std::f64::consts::PI * 25.0 / 2.0 - std::f64::consts::PI * 9.0;
    let hb = k.extrude(&tag("hb"), hp.body, &Extrude { distance: 10.0, direction: None }).unwrap();
    assert!(close(k.mass_properties(hb.body).unwrap().volume, area * 10.0, 1e-4), "clockwise arc bites inward");
    covered(&hb.record);
    let ht = k.topology(hb.body).unwrap();
    let role_face = |role: FaceRole| {
        hb.record.face_roles.iter().filter(|o| o.role == role).map(|o| ht.face(o.face).unwrap()).collect::<Vec<_>>()
    };
    let bite = role_face(FaceRole::Side { loop_index: 0, segment: 3 });
    assert!(!bite.is_empty() && bite.iter().all(|f| f.centroid.y < 15.0), "the bite's wall dips below the top edge");
    let wall = role_face(FaceRole::Side { loop_index: 1, segment: 0 });
    assert!(!wall.is_empty(), "the hole wall names loop 1");
    for f in &wall {
        assert!(((f.centroid.x + 10.0).powi(2) + (f.centroid.y + 5.0).powi(2)).sqrt() < 3.0 + 1e-6);
    }

    // A quarter revolution has a start cap, an end cap and four sides.
    let section = k
        .create_body_from_profile(&tag("sec"), &Profile::rectangle(Plane::ZX, Point2::new(3.0, 25.0), 6.0, 10.0))
        .unwrap();
    let r = k
        .revolve(
            &tag("r"),
            section.body,
            &Revolve { axis_origin: Vec3::ZERO, axis_direction: Vec3::Z, angle_deg: 90.0 },
        )
        .unwrap();
    covered(&r.record);
    let roles: Vec<FaceRole> = r.record.face_roles.iter().map(|o| o.role).collect();
    assert!(roles.contains(&FaceRole::StartCap) && roles.contains(&FaceRole::EndCap), "{roles:?}");
    for i in 0..4 {
        assert!(roles.contains(&FaceRole::Side { loop_index: 0, segment: i }), "side {i} in {roles:?}");
    }

    // Fillet faces name the edge they replace; shell walls name their outer face.
    let edges = top_edges(k, b.body);
    let et = k.topology(b.body).unwrap();
    let fl = k.fillet(&tag("fl"), b.body, &edges, 2.0).unwrap();
    covered(&fl.record);
    let flt = k.topology(fl.body).unwrap();
    let mut blended: Vec<EdgeId> = Vec::new();
    for o in &fl.record.face_roles {
        let FaceRole::Blend { edge } = o.role else { panic!("fillet face role {:?}", o.role) };
        assert!(edges.contains(&edge), "blend names a selected edge");
        let mid = et.edge(edge).unwrap().midpoint;
        assert!(flt.face(o.face).unwrap().centroid.distance(mid) < 2.0, "blend face sits on its edge");
        blended.push(edge);
    }
    blended.sort();
    blended.dedup();
    assert_eq!(blended.len(), 4, "each filleted edge has its face");

    let top = face_near(&et, Vec3::new(0.0, 0.0, 10.0));
    let sh = k.shell(&tag("sh"), b.body, &[top], 2.0).unwrap();
    covered(&sh.record);
    for o in &sh.record.face_roles {
        let FaceRole::Offset { face } = o.role else { panic!("shell face role {:?}", o.role) };
        assert!(et.face(face).is_some(), "the offset names a face of the input");
        assert_ne!(face, top, "the open face has no inner wall");
    }
    for body in [p.body, b.body, hp.body, hb.body, section.body, r.body, fl.body, sh.body] {
        k.release(body).unwrap();
    }
}

/// Every face lists the origins that made it, by the caller's tags: a sweep's caps and
/// sides, the union of both origins on a face merged from two coplanar ones, and the same
/// origin on every piece of a face a later cut splits.
pub fn faces_keep_their_origins(k: &dyn Kernel) {
    let solid = |name: &str, plane: Plane, center: Point2, w: f64, h: f64, depth: f64| {
        let p = k.create_body_from_profile(&tag(&format!("{name}.profile")), &Profile::rectangle(plane, center, w, h));
        let p = p.unwrap();
        let swept = k.extrude(&tag(&format!("{name}.sweep")), p.body, &Extrude { distance: depth, direction: None });
        k.release(p.body).unwrap();
        swept.unwrap().body
    };
    let origin = |name: &str, role: FaceRole| crate::Origin { tag: tag(&format!("{name}.sweep")), role };
    let carrying = |body: BodyId, o: &crate::Origin| {
        k.topology(body).unwrap().faces.into_iter().filter(|f| f.origins.contains(o)).collect::<Vec<_>>()
    };
    let plate = solid("plate", Plane::XY, Point2::new(60.0, 45.0), 120.0, 90.0, 10.0);
    let t = k.topology(plate).unwrap();
    assert_eq!(t.faces.len(), 6);
    assert!(t.faces.iter().all(|f| f.origins.len() == 1), "a lone sweep's faces have one origin each");
    let top = origin("plate", FaceRole::EndCap);
    let side = origin("plate", FaceRole::Side { loop_index: 0, segment: 1 });
    assert!(near(carrying(plate, &top)[0].centroid, Vec3::new(60.0, 45.0, 10.0), 1e-6));
    assert!(near(carrying(plate, &side)[0].centroid, Vec3::new(120.0, 45.0, 5.0), 1e-6));

    // A flush tab past the plate's +X side: the tops merge into one face with both origins.
    let tab = solid("tab", Plane::XY, Point2::new(130.0, 45.0), 20.0, 20.0, 10.0);
    let merged = k.boolean(&tag("tab.union"), BooleanOp::Union, plate, tab).unwrap().body;
    let tops = carrying(merged, &top);
    assert_eq!(tops.len(), 1, "one top face");
    assert!(tops[0].origins.contains(&origin("tab", FaceRole::EndCap)), "the merged top carries the tab's origin");
    assert!(close(tops[0].area, 120.0 * 90.0 + 400.0, 1e-6));

    // A groove across the plate splits the merged top: both pieces keep both origins they had.
    let down = Plane { origin: Vec3::new(0.0, 0.0, 10.0), normal: -Vec3::Z, x_dir: Vec3::X };
    let groove = solid("groove", down, Point2::new(100.0, -45.0), 6.0, 100.0, 3.0);
    let cut = k.boolean(&tag("groove.cut"), BooleanOp::Subtract, merged, groove).unwrap().body;
    let pieces = carrying(cut, &top);
    assert_eq!(pieces.len(), 2, "the groove splits the top in two");
    assert!(pieces.iter().all(|f| f.normal == Some(Vec3::Z)));
    let floor = carrying(cut, &origin("groove", FaceRole::EndCap));
    assert_eq!(floor.len(), 1, "the groove's floor is its end cap");
    assert!(near(floor[0].centroid, Vec3::new(100.0, 45.0, 7.0), 1e-6));
    for body in [plate, tab, merged, groove, cut] {
        k.release(body).unwrap();
    }
}

// Drawing queries.

/// The bracket of `recipes/bracket.json`, built step by step: a 120 x 90 x 10 plate with a
/// 10 x 90 x 60 flange at its -X end, a radius 12 boss with a radius 6 bore, four
/// radius 4.5 holes and a 40 x 12 slot through the flange. 19 faces, no fillets.
pub fn bracket(k: &dyn Kernel) -> BodyId {
    let extrude = |name: &str, profile: Profile, distance: f64| {
        let p = k.create_body_from_profile(&tag(&format!("{name}_sketch")), &profile).expect("bracket profile");
        let body = k.extrude(&tag(name), p.body, &Extrude { distance, direction: None }).expect("bracket extrude").body;
        k.release(p.body).unwrap();
        body
    };
    let combine = |name: &str, op: BooleanOp, target: BodyId, tool: BodyId| {
        let body = k.boolean(&tag(name), op, target, tool).expect("bracket boolean").body;
        k.release(target).unwrap();
        k.release(tool).unwrap();
        body
    };
    let disc = |x: f64, y: f64, z: f64, r: f64| {
        Profile::circle(Plane::XY.with_origin(Vec3::new(x, y, z)), Point2::new(0.0, 0.0), r)
    };
    let plate = extrude("plate", Profile::rectangle(Plane::XY, Point2::new(60.0, 45.0), 120.0, 90.0), 10.0);
    let flange = extrude("flange", Profile::rectangle(Plane::XY, Point2::new(5.0, 45.0), 10.0, 90.0), 60.0);
    let mut body = combine("angle", BooleanOp::Union, plate, flange);
    body = combine("bossed", BooleanOp::Union, body, extrude("boss", disc(70.0, 45.0, 10.0, 12.0), 20.0));
    body = combine("bored", BooleanOp::Subtract, body, extrude("bore_tool", disc(70.0, 45.0, -1.0, 6.0), 40.0));
    for (i, (x, y)) in [(25.0, 23.0), (25.0, 67.0), (115.0, 23.0), (115.0, 67.0)].into_iter().enumerate() {
        let tool = extrude(&format!("hole{}_tool", i + 1), disc(x, y, -1.0, 4.5), 12.0);
        body = combine(&format!("hole{}", i + 1), BooleanOp::Subtract, body, tool);
    }
    let slot_plane = Plane::YZ.with_origin(Vec3::new(-1.0, 0.0, 0.0));
    let slot = extrude("slot_tool", Profile::rectangle(slot_plane, Point2::new(45.0, 31.0), 40.0, 12.0), 12.0);
    combine("slot", BooleanOp::Subtract, body, slot)
}

/// The four views of the bracket the drawing checks use.
pub const BRACKET_VIEWS: [StandardView; 4] =
    [StandardView::Front, StandardView::Top, StandardView::Right, StandardView::Isometric];

/// The bracket's four standard views, for backends' golden tests.
pub fn bracket_views(k: &dyn Kernel, body: BodyId) -> Vec<(StandardView, ViewCurves)> {
    BRACKET_VIEWS
        .iter()
        .map(|&v| (v, k.view_curves(body, &ViewSpec::standard(v)).unwrap_or_else(|e| panic!("{} view: {e}", v.name()))))
        .collect()
}

/// The distance from `p` to the nearest point of a polyline.
fn distance_to_polyline(p: Vec3, points: &[Vec3]) -> f64 {
    points
        .windows(2)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            let ab = b - a;
            let t = if ab.dot(ab) > 0.0 { ((p - a).dot(ab) / ab.dot(ab)).clamp(0.0, 1.0) } else { 0.0 };
            p.distance(a + ab * t)
        })
        .fold(f64::INFINITY, f64::min)
}

/// Every curve of the bracket's views is tagged with an entity of the body: an edge it
/// lies on, or a curved face it outlines. Each view shows visible and hidden curves.
pub fn view_curves_are_tagged(k: &dyn Kernel) {
    let b = bracket(k);
    let t = k.topology(b).unwrap();
    assert_eq!(t.faces.len(), 19, "bracket faces");
    let edge_points: std::collections::HashMap<EdgeId, Vec<Vec3>> =
        t.edges.iter().map(|e| (e.id, k.edge_curve(b, e.id).expect("edge curve").sample(1e-4))).collect();
    for (view, curves) in bracket_views(k, b) {
        let (visible, hidden) = curves.counts();
        assert!(visible > 0 && hidden > 0, "{} view: {visible} visible, {hidden} hidden", view.name());
        for c in &curves.curves {
            match c.source {
                CurveSource::Edge(e) => {
                    let on =
                        &edge_points.get(&e).unwrap_or_else(|| panic!("{} view: {e} is not on the body", view.name()));
                    for f in [0.0, 0.5, 1.0] {
                        let d = distance_to_polyline(c.curve.point_at(f), on);
                        assert!(d < 1e-3, "{} view: a curve tagged {e} is {d} mm off it", view.name());
                    }
                }
                CurveSource::Silhouette(f) | CurveSource::Seam(f) => {
                    let face = t.faces.iter().find(|x| x.id == f);
                    let face = face.unwrap_or_else(|| panic!("{} view: {f} is not on the body", view.name()));
                    assert_ne!(
                        face.surface,
                        SurfaceKind::Plane,
                        "{} view: a planar face has no silhouette",
                        view.name()
                    );
                }
                other => panic!("{} view: curve with source {other:?}", view.name()),
            }
        }
    }
    k.release(b).unwrap();
}

/// Repeated calls give the same curves.
pub fn view_curves_are_repeatable(k: &dyn Kernel) {
    let b = bracket(k);
    let first: Vec<String> = bracket_views(k, b).iter().map(|(_, c)| c.canonical_text()).collect();
    for _ in 0..2 {
        let again: Vec<String> = bracket_views(k, b).iter().map(|(_, c)| c.canonical_text()).collect();
        assert_eq!(first, again, "a repeated view differs");
    }
    k.release(b).unwrap();
}

/// A view of a sheet body is refused or empty, never a crash.
pub fn sheet_views_are_refused(k: &dyn Kernel) {
    let p = k
        .create_body_from_profile(&tag("sheet"), &Profile::rectangle(Plane::XY, Point2::new(0.0, 0.0), 4.0, 4.0))
        .unwrap();
    match k.view_curves(p.body, &ViewSpec::standard(StandardView::Top)) {
        Err(KernelError::Unsupported(_)) => {}
        Ok(v) => assert!(v.curves.is_empty(), "{v:?}"),
        Err(e) => panic!("a view of a sheet failed with {e}"),
    }
    let b = block(k);
    assert!(matches!(k.view_curves(b, &ViewSpec::new(Vec3::Z, Vec3::Z)), Err(KernelError::InvalidInput(_))));
    let unknown = k.view_curves(BodyId::from_raw(u64::MAX), &ViewSpec::standard(StandardView::Top));
    assert!(matches!(unknown, Err(KernelError::UnknownBody(_))), "{unknown:?}");
    k.release(p.body).unwrap();
    k.release(b).unwrap();
}

/// Joins curves end to start and reports whether they form one closed loop.
fn closed_loop(curves: &[Curve3], tol: f64) -> bool {
    let mut left: Vec<&Curve3> = curves.iter().collect();
    let Some(first) = left.pop() else { return false };
    let (start, mut at) = (first.start(), first.end());
    while !left.is_empty() {
        let Some(i) = left.iter().position(|c| c.start().distance(at) < tol || c.end().distance(at) < tol) else {
            return false;
        };
        let c = left.swap_remove(i);
        at = if c.start().distance(at) < tol { c.end() } else { c.start() };
    }
    at.distance(start) < tol
}

/// The section of the block through its middle is four lines in one closed loop, each
/// tagged with the side face it cuts.
pub fn box_section_is_a_closed_loop(k: &dyn Kernel) {
    let b = block(k);
    let t = k.topology(b).unwrap();
    let curves = k.section_curves(b, &Plane::XY.with_origin(Vec3::new(0.0, 0.0, 5.0))).expect("section");
    assert_eq!(curves.len(), 4, "{curves:?}");
    let mut lengths: Vec<f64> = curves.iter().map(|c| c.curve.length()).collect();
    lengths.sort_by(f64::total_cmp);
    for (got, want) in lengths.iter().zip([30.0, 30.0, 40.0, 40.0]) {
        assert!(close(*got, want, 1e-6), "section lengths {lengths:?}");
    }
    for c in &curves {
        assert!(matches!(c.curve, Curve3::Line { .. }), "{:?}", c.curve);
        let CurveSource::Section(f) = c.source else { panic!("section curve with source {:?}", c.source) };
        let face = t.faces.iter().find(|x| x.id == f).expect("section face on the body");
        assert!((face.centroid.z - 5.0).abs() < 1e-6, "a section curve is tagged with a cap face");
    }
    let geometry: Vec<Curve3> = curves.into_iter().map(|c| c.curve).collect();
    assert!(closed_loop(&geometry, 1e-6), "the section is not one closed loop");
    k.release(b).unwrap();
}

/// Section A-A of the bracket: the plane through the boss centre parallel to the front
/// face, seen from the front, so the cut runs through the boss, its bore, the plate and
/// the flange with its slot.
pub fn bracket_section_spec() -> SectionSpec {
    SectionSpec { view: ViewSpec::standard(StandardView::Front), origin: Vec3::new(70.0, 45.0, 0.0) }
}

/// Checks that every source of a section names an entity of the uncut body: an edge it
/// lies on, a curved face it outlines, or a face whose surface it lies on.
fn section_sources_resolve(k: &dyn Kernel, body: BodyId, section: &SectionView, what: &str) {
    let t = k.topology(body).unwrap();
    let edge_points: std::collections::HashMap<EdgeId, Vec<Vec3>> =
        t.edges.iter().map(|e| (e.id, k.edge_curve(body, e.id).expect("edge curve").sample(1e-4))).collect();
    let boundaries = section.regions.iter().flat_map(|r| r.outer.iter().chain(r.holes.iter().flatten()));
    for c in section.curves.curves.iter().chain(boundaries) {
        match c.source {
            CurveSource::Edge(e) => {
                let on = edge_points.get(&e).unwrap_or_else(|| panic!("{what}: {e} is not on the uncut body"));
                for f in [0.0, 0.5, 1.0] {
                    let d = distance_to_polyline(c.curve.point_at(f), on);
                    assert!(d < 1e-3, "{what}: a curve tagged {e} is {d} mm off it");
                }
            }
            CurveSource::Silhouette(f) | CurveSource::Seam(f) | CurveSource::Section(f) => {
                assert!(t.face(f).is_some(), "{what}: {f} is not on the uncut body");
            }
            CurveSource::Unknown => panic!("{what}: a section curve has no source: {:?}", c.curve),
        }
    }
}

/// The block cut through its middle seen from above: one region, the 40 x 30 rectangle,
/// whose four sides are tagged with the side faces they cut; the view shows those four
/// sides visible and names the uncut block's entities; the block is untouched.
pub fn box_section_view_hatches_the_cut(k: &dyn Kernel) {
    let b = block(k);
    let before = fixed(k, b);
    let spec = SectionSpec { view: ViewSpec::standard(StandardView::Top), origin: Vec3::new(0.0, 0.0, 5.0) };
    let section = k.section_view(b, &spec).expect("section view");
    let frame = spec.view.frame().unwrap();
    assert_eq!(section.regions.len(), 1, "{:?}", section.regions);
    let region = &section.regions[0];
    assert!(region.holes.is_empty());
    assert!(close(region.area(&frame), 1200.0, 1e-9), "area {}", region.area(&frame));
    let outer: Vec<Curve3> = region.outer.iter().map(|c| c.curve.clone()).collect();
    assert!(closed_loop(&outer, 1e-6), "the region boundary is not closed");
    let t = k.topology(b).unwrap();
    for c in &region.outer {
        let CurveSource::Section(f) = c.source else { panic!("boundary curve with source {:?}", c.source) };
        let face = t.face(f).expect("the cut face is on the block");
        assert!((face.centroid.z - 5.0).abs() < 1e-6, "a boundary is tagged with a cap face");
    }
    let cut_sides = section.curves.curves.iter().filter(|c| c.visible && matches!(c.source, CurveSource::Section(_)));
    assert_eq!(cut_sides.count(), 4, "{:?}", section.curves.curves);
    section_sources_resolve(k, b, &section, "block section");
    assert_eq!(fixed(k, b), before, "a section changed its body");
    k.release(b).unwrap();
}

/// Section A-A of the bracket ([`bracket_section_spec`]): three regions, the plate with
/// the flange's foot and half the boss (910 mm²), the plate's other end with the boss's
/// other half (560 mm²) and the flange above the slot (230 mm²); every source names an
/// entity of the uncut bracket.
pub fn bracket_section_view_has_three_regions(k: &dyn Kernel) {
    let b = bracket(k);
    let spec = bracket_section_spec();
    let section = k.section_view(b, &spec).expect("section A-A");
    let areas = section.region_areas(&spec.view.frame().unwrap());
    assert_eq!(areas.len(), 3, "{areas:?}");
    for (got, want) in areas.iter().zip([910.0, 560.0, 230.0]) {
        assert!(close(*got, want, 1e-6), "region areas {areas:?}");
    }
    let (visible, _) = section.curves.counts();
    assert!(visible > 0);
    section_sources_resolve(k, b, &section, "section A-A");
    k.release(b).unwrap();
}

/// A plane in front of a body cuts nothing and gives the plain view; a plane behind it
/// leaves no material and is refused.
pub fn section_planes_off_the_body(k: &dyn Kernel) {
    let b = block(k);
    let view = ViewSpec::standard(StandardView::Top);
    let front = k.section_view(b, &SectionSpec { view, origin: Vec3::new(0.0, 0.0, 50.0) }).expect("plane in front");
    assert!(front.regions.is_empty());
    let plain = k.view_curves(b, &view).unwrap();
    assert_eq!(front.curves.canonical_text(), plain.canonical_text());
    let behind = k.section_view(b, &SectionSpec { view, origin: Vec3::new(0.0, 0.0, -1.0) });
    assert!(matches!(behind, Err(KernelError::InvalidInput(_))), "{behind:?}");
    k.release(b).unwrap();
}

/// Edge curves and face loops come out exact: lines with the topology's lengths, full
/// circles with the right radius, loops that close.
pub fn edge_curves_are_exact(k: &dyn Kernel) {
    let b = block(k);
    let t = k.topology(b).unwrap();
    for e in &t.edges {
        let c = k.edge_curve(b, e.id).expect("edge curve");
        assert!(matches!(c, Curve3::Line { .. }), "block edge {}: {c:?}", e.id);
        assert!(close(c.length(), e.length, 1e-9), "block edge {} length", e.id);
        assert!(c.point_at(0.5).distance(e.midpoint) < 1e-9, "block edge {} midpoint", e.id);
    }
    let top = face_near(&t, Vec3::new(0.0, 0.0, 10.0));
    let loops = k.face_loops(b, top).expect("face loops");
    assert_eq!(loops.len(), 1, "the block's top has one loop");
    assert_eq!(loops[0].len(), 4);
    for w in 0..4 {
        let (a, n) = (&loops[0][w], &loops[0][(w + 1) % 4]);
        assert!(a.end().distance(n.start()) < 1e-9, "the top loop does not run head to tail");
    }
    assert!(matches!(k.edge_curve(b, EdgeId::from_raw(u64::MAX)), Err(KernelError::UnknownEdge { .. })));
    assert!(matches!(k.face_loops(b, FaceId::from_raw(u64::MAX)), Err(KernelError::UnknownFace { .. })));

    let c = cylinder(k, Vec3::new(10.0, 0.0, 5.0), 6.0, 20.0);
    let ct = k.topology(c).unwrap();
    let mut circles = 0;
    for e in &ct.edges {
        match k.edge_curve(c, e.id).expect("cylinder edge curve") {
            Curve3::Circle { center, radius, normal } => {
                circles += 1;
                assert!(close(radius, 6.0, 1e-9) && (center.x - 10.0).abs() < 1e-9, "circle {center:?} r {radius}");
                assert!(normal.cross(Vec3::Z).length() < 1e-9, "circle normal {normal:?}");
            }
            Curve3::Line { a, b: end } => assert!(close(a.distance(end), 20.0, 1e-9), "the seam is 20 long"),
            other => panic!("cylinder edge {}: {other:?}", e.id),
        }
    }
    assert_eq!(circles, 2, "a cylinder has two circular edges");
    let cap = face_near(&ct, Vec3::new(10.0, 0.0, 25.0));
    let cap_loops = k.face_loops(c, cap).expect("cap loops");
    assert_eq!(cap_loops.len(), 1);
    assert!(cap_loops[0].iter().all(|x| matches!(x, Curve3::Circle { .. })), "{cap_loops:?}");
    for body in [b, c] {
        k.release(body).unwrap();
    }
}

/// The outline of a cylinder seen side on has two silhouette lines along its axis, each
/// tagged with the cylinder's side face. Any other curve is an edge of the body.
pub fn silhouettes_outline_curved_faces(k: &dyn Kernel) {
    let c = cylinder(k, Vec3::new(10.0, 0.0, 5.0), 6.0, 20.0);
    let t = k.topology(c).unwrap();
    let side = t.faces.iter().find(|f| f.surface == SurfaceKind::Cylinder).expect("side face").id;
    // Diagonal, so the seam is not on the outline.
    let dir = Vec3::new(1.0, -1.0, 0.0);
    let curves = k.silhouette_curves(c, dir).expect("silhouettes");
    let (rims, others): (Vec<_>, Vec<_>) = curves.iter().partition(|s| s.source == CurveSource::Silhouette(side));
    assert_eq!(rims.len(), 2, "{curves:?}");
    for s in rims {
        let (a, b) = (s.curve.start(), s.curve.end());
        assert!(close(a.distance(b), 20.0, 1e-6), "silhouette length {}", a.distance(b));
        for p in [a, b] {
            let radial = Vec3::new(p.x - 10.0, p.y, 0.0);
            assert!(close(radial.length(), 6.0, 1e-6) && radial.dot(dir).abs() < 1e-6, "silhouette point {p:?}");
        }
    }
    for s in others {
        let CurveSource::Edge(e) = s.source else { panic!("outline curve with source {:?}", s.source) };
        assert!(t.edges.iter().any(|x| x.id == e), "{e} is not on the cylinder");
    }
    k.release(c).unwrap();
}

/// Lines-and-arcs conversion keeps arcs as one arc and turns a free-form spline into a
/// chain of lines and arcs from its start to its end.
pub fn lines_and_arcs_are_lines_and_arcs(k: &dyn Kernel) {
    let w = std::f64::consts::FRAC_1_SQRT_2;
    let quarter = Curve3::Nurbs {
        degree: 2,
        knots: vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        points: vec![Vec3::new(5.0, 0.0, 0.0), Vec3::new(5.0, 5.0, 0.0), Vec3::new(0.0, 5.0, 0.0)],
        weights: vec![1.0, w, 1.0],
    };
    let pieces = k.lines_and_arcs(&quarter, 1e-3).expect("lines and arcs of an arc");
    assert_eq!(pieces.len(), 1, "{pieces:?}");
    assert!(matches!(pieces[0], Curve3::Arc { radius, .. } if close(radius, 5.0, 1e-6)), "{pieces:?}");

    let wave = Curve3::Nurbs {
        degree: 3,
        knots: vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        points: vec![Vec3::ZERO, Vec3::new(10.0, 10.0, 0.0), Vec3::new(20.0, -10.0, 0.0), Vec3::new(30.0, 0.0, 0.0)],
        weights: vec![],
    };
    let pieces = k.lines_and_arcs(&wave, 1e-3).expect("lines and arcs of a spline");
    assert!(pieces.len() > 1, "{pieces:?}");
    assert!(pieces.iter().all(|p| matches!(p, Curve3::Line { .. } | Curve3::Arc { .. })), "{pieces:?}");
    assert!(
        pieces[0].start().distance(wave.start()) < 1e-3 && pieces.last().unwrap().end().distance(wave.end()) < 1e-3
    );
    for w in pieces.windows(2) {
        assert!(w[0].end().distance(w[1].start()) < 1e-3, "lines and arcs do not chain");
    }
    let d = deviation(&wave, &pieces, 1e-4);
    assert!(d <= 1e-3 + 2e-5, "the pieces are {d} mm off the spline");
}

/// The views the mesh comparison uses: the isometric and two general directions, where
/// no face is seen edge on. In an orthographic view an edge that lies exactly along the
/// outline of a face is a tie between visible and hidden, which an exact and a mesh
/// method may break differently without either being wrong.
pub fn comparison_views() -> Vec<ViewSpec> {
    vec![
        ViewSpec::standard(StandardView::Isometric),
        ViewSpec::new(Vec3::new(1.0, -2.0, 3.0).normalized().unwrap(), Vec3::Z),
        ViewSpec::new(Vec3::new(-2.0, -1.0, 1.5).normalized().unwrap(), Vec3::Z),
    ]
}

/// For each edge of `body`, the visible length the kernel's views and the mesh views
/// give, and the edge's length; the worst difference as a fraction of the edge's length
/// comes first.
pub fn visible_length_differences(k: &dyn Kernel, body: BodyId, views: &[ViewSpec]) -> Vec<(f64, String)> {
    let t = k.topology(body).unwrap();
    let mut out = Vec::new();
    for (i, spec) in views.iter().enumerate() {
        let exact = visible_length_by_source(&k.view_curves(body, spec).expect("view curves"));
        let mesh = visible_length_by_source(&mesh_hlr::view_curves(k, body, spec).expect("mesh view curves"));
        for e in &t.edges {
            let s = CurveSource::Edge(e.id);
            let (a, b) = (exact.get(&s).copied().unwrap_or(0.0), mesh.get(&s).copied().unwrap_or(0.0));
            let diff = (a - b).abs() / e.length;
            out.push((diff, format!("view {i}: {} of length {:.3}: exact {a:.4}, mesh {b:.4}", e.id, e.length)));
        }
    }
    out.sort_by(|x, y| y.0.total_cmp(&x.0));
    out
}

/// The kernel's views agree with [`mesh_hlr`] on the visible length of every edge to
/// within 2 percent of the edge's length, on the block, the cylinder and the bracket.
pub fn view_curves_match_mesh_hlr(k: &dyn Kernel) {
    let bodies =
        [("block", block(k)), ("cylinder", cylinder(k, Vec3::new(10.0, 0.0, 5.0), 6.0, 20.0)), ("bracket", bracket(k))];
    for (name, body) in bodies {
        let diffs = visible_length_differences(k, body, &comparison_views());
        let (worst, what) = &diffs[0];
        assert!(*worst <= 0.02, "{name}: {what} ({:.2} percent)", worst * 100.0);
    }
}

// Exchange.

/// The bracket's STEP export writes one solid with the body's faces, a proven deviation
/// within the requested tolerance and an AP242 header, and leaves the body unchanged.
pub fn step_export_reports_what_it_wrote(k: &dyn Kernel) {
    let body = bracket(k);
    let before = fixed(k, body);
    let options = crate::StepOptions { product_name: "bracket".into(), ..crate::StepOptions::default() };
    let step = k.export_step(body, &options).expect("bracket STEP export");
    let r = &step.report;
    assert_eq!((r.solids, r.faces), (1, before.topology.faces.len()), "STEP solids and faces");
    assert!(r.within_tolerance && r.warnings.is_empty(), "STEP deviation within tolerance: {r:?}");
    assert!(r.max_deviation_mm >= 0.0 && r.max_deviation_mm <= options.tolerance, "STEP deviation {r:?}");
    assert!(step.text.starts_with("ISO-10303-21;"), "STEP header");
    assert!(step.text.contains("AP242"), "STEP schema");
    assert!(step.text.contains("'bracket'"), "STEP product name");
    assert_eq!(step.text.matches("ADVANCED_FACE(").count(), r.faces, "STEP faces in the file");
    assert_eq!(fixed(k, body), before, "STEP export leaves the body unchanged");

    let refused = k.export_step(body, &crate::StepOptions { tolerance: 0.0, ..options });
    assert!(matches!(refused, Err(KernelError::InvalidInput(_))), "zero tolerance refused: {refused:?}");
}

/// The screen's section, cut from the preview mesh with no boolean
/// ([`section::by_mesh_slice`](crate::drawing::section::by_mesh_slice)), hatches the
/// same regions as the boolean section within the mesh's chord tolerance, names the
/// uncut body's entities, closes every region, and refuses the same planes.
pub fn mesh_sections_match_boolean_sections(k: &dyn Kernel) {
    use crate::drawing::section::by_mesh_slice;
    let preview = |spec: SectionSpec| SectionSpec { view: ViewSpec { preview: true, ..spec.view }, ..spec };
    let region_loops_close = |section: &SectionView, what: &str| {
        for region in &section.regions {
            for boundary in std::iter::once(&region.outer).chain(&region.holes) {
                let geometry: Vec<Curve3> = boundary.iter().map(|c| c.curve.clone()).collect();
                assert!(closed_loop(&geometry, 1e-6), "{what}: a region boundary is not closed");
                assert!(
                    boundary.iter().all(|c| matches!(c.source, CurveSource::Section(_))),
                    "{what}: boundary source"
                );
            }
        }
    };

    // The bracket's Section A-A: three regions of 910, 560 and 230 mm².
    let b = bracket(k);
    let spec = bracket_section_spec();
    let frame = spec.view.frame().unwrap();
    let boolean = k.section_view(b, &spec).expect("boolean section A-A");
    let sliced = by_mesh_slice(k, b, &preview(spec)).expect("mesh section A-A");
    assert!(sliced.curves.unproved, "a section from the preview mesh is unproved");
    let (exact, approximate) = (boolean.region_areas(&frame), sliced.region_areas(&frame));
    assert_eq!(exact.len(), approximate.len(), "regions {exact:?} vs {approximate:?}");
    for (want, got) in exact.iter().zip(&approximate) {
        assert!((want - got).abs() < 1.0, "region areas {exact:?} vs {approximate:?}");
    }
    let (visible, _) = sliced.curves.counts();
    assert!(visible > 0);
    section_sources_resolve(k, b, &sliced, "mesh section A-A");
    region_loops_close(&sliced, "mesh section A-A");
    // Every curve the view keeps lies behind or on the cutting plane.
    let n = frame.direction;
    for c in &sliced.curves.curves {
        for f in [0.0, 0.5, 1.0] {
            let past = (c.curve.point_at(f) - spec.origin).dot(n);
            assert!(past < 1e-3, "mesh section A-A: a curve lies {past} mm in front of the plane");
        }
    }
    k.release(b).unwrap();

    // The block through its middle: one region of 1200 mm² with four visible sides.
    let b = block(k);
    let before = fixed(k, b);
    let spec = preview(SectionSpec { view: ViewSpec::standard(StandardView::Top), origin: Vec3::new(0.0, 0.0, 5.0) });
    let section = by_mesh_slice(k, b, &spec).expect("mesh block section");
    assert_eq!(section.regions.len(), 1, "{:?}", section.regions);
    let area = section.regions[0].area(&spec.view.frame().unwrap());
    assert!((area - 1200.0).abs() < 1e-2, "area {area}");
    region_loops_close(&section, "mesh block section");
    section_sources_resolve(k, b, &section, "mesh block section");
    assert_eq!(fixed(k, b), before, "a mesh section changed its body");
    // A plane in front cuts nothing; one behind the body is refused.
    let view = ViewSpec { preview: true, ..ViewSpec::standard(StandardView::Top) };
    let front = by_mesh_slice(k, b, &SectionSpec { view, origin: Vec3::new(0.0, 0.0, 50.0) }).expect("in front");
    assert!(front.regions.is_empty());
    let behind = by_mesh_slice(k, b, &SectionSpec { view, origin: Vec3::new(0.0, 0.0, -1.0) });
    assert!(matches!(behind, Err(KernelError::InvalidInput(_))), "{behind:?}");
    k.release(b).unwrap();

    // A bored cylinder cut across: one region with one hole, the annulus's area.
    let outer = cylinder(k, Vec3::ZERO, 10.0, 20.0);
    let inner = cylinder(k, Vec3::new(0.0, 0.0, -1.0), 4.0, 22.0);
    let bored = k.boolean(&tag("mesh_section_bore"), BooleanOp::Subtract, outer, inner).expect("bore").body;
    let spec = preview(SectionSpec { view: ViewSpec::standard(StandardView::Top), origin: Vec3::new(0.0, 0.0, 10.0) });
    let section = by_mesh_slice(k, bored, &spec).expect("mesh annulus section");
    assert_eq!(section.regions.len(), 1, "{:?}", section.regions);
    assert_eq!(section.regions[0].holes.len(), 1);
    let area = section.regions[0].area(&spec.view.frame().unwrap());
    assert!((area - std::f64::consts::PI * 84.0).abs() < 0.5, "annular area {area}");
    region_loops_close(&section, "mesh annulus section");
    section_sources_resolve(k, bored, &section, "mesh annulus section");
    for id in [bored, outer, inner] {
        k.release(id).unwrap();
    }
}

/// Single-entity measurements carry exact or bounded evidence, and reject foreign ids.
pub fn measures_say_how_they_are_known(k: &dyn Kernel) {
    for depth in [10.0, 76.2] {
        let profile = k
            .create_body_from_profile(
                &tag("measure_profile"),
                &Profile::rectangle(Plane::XY, Point2::new(0.0, 0.0), 40.0, 30.0),
            )
            .unwrap();
        let body =
            k.extrude(&tag("measure_block"), profile.body, &Extrude { distance: depth, direction: None }).unwrap().body;
        let topology = k.topology(body).unwrap();
        let edge = topology.edges.iter().find(|e| (e.bbox.max.z - e.bbox.min.z - depth).abs() < 1e-5).unwrap();
        let m = k.edge_length(body, edge.id).unwrap();
        assert_eq!(m.value, depth);
        assert!(m.lo <= depth && depth <= m.hi && m.hi - m.lo <= 1e-10, "{m:?}");
        if depth == 10.0 {
            assert_eq!(m.certainty, Certainty::Exact);
            assert_eq!((m.value, m.lo, m.hi), (10.0, 10.0, 10.0));
        }
        assert!(k.edge_length(body, EdgeId::from_raw(u64::MAX)).is_err());
        assert!(k.face_area(body, FaceId::from_raw(u64::MAX)).is_err());
        k.release(body).unwrap();
        k.release(profile.body).unwrap();
    }
    let body = cylinder(k, Vec3::ZERO, 10.0, 20.0);
    let topology = k.topology(body).unwrap();
    let side = topology.faces.iter().find(|f| f.surface == SurfaceKind::Cylinder).unwrap();
    let m = k.face_area(body, side.id).unwrap();
    assert_eq!(m.certainty, Certainty::Bounded);
    assert!(m.lo <= 1256.6370614359173 && 1256.6370614359173 <= m.hi, "{m:?}");
    assert!(m.hi - m.lo <= 3e-9);
    k.release(body).unwrap();
}

/// The V2 batch 1 part: the 40 x 30 x `depth` block minus a radius 6 bore about Z.
pub fn holed_block(k: &dyn Kernel, depth: f64) -> BodyId {
    let p = k
        .create_body_from_profile(
            &tag("pair_sketch"),
            &Profile::rectangle(Plane::XY, Point2::new(0.0, 0.0), 40.0, 30.0),
        )
        .expect("block profile");
    let block = k.extrude(&tag("pair_block"), p.body, &Extrude { distance: depth, direction: None }).unwrap().body;
    let bore = cylinder(k, Vec3::new(0.0, 0.0, -1.0), 6.0, depth + 2.0);
    k.boolean(&tag("pair_bore"), BooleanOp::Subtract, block, bore).expect("bore").body
}

/// The straight edge from `a` to `b`, found by its display box. The query under test
/// proves which edge this is.
fn line_between(t: &Topology, a: Vec3, b: Vec3) -> EdgeId {
    let (lo, hi) =
        (Vec3::new(a.x.min(b.x), a.y.min(b.y), a.z.min(b.z)), Vec3::new(a.x.max(b.x), a.y.max(b.y), a.z.max(b.z)));
    let near = |p: Vec3, q: Vec3| (p - q).length() < 0.01;
    t.edges
        .iter()
        .find(|e| e.curve == CurveKind::Line && near(e.bbox.min, lo) && near(e.bbox.max, hi))
        .unwrap_or_else(|| panic!("no line from {a:?} to {b:?}"))
        .id
}

/// The planar face lying in the plane `axis` = `at`, found by its centroid.
fn plane_face(t: &Topology, axis: usize, at: f64) -> FaceId {
    t.faces
        .iter()
        .find(|f| {
            f.surface == SurfaceKind::Plane && ([f.centroid.x, f.centroid.y, f.centroid.z][axis] - at).abs() < 1e-3
        })
        .unwrap_or_else(|| panic!("no plane at {at} on axis {axis}"))
        .id
}

/// An outward nonzero enclosure of sqrt(`n`) no wider than 1e-8. `mul_add` rounds
/// once, so the sign of `lo * lo - n` is exact.
fn encloses_root(m: Measurement, n: f64) {
    assert_eq!(m.certainty, Certainty::Bounded, "{m:?}");
    assert!(m.lo.mul_add(m.lo, -n) <= 0.0 && m.hi.mul_add(m.hi, -n) >= 0.0, "sqrt({n}) outside {m:?}");
    assert!(m.lo < m.hi && m.hi - m.lo <= 1e-8, "{m:?}");
    assert!((m.value - n.sqrt()).abs() <= 1e-8, "{m:?}");
}

/// A rational result, exact or honestly bounded, enclosing `v` within 1e-8.
fn encloses(m: Measurement, v: f64) {
    assert!(m.lo <= v && v <= m.hi && m.hi - m.lo <= 1e-8 && (m.value - v).abs() <= 1e-8, "{v} outside {m:?}");
}

fn exactly(m: Measurement, v: f64) {
    assert_eq!((m.value, m.lo, m.hi, m.certainty), (v, v, v, Certainty::Exact));
}

/// V2 batch 2 K1: finite straight-edge pairs and midpoints on the holed block. E0 is
/// the bottom front edge, E1 the top back edge, E2 the top right edge, E3 the front
/// right vertical edge.
pub fn finite_edge_pairs_and_points_have_proven_bounds(k: &dyn Kernel) {
    use PairEndpoint::{Edge, EdgeMidpoint};
    use PairQuantity::{DirectionAngle, MinimumDistance, PointDistance};
    for depth in [10.0, 15.0] {
        let body = holed_block(k, depth);
        let t = k.topology(body).unwrap();
        let v = Vec3::new;
        let e0 = line_between(&t, v(-20.0, -15.0, 0.0), v(20.0, -15.0, 0.0));
        let e1 = line_between(&t, v(-20.0, 15.0, depth), v(20.0, 15.0, depth));
        let e2 = line_between(&t, v(20.0, -15.0, depth), v(20.0, 15.0, depth));
        let e3 = line_between(&t, v(20.0, -15.0, 0.0), v(20.0, -15.0, depth));
        for (edge, point) in [(e0, [0.0, -15.0, 0.0]), (e2, [20.0, 0.0, depth])] {
            let proof = k.edge_midpoint(body, edge).unwrap();
            for (c, v) in proof.coordinates.iter().zip(point) {
                assert!(c.lo <= v && v <= c.hi && c.hi - c.lo <= 1e-9 && (c.value - v).abs() <= 1e-9, "{v} {c:?}");
            }
        }
        let pair = |a, b, q| {
            let m = k.measure_pair(body, a, b, q).unwrap();
            assert_eq!(m, k.measure_pair(body, b, a, q).unwrap(), "{q:?} is symmetric");
            m
        };
        encloses_root(pair(Edge(e0), Edge(e1), MinimumDistance), 900.0 + depth * depth);
        encloses(pair(Edge(e0), Edge(e2), MinimumDistance), depth);
        exactly(pair(Edge(e0), Edge(e3), MinimumDistance), 0.0);
        exactly(pair(Edge(e0), Edge(e0), MinimumDistance), 0.0);
        encloses_root(pair(EdgeMidpoint(e0), EdgeMidpoint(e2), PointDistance), 625.0 + depth * depth);
        exactly(pair(Edge(e0), Edge(e1), DirectionAngle), 0.0);
        exactly(pair(Edge(e0), Edge(e2), DirectionAngle), 90.0);
        exactly(pair(Edge(e0), Edge(e3), DirectionAngle), 90.0);
        k.release(body).unwrap();
    }
}

/// V2 batch 2 K2: the holed caps' trimmed minimum distance, before and after rounding
/// E0. The supporting planes alone would answer the same here; Catallax's own contract
/// covers disjoint patches whose plane gap is not the answer.
pub fn parallel_face_distance_respects_trims_and_completion(k: &dyn Kernel) {
    use PairEndpoint::Face;
    for (depth, fillet) in [(10.0, false), (15.0, false), (15.0, true)] {
        let mut body = holed_block(k, depth);
        if fillet {
            let t = k.topology(body).unwrap();
            let e0 = line_between(&t, Vec3::new(-20.0, -15.0, 0.0), Vec3::new(20.0, -15.0, 0.0));
            body = k.fillet(&tag("pair_fillet"), body, &[e0], 2.0).expect("fillet E0").body;
        }
        let t = k.topology(body).unwrap();
        let (f0, f1) = (plane_face(&t, 2, 0.0), plane_face(&t, 2, depth));
        for (a, b) in [(f0, f1), (f1, f0)] {
            encloses(k.measure_pair(body, Face(a), Face(b), PairQuantity::MinimumDistance).unwrap(), depth);
        }
    }
}

/// V2 batch 2 K3: named refusals for every unsupported pair, with the valid request
/// still answering after each one.
pub fn query_domains_and_work_are_explicit(k: &dyn Kernel) {
    use PairEndpoint::{Edge, EdgeMidpoint, Face};
    use PairQuantity::{DirectionAngle, MinimumDistance, PointDistance};
    let body = holed_block(k, 10.0);
    let t = k.topology(body).unwrap();
    let v = Vec3::new;
    let e0 = line_between(&t, v(-20.0, -15.0, 0.0), v(20.0, -15.0, 0.0));
    let e1 = line_between(&t, v(-20.0, 15.0, 10.0), v(20.0, 15.0, 10.0));
    let rim = t.edges.iter().find(|e| e.curve != CurveKind::Line && e.bbox.max.z < 0.01).expect("bottom bore rim").id;
    let wall = t.faces.iter().find(|f| f.surface == SurfaceKind::Cylinder).expect("bore wall").id;
    let (f0, front) = (plane_face(&t, 2, 0.0), plane_face(&t, 1, -15.0));
    let refused = |code: &str, body: BodyId, a, b, q| {
        let error = k.measure_pair(body, a, b, q).expect_err(code).to_string();
        assert!(error.contains(code), "{code}: {error}");
    };
    let valid = || encloses_root(k.measure_pair(body, Edge(e0), Edge(e1), MinimumDistance).unwrap(), 1000.0);
    let cases = [
        ("measure.unsupported-curve", Edge(e0), Edge(rim), MinimumDistance),
        ("measure.unsupported-curve", Edge(rim), Edge(e0), DirectionAngle),
        ("measure.unsupported-curve", EdgeMidpoint(e0), EdgeMidpoint(rim), PointDistance),
        ("measure.unsupported-pair", Edge(e0), Face(f0), MinimumDistance),
        ("measure.unsupported-pair", Face(f0), Edge(e0), MinimumDistance),
        ("measure.unsupported-pair", Face(f0), Face(front), DirectionAngle),
        ("measure.unsupported-pair", Edge(e0), Edge(e1), PointDistance),
        ("measure.unsupported-pair", EdgeMidpoint(e0), EdgeMidpoint(e1), MinimumDistance),
        ("measure.nonplanar-face", Face(wall), Face(f0), MinimumDistance),
        ("measure.nonparallel-faces", Face(f0), Face(front), MinimumDistance),
    ];
    for (code, a, b, q) in cases {
        refused(code, body, a, b, q);
        valid();
    }
    let error = k.edge_midpoint(body, rim).expect_err("a closed rim has no unique midpoint").to_string();
    assert!(error.contains("measure.unsupported-curve"), "{error}");
    valid();

    // A line along (1, 1, 0) is neither parallel nor perpendicular to X.
    let profile = Profile {
        plane: Plane::XY,
        outer: Loop(vec![
            Segment::Line { from: Point2::new(0.0, 0.0), to: Point2::new(10.0, 0.0) },
            Segment::Line { from: Point2::new(10.0, 0.0), to: Point2::new(10.0, 10.0) },
            Segment::Line { from: Point2::new(10.0, 10.0), to: Point2::new(0.0, 0.0) },
        ]),
        holes: Vec::new(),
    };
    let p = k.create_body_from_profile(&tag("wedge_sketch"), &profile).unwrap();
    let wedge = k.extrude(&tag("wedge"), p.body, &Extrude { distance: 5.0, direction: None }).unwrap().body;
    let w = k.topology(wedge).unwrap();
    let x = line_between(&w, v(0.0, 0.0, 0.0), v(10.0, 0.0, 0.0));
    let diagonal = line_between(&w, v(0.0, 0.0, 0.0), v(10.0, 10.0, 0.0));
    refused("measure.angle-domain", wedge, Edge(diagonal), Edge(x), DirectionAngle);
    exactly(k.measure_pair(wedge, Edge(diagonal), Edge(x), MinimumDistance).unwrap(), 0.0);

    // Handles from another body, or unknown ones, keep their typed errors.
    for (a, b) in [(Edge(x), Edge(e1)), (Edge(e0), Edge(EdgeId::from_raw(u64::MAX)))] {
        let error = k.measure_pair(body, a, b, MinimumDistance).expect_err("foreign or unknown edge");
        assert!(matches!(error, KernelError::UnknownEdge { .. }), "{error}");
    }
    let error = k.measure_pair(body, Face(FaceId::from_raw(u64::MAX)), Face(f0), MinimumDistance).unwrap_err();
    assert!(matches!(error, KernelError::UnknownFace { .. }), "{error}");
    assert!(matches!(k.edge_midpoint(body, EdgeId::from_raw(u64::MAX)), Err(KernelError::UnknownEdge { .. })));
    valid();
    k.release(wedge).unwrap();
    k.release(body).unwrap();
}

/// The support plane of the face of `body` whose outward normal is `normal`.
fn support(k: &dyn Kernel, body: BodyId, normal: Vec3) -> ExactAxisPlane {
    let topology = k.topology(body).unwrap();
    let face = topology.faces.iter().find(|f| f.normal == Some(normal)).unwrap();
    k.face_support_plane(body, face.id).unwrap()
}

fn plate(k: &dyn Kernel, name: &str, width: f64, height: f64, depth: f64) -> BodyId {
    let profile = Profile::rectangle(Plane::XY, Point2::new(0.0, 0.0), width, height);
    let base = k.create_body_from_profile(&tag(&format!("{name}_profile")), &profile).unwrap();
    k.extrude(&tag(name), base.body, &Extrude { distance: depth, direction: None }).unwrap().body
}

/// Reads the six signed plane equations of a block, exactly, through each face's own sense.
pub fn support_planes_are_native_and_exact(k: &dyn Kernel) {
    let whole = |n: i64| ExactRational { numerator: n.to_string(), denominator: "1".into() };
    let equation = |p: ExactAxisPlane| (p.axis, p.positive, p.offset);
    let check = |body, planes: &[(Vec3, Axis, bool, i64)]| {
        for &(normal, axis, positive, offset) in planes {
            assert_eq!(equation(support(k, body, normal)), (axis, positive, whole(offset)), "{normal:?}");
        }
    };
    let a = plate(k, "a", 40.0, 30.0, 10.0);
    let sides = [
        (Vec3::X, Axis::X, true, 20),
        (-Vec3::X, Axis::X, false, 20),
        (Vec3::Y, Axis::Y, true, 15),
        (-Vec3::Y, Axis::Y, false, 15),
        (-Vec3::Z, Axis::Z, false, 0),
    ];
    check(a, &sides);
    check(a, &[(Vec3::Z, Axis::Z, true, 10)]);
    // A taller revision moves only its top.
    let taller = plate(k, "taller", 40.0, 30.0, 14.0);
    check(taller, &sides);
    check(taller, &[(Vec3::Z, Axis::Z, true, 14)]);
    let b = plate(k, "b", 20.0, 10.0, 6.0);
    check(b, &[(Vec3::X, Axis::X, true, 10), (Vec3::Y, Axis::Y, true, 5), (Vec3::Z, Axis::Z, true, 6)]);
    check(b, &[(-Vec3::Z, Axis::Z, false, 0)]);
    // A decimal depth stays the rational it is.
    let inches = plate(k, "inches", 40.0, 30.0, 76.2);
    let top = support(k, inches, Vec3::Z);
    assert_eq!(top.offset, ExactRational { numerator: "381".into(), denominator: "5".into() }, "{top:?}");
    assert_eq!(top.normal(), [0, 0, 1]);
}

/// Faces this query does not cover, and handles that name nothing, are refused by kind.
pub fn support_plane_refusals(k: &dyn Kernel) {
    let wall = cylinder(k, Vec3::ZERO, 10.0, 20.0);
    let topology = k.topology(wall).unwrap();
    let side = topology.faces.iter().find(|f| f.surface == SurfaceKind::Cylinder).unwrap();
    assert!(matches!(k.face_support_plane(wall, side.id), Err(KernelError::Unsupported(_))));
    let cap = topology.faces.iter().find(|f| f.surface == SurfaceKind::Plane).unwrap();
    k.face_support_plane(wall, cap.id).unwrap();

    let corners = [Point2::new(0.0, 0.0), Point2::new(30.0, 0.0), Point2::new(0.0, 40.0)];
    let sides = (0..3).map(|i| Segment::Line { from: corners[i], to: corners[(i + 1) % 3] }).collect();
    let wedge = Profile { plane: Plane::XY, outer: Loop(sides), holes: Vec::new() };
    let base = k.create_body_from_profile(&tag("wedge_profile"), &wedge).unwrap();
    let wedge = k.extrude(&tag("wedge"), base.body, &Extrude { distance: 5.0, direction: None }).unwrap().body;
    let topology = k.topology(wedge).unwrap();
    let slope = topology.faces.iter().find(|f| f.normal.is_some_and(|n| n.x > 0.1 && n.y > 0.1)).unwrap();
    assert!(matches!(k.face_support_plane(wedge, slope.id), Err(KernelError::Unsupported(_))));

    let foreign = FaceId::from_raw(u64::MAX);
    assert!(matches!(k.face_support_plane(wall, foreign), Err(KernelError::UnknownFace { .. })));
    k.release(wedge).unwrap();
    assert!(k.face_support_plane(wedge, slope.id).is_err());
    // The refusals took nothing from the body they did not ask about.
    let history = k.history(wall).unwrap();
    k.face_support_plane(wall, cap.id).unwrap();
    assert_eq!(k.history(wall).unwrap(), history);
}
