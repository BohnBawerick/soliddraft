// SPDX-License-Identifier: MIT
//
// See engine.h. Every kernel call goes through SM_API where SM_API has the call; the
// SMLib classes are used directly only to read topology, to keep entity tags in
// attributes, and to walk tessellation output.

#include "soliddraft-kernel-smlib/cpp/engine.h"
#include "soliddraft-kernel-smlib/src/ffi.rs.h"

#include <SmApiBrep.h>
#include <SmApiCurves.h>
#include <SmApiGeneral.h>
#include <SmApiQueries.h>
#include <SmApiTrimmedSurfaces.h>

#include <SmAssertArray.h>
#include <SmAttribute.h>
#include <SmAxis2Placement.h>
#include <SmBSplineCurve.h>
#include <SmBrep.h>
#include <SmCircle.h>
#include <SmContext.h>
#include <SmEdge.h>
#include <SmFace.h>
#include <SmFaceuse.h>
#include <SmLine.h>
#include <SmPlane.h>
#include <SmPoly.h>
#include <SmRegion.h>
#include <SmSurface.h>
#include <SmVertex.h>

#include <algorithm>
#include <cmath>
#include <climits>
#include <functional>
#include <stdexcept>
#include <string>
#include <unordered_map>
#include <vector>

namespace soliddraft_smlib {
namespace {

// Application attribute ids, above SMLib's reserved range (see SmAttribute.h).
constexpr ULONG kFaceTagAttr = SM_AI_USER_1 + 701;
constexpr ULONG kEdgeTagAttr = SM_AI_USER_1 + 702;
constexpr ULONG kVertexTagAttr = SM_AI_USER_1 + 703;

// Accuracy for per-face area and centroid integration: the tightest SM_API accepts.
constexpr double kFaceAccuracy = 1e-4;
// Accuracy for edge length integration, absolute.
constexpr double kEdgeAccuracy = 1e-9;

[[noreturn]] void fail(long status, const std::string& what) {
    throw std::runtime_error(std::to_string(status) + "|" + what);
}

void check_status(SmApiStatus status, const char* call) {
    if (status != SM_SUCCESS) fail(status, call);
}

SmPoint3d pt(const V3& v) { return SmPoint3d(v.x, v.y, v.z); }
SmVector3d vec(const V3& v) { return SmVector3d(v.x, v.y, v.z); }
V3 v3(const SmPoint3d& p) { return V3{p.x, p.y, p.z}; }

const SmContext& context() { return *SmApiGetOrCreateContext(); }

// Deletes SMLib objects that are still owned when a scope exits.
struct Owned {
    std::vector<SmObject*> objects;
    ~Owned() {
        for (SmObject* o : objects) delete o;
    }
    void release_all() { objects.clear(); }
};

struct Curves {
    SmTArray<SmCurve*> list;
    bool owned = true;
    ~Curves() {
        if (owned)
            for (ULONG i = 0; i < list.GetSize(); ++i) delete list[i];
    }
};

// Builds SMLib curves for the profile loops, in local coordinates.
void build_curves(rust::Slice<const CurveIn> curves, rust::Slice<const uint32_t> loop_sizes, Curves& out) {
    size_t total = 0;
    for (uint32_t n : loop_sizes) total += n;
    if (total != curves.size() || curves.empty()) fail(SM_ERR_INVALID_INPUT, "profile loop sizes do not match curves");
    for (const CurveIn& c : curves) {
        switch (c.kind) {
            case 0: {
                SmLine* line = nullptr;
                check_status(SmApiCreateLineSegment(pt(c.a), pt(c.b), line), "SmApiCreateLineSegment");
                out.list.Add(line);
                break;
            }
            case 1: {
                SmBSplineCurve* arc = nullptr;
                check_status(SmApiCreateArc(pt(c.center), c.radius, c.start_deg, c.end_deg, arc), "SmApiCreateArc");
                out.list.Add(arc);
                break;
            }
            case 2: {
                SmCircle* circle = nullptr;
                check_status(SmApiCreateCircle(pt(c.center), c.radius, circle), "SmApiCreateCircle");
                out.list.Add(circle);
                break;
            }
            default:
                fail(SM_ERR_INVALID_INPUT, "unknown profile curve kind");
        }
    }
}

// Moves a body from the frame's local coordinates to world coordinates.
void place(SmBrep* brep, const Frame& frame) {
    SmAxis2Placement placement(pt(frame.origin), vec(frame.x_dir), vec(frame.y_dir));
    check_status(SmApiTransform(brep, placement), "SmApiTransform");
}

uint64_t read_tag(const SmAObject* object, ULONG attr) {
    const SmAttribute* a = object->FindAttribute(attr);
    if (a == nullptr) return 0;
    long value = static_cast<const SmLongAttribute*>(a)->GetValue();
    return value > 0 ? static_cast<uint64_t>(value) : 0;
}

void write_tag(SmAObject* object, ULONG attr, uint64_t tag) {
    if (tag > static_cast<uint64_t>(LONG_MAX)) fail(SM_ERR, "entity tag space exhausted");
    if (SmAttribute* old = object->FindAttribute(attr)) object->RemoveAttribute(old);
    object->AddAttribute(new (context()) SmLongAttribute(attr, static_cast<long>(tag), SM_AB_COPY));
}

uint8_t surface_kind(const SmFace* face) {
    const SmSurface* s = face->GetSurface();
    if (s == nullptr) return 6;
    switch (s->GetType()) {
        case SmPlane_TYPE: return 0;
        case SmCylinder_TYPE: return 1;
        case SmCone_TYPE: return 2;
        case SmSphere_TYPE: return 3;
        case SmTorus_TYPE: return 4;
        case SmBSplineSurface_TYPE: return 5;
        default: return 6;
    }
}

uint8_t curve_kind(const SmEdge* edge) {
    const SmCurve* c = edge->GetCurve();
    if (c == nullptr) return 4;
    switch (c->GetType()) {
        case SmLine_TYPE: return 0;
        case SmCircle_TYPE: return 1;
        case SmEllipse_TYPE: return 2;
        case SmBSplineCurve_TYPE: return 3;
        default: return 4;
    }
}

// Copies tags from src to dst entity by entity, in topology order, wherever dst lacks
// one. Used after a deep copy, which preserves order.
template <class T>
void copy_tags(const SmTArray<T*>& src, SmTArray<T*>& dst, ULONG attr) {
    if (src.GetSize() != dst.GetSize()) fail(SM_ERR, "copy changed the entity count");
    for (ULONG i = 0; i < src.GetSize(); ++i) {
        uint64_t tag = read_tag(src[i], attr);
        if (tag != 0 && read_tag(dst[i], attr) != tag) write_tag(dst[i], attr, tag);
    }
}

SmVector3d unit(const SmVector3d& n, const SmVector3d& fallback) {
    double len = std::sqrt(n.x * n.x + n.y * n.y + n.z * n.z);
    if (!std::isfinite(len) || len <= 1e-30) return fallback;
    return n / len;
}

// The normal of a tessellation vertex on one source face.
SmVector3d vertex_normal(SmPolyVertex* vertex, SmFace* face, const SmVector3d& fallback) {
    const auto& normals = vertex->GetNormalsRef();
    const auto& faces = vertex->GetFacesRef();
    for (ULONG i = 0; i < faces.GetSize() && i < normals.GetSize(); ++i)
        if (faces[i] == static_cast<SmTopology*>(face)) return unit(normals[i], fallback);
    return fallback;
}

// The unit normal of a planar face, pointing out of the material. A face's surface
// normal says nothing about which side the material is on: a subtraction keeps the
// tool's surfaces as they were. SMLib's radial-edge topology gives each face two
// faceuses, one per side, and a faceuse's normal points into the region its shell
// bounds. So the outward normal is the reverse of the faceuse on the solid (non-void)
// side. A sheet has no solid region and keeps its surface normal.
SmVector3d plane_normal(SmFace* face) {
    const SmPlane* plane = static_cast<const SmPlane*>(face->GetSurface());
    SmVector3d n = unit(plane->GetPosition().GetZAxis(), SmVector3d(0, 0, 1));
    SmFaceuse* uses[2] = {nullptr, nullptr};
    face->GetFaceuses(uses[0], uses[1]);
    for (SmFaceuse* use : uses) {
        if (use == nullptr || use->GetMate() == nullptr) continue;
        SmRegion* inside = use->GetRegion();
        SmRegion* outside = use->GetMate()->GetRegion();
        if (inside != nullptr && outside != nullptr && !inside->IsVoid() && outside->IsVoid())
            return use->GetOrientation() == SM_OT_OPPOSITE ? n : -n;
    }
    return n;
}

}  // namespace

Engine::Engine() {
    // Once, on the kernel thread, before any other SM_API call.
    SmApiCreateContext();
    SmGetAssertValidStream() = FALSE;
}

Engine::~Engine() {
    for (auto& entry : bodies_) delete entry.second;
}

std::unique_ptr<Engine> new_engine() { return std::make_unique<Engine>(); }

uint64_t Engine::store(SmBrep* brep) {
    uint64_t handle = next_handle_++;
    bodies_[handle] = brep;
    return handle;
}

SmBrep* Engine::get(uint64_t body) const {
    auto it = bodies_.find(body);
    if (it == bodies_.end()) fail(SM_ERR_INVALID_INPUT, "unknown body handle");
    return it->second;
}

SmBrep* Engine::copy_of(uint64_t body) const {
    SmBrep* src = get(body);
    SmBrep* dst = nullptr;
    check_status(SmApiBrepCopy(src, dst), "SmApiBrepCopy");
    SmTArray<SmFace*> sf, df;
    src->GetFaces(sf);
    dst->GetFaces(df);
    SmTArray<SmEdge*> se, de;
    src->GetEdges(se);
    dst->GetEdges(de);
    SmTArray<SmVertex*> sv, dv;
    src->GetVertices(sv);
    dst->GetVertices(dv);
    copy_tags(sf, df, kFaceTagAttr);
    copy_tags(se, de, kEdgeTagAttr);
    copy_tags(sv, dv, kVertexTagAttr);
    return dst;
}

uint64_t Engine::make_profile(rust::Slice<const CurveIn> curves, rust::Slice<const uint32_t> loop_sizes,
                              const Frame& frame) {
    Curves c;
    build_curves(curves, loop_sizes, c);
    SmBrep* brep = nullptr;
    check_status(SmApiCreatePlanarFaces(c.list, brep), "SmApiCreatePlanarFaces");
    c.owned = false;  // consumed on success
    Owned guard{{brep}};
    place(brep, frame);
    guard.release_all();
    return store(brep);
}

uint64_t Engine::extrude(rust::Slice<const CurveIn> curves, rust::Slice<const uint32_t> loop_sizes,
                         const Frame& frame, const V3& local_dir, double distance) {
    Curves c;
    build_curves(curves, loop_sizes, c);
    SmBrep* brep = nullptr;
    check_status(SmApiCreateLinearSweep(c.list, vec(local_dir), distance, TRUE, brep), "SmApiCreateLinearSweep");
    Owned guard{{brep}};
    place(brep, frame);
    guard.release_all();
    return store(brep);
}

uint64_t Engine::revolve(rust::Slice<const CurveIn> curves, rust::Slice<const uint32_t> loop_sizes,
                         const Frame& frame, const V3& local_origin, const V3& local_axis, double angle_deg) {
    Curves c;
    build_curves(curves, loop_sizes, c);
    SmBrep* brep = nullptr;
    check_status(SmApiCreateRotationalSweep(c.list, pt(local_origin), vec(local_axis), angle_deg, TRUE, brep),
                 "SmApiCreateRotationalSweep");
    Owned guard{{brep}};
    place(brep, frame);
    guard.release_all();
    return store(brep);
}

uint64_t Engine::boolean_op(uint8_t kind, uint64_t target, uint64_t tool) {
    SmBrep* a = copy_of(target);
    SmBrep* b = nullptr;
    try {
        b = copy_of(tool);
    } catch (...) {
        delete a;
        throw;
    }
    SmBrep* result = nullptr;
    SmApiStatus status = SM_ERR_INVALID_INPUT;
    const char* call = "boolean";
    switch (kind) {
        case 0: status = SmApiBooleanUnion(a, b, result); call = "SmApiBooleanUnion"; break;
        case 1: status = SmApiBooleanDifference(a, b, result); call = "SmApiBooleanDifference"; break;
        case 2: status = SmApiBooleanIntersection(a, b, result); call = "SmApiBooleanIntersection"; break;
        default: break;
    }
    if (status != SM_SUCCESS || result == nullptr) {
        // On failure both inputs stay allocated; they are private copies.
        delete a;
        delete b;
        fail(status == SM_SUCCESS ? SM_ERR : status, std::string(call) + (result ? "" : " returned no body"));
    }
    // On success the tool copy is deleted by the kernel and the result aliases the target copy.
    return store(result);
}

uint64_t Engine::blend(uint64_t body, rust::Slice<const uint64_t> edge_tags, double size, bool chamfer) {
    SmBrep* work = copy_of(body);
    Owned guard{{work}};
    SmTArray<SmEdge*> all, chosen;
    work->GetEdges(all);
    for (uint64_t want : edge_tags) {
        SmEdge* found = nullptr;
        for (ULONG i = 0; i < all.GetSize(); ++i)
            if (read_tag(all[i], kEdgeTagAttr) == want) found = all[i];
        if (found == nullptr) fail(SM_ERR_INVALID_INPUT, "edge tag not on body");
        chosen.Add(found);
    }
    // Cross section 0 is linear (chamfer), 1 is circular. G1 continuity, unit thumbweight.
    check_status(SmApiFilletEdges(work, chosen, size, chamfer ? 0 : 1, 1, 1.0), "SmApiFilletEdges");
    guard.release_all();
    return store(work);
}

uint64_t Engine::shell(uint64_t body, rust::Slice<const uint64_t> open_face_tags, double thickness) {
    SmBrep* src = get(body);
    SmTArray<SmFace*> all;
    src->GetFaces(all);
    SmTArray<const SmFace*> chosen;
    for (uint64_t want : open_face_tags) {
        const SmFace* found = nullptr;
        for (ULONG i = 0; i < all.GetSize(); ++i)
            if (read_tag(all[i], kFaceTagAttr) == want) found = all[i];
        if (found == nullptr) fail(SM_ERR_INVALID_INPUT, "face tag not on body");
        chosen.Add(found);
    }
    SmBrep* result = nullptr;
    // A negative offset shells inward, keeping the outer skin where it is.
    check_status(SmApiShellBrepFull(src, -thickness, TRUE, TRUE, FALSE, chosen, result), "SmApiShellBrepFull");
    if (result == nullptr) fail(SM_ERR, "SmApiShellBrepFull returned no body");
    return store(result);
}

Snapshot Engine::snapshot(uint64_t body) const {
    SmBrep* brep = get(body);
    SmTArray<SmFace*> faces;
    SmTArray<SmEdge*> edges;
    SmTArray<SmVertex*> vertices;
    brep->GetFaces(faces);
    brep->GetEdges(edges);
    brep->GetVertices(vertices);
    std::unordered_map<const SmFace*, uint32_t> face_index;
    std::unordered_map<const SmEdge*, uint32_t> edge_index;
    std::unordered_map<const SmVertex*, uint32_t> vertex_index;
    for (ULONG i = 0; i < faces.GetSize(); ++i) face_index[faces[i]] = i;
    for (ULONG i = 0; i < edges.GetSize(); ++i) edge_index[edges[i]] = i;
    for (ULONG i = 0; i < vertices.GetSize(); ++i) vertex_index[vertices[i]] = i;

    Snapshot out;
    SmBoolean manifold = FALSE;
    check_status(SmApiBrepIsManifoldSolid(brep, manifold), "SmApiBrepIsManifoldSolid");
    out.manifold_solid = manifold != FALSE;
    SmPoint3d lo, hi;
    check_status(SmApiBrepBoundingBox(brep, TRUE, lo, hi), "SmApiBrepBoundingBox");
    out.bbox_min = v3(lo);
    out.bbox_max = v3(hi);

    for (ULONG i = 0; i < faces.GetSize(); ++i) {
        SmFace* f = faces[i];
        FaceRec rec;
        rec.tag = read_tag(f, kFaceTagAttr);
        rec.surface = surface_kind(f);
        SmPoint3d centroid;
        check_status(SmApiFaceComputeCentroid(f, kFaceAccuracy, rec.area, centroid), "SmApiFaceComputeCentroid");
        rec.centroid = v3(centroid);
        check_status(SmApiFaceBoundingBox(f, TRUE, lo, hi), "SmApiFaceBoundingBox");
        rec.bbox_min = v3(lo);
        rec.bbox_max = v3(hi);
        SmTArray<SmEdge*> fe;
        f->GetEdges(fe);
        for (ULONG j = 0; j < fe.GetSize(); ++j) {
            auto it = edge_index.find(fe[j]);
            if (it == edge_index.end()) fail(SM_ERR, "face edge missing from the body's edge list");
            rec.edge_indices.push_back(it->second);
        }
        rec.normal = V3{0, 0, 0};
        if (rec.surface == 0) rec.normal = v3(plane_normal(f));
        out.faces.push_back(std::move(rec));
    }
    for (ULONG i = 0; i < edges.GetSize(); ++i) {
        SmEdge* e = edges[i];
        EdgeRec rec;
        rec.tag = read_tag(e, kEdgeTagAttr);
        rec.curve = curve_kind(e);
        check_status(SmApiEdgeComputeLength(e, kEdgeAccuracy, rec.length), "SmApiEdgeComputeLength");
        double t0 = 0, t1 = 0;
        check_status(SmApiEdgeParameterRange(e, t0, t1), "SmApiEdgeParameterRange");
        SmPoint3d mid;
        if (e->GetCurve() == nullptr || e->GetCurve()->EvaluatePoint(0.5 * (t0 + t1), mid) != SM_SUCCESS)
            fail(SM_ERR, "edge curve evaluation");
        rec.midpoint = v3(mid);
        check_status(SmApiEdgeBoundingBox(e, TRUE, lo, hi), "SmApiEdgeBoundingBox");
        rec.bbox_min = v3(lo);
        rec.bbox_max = v3(hi);
        SmTArray<SmFace*> ef;
        e->GetFaces(ef);
        for (ULONG j = 0; j < ef.GetSize(); ++j) {
            auto it = face_index.find(ef[j]);
            if (it == face_index.end()) fail(SM_ERR, "edge face missing from the body's face list");
            rec.face_indices.push_back(it->second);
        }
        SmVertex* start = e->GetStartVertex();
        SmVertex* end = e->GetEndVertex();
        // A closed edge starts and ends at the same vertex; list it once.
        for (SmVertex* v : {start, end == start ? nullptr : end}) {
            if (v == nullptr) continue;
            auto it = vertex_index.find(v);
            if (it == vertex_index.end()) fail(SM_ERR, "edge vertex missing from the body's vertex list");
            rec.vertex_indices.push_back(it->second);
        }
        out.edges.push_back(std::move(rec));
    }
    for (ULONG i = 0; i < vertices.GetSize(); ++i) {
        SmVertex* v = vertices[i];
        VertexRec rec;
        rec.tag = read_tag(v, kVertexTagAttr);
        SmPoint3d p;
        check_status(SmApiVertexGetPoint(v, p), "SmApiVertexGetPoint");
        rec.point = v3(p);
        out.vertices.push_back(std::move(rec));
    }
    for (ULONG i = 0; i < edges.GetSize(); ++i)
        for (uint32_t vi : out.edges[i].vertex_indices) out.vertices[vi].edge_indices.push_back(i);
    return out;
}

void Engine::set_tags(uint64_t body, rust::Slice<const uint64_t> face_tags, rust::Slice<const uint64_t> edge_tags,
                      rust::Slice<const uint64_t> vertex_tags) {
    SmBrep* brep = get(body);
    SmTArray<SmFace*> faces;
    SmTArray<SmEdge*> edges;
    SmTArray<SmVertex*> vertices;
    brep->GetFaces(faces);
    brep->GetEdges(edges);
    brep->GetVertices(vertices);
    if (faces.GetSize() != face_tags.size() || edges.GetSize() != edge_tags.size() ||
        vertices.GetSize() != vertex_tags.size())
        fail(SM_ERR_INVALID_INPUT, "tag counts do not match the body");
    for (ULONG i = 0; i < vertices.GetSize(); ++i)
        if (read_tag(vertices[i], kVertexTagAttr) != vertex_tags[i])
            write_tag(vertices[i], kVertexTagAttr, vertex_tags[i]);
    for (ULONG i = 0; i < faces.GetSize(); ++i)
        if (read_tag(faces[i], kFaceTagAttr) != face_tags[i]) write_tag(faces[i], kFaceTagAttr, face_tags[i]);
    for (ULONG i = 0; i < edges.GetSize(); ++i)
        if (read_tag(edges[i], kEdgeTagAttr) != edge_tags[i]) write_tag(edges[i], kEdgeTagAttr, edge_tags[i]);
}

CheckOut Engine::check(uint64_t body) const {
    SmBrep* brep = get(body);
    CheckOut out;
    SmBoolean manifold = FALSE;
    check_status(SmApiBrepIsManifoldSolid(brep, manifold), "SmApiBrepIsManifoldSolid");
    out.manifold_solid = manifold != FALSE;
    out.face_count = brep->GetNumFaces();
    SmAssertArray asserts;
    out.assert_valid = brep->AssertValid(&asserts, SM_LEVEL_0, SM_WALK) != FALSE;
    for (ULONG i = 0; i < asserts.GetSize(); ++i) {
        const SmAssertReport* r = asserts[i];
        out.issues.push_back(std::string("SMLib AssertValid: ") + (r ? r->GetAssertTypeString() : "unknown"));
    }
    if (!out.assert_valid && out.issues.empty()) out.issues.push_back("SMLib AssertValid failed");
    return out;
}

double Engine::volume(uint64_t body) const {
    double v = 0;
    check_status(SmApiBrepComputeVolume(get(body), 1e-6, v), "SmApiBrepComputeVolume");
    return v;
}

MassOut Engine::mass(uint64_t body) const {
    SmBrep* brep = get(body);
    MassOut out{};
    SmBoolean manifold = FALSE;
    check_status(SmApiBrepIsManifoldSolid(brep, manifold), "SmApiBrepIsManifoldSolid");
    if (manifold) {
        double area = 0, vol = 0, mass = 0;
        SmPoint3d centroid;
        SmVector3d moments, products;
        // First pass finds the centroid; the second takes moments about it.
        check_status(SmApiBrepComputeMassProperties(brep, 1e-4, 1.0, SmPoint3d(0, 0, 0), area, vol, mass, centroid,
                                                    moments, products),
                     "SmApiBrepComputeMassProperties");
        SmPoint3d about = centroid;
        check_status(SmApiBrepComputeMassProperties(brep, 1e-4, 1.0, about, area, vol, mass, centroid, moments,
                                                    products),
                     "SmApiBrepComputeMassProperties");
        out.area = area;
        out.volume = vol;
        out.centroid = v3(centroid);
        out.moments = V3{moments.x, moments.y, moments.z};
        out.products = V3{products.x, products.y, products.z};
        return out;
    }
    // A sheet: area and area centroid only.
    SmTArray<SmFace*> faces;
    brep->GetFaces(faces);
    double total = 0;
    SmVector3d weighted(0, 0, 0);
    for (ULONG i = 0; i < faces.GetSize(); ++i) {
        double area = 0;
        SmPoint3d c;
        check_status(SmApiFaceComputeCentroid(faces[i], kFaceAccuracy, area, c), "SmApiFaceComputeCentroid");
        total += area;
        weighted += SmVector3d(c.x, c.y, c.z) * area;
    }
    out.area = total;
    if (total > 0) out.centroid = V3{weighted.x / total, weighted.y / total, weighted.z / total};
    return out;
}

namespace {

// One tessellation of brep. Face tags are read through `tags`, which the levels of one
// call share, so each face's attribute is read once however many levels there are.
MeshOut mesh_of(SmBrep* brep, const TessIn& in, std::unordered_map<SmFace*, uint64_t>& tags) {
    SmTessellationParams params;
    params.dChordHeightTol = in.chord_height;
    params.dCurveAngleTolDeg = in.curve_angle_deg;
    params.dSurfaceAngleTolDeg = in.surface_angle_deg;
    params.dMaxEdgeLength = in.max_edge_length;
    params.dMaxAspectRatio = in.max_aspect_ratio;
    SmPolyBrep* poly = nullptr;
    check_status(SmApiTessellate(brep, poly, params), "SmApiTessellate");
    if (poly == nullptr) fail(SM_ERR, "SmApiTessellate returned no mesh");
    SmObjDelete cleanup(poly);

    MeshOut out;
    SmTArray<SmPolyFace*> pfaces;
    poly->GetPolyFaces(pfaces);
    // Vertices are per source face, so each carries that face's normal.
    std::unordered_map<SmFace*, std::unordered_map<SmPolyVertex*, uint32_t>> index;
    for (ULONG i = 0; i < pfaces.GetSize(); ++i) {
        SmPolyFace* pf = pfaces[i];
        SmPolyLoop* loop = pf->GetOuterPolyLoop();
        if (loop == nullptr) continue;
        SmTArray<SmPolyEdge*> pedges;
        loop->GetPolyEdges(pedges);
        if (pedges.GetSize() < 3) continue;
        SmFace* source = pf->GetOriginalFace();
        uint64_t tag = 0;
        if (source) {
            auto known = tags.find(source);
            tag = known != tags.end() ? known->second : tags.emplace(source, read_tag(source, kFaceTagAttr)).first->second;
        }
        SmVector3d face_normal = unit(pf->GetNormal(TRUE, FALSE), SmVector3d(0, 0, 1));
        auto& local = index[source];
        std::vector<uint32_t> corner;
        for (ULONG j = 0; j < pedges.GetSize(); ++j) {
            SmPolyVertex* pv = pedges[j]->GetStartPolyVertex();
            auto it = local.find(pv);
            if (it == local.end()) {
                uint32_t idx = static_cast<uint32_t>(out.positions.size() / 3);
                const SmPoint3d& p = pv->GetPoint();
                SmVector3d n = source ? vertex_normal(pv, source, face_normal) : face_normal;
                for (double c : {p.x, p.y, p.z}) out.positions.push_back(float(c));
                for (double c : {n.x, n.y, n.z}) out.normals.push_back(float(c));
                it = local.emplace(pv, idx).first;
            }
            corner.push_back(it->second);
        }
        // Fan from the first corner; tessellation polygons are convex.
        for (size_t k = 1; k + 1 < corner.size(); ++k) {
            for (uint32_t c : {corner[0], corner[k], corner[k + 1]}) out.indices.push_back(c);
            out.triangle_face_tags.push_back(tag);
        }
    }
    return out;
}

}  // namespace

MeshOut Engine::tessellate(uint64_t body, const TessIn& in) const {
    std::unordered_map<SmFace*, uint64_t> tags;
    return mesh_of(get(body), in, tags);
}

rust::Vec<MeshOut> Engine::tessellate_levels(uint64_t body, rust::Slice<const TessIn> levels) const {
    SmBrep* brep = get(body);
    std::unordered_map<SmFace*, uint64_t> tags;
    rust::Vec<MeshOut> out;
    for (const TessIn& in : levels) out.push_back(mesh_of(brep, in, tags));
    return out;
}

EdgeSamples Engine::sample_edges(uint64_t body, double angle_deg, double chord_height) const {
    SmBrep* brep = get(body);
    SmTArray<SmEdge*> edges;
    brep->GetEdges(edges);
    const double max_turn = (angle_deg > 0 && std::isfinite(angle_deg) ? angle_deg : 15.0) * M_PI / 180.0;
    EdgeSamples out;
    for (ULONG i = 0; i < edges.GetSize(); ++i) {
        SmEdge* e = edges[i];
        const SmCurve* curve = e->GetCurve();
        if (curve == nullptr) fail(SM_ERR, "edge has no curve");
        double t0 = 0, t1 = 0;
        check_status(SmApiEdgeParameterRange(e, t0, t1), "SmApiEdgeParameterRange");
        auto eval = [&](double t) {
            SmPoint3d p;
            if (curve->EvaluatePoint(t, p) != SM_SUCCESS) fail(SM_ERR, "edge curve evaluation");
            return p;
        };
        auto tangent = [&](double t) {
            SmVector3d d;
            check_status(SmApiEdgeTangent(e, t, d), "SmApiEdgeTangent");
            return d;
        };
        std::vector<SmPoint3d> pts{eval(t0)};
        if (curve->GetType() == SmLine_TYPE) {
            pts.push_back(eval(t1));
        } else {
            // Halve each interval until the tangent turns little enough across it and the
            // chord stays close enough to the curve.
            std::function<void(double, double, const SmPoint3d&, const SmPoint3d&, int)> refine =
                [&](double a, double b, const SmPoint3d& pa, const SmPoint3d& pb, int depth) {
                    double m = 0.5 * (a + b);
                    SmPoint3d pm = eval(m);
                    bool split = false;
                    if (depth < 14) {
                        SmVector3d ta = tangent(a), tb = tangent(b);
                        double c = ta.x * tb.x + ta.y * tb.y + ta.z * tb.z;
                        if (std::acos(std::clamp(c, -1.0, 1.0)) > max_turn) split = true;
                        if (chord_height > 0) {
                            SmVector3d dev = pm - (pa + pb) * 0.5;
                            if (std::sqrt(dev.x * dev.x + dev.y * dev.y + dev.z * dev.z) > chord_height) split = true;
                        }
                    }
                    if (split) {
                        refine(a, m, pa, pm, depth + 1);
                        refine(m, b, pm, pb, depth + 1);
                    } else {
                        pts.push_back(pb);
                    }
                };
            const int initial = 4;
            SmPoint3d prev = pts.front();
            for (int k = 0; k < initial; ++k) {
                double a = t0 + (t1 - t0) * k / initial, b = t0 + (t1 - t0) * (k + 1) / initial;
                SmPoint3d pb = eval(b);
                refine(a, b, prev, pb, 0);
                prev = pb;
            }
        }
        // Run from the start vertex, whichever way the curve is parameterised.
        if (SmVertex* start = e->GetStartVertex()) {
            SmPoint3d s = start->GetPoint();
            auto d2 = [&](const SmPoint3d& p) {
                return (p.x - s.x) * (p.x - s.x) + (p.y - s.y) * (p.y - s.y) + (p.z - s.z) * (p.z - s.z);
            };
            if (d2(pts.back()) < d2(pts.front())) std::reverse(pts.begin(), pts.end());
        }
        out.tags.push_back(read_tag(e, kEdgeTagAttr));
        out.counts.push_back(static_cast<uint32_t>(pts.size()));
        for (const SmPoint3d& p : pts)
            for (double c : {p.x, p.y, p.z}) out.points.push_back(c);
    }
    return out;
}

void Engine::release(uint64_t body) {
    auto it = bodies_.find(body);
    if (it == bodies_.end()) return;
    delete it->second;
    bodies_.erase(it);
}

uint64_t Engine::body_count() const { return bodies_.size(); }

}  // namespace soliddraft_smlib
