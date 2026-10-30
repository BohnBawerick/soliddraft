#!/usr/bin/env python3
"""Compare scenario answers and STEP bytes; exit 1 on any difference."""
import argparse
import json
import pathlib
import re
import sys
import zipfile

BYTES = {"part.json", "recipe.json", "measurement.json", "display.json", "view.json", "view.svg",
         "viewport.json", "viewport.png", "drawing.svg", "drawing.stderr"}
TOP_BYTES = {"scenario.json", "protocol.json"}
UUID = re.compile(rb"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")


def normalise(data, pins):
    if pins is None:
        return data
    base, candidate = pins
    return UUID.sub(b"00000000-0000-0000-0000-000000000000", data).replace(candidate.encode(), base.encode())


def zip_entries(path):
    with zipfile.ZipFile(path) as z:
        return {i.filename: (i.date_time, z.read(i.filename)) for i in z.infolist()}


def text(path, bundle, pins=None):
    """File bytes with the bundle's own absolute path replaced by the scenario's {bundle} token."""
    return normalise(path.read_bytes(), pins).replace(str(bundle.resolve()).encode(), b"{bundle}")


def complete(bundle):
    """A bundle the runner finished: every scenario step has its result."""
    scenario = json.loads((bundle / "scenario.json").read_text())
    result = json.loads((bundle / "result.json").read_text())
    return len(result["steps"]) == len(scenario["steps"])


def problems(a, b, pins=None):
    out = [f"{side} bundle is incomplete" for side, d in (("base", a), ("candidate", b)) if not complete(d)]
    for name in TOP_BYTES:
        if text(a / name, a, pins) != text(b / name, b, pins):
            out.append(name)
    steps_a = sorted(p.name for p in (a / "commands").iterdir())
    steps_b = sorted(p.name for p in (b / "commands").iterdir())
    if steps_a != steps_b:
        return out + [f"step directories {steps_a} vs {steps_b}"]
    for step in steps_a:
        da, db = a / "commands" / step, b / "commands" / step
        files_a = {p.name for p in da.iterdir()}
        files_b = {p.name for p in db.iterdir()}
        if files_a != files_b:
            out.append(f"{step}: file sets differ {sorted(files_a ^ files_b)}")
        for name in sorted(files_a & files_b):
            if name in BYTES or name.startswith("question-"):
                left, right = ((d / name).read_bytes() for d in (da, db))
                if name.endswith(".json"):
                    left, right = normalise(left, pins), normalise(right, pins)
                if left != right:
                    out.append(f"{step}/{name}")
        if "command.json" in files_a & files_b:
            ca, cb = (json.loads(text(d / "command.json", root, pins)) for d, root in ((da, a), (db, b)))
            for c in (ca, cb):
                c["summary"].get("edit_state", {}).pop("document", None)
                c["rebuild"].pop("meshes_rebuilt", None)
            if ca != cb:
                out.append(f"{step}/command.json")
        if "part.dwprt" in files_a & files_b:
            za, zb = zip_entries(da / "part.dwprt"), zip_entries(db / "part.dwprt")
            if za.keys() != zb.keys():
                out.append(f"{step}/part.dwprt entries")
            for entry in za.keys() & zb.keys():
                (ta, ba), (tb, bb) = za[entry], zb[entry]
                if entry == "manifest.json":
                    ma, mb = json.loads(normalise(ba, pins)), json.loads(normalise(bb, pins))
                    ma.pop("id"), mb.pop("id")
                    same = ma == mb and ta == tb
                else:
                    same = (ta, ba) == (tb, bb)
                if not same:
                    out.append(f"{step}/part.dwprt:{entry}")
    steps_a = {p.relative_to(a): p.read_bytes() for p in a.rglob("*.step")}
    steps_b = {p.relative_to(b): p.read_bytes() for p in b.rglob("*.step")}
    for name in sorted(steps_a.keys() | steps_b.keys()):
        if steps_a.get(name) != steps_b.get(name):
            out.append(f"{name}: STEP bytes differ")
    return out


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("base", type=pathlib.Path)
    parser.add_argument("candidate", type=pathlib.Path)
    parser.add_argument("--base-pin")
    parser.add_argument("--candidate-pin")
    args = parser.parse_args()
    pins = None
    if args.base_pin is not None or args.candidate_pin is not None:
        if not all(re.fullmatch(r"[0-9a-f]{40}", p or "") for p in (args.base_pin, args.candidate_pin)):
            parser.error("both pins must be full commit SHAs")
        pins = args.base_pin, args.candidate_pin
    base, candidate = args.base, args.candidate
    names = sorted(p.name for p in base.iterdir() if (p / "scenario.json").exists())
    if not names:
        sys.exit("base has no scenario bundles")
    if names != sorted(p.name for p in candidate.iterdir() if (p / "scenario.json").exists()):
        sys.exit("the two directories hold different scenarios")
    failed = False
    for name in names:
        found = problems(base / name, candidate / name, pins)
        print(f"{name}: " + ("; ".join(found) or "no differences"))
        failed |= bool(found)
    sys.exit(1 if failed else 0)
