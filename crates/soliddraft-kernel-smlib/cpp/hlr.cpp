// SPDX-License-Identifier: MIT
//
// The drawing queries of the Engine (see engine.h): hidden-curve removal with SMLib's
// SmHCR, silhouettes, planar sections, exact edge curves, face loops and lines-and-arcs
// conversion.
//
// SmHCR splits each curve where it crosses the projected outline and decides each piece
// by casting one ray. It can miss a crossing, for example where an edge seen through a
// hole or slot passes behind the far rim, and then decides a whole piece by one point.
// The Rust side finds every crossing itself and asks hidden_points() about the new
// pieces, with SmHCR's own ray caster.
//
// SmHCR works on the exact B-rep and returns copies of curve intervals, not entities.
// Each copy is tagged here by geometry: when three interior points of it lie on one edge
// of the body it is a piece of that edge (or of a seam, when the edge is a seam);
// otherwise, when they lie on one face, it is that face's silhouette. Edge pieces are
// exact copies, so the edge test is tight; silhouettes are approximations within the
// HCR tolerance, so the face test allows that much.

#include "soliddraft-kernel-smlib/cpp/engine.h"
#include "soliddraft-kernel-smlib/src/ffi.rs.h"

#include <SmApiBrep.h>
#include <SmApiCurves.h>
#include <SmApiGeneral.h>
#include <SmApiIntersectors.h>
#include <SmApiQueries.h>

#include <SmAttribute.h>
#include <SmBSplineCurve.h>
#include <SmBrep.h>
#include <SmContext.h>
#include <SmCurve.h>
#include <SmEdge.h>
#include <SmExtent1d.h>
#include <SmFace.h>
#include <SmHCR.h>
#include <SmVertex.h>

#include <algorithm>
#include <cmath>
#include <stdexcept>
#include <string>
#include <vector>

namespace soliddraft_smlib {
namespace {

// The tag attributes engine.cpp keeps on faces and edges. Must match engine.cpp.
constexpr ULONG kFaceTagAttr = SM_AI_USER_1 + 701;
constexpr ULONG kEdgeTagAttr = SM_AI_USER_1 + 702;

// CurveOut::kind.
constexpr uint8_t kSpline = 0;
constexpr uint8_t kLine = 1;
constexpr uint8_t kPolyline = 2;

// CurveOut::source.
constexpr uint8_t kUnknown = 0;
constexpr uint8_t kEdge = 1;
constexpr uint8_t kSilhouette = 2;
constexpr uint8_t kSeam = 3;
constexpr uint8_t kSection = 4;

// The angle in degrees under which SmHCR counts an edge as smooth (tangent).
constexpr double kSmoothAngleDeg = 2.0;

// HCR's defaults when the caller gives none: walk silhouettes to 0.01 mm and 5 degrees.
constexpr double kDefaultApproxTol = 0.01;
constexpr double kDefaultAngleTolDeg = 5.0;
constexpr double kPi = 3.14159265358979323846;

[[noreturn]] void fail(long status, const std::string& what) {
    throw std::runtime_error(std::to_string(status) + "|" + what);
}

void check_status(SmApiStatus status, const char* call) {
    if (status != SM_SUCCESS) fail(status, call);
}

uint64_t read_tag(const SmAObject* object, ULONG attr) {
    const SmAttribute* a = object->FindAttribute(attr);
    if (a == nullptr) return 0;
    long value = static_cast<const SmLongAttribute*>(a)->GetValue();
    return value > 0 ? static_cast<uint64_t>(value) : 0;
}

const SmContext& context() { return *SmApiGetOrCreateContext(); }

SmVector3d vec(const V3& v) { return SmVector3d(v.x, v.y, v.z); }

void push(rust::Vec<double>& out, const SmPoint3d& p) {
    out.push_back(p.x);
    out.push_back(p.y);
    out.push_back(p.z);
}

// Curves this code owns, deleted when the scope exits.
struct OwnedCurves {
    SmTArray<SmCurve*> list;
    ~OwnedCurves() {
        for (ULONG i = 0; i < list.GetSize(); ++i) delete list[i];
    }
};

bool within(const SmPoint3d& p, const SmPoint3d& lo, const SmPoint3d& hi, double tol) {
    return p.x >= lo.x - tol && p.y >= lo.y - tol && p.z >= lo.z - tol && p.x <= hi.x + tol && p.y <= hi.y + tol &&
           p.z <= hi.z + tol;
}

// A curve's exact form. Every SMLib curve with an exact form is a B-spline (lines,
// circles and ellipses derive from SmBSplineCurve); anything else is sampled. The
// points at the start, middle and end of the parameter range go in `check`, so the
// Rust side can confirm the exact form reproduces the curve before trusting it.
CurveOut export_curve(const SmCurve* curve) {
    CurveOut out;
    out.kind = kPolyline;
    out.degree = 0;
    out.visible = true;
    out.source = kUnknown;
    out.tag = 0;
    out.smooth = false;
    SmExtent1d range = curve->GetNaturalInterval();
    for (double f : {0.0, 0.5, 1.0}) {
        SmPoint3d p;
        if (curve->EvaluatePoint(range.Evaluate(f), p) != SM_SUCCESS) fail(SM_ERR, "curve evaluation");
        push(out.check, p);
    }
    if (curve->GetType() == SmLine_TYPE) {
        out.kind = kLine;
        for (size_t i : {0, 1, 2, 6, 7, 8}) out.points.push_back(out.check[i]);
        return out;
    }
    if (const SmBSplineCurve* spline = SM_CAST_PTR(SmBSplineCurve, curve)) {
        SmTArray<double> knots, weights;
        SmTArray<SmPoint3d> points;
        ULONG degree = spline->GetDegree();
        if (spline->GetKnotsAll(knots) == SM_SUCCESS && spline->GetControlPolygon(points, weights) == SM_SUCCESS &&
            points.GetSize() > 0 && knots.GetSize() == points.GetSize() + degree + 1) {
            out.kind = kSpline;
            out.degree = static_cast<uint32_t>(degree);
            for (ULONG i = 0; i < knots.GetSize(); ++i) out.knots.push_back(knots[i]);
            for (ULONG i = 0; i < points.GetSize(); ++i) push(out.points, points[i]);
            if (spline->IsRational() && weights.GetSize() == points.GetSize())
                for (ULONG i = 0; i < weights.GetSize(); ++i) out.weights.push_back(weights[i]);
            return out;
        }
    }
    SmTArray<SmPoint3d> samples;
    if (curve->Tessellate(range, 0.0, 2.0, 2, nullptr, &samples) != SM_SUCCESS || samples.GetSize() < 2)
        fail(SM_ERR, "curve tessellation");
    for (ULONG i = 0; i < samples.GetSize(); ++i) push(out.points, samples[i]);
    return out;
}

// Finds the entity a curve lies on. Built once per query from the body's faces and
// edges, with loose bounding boxes to skip far entities quickly.
class Tagger {
public:
    Tagger(SmBrep* brep, double edge_tol, double face_tol) : edge_tol_(edge_tol), face_tol_(face_tol) {
        SmTArray<SmEdge*> edges;
        SmTArray<SmFace*> faces;
        brep->GetEdges(edges);
        brep->GetFaces(faces);
        for (ULONG i = 0; i < edges.GetSize(); ++i) {
            Entry e;
            e.edge = edges[i];
            check_status(SmApiEdgeBoundingBox(e.edge, FALSE, e.lo, e.hi), "SmApiEdgeBoundingBox");
            e.tag = read_tag(e.edge, kEdgeTagAttr);
            e.smooth = e.edge->IsTangentEdge(kSmoothAngleDeg) != FALSE;
            if (e.edge->IsSeam()) {
                SmTArray<SmFace*> ef;
                e.edge->GetFaces(ef);
                if (ef.GetSize() > 0) {
                    e.seam = true;
                    e.tag = read_tag(ef[0], kFaceTagAttr);
                }
            }
            edges_.push_back(e);
        }
        for (ULONG i = 0; i < faces.GetSize(); ++i) {
            Entry f;
            f.face = faces[i];
            check_status(SmApiFaceBoundingBox(f.face, FALSE, f.lo, f.hi), "SmApiFaceBoundingBox");
            f.tag = read_tag(f.face, kFaceTagAttr);
            faces_.push_back(f);
        }
    }

    // Sets out.source and out.tag. `on_face` is the source to report for a curve that
    // lies on a face but on no edge.
    void tag(const SmCurve* curve, CurveOut& out, uint8_t on_face) const {
        SmExtent1d range = curve->GetNaturalInterval();
        SmPoint3d samples[3];
        const double at[3] = {0.27, 0.5, 0.73};
        for (int i = 0; i < 3; ++i)
            if (curve->EvaluatePoint(range.Evaluate(at[i]), samples[i]) != SM_SUCCESS) return;
        if (on_face != kSection) {
            if (const Entry* e = best(edges_, samples, edge_tol_, true)) {
                out.source = e->seam ? kSeam : kEdge;
                out.tag = e->tag;
                out.smooth = e->smooth;
                return;
            }
        }
        if (const Entry* f = best(faces_, samples, face_tol_, false)) {
            out.source = on_face;
            out.tag = f->tag;
        }
    }

private:
    struct Entry {
        SmEdge* edge = nullptr;
        SmFace* face = nullptr;
        SmPoint3d lo, hi;
        uint64_t tag = 0;
        bool seam = false;
        bool smooth = false;
    };

    // The entity every sample lies within `tol` of, the nearest one when several do.
    static const Entry* best(const std::vector<Entry>& list, const SmPoint3d (&samples)[3], double tol, bool edges) {
        const Entry* found = nullptr;
        double found_dist = 0;
        for (const Entry& e : list) {
            bool near = true;
            for (const SmPoint3d& s : samples) near = near && within(s, e.lo, e.hi, tol);
            if (!near) continue;
            double worst = 0;
            for (const SmPoint3d& s : samples) {
                SmPoint3d closest;
                double dist = 0, param = 0;
                SmApiStatus st = edges ? SmApiEdgeClosestPoint(e.edge, s, closest, param, dist)
                                       : SmApiFaceClosestPoint(e.face, s, closest, dist);
                if (st != SM_SUCCESS) dist = 1e300;
                worst = std::max(worst, dist);
                if (worst > tol) break;
            }
            if (worst <= tol && (found == nullptr || worst < found_dist)) {
                found = &e;
                found_dist = worst;
            }
        }
        return found;
    }

    std::vector<Entry> edges_;
    std::vector<Entry> faces_;
    double edge_tol_;
    double face_tol_;
};

// The tolerance for "lies on an edge": a millionth of the body's size, at least 1e-7.
double edge_tolerance(SmBrep* brep) {
    SmPoint3d lo, hi;
    check_status(SmApiBrepBoundingBox(brep, FALSE, lo, hi), "SmApiBrepBoundingBox");
    double diag = std::sqrt((hi.x - lo.x) * (hi.x - lo.x) + (hi.y - lo.y) * (hi.y - lo.y) + (hi.z - lo.z) * (hi.z - lo.z));
    return std::max(1e-7, 1e-6 * diag);
}

SmEdge* edge_by_tag(SmBrep* brep, uint64_t tag) {
    SmTArray<SmEdge*> edges;
    brep->GetEdges(edges);
    for (ULONG i = 0; i < edges.GetSize(); ++i)
        if (read_tag(edges[i], kEdgeTagAttr) == tag) return edges[i];
    fail(SM_ERR_INVALID_INPUT, "edge tag not on body");
}

SmFace* face_by_tag(SmBrep* brep, uint64_t tag) {
    SmTArray<SmFace*> faces;
    brep->GetFaces(faces);
    for (ULONG i = 0; i < faces.GetSize(); ++i)
        if (read_tag(faces[i], kFaceTagAttr) == tag) return faces[i];
    fail(SM_ERR_INVALID_INPUT, "face tag not on body");
}

// The edge's curve over the edge's interval, exported.
CurveOut export_edge(const SmEdge* edge) {
    SmCurve* copy = nullptr;
    if (edge->GetCurve() == nullptr || edge->GetCurve()->Copy(context(), copy) != SM_SUCCESS || copy == nullptr)
        fail(SM_ERR, "edge curve copy");
    SmObjDelete cleanup(copy);
    SmExtent1d interval = edge->GetInterval();
    if (copy->Trim(interval) != SM_SUCCESS) fail(SM_ERR, "edge curve trim");
    return export_curve(copy);
}

// The transform SmHCR's view-vector constructor builds: it maps the view direction to
// +Z. Building it here lets hidden_points() map points into SmHCR's scene.
SmAxis2Placement view_transform(const V3& direction) {
    SmPoint3d origin(0, 0, 0);
    SmVector3d view = vec(direction), perp1, perp2;
    if (view.MakeUnitOrthoVectors(NULL, view, perp1, perp2) != SM_SUCCESS) fail(SM_ERR_INVALID_INPUT, "view direction");
    SmAxis2Placement transform;
    if (transform.SetSTEPCanonical(origin, view, perp1) != SM_SUCCESS) fail(SM_ERR, "view transform");
    transform.Invert(transform);
    return transform;
}

// An SmHCR for a view, built the way SmHCR's own view-vector constructor does it.
struct Hcr {
    SmAxis2Placement transform;
    SmHCR hcr;
    Hcr(SmBrep* brep, const ViewIn& view)
        : transform(view_transform(view.direction)),
          hcr(context(), brep, transform, view.approx_tol > 0 ? view.approx_tol : kDefaultApproxTol,
              (view.angle_tol_deg > 0 ? view.angle_tol_deg : kDefaultAngleTolDeg) * kPi / 180.0,
              view.seams ? TRUE : FALSE, view.smooth_edges ? TRUE : FALSE) {}
};

}  // namespace

rust::Vec<CurveOut> Engine::view_curves(uint64_t body, const ViewIn& view) const {
    SmBrep* brep = get(body);
    double approx = view.approx_tol > 0 ? view.approx_tol : kDefaultApproxTol;
    Hcr state(brep, view);
    SmHCR& hcr = state.hcr;
    check_status(hcr.ComputeGlobalVisibility(), "SmHCR::ComputeGlobalVisibility");
    // The curves stay owned by the SmHCR object.
    SmTArray<SmCurve*> visible, hidden;
    hcr.GetVisibleCurves(visible);
    hcr.GetInvisibleCurves(hidden);

    double edge_tol = edge_tolerance(brep);
    Tagger tagger(brep, edge_tol, 2 * approx + edge_tol);
    rust::Vec<CurveOut> out;
    for (int pass = 0; pass < 2; ++pass) {
        const SmTArray<SmCurve*>& list = pass == 0 ? visible : hidden;
        for (ULONG i = 0; i < list.GetSize(); ++i) {
            if (list[i] == nullptr) continue;
            CurveOut c = export_curve(list[i]);
            c.visible = pass == 0;
            tagger.tag(list[i], c, kSilhouette);
            out.push_back(std::move(c));
        }
    }
    return out;
}

rust::Vec<uint8_t> Engine::hidden_points(uint64_t body, const ViewIn& view, rust::Slice<const double> points) const {
    if (points.size() % 3 != 0) fail(SM_ERR_INVALID_INPUT, "points are not x, y, z triples");
    SmBrep* brep = get(body);
    Hcr state(brep, view);
    // SmHCR's scene is the body under `transform`. Which of the transform's two point
    // maps that is depends on SMLib's convention, so check both against the scene's
    // vertices (a copy keeps vertex order) and use the one that matches.
    SmTArray<SmVertex*> original, scene;
    brep->GetVertices(original);
    state.hcr.GetSceneBrep()->GetVertices(scene);
    if (original.GetSize() == 0 || original.GetSize() != scene.GetSize()) fail(SM_ERR, "SmHCR scene vertices");
    double tol = edge_tolerance(brep);
    bool forward = true, inverse = true;
    for (ULONG i = 0; i < original.GetSize(); ++i) {
        SmPoint3d p = original[i]->GetPoint(), want = scene[i]->GetPoint(), a, b;
        state.transform.TransformPoint(p, a);
        state.transform.InvTransformPoint(p, b);
        forward = forward && (a - want).LengthSquared() <= tol * tol;
        inverse = inverse && (b - want).LengthSquared() <= tol * tol;
    }
    if (!forward && !inverse) fail(SM_ERR, "cannot map points into the SmHCR scene");
    rust::Vec<uint8_t> out;
    for (size_t i = 0; i < points.size(); i += 3) {
        SmPoint3d p(points[i], points[i + 1], points[i + 2]), q;
        if (forward)
            state.transform.TransformPoint(p, q);
        else
            state.transform.InvTransformPoint(p, q);
        out.push_back(state.hcr.IsPointVisible(q) ? 0 : 1);
    }
    return out;
}

rust::Vec<CurveOut> Engine::silhouette_curves(uint64_t body, const V3& direction, double approx_tol) const {
    // SmApiCreateSilhouetteCurves takes a mutable body: work on a private copy.
    SmBrep* work = copy_of(body);
    SmObjDelete cleanup(work);
    OwnedCurves curves;
    check_status(SmApiCreateSilhouetteCurves(work, SmVector3d(0, 0, 0), vec(direction), curves.list),
                 "SmApiCreateSilhouetteCurves");
    double edge_tol = edge_tolerance(work);
    double approx = approx_tol > 0 ? approx_tol : kDefaultApproxTol;
    Tagger tagger(work, edge_tol, 2 * approx + edge_tol);
    rust::Vec<CurveOut> out;
    for (ULONG i = 0; i < curves.list.GetSize(); ++i) {
        if (curves.list[i] == nullptr) continue;
        CurveOut c = export_curve(curves.list[i]);
        tagger.tag(curves.list[i], c, kSilhouette);
        out.push_back(std::move(c));
    }
    return out;
}

rust::Vec<CurveOut> Engine::section_curves(uint64_t body, const V3& origin, const V3& normal) const {
    SmBrep* brep = get(body);
    OwnedCurves curves;
    check_status(SmApiIntersectBrepWithPlane(brep, vec(origin), vec(normal), curves.list),
                 "SmApiIntersectBrepWithPlane");
    double edge_tol = edge_tolerance(brep);
    // Section curves are fitted to the intersection; allow the default HCR tolerance.
    Tagger tagger(brep, edge_tol, kDefaultApproxTol + edge_tol);
    rust::Vec<CurveOut> out;
    for (ULONG i = 0; i < curves.list.GetSize(); ++i) {
        if (curves.list[i] == nullptr) continue;
        CurveOut c = export_curve(curves.list[i]);
        tagger.tag(curves.list[i], c, kSection);
        out.push_back(std::move(c));
    }
    return out;
}

CurveOut Engine::edge_curve(uint64_t body, uint64_t edge_tag) const {
    SmEdge* edge = edge_by_tag(get(body), edge_tag);
    CurveOut out = export_edge(edge);
    out.source = kEdge;
    out.tag = edge_tag;
    return out;
}

FaceLoopsOut Engine::face_loops(uint64_t body, uint64_t face_tag) const {
    SmFace* face = face_by_tag(get(body), face_tag);
    SmTArray<ULONG> counts;
    SmTArray<SmEdge*> edges;
    SmTArray<SmOrientType> orientations;
    check_status(SmApiFaceGetBoundaryLoops(face, counts, edges, orientations), "SmApiFaceGetBoundaryLoops");
    if (orientations.GetSize() != edges.GetSize()) fail(SM_ERR, "boundary loop orientations do not match edges");
    FaceLoopsOut out;
    for (ULONG i = 0; i < counts.GetSize(); ++i) out.loop_sizes.push_back(static_cast<uint32_t>(counts[i]));
    for (ULONG i = 0; i < edges.GetSize(); ++i) {
        CurveOut c = export_edge(edges[i]);
        c.source = kEdge;
        c.tag = read_tag(edges[i], kEdgeTagAttr);
        out.curves.push_back(std::move(c));
        out.reversed.push_back(orientations[i] != SM_OT_SAME ? 1 : 0);
    }
    return out;
}

rust::Vec<CurveOut> Engine::lines_and_arcs(const NurbsIn& curve) const {
    size_t n = curve.points.size() / 3;
    size_t p = curve.degree;
    if (n == 0 || curve.points.size() != 3 * n || curve.knots.size() != n + p + 1 ||
        (!curve.weights.empty() && curve.weights.size() != n))
        fail(SM_ERR_INVALID_INPUT, "malformed spline");
    // SM_API wants unique knots with multiplicities.
    SmTArray<double> knots;
    SmTArray<ULONG> mults;
    for (double k : curve.knots) {
        if (knots.GetSize() > 0 && k == knots.GetLast()) {
            mults[mults.GetSize() - 1] += 1;
        } else {
            knots.Add(k);
            mults.Add(1);
        }
    }
    SmTArray<SmPoint3d> points;
    for (size_t i = 0; i < n; ++i)
        points.Add(SmPoint3d(curve.points[3 * i], curve.points[3 * i + 1], curve.points[3 * i + 2]));
    SmTArray<double> weights;
    for (double w : curve.weights) weights.Add(w);
    SmBSplineCurve* spline = nullptr;
    check_status(SmApiCreateCanonicalCurve(points, knots, mults, curve.weights.empty() ? nullptr : &weights,
                                           static_cast<ULONG>(p), SM_CF_UNSPECIFIED, spline),
                 "SmApiCreateCanonicalCurve");
    if (spline == nullptr) fail(SM_ERR, "SmApiCreateCanonicalCurve returned no curve");
    SmObjDelete cleanup(spline);
    SmTArray<SmBSplineCurve*> pieces;
    SmApiStatus status = SmApiConvertToLinesAndArcs(spline, pieces);
    rust::Vec<CurveOut> out;
    try {
        check_status(status, "SmApiConvertToLinesAndArcs");
        for (ULONG i = 0; i < pieces.GetSize(); ++i) out.push_back(export_curve(pieces[i]));
    } catch (...) {
        for (ULONG i = 0; i < pieces.GetSize(); ++i) delete pieces[i];
        throw;
    }
    for (ULONG i = 0; i < pieces.GetSize(); ++i) delete pieces[i];
    return out;
}

}  // namespace soliddraft_smlib
