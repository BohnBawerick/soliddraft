#!/usr/bin/env python3
"""Check a labelled graded-field STL against the field's own formula.

The grade is linear along one part axis, clamped beyond its anchors:
v(p) = start + (end - start) * clamp((p[axis] - from) / (to - from), 0, 1), with slope
K = |end - start| / |to - from|. A graded cubic lattice is every point of the box
[lo, hi] whose distance beta to the nearest grid line (cell size a, a node at the part
origin) is at most the radius v(p). A graded network gyroid is every point of the box
where g = sin X cos Y + sin Y cos Z + sin Z cos X, X = 2 pi x / a, is at most the level
v(p). Every expected position comes from these formulas, none from the kernel.

beta - v has Lipschitz constant 1 + K and g - v at most 4 pi sqrt(3) / a + K, so a mesh
within w of the boundary has every vertex inside a residual band of that constant times
w, or on a clipping face with the field below the band. Exact boundary points, found
from the formula alone, must lie within w of the mesh, and points inside the material on
each clipping face must meet facets facing out of the box. The mesh is a sphere: one
closed, oriented, edge-manifold component with Euler characteristic 2. Its bounding
box lies within w of the designed tight box. Exit status 0 means every check passed;
the JSON report names each.
"""

import argparse
import importlib.util
import json
import math
import pathlib
import sys
from fractions import Fraction
from itertools import product

SLACK = 1e-8  # binary64 arithmetic in this checker, far below every margin in use


def _load(name, file):
    spec = importlib.util.spec_from_file_location(name, pathlib.Path(__file__).with_name(file))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


_gyroid = _load("check_gyroid_stl", "check-gyroid-stl.py")
read, label, weld, topology, signed_volume = (_gyroid.read, _gyroid.label, _gyroid.weld, _gyroid.topology,
                                              _gyroid.signed_volume)
area_vector, Triangles = _gyroid.area_vector, _gyroid.Triangles
AXES = "xyz"


class Grade:
    def __init__(self, axis, start_at, end_at, start, end):
        self.axis = AXES.index(axis)
        self.start_at, self.end_at = float(Fraction(start_at)), float(Fraction(end_at))
        self.start, self.end = float(Fraction(start)), float(Fraction(end))
        self.slope = abs(self.end - self.start) / abs(self.end_at - self.start_at)

    def __call__(self, p):
        t = (p[self.axis] - self.start_at) / (self.end_at - self.start_at)
        return self.start + (self.end - self.start) * min(1.0, max(0.0, t))


def lattice_field(cell):
    """beta(p): the distance to the nearest line of the cubic grid."""
    def beta(p):
        near = [abs(c - cell * round(c / cell)) for c in p]
        return min(math.hypot(near[1], near[2]), math.hypot(near[0], near[2]), math.hypot(near[0], near[1]))
    return beta, 1.0


def gyroid_field(period):
    def g(p):
        x, y, z = (2 * math.pi * v / period for v in p)
        return math.sin(x) * math.cos(y) + math.sin(y) * math.cos(z) + math.sin(z) * math.cos(x)
    return g, 4 * math.pi * math.sqrt(3) / period


def root(f, a, b):
    """The one sign change of f on [a, b] by bisection, when f(a) < 0 < f(b)."""
    for _ in range(80):
        m = (a + b) / 2
        if f(m) < 0:
            a = m
        else:
            b = m
    return (a + b) / 2


def boundary_samples(residual, lo, hi, axis, n=13):
    """Exact boundary points: along lines parallel to the grade axis through an n by n
    grid of the other two coordinates, where residual changes sign once from below."""
    others = [i for i in range(3) if i != axis]
    points = []
    for i, j in product(range(1, n), repeat=2):
        base = [0.0, 0.0, 0.0]
        base[others[0]] = lo[others[0]] + (hi[others[0]] - lo[others[0]]) * i / n
        base[others[1]] = lo[others[1]] + (hi[others[1]] - lo[others[1]]) * j / n

        def along(t, base=base):
            p = list(base)
            p[axis] = t
            return p

        f = lambda t: residual(along(t))  # noqa: E731
        a, b = lo[axis], hi[axis]
        if f(a) < 0 < f(b):
            points.append(tuple(along(root(f, a, b))))
        elif f(b) < 0 < f(a):
            points.append(tuple(along(root(lambda t: -f(t), a, b))))
    return points


def radial_samples(residual, lo, hi, axis, n=12):
    """Exact boundary points of a beam along the grade axis: in each of n directions
    about the axis, at nine stations, the radius where the residual changes sign."""
    others = [i for i in range(3) if i != axis]
    points = []
    reach = max(abs(lo[o]) for o in others) + max(abs(hi[o]) for o in others)
    for station in range(1, 10):
        t = lo[axis] + (hi[axis] - lo[axis]) * station / 10
        for k in range(n):
            direction = (math.cos(2 * math.pi * k / n), math.sin(2 * math.pi * k / n))

            def at(s, t=t, direction=direction):
                p = [0.0, 0.0, 0.0]
                p[axis] = t
                p[others[0]], p[others[1]] = s * direction[0], s * direction[1]
                return p

            f = lambda s: residual(at(s))  # noqa: E731
            if f(0.0) < 0 < f(reach):
                points.append(tuple(at(root(f, 0.0, reach))))
    return points


def face_samples(residual, lo, hi, band, n=7):
    """Points on each clipping face at least the band inside the material, so the field's
    own boundary is more than w away, with that face's outward axis. A coarse bound can
    leave no such point."""
    out = []
    for axis in range(3):
        others = [i for i in range(3) if i != axis]
        for sign, value in ((-1, lo[axis]), (1, hi[axis])):
            found = 0
            for i, j in product(range(1, n), repeat=2):
                p = [0.0, 0.0, 0.0]
                p[axis] = value
                p[others[0]] = lo[others[0]] + (hi[others[0]] - lo[others[0]]) * i / n
                p[others[1]] = lo[others[1]] + (hi[others[1]] - lo[others[1]]) * j / n
                if residual(p) < -band and found < 3:
                    out.append((tuple(p), (axis, sign)))
                    found += 1
    return out


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("stl")
    parser.add_argument("--kind", choices=["lattice", "gyroid"], required=True)
    parser.add_argument("--box", nargs=6, type=float, required=True, metavar=("X0", "Y0", "Z0", "X1", "Y1", "Z1"))
    parser.add_argument("--tight", nargs=6, type=float, required=True, metavar=("X0", "Y0", "Z0", "X1", "Y1", "Z1"),
                        help="the designed bounding box of the material")
    parser.add_argument("--cell", type=float, help="lattice cell size")
    parser.add_argument("--period", type=float, help="gyroid period")
    parser.add_argument("--grade", required=True, metavar="AXIS,FROM,TO,START,END",
                        help="the axis, exact anchors and exact values, such as --grade=z,-1,1,-1/10,1/10")
    parser.add_argument("--accuracy", required=True, help="exact requested accuracy, such as 1/10")
    parser.add_argument("--caps", nargs=2, type=int, required=True, metavar=("CELLS", "VERTICES"))
    args = parser.parse_args()
    lo, hi = args.box[:3], args.box[3:]
    tight_lo, tight_hi = args.tight[:3], args.tight[3:]
    grade = Grade(*args.grade.split(","))
    field, lipschitz = lattice_field(args.cell) if args.kind == "lattice" else gyroid_field(args.period)

    def residual(p):
        return field(p) - grade(p)

    header, count, size, triangles = read(args.stl)
    checks, w = label(header, args.accuracy, args.caps)
    checks["length is 84 + 50 n"] = size == 84 + 50 * count and count > 0
    if not triangles:
        print(json.dumps({"triangles": count, "bytes": size, "checks": checks}, indent=2))
        sys.exit(1)
    checks["coordinates finite"] = all(math.isfinite(v) for t in triangles for p in t for v in p)
    eps = w + SLACK
    band = (lipschitz + grade.slope) * w + SLACK

    points, faces = weld(triangles)
    degenerate, paired, cycles, components = topology(points, faces)
    edges = 3 * len(faces) // 2
    euler = len(points) - edges + len(faces)
    checks["no degenerate facet"] = degenerate == 0
    checks["every edge once in each direction"] = paired
    checks["every vertex link is one cycle"] = cycles
    checks["one component"] = components == 1
    checks["Euler characteristic 2"] = euler == 2
    checks["vertices within the vertex cap"] = len(points) <= args.caps[1]
    volume = signed_volume(points, faces)
    checks["outward orientation"] = volume > 0

    mins = [min(p[i] for p in points) for i in range(3)]
    maxs = [max(p[i] for p in points) for i in range(3)]
    checks["bounding box within the bound of the tight box"] = all(
        abs(mins[i] - tight_lo[i]) <= eps and abs(maxs[i] - tight_hi[i]) <= eps for i in range(3))

    def near_face(p):
        return any(abs(p[i] - lo[i]) <= eps or abs(p[i] - hi[i]) <= eps for i in range(3))

    stray = [p for p in points
             if not (abs(residual(p)) <= band or (residual(p) <= band and near_face(p)))]
    checks["every vertex lies within the residual band"] = not stray

    if args.kind == "lattice":
        samples = radial_samples(residual, lo, hi, grade.axis)
    else:
        samples = boundary_samples(residual, lo, hi, grade.axis)
    caps = face_samples(residual, lo, hi, band)
    checks["boundary samples found"] = len(samples) >= 50
    index = Triangles(triangles, samples + [p for p, _ in caps], eps)
    missing, inward = [], []
    for p in samples:
        distance, _ = index.nearest(p)
        if distance > eps:
            missing.append(("boundary", p, distance))
    for p, (axis, sign) in caps:
        distance, facet = index.nearest(p)
        if distance > eps:
            missing.append(("face patch", p, distance))
        elif area_vector(*facet)[axis] * sign <= 0:
            inward.append(p)
    checks["every boundary sample is on the mesh"] = not missing
    checks["face patch normals point out of the box"] = not inward

    report = {"triangles": count, "vertices": len(points), "components": components, "euler": euler,
              "bound_mm": w, "band": band, "signed_volume_mm3": volume, "boundary_samples": len(samples),
              "face_samples": len(caps), "bounding_box": [mins, maxs], "stray_vertices": stray[:5],
              "missing": missing[:5], "inward": inward[:5], "checks": checks}
    print(json.dumps(report, indent=2))
    sys.exit(0 if all(checks.values()) else 1)


if __name__ == "__main__":
    main()
