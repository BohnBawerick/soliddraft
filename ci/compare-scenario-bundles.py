#!/usr/bin/env python3
"""Compare scenario bundles from the base and S0 builds; exit 1 on any difference (S0 test T2)."""
import json
import pathlib
import sys
import zipfile

BYTES = {"part.json", "recipe.json", "measurement.json", "display.json", "view.json", "view.svg",
         "viewport.json", "viewport.png", "drawing.svg", "drawing.stderr"}
TOP_BYTES = {"scenario.json", "protocol.json"}


def zip_entries(path):
    with zipfile.ZipFile(path) as z:
        return {i.filename: (i.date_time, z.read(i.filename)) for i in z.infolist()}


def text(path, bundle):
    """File bytes with the bundle's own absolute path replaced by the scenario's {bundle} token."""
    return path.read_bytes().replace(str(bundle.resolve()).encode(), b"{bundle}")


def complete(bundle):
    """A bundle the runner finished: every scenario step has its result."""
    scenario = json.loads((bundle / "scenario.json").read_text())
    result = json.loads((bundle / "result.json").read_text())
    return len(result["steps"]) == len(scenario["steps"])


def problems(a, b):
    out = [f"{side} bundle is incomplete" for side, d in (("base", a), ("candidate", b)) if not complete(d)]
    for name in TOP_BYTES:
        if text(a / name, a) != text(b / name, b):
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
                if (da / name).read_bytes() != (db / name).read_bytes():
                    out.append(f"{step}/{name}")
        if "command.json" in files_a & files_b:
            ca, cb = (json.loads(text(d / "command.json", root)) for d, root in ((da, a), (db, b)))
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
                    ma, mb = json.loads(ba), json.loads(bb)
                    ma.pop("id"), mb.pop("id")
                    same = ma == mb and ta == tb
                else:
                    same = (ta, ba) == (tb, bb)
                if not same:
                    out.append(f"{step}/part.dwprt:{entry}")
    return out


if __name__ == "__main__":
    base, candidate = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
    names = sorted(p.name for p in base.iterdir() if (p / "scenario.json").exists())
    if names != sorted(p.name for p in candidate.iterdir() if (p / "scenario.json").exists()):
        sys.exit("the two directories hold different scenarios")
    failed = False
    for name in names:
        found = problems(base / name, candidate / name)
        print(f"{name}: " + ("; ".join(found) or "no differences"))
        failed |= bool(found)
    sys.exit(1 if failed else 0)
