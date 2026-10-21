// SPDX-License-Identifier: MIT
//
// The C++ half of the SMLib backend. One Engine owns every SmBrep the backend has
// handed out, keyed by a 64-bit handle. It lives on the backend's kernel thread and is
// never touched from any other thread: SM_API's process-global context is not
// synchronised.
//
// Errors are thrown as std::runtime_error with the text "<status>|<message>", which the
// cxx bridge turns into a Rust Err.

#pragma once

#include <cstdint>
#include <memory>
#include <unordered_map>

#include "rust/cxx.h"

class SmBrep;
class SmCurve;
template <class T> class SmTArray;

namespace soliddraft_smlib {

struct V3;
struct Frame;
struct CurveIn;
struct Snapshot;
struct MassOut;
struct MeshOut;
struct TessIn;
struct CheckOut;
struct ViewIn;
struct CurveOut;
struct FaceLoopsOut;
struct NurbsIn;
struct EdgeSamples;

class Engine {
public:
    Engine();
    ~Engine();
    Engine(const Engine&) = delete;
    Engine& operator=(const Engine&) = delete;

    // Builds a planar sheet body from closed loops drawn in the frame's local XY plane.
    uint64_t make_profile(rust::Slice<const CurveIn> curves, rust::Slice<const uint32_t> loop_sizes,
                          const Frame& frame);
    // Sweeps the loops along the local direction by distance, capped.
    uint64_t extrude(rust::Slice<const CurveIn> curves, rust::Slice<const uint32_t> loop_sizes, const Frame& frame,
                     const V3& local_dir, double distance);
    // Turns the loops about a local axis by angle_deg, capped.
    uint64_t revolve(rust::Slice<const CurveIn> curves, rust::Slice<const uint32_t> loop_sizes, const Frame& frame,
                     const V3& local_origin, const V3& local_axis, double angle_deg);
    // kind: 0 union, 1 subtract, 2 intersect. Inputs are copied, never changed.
    uint64_t boolean_op(uint8_t kind, uint64_t target, uint64_t tool);
    // Fillets (circular) or chamfers (linear) the edges carrying the given tags.
    uint64_t blend(uint64_t body, rust::Slice<const uint64_t> edge_tags, double size, bool chamfer);
    // Shells the body, leaving the faces carrying the given tags open.
    uint64_t shell(uint64_t body, rust::Slice<const uint64_t> open_face_tags, double thickness);

    // Reports every face and edge with the tag attribute it carries (0 if none).
    Snapshot snapshot(uint64_t body) const;
    // Writes tags onto the faces, edges and vertices, in the order snapshot() reports them.
    void set_tags(uint64_t body, rust::Slice<const uint64_t> face_tags, rust::Slice<const uint64_t> edge_tags,
                  rust::Slice<const uint64_t> vertex_tags);
    CheckOut check(uint64_t body) const;
    MassOut mass(uint64_t body) const;
    double volume(uint64_t body) const;
    MeshOut tessellate(uint64_t body, const TessIn& params) const;
    // One mesh per entry of levels, in order, from one lookup of the body.
    rust::Vec<MeshOut> tessellate_levels(uint64_t body, rust::Slice<const TessIn> levels) const;
    // Samples every edge along its curve until the turn between samples is at most
    // angle_deg and, when chord_height > 0, the chord is that close to the curve.
    EdgeSamples sample_edges(uint64_t body, double angle_deg, double chord_height) const;
    void release(uint64_t body);
    uint64_t body_count() const;

    // Drawing queries, in hlr.cpp.
    // Visible and hidden curves of a parallel view (SmHCR), each tagged with its source.
    rust::Vec<CurveOut> view_curves(uint64_t body, const ViewIn& view) const;
    // 1 for each point (x, y, z triples) the body hides in the view, 0 for each it does not.
    rust::Vec<uint8_t> hidden_points(uint64_t body, const ViewIn& view, rust::Slice<const double> points) const;
    // Silhouette curves seen from a direction, without visibility.
    rust::Vec<CurveOut> silhouette_curves(uint64_t body, const V3& direction, double approx_tol) const;
    // Where a plane cuts the body.
    rust::Vec<CurveOut> section_curves(uint64_t body, const V3& origin, const V3& normal) const;
    // The exact curve of the edge carrying a tag.
    CurveOut edge_curve(uint64_t body, uint64_t edge_tag) const;
    // The boundary loops of the face carrying a tag.
    FaceLoopsOut face_loops(uint64_t body, uint64_t face_tag) const;
    // A spline as lines and arcs (SmApiConvertToLinesAndArcs).
    rust::Vec<CurveOut> lines_and_arcs(const NurbsIn& curve) const;

private:
    uint64_t store(SmBrep* brep);
    SmBrep* get(uint64_t body) const;
    SmBrep* copy_of(uint64_t body) const;

    std::unordered_map<uint64_t, SmBrep*> bodies_;
    uint64_t next_handle_ = 1;
};

std::unique_ptr<Engine> new_engine();

}  // namespace soliddraft_smlib
