#!/usr/bin/env python3
"""Checks a STEP export against a scenario's independent expectations, read twice.

usage: check-step-oracle.py <catallax checkout> <pinned revision> <file.step> <oracle.json>

The direct no-healing read and the ordinary read of the file must each give exactly one
valid solid, the expected surface kinds, the volume and every material point. Volume is
integrated at Gauss-Kronrod epsilons 1e-6, 1e-7 and 1e-8. The last two must agree within
1e-10 relative; a failed or nonfinite integration fails. The allowance on the reference is
the oracle's `fixed` plus the file's area times its 1e-7 mm bound plus the observed spread,
and at least its `floor`. `inside` and `outside` are points strictly away from the boundary.

Runs under the kernel gate's OCP 8 binding, with the pinned checkout's reader.
"""

import importlib.metadata
import json
import math
import pathlib
import subprocess
import sys

FILE_BOUND = 1e-7
EPSILONS = (1e-6, 1e-7, 1e-8)
CONVERGENCE = 1e-10
SURFACE = {"GeomAbs_Plane": "plane", "GeomAbs_Cylinder": "cylinder", "GeomAbs_Cone": "cone",
           "GeomAbs_Sphere": "sphere", "GeomAbs_Torus": "torus", "GeomAbs_BSplineSurface": "spline"}


def solids(shape):
    from OCP.TopAbs import TopAbs_SOLID
    from OCP.TopExp import TopExp_Explorer
    from OCP.TopoDS import TopoDS

    found = []
    explorer = TopExp_Explorer(shape, TopAbs_SOLID)
    while explorer.More():
        found.append(TopoDS.Solid(explorer.Current()))
        explorer.Next()
    return found


def surfaces(solid):
    from OCP.BRepAdaptor import BRepAdaptor_Surface
    from OCP.TopAbs import TopAbs_FACE
    from OCP.TopExp import TopExp_Explorer
    from OCP.TopoDS import TopoDS

    kinds = {}
    explorer = TopExp_Explorer(solid, TopAbs_FACE)
    while explorer.More():
        kind = BRepAdaptor_Surface(TopoDS.Face(explorer.Current())).GetType().name
        kinds[SURFACE.get(kind, kind)] = kinds.get(SURFACE.get(kind, kind), 0) + 1
        explorer.Next()
    return kinds


def volumes(solid):
    from OCP.BRepGProp import BRepGProp
    from OCP.GProp import GProp_GProps

    found = []
    for epsilon in EPSILONS:
        properties = GProp_GProps()
        error = BRepGProp.VolumePropertiesGK_s(solid, properties, epsilon, True, False, False, False, False)
        if error < 0 or not math.isfinite(properties.Mass()):
            raise ValueError(f"volume integration failed at {epsilon}")
        found.append(properties.Mass())
    return found


def area(solid):
    from OCP.BRepGProp import BRepGProp
    from OCP.GProp import GProp_GProps

    properties = GProp_GProps()
    BRepGProp.SurfaceProperties_s(solid, properties, False, False)
    return properties.Mass()


def state(solid, point):
    from OCP.BRepClass3d import BRepClass3d_SolidClassifier
    from OCP.gp import gp_Pnt
    from OCP.TopAbs import TopAbs_IN, TopAbs_OUT

    classifier = BRepClass3d_SolidClassifier(solid)
    classifier.Perform(gp_Pnt(*point), FILE_BOUND)
    return {TopAbs_IN: "inside", TopAbs_OUT: "outside"}.get(classifier.State(), "boundary")


def failures(label, solid, oracle):
    from OCP.BRepCheck import BRepCheck_Analyzer

    found = []
    if not BRepCheck_Analyzer(solid).IsValid():
        found.append(f"{label}: invalid solid")
    kinds = surfaces(solid)
    if kinds != oracle["surfaces"]:
        found.append(f"{label}: surfaces {kinds}, expected {oracle['surfaces']}")
    values = volumes(solid)
    spread = max(values) - min(values)
    if abs(values[-1] - values[-2]) > CONVERGENCE * max(1, abs(values[-1])):
        found.append(f"{label}: volume did not converge: {values}")
    allowance = max(oracle.get("floor", 0.0), oracle.get("fixed", 0.0) + area(solid) * FILE_BOUND + spread)
    if abs(values[-1] - oracle["volume"]) > allowance:
        found.append(f"{label}: volume {values[-1]}, expected {oracle['volume']} within {allowance}")
    for expected in ("inside", "outside"):
        for point in oracle.get(expected, []):
            actual = state(solid, point)
            if actual != expected:
                found.append(f"{label}: {point} is {actual}, expected {expected}")
    print(json.dumps({"read": label, "surfaces": kinds, "volumes": values, "allowance": allowance}))
    return found


def main():
    checkout, revision, step, oracle = sys.argv[1:]
    assert importlib.metadata.version("cadquery-ocp") == "8.0.1.1.0", "the oracle requires OCP 8.0.1.1.0"
    actual = subprocess.check_output(["git", "-C", checkout, "rev-parse", "HEAD"], text=True).strip()
    assert actual == revision, "the reader's checkout differs from the adapter pin"
    sys.path.insert(0, str(pathlib.Path(checkout) / "tools/checks"))
    from OCP.IFSelect import IFSelect_RetDone
    from OCP.STEPControl import STEPControl_Reader
    from raw_step import raw_shapes

    oracle = json.loads(pathlib.Path(oracle).read_text())
    if not oracle.get("inside") or not oracle.get("outside"):
        raise ValueError("an oracle needs inside and outside points")
    raw = raw_shapes(step)
    reader = STEPControl_Reader()
    if reader.ReadFile(str(step)) != IFSelect_RetDone:
        raise ValueError("the ordinary reader refused the file")
    reader.TransferRoots()
    ordinary = solids(reader.OneShape())
    errors = []
    for label, found in (("raw", raw), ("ordinary", ordinary)):
        if len(found) != 1:
            errors.append(f"{label}: {len(found)} solids, expected one")
        else:
            errors += failures(label, found[0], oracle)
    for error in errors:
        print(f"FAIL: {error}", file=sys.stderr)
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
