//! The cxx bridge to the C++ engine in `cpp/engine.cpp`.

#[allow(unsafe_code, missing_docs)]
#[cxx::bridge(namespace = "soliddraft_smlib")]
pub(crate) mod bridge {
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    struct V3 {
        x: f64,
        y: f64,
        z: f64,
    }

    /// A right-handed placement: local XY is the sketch plane.
    #[derive(Clone, Copy, Debug)]
    struct Frame {
        origin: V3,
        x_dir: V3,
        y_dir: V3,
    }

    /// One profile curve in the frame's local XY plane (z = 0).
    /// kind 0: line from `a` to `b`.
    /// kind 1: arc about `center` from angle `start_deg` to `end_deg`, counter-clockwise.
    /// kind 2: full circle about `center`.
    #[derive(Clone, Copy, Debug)]
    struct CurveIn {
        kind: u8,
        a: V3,
        b: V3,
        center: V3,
        radius: f64,
        start_deg: f64,
        end_deg: f64,
    }

    /// A face as the kernel holds it. `tag` is the tag attribute found on it, 0 if none;
    /// several faces can share one after a split.
    #[derive(Clone, Debug)]
    struct FaceRec {
        tag: u64,
        /// 0 plane, 1 cylinder, 2 cone, 3 sphere, 4 torus, 5 spline, 6 other.
        surface: u8,
        area: f64,
        centroid: V3,
        bbox_min: V3,
        bbox_max: V3,
        /// Indices into `Snapshot::edges`.
        edge_indices: Vec<u32>,
        /// For a planar face, the unit normal out of the material; zero otherwise.
        normal: V3,
    }

    /// An edge as the kernel holds it; `tag` as for faces.
    #[derive(Clone, Debug)]
    struct EdgeRec {
        tag: u64,
        /// 0 line, 1 circle, 2 ellipse, 3 spline, 4 other.
        curve: u8,
        length: f64,
        midpoint: V3,
        bbox_min: V3,
        bbox_max: V3,
        /// Indices into `Snapshot::faces`.
        face_indices: Vec<u32>,
        /// Indices into `Snapshot::vertices`: start and end, or one for a closed edge.
        vertex_indices: Vec<u32>,
    }

    /// A vertex as the kernel holds it; `tag` as for faces.
    #[derive(Clone, Debug)]
    struct VertexRec {
        tag: u64,
        point: V3,
        /// Indices into `Snapshot::edges`.
        edge_indices: Vec<u32>,
    }

    #[derive(Clone, Debug)]
    struct Snapshot {
        manifold_solid: bool,
        faces: Vec<FaceRec>,
        edges: Vec<EdgeRec>,
        vertices: Vec<VertexRec>,
        bbox_min: V3,
        bbox_max: V3,
    }

    /// Every edge sampled along its curve: `counts[i]` points for the edge tagged
    /// `tags[i]`, packed in `points` as x, y, z.
    #[derive(Clone, Debug)]
    struct EdgeSamples {
        tags: Vec<u64>,
        counts: Vec<u32>,
        points: Vec<f64>,
    }

    #[derive(Clone, Debug)]
    struct CheckOut {
        manifold_solid: bool,
        assert_valid: bool,
        face_count: u64,
        issues: Vec<String>,
    }

    #[derive(Clone, Copy, Debug)]
    struct MassOut {
        area: f64,
        volume: f64,
        centroid: V3,
        /// Moments about the centroid along world axes: Ixx, Iyy, Izz.
        moments: V3,
        /// Products about the centroid as raw integrals: Iyz, Izx, Ixy.
        products: V3,
    }

    #[derive(Clone, Copy, Debug)]
    struct TessIn {
        chord_height: f64,
        curve_angle_deg: f64,
        surface_angle_deg: f64,
        max_edge_length: f64,
        max_aspect_ratio: f64,
    }

    #[derive(Clone, Debug)]
    struct MeshOut {
        positions: Vec<f32>,
        normals: Vec<f32>,
        indices: Vec<u32>,
        triangle_face_tags: Vec<u64>,
    }

    /// A view for `view_curves`. Zero tolerances use SmHCR's defaults.
    #[derive(Clone, Copy, Debug)]
    struct ViewIn {
        /// Towards the eye.
        direction: V3,
        approx_tol: f64,
        angle_tol_deg: f64,
        seams: bool,
        smooth_edges: bool,
    }

    /// A curve out of the kernel, in model space.
    /// kind 0: B-spline from `degree`, `knots`, `points` (x, y, z triples) and `weights`
    /// (empty unless rational). kind 1: line, `points` holds its two ends. kind 2:
    /// polyline through `points`, for a curve with no exact form.
    #[derive(Clone, Debug)]
    struct CurveOut {
        kind: u8,
        degree: u32,
        knots: Vec<f64>,
        points: Vec<f64>,
        weights: Vec<f64>,
        /// The kernel's points at the start, middle and end of the parameter range, to
        /// check the exact form against.
        check: Vec<f64>,
        visible: bool,
        /// 0 unknown, 1 edge, 2 silhouette, 3 seam, 4 section.
        source: u8,
        /// The edge's tag for an edge, the face's tag otherwise; 0 when unknown.
        tag: u64,
        /// True for a piece of an edge between tangent faces.
        smooth: bool,
    }

    /// A face's boundary: curves loop by loop, outer loop first. `reversed[i]` is 1
    /// when the loop runs against curve i's direction.
    #[derive(Clone, Debug)]
    struct FaceLoopsOut {
        curves: Vec<CurveOut>,
        reversed: Vec<u8>,
        loop_sizes: Vec<u32>,
    }

    /// A spline going into the kernel, laid out as in `CurveOut`.
    #[derive(Clone, Debug)]
    struct NurbsIn {
        degree: u32,
        knots: Vec<f64>,
        points: Vec<f64>,
        weights: Vec<f64>,
    }

    unsafe extern "C++" {
        include!("soliddraft-kernel-smlib/cpp/engine.h");

        type Engine;

        fn new_engine() -> UniquePtr<Engine>;

        fn make_profile(self: Pin<&mut Engine>, curves: &[CurveIn], loop_sizes: &[u32], frame: &Frame) -> Result<u64>;
        fn extrude(
            self: Pin<&mut Engine>,
            curves: &[CurveIn],
            loop_sizes: &[u32],
            frame: &Frame,
            local_dir: &V3,
            distance: f64,
        ) -> Result<u64>;
        fn revolve(
            self: Pin<&mut Engine>,
            curves: &[CurveIn],
            loop_sizes: &[u32],
            frame: &Frame,
            local_origin: &V3,
            local_axis: &V3,
            angle_deg: f64,
        ) -> Result<u64>;
        fn boolean_op(self: Pin<&mut Engine>, kind: u8, target: u64, tool: u64) -> Result<u64>;
        fn blend(self: Pin<&mut Engine>, body: u64, edge_tags: &[u64], size: f64, chamfer: bool) -> Result<u64>;
        fn shell(self: Pin<&mut Engine>, body: u64, open_face_tags: &[u64], thickness: f64) -> Result<u64>;
        fn snapshot(self: &Engine, body: u64) -> Result<Snapshot>;
        fn set_tags(
            self: Pin<&mut Engine>,
            body: u64,
            face_tags: &[u64],
            edge_tags: &[u64],
            vertex_tags: &[u64],
        ) -> Result<()>;
        fn check(self: &Engine, body: u64) -> Result<CheckOut>;
        fn mass(self: &Engine, body: u64) -> Result<MassOut>;
        fn volume(self: &Engine, body: u64) -> Result<f64>;
        fn tessellate(self: &Engine, body: u64, params: &TessIn) -> Result<MeshOut>;
        fn tessellate_levels(self: &Engine, body: u64, levels: &[TessIn]) -> Result<Vec<MeshOut>>;
        fn sample_edges(self: &Engine, body: u64, angle_deg: f64, chord_height: f64) -> Result<EdgeSamples>;
        fn release(self: Pin<&mut Engine>, body: u64);
        fn body_count(self: &Engine) -> u64;

        fn view_curves(self: &Engine, body: u64, view: &ViewIn) -> Result<Vec<CurveOut>>;
        fn hidden_points(self: &Engine, body: u64, view: &ViewIn, points: &[f64]) -> Result<Vec<u8>>;
        fn silhouette_curves(self: &Engine, body: u64, direction: &V3, approx_tol: f64) -> Result<Vec<CurveOut>>;
        fn section_curves(self: &Engine, body: u64, origin: &V3, normal: &V3) -> Result<Vec<CurveOut>>;
        fn edge_curve(self: &Engine, body: u64, edge_tag: u64) -> Result<CurveOut>;
        fn face_loops(self: &Engine, body: u64, face_tag: u64) -> Result<FaceLoopsOut>;
        fn lines_and_arcs(self: &Engine, curve: &NurbsIn) -> Result<Vec<CurveOut>>;
    }
}

pub(crate) use bridge::*;
