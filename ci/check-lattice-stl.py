#!/usr/bin/env python3
"""Check a labelled lattice STL against a hand-derived cubic lattice.

The lattice is every line parallel to an axis through the points with all coordinates
in a*Z (the part origin is a node), thickened to radius r and clipped to the box
[lo, hi]. The expected Euler characteristic comes from the clipped beam graph, not
from the kernel. Exit status 0 means every check passed; the JSON report names each.
"""

import argparse
import json
import math
import struct
import sys
from collections import defaultdict
from fractions import Fraction

SLACK = 1e-9  # binary64 arithmetic in this checker, far below any accuracy in use


def read(path):
    data = open(path, "rb").read()
    if len(data) < 84:
        raise SystemExit("file shorter than an STL header")
    header = data[:80]
    (count,) = struct.unpack_from("<I", data, 80)
    triangles = []
    for i in range(count):
        values = struct.unpack_from("<12f", data, 84 + 50 * i)
        triangles.append((values[3:6], values[6:9], values[9:12]))
    return header, count, len(data), triangles


def label(header, accuracy):
    num, den = (int(x) for x in accuracy.split("/")) if "/" in accuracy else (int(accuracy), 1)
    n, d = struct.unpack_from("<qQ", header, 12)
    u, v, r, w = struct.unpack_from("<4d", header, 44)
    want_n, want_d = num // math.gcd(num, den), den // math.gcd(num, den)
    return {
        "magic": header[:8] == b"CXPRINT1",
        "field route": header[8] == 2,
        "millimetres": header[9] == 1,
        "binary32 coordinates": header[10] == 1,
        "cell cap applies": header[11] == 1,
        "requested accuracy": (n, d) == (want_n, want_d),
        "reserved zero": header[76:80] == b"\0\0\0\0",
        "achieved bound within request": 0 < Fraction(w) <= Fraction(want_n, want_d),
        "achieved bound covers its parts": Fraction(u) + Fraction(v) + Fraction(r) <= Fraction(w),
    }, w


def topology(triangles):
    index, faces = {}, []
    for t in triangles:
        faces.append(tuple(index.setdefault(p, len(index)) for p in t))
    directed = defaultdict(int)
    for a, b, c in faces:
        for e in ((a, b), (b, c), (c, a)):
            directed[e] += 1
    undirected = {tuple(sorted(e)) for e in directed}
    closed = all(directed[(a, b)] == 1 and directed.get((b, a)) == 1 for a, b in directed)
    parent = list(range(len(index)))

    def root(x):
        while parent[x] != x:
            parent[x] = parent[parent[x]]
            x = parent[x]
        return x

    for a, b, c in faces:
        for x, y in ((a, b), (b, c)):
            parent[root(x)] = root(y)
    components = len({root(v) for f in faces for v in f})
    euler = len(index) - len(undirected) + len(faces)
    return closed, components, euler, list(index)


def line_distance(p, a):
    near = [abs(c - a * round(c / a)) for c in p]
    return min(math.hypot(near[1], near[2]), math.hypot(near[0], near[2]), math.hypot(near[0], near[1]))


def closest_on_triangle(p, a, b, c):
    # Ericson, Real-Time Collision Detection, 5.1.5.
    sub = lambda x, y: (x[0] - y[0], x[1] - y[1], x[2] - y[2])
    dot = lambda x, y: x[0] * y[0] + x[1] * y[1] + x[2] * y[2]
    add = lambda x, y, s: (x[0] + s * y[0], x[1] + s * y[1], x[2] + s * y[2])
    ab, ac, ap = sub(b, a), sub(c, a), sub(p, a)
    d1, d2 = dot(ab, ap), dot(ac, ap)
    if d1 <= 0 and d2 <= 0:
        return a
    bp = sub(p, b)
    d3, d4 = dot(ab, bp), dot(ac, bp)
    if d3 >= 0 and d4 <= d3:
        return b
    vc = d1 * d4 - d3 * d2
    if vc <= 0 and d1 >= 0 and d3 <= 0:
        return add(a, ab, d1 / (d1 - d3))
    cp = sub(p, c)
    d5, d6 = dot(ab, cp), dot(ac, cp)
    if d6 >= 0 and d5 <= d6:
        return c
    vb = d5 * d2 - d1 * d6
    if vb <= 0 and d2 >= 0 and d6 <= 0:
        return add(a, ac, d2 / (d2 - d6))
    va = d3 * d6 - d5 * d4
    if va <= 0 and d4 - d3 >= 0 and d5 - d6 >= 0:
        return add(b, sub(c, b), (d4 - d3) / ((d4 - d3) + (d5 - d6)))
    denom = 1 / (va + vb + vc)
    v, w = vb * denom, vc * denom
    return (a[0] + ab[0] * v + ac[0] * w, a[1] + ab[1] * v + ac[1] * w, a[2] + ab[2] * v + ac[2] * w)


def surface_samples(lo, hi, a, r):
    """One true-surface point per clipped beam: the surface point above each beam
    midpoint between two nodes, and the axis point where each stub meets the box."""
    nodes = [[a * k for k in range(math.ceil(lo[i] / a), math.floor(hi[i] / a) + 1)] for i in range(3)]
    samples = []
    for axis in range(3):
        u, v = [i for i in range(3) if i != axis]
        along = nodes[axis]
        for x in nodes[u]:
            for y in nodes[v]:
                for s, e in zip(along, along[1:]):
                    p = [0.0, 0.0, 0.0]
                    p[axis], p[u], p[v] = (s + e) / 2, x + r, y
                    samples.append(("beam", tuple(p)))
                for end in (lo[axis], hi[axis]):
                    p = [0.0, 0.0, 0.0]
                    p[axis], p[u], p[v] = end, x, y
                    samples.append(("stub end", tuple(p)))
    return samples


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("stl")
    parser.add_argument("--box", nargs=6, type=float, required=True, metavar=("X0", "Y0", "Z0", "X1", "Y1", "Z1"))
    parser.add_argument("--cell", type=float, required=True)
    parser.add_argument("--radius", type=float, required=True)
    parser.add_argument("--accuracy", required=True, help="exact requested accuracy, such as 1/10")
    parser.add_argument("--euler", type=int, required=True)
    parser.add_argument("--caps", nargs=2, type=int, metavar=("CELLS", "VERTICES"),
                        help="the cell and vertex caps the header must record")
    args = parser.parse_args()
    lo, hi, a, r = args.box[:3], args.box[3:], args.cell, args.radius

    header, count, size, triangles = read(args.stl)
    checks, eps = label(header, args.accuracy)
    eps += SLACK
    checks["length is 84 + 50 n"] = size == 84 + 50 * count and count > 0
    closed, components, euler, points = topology(triangles)
    checks["closed and oppositely paired"] = closed
    checks["one component"] = components == 1
    checks["Euler characteristic"] = euler == args.euler
    if args.caps:
        checks["header records the caps"] = struct.unpack_from("<QQ", header, 28) == tuple(args.caps)
        checks["vertices within the vertex cap"] = len(points) <= args.caps[1]

    mins = [min(p[i] for p in points) for i in range(3)]
    maxs = [max(p[i] for p in points) for i in range(3)]
    checks["bounding box within the bound of the box"] = all(
        abs(mins[i] - lo[i]) <= eps and abs(maxs[i] - hi[i]) <= eps for i in range(3))

    def near_face(p):
        return any(abs(p[i] - lo[i]) <= eps or abs(p[i] - hi[i]) <= eps for i in range(3))

    stray = [p for p in points if not (abs(line_distance(p, a) - r) <= eps
                                       or (near_face(p) and line_distance(p, a) <= r + eps))]
    checks["every vertex lies within the bound of the true surface"] = not stray

    # Coverage: every clipped beam has mesh within the bound of a true-surface point.
    grid, size_cell = defaultdict(list), max(4 * eps, 0.5)
    key = lambda p: tuple(int(math.floor(c / size_cell)) for c in p)
    for t in triangles:
        k0 = key([min(v[i] for v in t) - eps for i in range(3)])
        k1 = key([max(v[i] for v in t) + eps for i in range(3)])
        for i in range(k0[0], k1[0] + 1):
            for j in range(k0[1], k1[1] + 1):
                for k in range(k0[2], k1[2] + 1):
                    grid[(i, j, k)].append(t)
    missing = []
    samples = surface_samples(lo, hi, a, r)
    for kind, s in samples:
        best = min((math.dist(s, closest_on_triangle(s, *t)) for t in grid.get(key(s), [])), default=math.inf)
        if best > eps:
            missing.append((kind, s, best))
    checks["every clipped beam is present"] = not missing

    report = {"triangles": count, "vertices": len(points), "euler": euler, "components": components,
              "bound_mm": eps - SLACK, "samples": len(samples), "stray_vertices": stray[:5],
              "missing": missing[:5], "checks": checks}
    print(json.dumps(report, indent=2))
    sys.exit(0 if all(checks.values()) else 1)


if __name__ == "__main__":
    main()
