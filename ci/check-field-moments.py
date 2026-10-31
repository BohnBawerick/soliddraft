#!/usr/bin/env python3
"""Check a field's proven moments, and its mass stand-in, against independent references.

usage: check-field-moments.py <report.json> --reference D0 [--step stand-in.step]
                              [--permutation N] [--max-work N]

<report.json> is a `soliddraft export --stand-in --json` line, whose report holds the
field's moments, or the `field_mass` tool's answer. The references are the V7d test
design's own: for D0-D2 the exact frustum moments as multiples of pi, enclosed by pi's
sixteen-digit bounds; for G0-G2 the exact volume and raw second moments, and centroid and
tensor intervals from an independent interval oracle. A returned enclosure must contain
each exact value, overlap each independent interval, and meet the requested width.
Every number is an exact rational; nothing is rounded to binary64.

With --step, the stand-in's proxy and its STEP-rounded bounds must lie inside the field's
enclosures, the file must hold one massless SHELL_BASED_SURFACE_MODEL and the reported
number of MANIFOLD_SOLID_BREP solids, and, where OpenCASCADE's Python binding is
installed, OpenCASCADE must read those solids as valid and their summed volume
properties, the envelope left out, must match the proxy within 1e-6 mm3, 1e-6 mm and
1e-5 mm5. That last comparison is a smoke check of the file, not a certified inclusion.

--permutation N reads the returned values in the design's frame after the cyclic
permutation (x, y, z) -> (z, x, y) applied N times.
"""

import argparse
import hashlib
import json
import pathlib
import re
import sys
from fractions import Fraction as Q

PI = (Q(3141592653589793, 10**15), Q(3141592653589794, 10**15))
WIDTHS = (Q(1, 100), Q(1, 1000), Q(1, 10))


def interval(lo, hi):
    return (Q(lo), Q(hi))


def exact(value):
    return (Q(value), Q(value))


def times_pi(coefficient):
    c = Q(coefficient)
    return (c * PI[0], c * PI[1])


def add(a, b):
    return (a[0] + b[0], a[1] + b[1])


def sub(a, b):
    return (a[0] - b[1], a[1] - b[0])


def mul(a, b):
    products = [a[0] * b[0], a[0] * b[1], a[1] * b[0], a[1] * b[1]]
    return (min(products), max(products))


def contains(outer, inner):
    return outer[0] <= inner[0] and inner[1] <= outer[1]


def meets(a, b):
    return a[0] <= b[1] and b[0] <= a[1]


def zero():
    return exact(0)


def d_state(v, cz, ixx, izz):
    return {"volume": times_pi(v), "centroid": [zero(), zero(), exact(cz)],
            "inertia": [[times_pi(ixx), zero(), zero()], [zero(), times_pi(ixx), zero()],
                        [zero(), zero(), times_pi(izz)]], "raw": None}


def g_state(v, raw, rows):
    r = [interval(*row) for row in rows]
    return {"volume": exact(v), "centroid": r[0:3],
            "inertia": [[r[3], r[6], r[7]], [r[6], r[4], r[8]], [r[7], r[8], r[5]]],
            "raw": [Q(x) for x in raw] + [Q(0)] * 3}


REFERENCES = {
    "D0": d_state("70/3", "365/14", "1321/7", "31"),
    "D1": d_state("94/3", "2569/94", "48517/141", "47"),
    "D2": d_state("70/3", "335/14", "1321/7", "31"),
    "G0": g_state("4", ["4/3", "4/3", "4/3"], [
        ("-7323723/25000000", "-140831983/500000000"),
        ("-293376209/1000000000", "-282357203/1000000000"),
        ("-118964903/500000000", "-113220591/500000000"),
        ("2095945899/1000000000", "2142661869/1000000000"),
        ("2096948019/1000000000", "2144225871/1000000000"),
        ("1979111989/1000000000", "2030425947/1000000000"),
        ("159059699/500000000", "171888487/500000000"),
        ("127560643/500000000", "139402559/500000000"),
        ("63937299/250000000", "279211777/1000000000")]),
    "G1": g_state("24/5", ["288/125", "8/5", "8/5"], [
        ("-10000177/25000000", "-386465873/1000000000"),
        ("-268183199/1000000000", "-128022813/500000000"),
        ("-43057467/200000000", "-203326091/1000000000"),
        ("52645997/20000000", "1343437931/500000000"),
        ("2913499361/1000000000", "1494326311/500000000"),
        ("2790746121/1000000000", "2872406877/1000000000"),
        ("29685869/62500000", "257460427/500000000"),
        ("377177257/1000000000", "206679499/500000000"),
        ("24989163/100000000", "277134941/1000000000")]),
    "G2": g_state("4", ["4/3", "4/3", "4/3"], [
        ("-25674223/100000000", "-49246889/200000000"),
        ("-256756431/1000000000", "-247014493/1000000000"),
        ("-6191257/20000000", "-18749169/62500000"),
        ("126228411/62500000", "2062633937/1000000000"),
        ("1009841873/500000000", "2064172967/1000000000"),
        ("2139304919/1000000000", "2180076419/1000000000"),
        ("243293907/1000000000", "131840437/500000000"),
        ("295468239/1000000000", "12716457/40000000"),
        ("148202127/500000000", "31792901/100000000")]),
}


def moments(field, reference, permutation, max_work):
    """Checks one field moment report; returns named checks."""
    axis = lambda i: (i + permutation) % 3  # noqa: E731
    ref = REFERENCES[reference]
    volume = interval(field["volume_mm3"]["lo"], field["volume_mm3"]["hi"])
    centroid = [interval(**field["centroid_mm"][axis(i)]) for i in range(3)]
    inertia = [[interval(**field["inertia_mm5"][axis(i)][axis(j)]) for j in range(3)] for i in range(3)]
    width = lambda x: x[1] - x[0]  # noqa: E731
    checks = {
        "scope is the field's own definition": field["scope"] == "original-field-definition",
        "requested widths are the design's": [Q(w) for w in field["requested_widths"]] == list(WIDTHS),
        "a positive volume is proven": volume[0] > 0,
        "volume width met": width(volume) <= WIDTHS[0],
        "centroid widths met": all(width(c) <= WIDTHS[1] for c in centroid),
        "inertia widths met": all(width(c) <= WIDTHS[2] for row in inertia for c in row),
        "work within the application allowance": 0 < field["work"] <= max_work,
        "volume contains the exact reference": contains(volume, ref["volume"]),
    }
    if ref["raw"] is None:
        checks["centroid contains the exact reference"] = all(contains(c, r) for c, r in zip(centroid, ref["centroid"]))
        checks["inertia contains the exact reference"] = all(
            contains(c, r) for row, rrow in zip(inertia, ref["inertia"]) for c, r in zip(row, rrow))
    else:
        checks["centroid overlaps the independent reference"] = all(
            meets(c, r) for c, r in zip(centroid, ref["centroid"]))
        checks["inertia overlaps the independent reference"] = all(
            meets(c, r) for row, rrow in zip(inertia, ref["inertia"]) for c, r in zip(row, rrow))
        # Raw second moments from the returned tensor, by the parallel-axis theorem.
        p = [add(inertia[i][i], mul(volume, add(mul(centroid[(i + 1) % 3], centroid[(i + 1) % 3]),
                                                mul(centroid[(i + 2) % 3], centroid[(i + 2) % 3]))))
             for i in range(3)]
        raw = [sub(add(p[(i + 1) % 3], p[(i + 2) % 3]), p[i]) for i in range(3)]
        raw = [(r[0] / 2, r[1] / 2) for r in raw]
        products = [sub(mul(volume, mul(centroid[i], centroid[j])), inertia[i][j]) for i, j in ((0, 1), (0, 2), (1, 2))]
        checks["raw second moments contain the exact references"] = all(
            contains(r, exact(x)) for r, x in zip(raw + products, ref["raw"]))
    return checks


def stand_in(report, step_path):
    field = report["field"]
    volume = interval(field["volume_mm3"]["lo"], field["volume_mm3"]["hi"])
    centroid = [interval(**c) for c in field["centroid_mm"]]
    inertia = [[interval(**c) for c in row] for row in field["inertia_mm5"]]
    exchanged = report["exchanged"]
    proxy = report["proxy"]
    data = pathlib.Path(step_path).read_bytes()
    text = data.decode()
    checks = {
        "labelled a mass stand-in, not field boundary geometry":
            report["label"] == "mass stand-in — not field boundary geometry",
        "a massless envelope and unit-density solids":
            report["representation"] == "massless-surface-envelope-plus-unit-density-solids",
        "no density or material metadata": report["density_metadata"] is False,
        "proxy inside the field enclosures": contains(volume, exact(proxy["volume_mm3"]))
            and all(contains(c, exact(x)) for c, x in zip(centroid, proxy["centroid_mm"]))
            and all(contains(c, exact(x)) for row, prow in zip(inertia, proxy["inertia_mm5"]) for c, x in zip(row, prow)),
        "STEP-rounded proxy inside the field enclosures":
            contains(volume, interval(**exchanged["volume_mm3"]))
            and all(contains(c, interval(**x)) for c, x in zip(centroid, exchanged["centroid_mm"]))
            and all(contains(c, interval(**x)) for row, xrow in zip(inertia, exchanged["inertia_mm5"])
                    for c, x in zip(row, xrow)),
        "inertia residual below 1e-9 mm5": Q(report["inertia_residual"]) < Q(1, 10**9),
        "report describes these bytes": hashlib.sha256(data).hexdigest() == report["sha256"],
        "one massless surface envelope": len(re.findall(r"=SHELL_BASED_SURFACE_MODEL\(", text)) == 1
            == report["surface_envelopes"],
        "every mass body is a solid": len(re.findall(r"=MANIFOLD_SOLID_BREP\(", text)) == report["mass_bodies"] >= 1,
    }
    seen = {}
    try:
        from OCP.BRepCheck import BRepCheck_Analyzer
        from OCP.BRepGProp import BRepGProp
        from OCP.GProp import GProp_GProps
        from OCP.IFSelect import IFSelect_RetDone
        from OCP.STEPControl import STEPControl_Reader
        from OCP.TopAbs import TopAbs_SOLID
        from OCP.TopExp import TopExp_Explorer
    except ImportError:
        seen["opencascade"] = "binding not installed; import check skipped"
        return checks, seen
    reader = STEPControl_Reader()
    if reader.ReadFile(str(step_path)) != IFSelect_RetDone:
        checks["OpenCASCADE reads the file"] = False
        return checks, seen
    reader.TransferRoots()
    shape = reader.OneShape()
    total = GProp_GProps()
    solids = 0
    valid = True
    explorer = TopExp_Explorer(shape, TopAbs_SOLID)
    while explorer.More():
        solid = explorer.Current()
        valid &= BRepCheck_Analyzer(solid).IsValid()
        props = GProp_GProps()
        BRepGProp.VolumeProperties_s(solid, props, 1e-12)
        total.Add(props)
        solids += 1
        explorer.Next()
    centre = total.CentreOfMass()
    matrix = total.MatrixOfInertia()
    tensor = [[matrix.Value(i + 1, j + 1) for j in range(3)] for i in range(3)]
    want_c = [float(Q(x)) for x in proxy["centroid_mm"]]
    want_i = [[float(Q(x)) for x in row] for row in proxy["inertia_mm5"]]
    seen = {"solids": solids, "volume": total.Mass(), "centroid": [centre.X(), centre.Y(), centre.Z()],
            "inertia": tensor}
    checks["OpenCASCADE reads the mass bodies alone as solids"] = solids == report["mass_bodies"]
    checks["OpenCASCADE finds the solids valid"] = bool(valid)
    checks["imported solid volume matches the proxy"] = abs(total.Mass() - float(Q(proxy["volume_mm3"]))) <= 1e-6
    checks["imported solid centroid matches the proxy"] = all(
        abs(a - b) <= 1e-6 for a, b in zip(seen["centroid"], want_c))
    checks["imported solid inertia matches the proxy"] = all(
        abs(a - b) <= 1e-5 for row, wrow in zip(tensor, want_i) for a, b in zip(row, wrow))
    return checks, seen


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("report", type=pathlib.Path)
    parser.add_argument("--reference", choices=sorted(REFERENCES), required=True)
    parser.add_argument("--step", type=pathlib.Path)
    parser.add_argument("--permutation", type=int, default=0)
    parser.add_argument("--max-work", type=int, default=19_999_999)
    args = parser.parse_args()
    text = args.report.read_text().strip()
    try:
        value = json.loads(text)
    except json.JSONDecodeError:
        # `soliddraft export --json` prints one line per file; the stand-in is the last.
        value = json.loads(text.splitlines()[-1])
    report = value["report"] if "report" in value else value
    field = report["field"] if "field" in report else report["field_mass"]
    checks = moments(field, args.reference, args.permutation, args.max_work)
    seen = {}
    if args.step is not None:
        more, seen = stand_in(report, args.step)
        checks.update(more)
        checks["stand-in work within the application allowance"] = 0 < report["work"] <= args.max_work
    print(json.dumps({"reference": args.reference, "checks": checks, "opencascade": seen}, indent=2, default=str))
    sys.exit(0 if all(checks.values()) else 1)


if __name__ == "__main__":
    main()
