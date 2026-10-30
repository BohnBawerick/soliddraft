#!/usr/bin/env python3
"""Check a labelled network gyroid STL against the gyroid's formula.

With X = 2 pi x / a and its cyclic equivalents, the material is every point of the box
[lo, hi] where g = sin X cos Y + sin Y cos Z + sin Z cos X is at most the level c. Every
expected position comes from that formula, none from the kernel. g is not a signed
distance: |grad g| <= L = 4 pi sqrt(3) / a gives necessary conditions on a mesh within w of
the boundary, and fixed boundary points give coverage. The native certificate alone covers
the rest of the surface. Exit status 0 means every check passed; the JSON report names each.
"""

import argparse
import importlib.util
import json
import math
import pathlib
import struct
import sys
from collections import defaultdict
from fractions import Fraction
from itertools import product

SLACK = 1e-8  # binary64 trigonometry and distances in this checker, far below every margin in use
HEADER = 84
FACET = 50

_spec = importlib.util.spec_from_file_location("check_lattice_stl", pathlib.Path(__file__).with_name("check-lattice-stl.py"))
_lattice = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_lattice)
closest_on_triangle = _lattice.closest_on_triangle


def read(path):
    data = pathlib.Path(path).read_bytes()
    if len(data) < HEADER:
        raise SystemExit("file shorter than an STL header")
    (count,) = struct.unpack_from("<I", data, 80)
    if count == 0 or len(data) != HEADER + FACET * count:
        return data[:80], count, len(data), []
    triangles = []
    for i in range(count):
        values = struct.unpack_from("<9f", data, HEADER + FACET * i + 12)
        triangles.append((values[:3], values[3:6], values[6:9]))
    return data[:80], count, len(data), triangles


def label(header, accuracy, caps):
    want = Fraction(accuracy)
    n, d = struct.unpack_from("<qQ", header, 12)
    parts = struct.unpack_from("<4d", header, 44)
    usable = all(math.isfinite(v) and v >= 0 for v in parts)
    model, tessellation, rounding, w = parts if usable else (0.0, 0.0, 0.0, 0.0)
    return {
        "magic": header[:8] == b"CXPRINT1",
        "field route": header[8] == 2,
        "millimetres": header[9] == 1,
        "binary32 coordinates": header[10] == 1,
        "cell cap applies": header[11] == 1,
        "requested accuracy": (n, d) == (want.numerator, want.denominator),
        "header records the caps": struct.unpack_from("<QQ", header, 28) == tuple(caps),
        "bound terms finite and nonnegative": usable,
        "reserved zero": header[76:80] == b"\0\0\0\0",
        "achieved bound within request": 0 < Fraction(w) <= want,
        "achieved bound covers its parts": Fraction(model) + Fraction(tessellation) + Fraction(rounding) <= Fraction(w),
    }, w


def weld(triangles):
    index, faces = {}, []
    for t in triangles:
        faces.append(tuple(index.setdefault(p, len(index)) for p in t))
    return list(index), faces


def area_vector(a, b, c):
    u = (b[0] - a[0], b[1] - a[1], b[2] - a[2])
    v = (c[0] - a[0], c[1] - a[1], c[2] - a[2])
    return (u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0])


def topology(points, faces):
    directed = defaultdict(int)
    links = [[] for _ in points]
    degenerate = 0
    for a, b, c in faces:
        if len({a, b, c}) != 3 or not any(area_vector(points[a], points[b], points[c])):
            degenerate += 1
        for x, y, z in ((a, b, c), (b, c, a), (c, a, b)):
            directed[(x, y)] += 1
            links[x].append((y, z))
    paired = all(count == 1 and directed.get((b, a)) == 1 for (a, b), count in directed.items())

    def single_cycle(link):
        after = dict(link)
        if len(after) != len(link) or not link:
            return False
        start = link[0][0]
        seen, current = 0, start
        while True:
            current = after.get(current)
            seen += 1
            if current is None or seen > len(link):
                return False
            if current == start:
                return seen == len(link)

    cycles = all(single_cycle(link) for link in links)
    parent = list(range(len(points)))

    def root(x):
        while parent[x] != x:
            parent[x] = parent[parent[x]]
            x = parent[x]
        return x

    for a, b, c in faces:
        for x, y in ((a, b), (b, c)):
            parent[root(x)] = root(y)
    components = len({root(v) for v in range(len(points))})
    return degenerate, paired, cycles, components


def signed_volume(points, faces):
    total = 0.0
    for a, b, c in faces:
        p, q, r = points[a], points[b], points[c]
        total += (p[0] * (q[1] * r[2] - q[2] * r[1]) + p[1] * (q[2] * r[0] - q[0] * r[2])
                  + p[2] * (q[0] * r[1] - q[1] * r[0]))
    return total / 6


def boundary_points(lo, hi, a, level):
    """The D4 construction: exact points on g = c, by periodicity and cyclic symmetry."""
    tenth = Fraction(a) / 10
    if level == 0:
        seeds, steps = (Fraction(0), 5 * tenth), (-1, 0)
    elif level == Fraction(1, 2):
        seeds, steps = (5 * tenth / 6, 25 * tenth / 6), (-1, 0)
    elif level == Fraction(-1, 2):
        seeds, steps = (-5 * tenth / 6, -25 * tenth / 6), (-1, 0, 1)
    else:
        raise ValueError(f"no fixed boundary points for level {level}")
    points = set()
    for seed in seeds:
        for point in ((seed, 0, 0), (0, seed, 0), (0, 0, seed)):
            for i, j, k in product(steps, steps, (0, 1)):
                p = tuple(float(c + 10 * tenth * t) for c, t in zip(point, (i, j, k)))
                if all(lo[n] <= p[n] <= hi[n] for n in range(3)):
                    points.add(p)
    return sorted(points)


def face_points(lo, hi, a):
    """One point on each clipping face where g = -1, so material reaches that face."""
    q = -a / 4
    return [((face, q, 0.0), (0, s)) for s, face in ((-1, lo[0]), (1, hi[0]))] \
        + [((0.0, face, q), (1, s)) for s, face in ((-1, lo[1]), (1, hi[1]))] \
        + [((q, 0.0, face), (2, s)) for s, face in ((-1, lo[2]), (1, hi[2]))]


class Triangles:
    """Triangles by grid cell, indexed only where a sample needs them."""

    def __init__(self, triangles, wanted, reach):
        self.size = max(4 * reach, 0.5)
        self.grid = defaultdict(list)
        cells = {self.key(p) for p in wanted}
        for t in triangles:
            k0 = self.key([min(v[i] for v in t) - reach for i in range(3)])
            k1 = self.key([max(v[i] for v in t) + reach for i in range(3)])
            for cell in product(*(range(k0[i], k1[i] + 1) for i in range(3))):
                if cell in cells:
                    self.grid[cell].append(t)

    def key(self, p):
        return tuple(int(math.floor(c / self.size)) for c in p)

    def nearest(self, p):
        return min(((math.dist(p, closest_on_triangle(p, *t)), t) for t in self.grid.get(self.key(p), [])),
                   default=(math.inf, None), key=lambda found: found[0])


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("stl")
    parser.add_argument("--box", nargs=6, type=float, required=True, metavar=("X0", "Y0", "Z0", "X1", "Y1", "Z1"))
    parser.add_argument("--period", type=float, required=True)
    parser.add_argument("--level", required=True, help="exact constant level, such as 1/2; write --level=-1/2")
    parser.add_argument("--accuracy", required=True, help="exact requested accuracy, such as 1/10")
    parser.add_argument("--caps", nargs=2, type=int, required=True, metavar=("CELLS", "VERTICES"),
                        help="the cell and vertex caps the header must record")
    args = parser.parse_args()
    lo, hi, a, c = args.box[:3], args.box[3:], args.period, float(Fraction(args.level))
    lipschitz = 4 * math.pi * math.sqrt(3) / a

    header, count, size, triangles = read(args.stl)
    checks, w = label(header, args.accuracy, args.caps)
    checks["length is 84 + 50 n"] = size == HEADER + FACET * count and count > 0
    if not triangles:
        print(json.dumps({"triangles": count, "bytes": size, "checks": checks}, indent=2))
        sys.exit(1)
    checks["coordinates finite"] = all(math.isfinite(v) for t in triangles for p in t for v in p)
    eps = w + SLACK
    band = lipschitz * w + SLACK

    points, faces = weld(triangles)
    degenerate, paired, cycles, components = topology(points, faces)
    checks["no degenerate facet"] = degenerate == 0
    checks["every edge once in each direction"] = paired
    checks["every vertex link is one cycle"] = cycles
    checks["one component"] = components == 1
    checks["vertices within the vertex cap"] = len(points) <= args.caps[1]
    volume = signed_volume(points, faces)
    checks["outward orientation"] = volume > 0

    mins = [min(p[i] for p in points) for i in range(3)]
    maxs = [max(p[i] for p in points) for i in range(3)]
    checks["bounding box within the bound of the box"] = all(
        abs(mins[i] - lo[i]) <= eps and abs(maxs[i] - hi[i]) <= eps for i in range(3))

    def g(p):
        x, y, z = (2 * math.pi * v / a for v in p)
        return math.sin(x) * math.cos(y) + math.sin(y) * math.cos(z) + math.sin(z) * math.cos(x)

    def near_face(p):
        return any(abs(p[i] - lo[i]) <= eps or abs(p[i] - hi[i]) <= eps for i in range(3))

    stray = []
    for p in points:
        value = g(p)
        if not (abs(value - c) <= band or (value <= c + band and near_face(p))):
            stray.append(p)
    checks["every vertex lies within the residual band"] = not stray

    samples = boundary_points(lo, hi, a, Fraction(args.level))
    caps = face_points(lo, hi, a)
    checks["face patches are material"] = all(g(p) < c for p, _ in caps)
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
    checks["cap normals point out of the box"] = not inward

    report = {"triangles": count, "vertices": len(points), "components": components, "bound_mm": w,
              "signed_volume_mm3": volume, "boundary_samples": len(samples), "face_samples": len(caps),
              "stray_vertices": stray[:5], "missing": missing[:5], "inward": inward[:5], "checks": checks}
    print(json.dumps(report, indent=2))
    sys.exit(0 if all(checks.values()) else 1)


if __name__ == "__main__":
    main()
