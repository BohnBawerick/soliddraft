#!/usr/bin/env python3
"""Opens a SolidDraft STEP export in OpenCASCADE and checks it against the export report.

usage: check-step.py <file.step> <report.json>

<report.json> is the acceptance test's STEP report and exact-body measurements. The check
passes when OpenCASCADE reads the file into one valid solid whose face count equals both
the report's and the exact body's less the faces the report says the export removed, and
whose volume is within the reported deviation of the exact volume. Each removed count
must be a whole number from 0 to the exact body's count. A surface moved by at most d changes the enclosed volume by at most
area * d to first order in d, so the allowance is area * d, plus OpenCASCADE's own
estimate of its integration error, plus the exact volume's measurement width (a
relative 1e-12 around Catallax's proven enclosure).

This checker belongs to CI. The application never runs OpenCASCADE.
"""

import json
import sys

from OCP.BRepCheck import BRepCheck_Analyzer
from OCP.BRepGProp import BRepGProp
from OCP.GProp import GProp_GProps
from OCP.IFSelect import IFSelect_RetDone
from OCP.STEPControl import STEPControl_Reader
from OCP.TopAbs import TopAbs_FACE, TopAbs_SOLID
from OCP.TopExp import TopExp
from OCP.TopTools import TopTools_IndexedMapOfShape

# Catallax's default measurement accuracy, relative to the value.
MEASUREMENT_WIDTH = 1e-12
# The relative accuracy requested from OpenCASCADE's adaptive integration.
INTEGRATION_EPS = 1e-12
# What the report says export-only repairs left out of the file, and the exact count each
# one comes off.
REMOVED_COUNTS = {"removed_faces": "faces", "removed_edges": "edges"}


def count(shape, kind):
    found = TopTools_IndexedMapOfShape()
    TopExp.MapShapes_s(shape, kind, found)
    return found.Extent()


def check(step_path, report):
    exact = report["body"]
    step = report["report"]
    reader = STEPControl_Reader()
    status = reader.ReadFile(step_path)
    if status != IFSelect_RetDone:
        return [f"OpenCASCADE could not read {step_path}: status {status}"], {}
    reader.TransferRoots()
    shape = reader.OneShape()
    props = GProp_GProps()
    integration_error = BRepGProp.VolumeProperties_s(shape, props, INTEGRATION_EPS)
    volume = props.Mass()
    deviation = step["max_deviation_mm"]
    allowance = exact["area"] * deviation + abs(integration_error * volume) + MEASUREMENT_WIDTH * exact["volume"]
    seen = {
        "solids": count(shape, TopAbs_SOLID),
        "faces": count(shape, TopAbs_FACE),
        "valid": BRepCheck_Analyzer(shape).IsValid(),
        "volume": volume,
        "exact_volume": exact["volume"],
        "volume_difference": volume - exact["volume"],
        "allowance": allowance,
        "max_deviation_mm": deviation,
        "integration_error": integration_error,
    }
    failures = []
    invalid = [key for key, counted in REMOVED_COUNTS.items()
               if type(step[key]) is not int or not 0 <= step[key] <= exact[counted]]
    for key in invalid:
        failures.append(f"the report's {key} is {step[key]!r}, not a whole number "
                        f"from 0 to the exact body's {exact[REMOVED_COUNTS[key]]}")
    if seen["solids"] != step["solids"]:
        failures.append(f"{seen['solids']} solids in OpenCASCADE, {step['solids']} in the report")
    if "removed_faces" not in invalid and not (seen["faces"] == step["faces"] == exact["faces"] - step["removed_faces"]):
        failures.append(
            f"{seen['faces']} faces in OpenCASCADE, {step['faces']} in the report, {exact['faces']} on the exact body"
        )
    if not seen["valid"]:
        failures.append("OpenCASCADE's shape check failed")
    if not step["within_tolerance"]:
        failures.append(f"the export reports a deviation over its tolerance: {step['max_deviation']} mm")
    if abs(volume - exact["volume"]) > allowance:
        failures.append(
            f"volume {volume!r} is {abs(volume - exact['volume'])!r} from the exact {exact['volume']!r}; "
            f"the reported deviation allows {allowance!r}"
        )
    return failures, seen


def main(argv):
    if len(argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    with open(argv[2]) as f:
        report = json.load(f)
    failures, seen = check(argv[1], report)
    print(json.dumps(seen, indent=2))
    for failure in failures:
        print(f"FAIL: {failure}", file=sys.stderr)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
