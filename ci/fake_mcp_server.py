#!/usr/bin/env python3
"""A synthetic stand-in for `soliddraft mcp --bundle`, for runner tests only.

FAKE_MODE picks one misbehaviour: ok, early (display not selectable), stale (display of the
previous revision), pending, silent (no display reply), stale-measure (measurement of the previous
revision), slow-measure (measurement takes 4.3 s), slow-click (the command takes 1.3 s), other-revision (the backend reports another Catallax sha)."""

import json
import os
import pathlib
import sys
import time

mode = os.environ.get("FAKE_MODE", "ok")
revision_pin = os.environ.get("FAKE_PIN", "")
if mode == "other-revision":
    revision_pin = "0" * 40
bundle = pathlib.Path(sys.argv[sys.argv.index("--bundle") + 1])
bundle.mkdir(parents=True)
sequence = 0
revision = 1


def write(directory, name, value):
    (directory / name).write_text(json.dumps(value))


def reply(request, result):
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)


for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    if request["method"] == "initialize":
        reply(request, {"protocolVersion": "2025-06-18"})
        continue
    name = request["params"]["name"]
    directory = bundle / f"{sequence:04}"
    if name == "scenario_prepare":
        reply(request, {"isError": False})
    elif name == "scenario_display":
        if mode == "silent":
            continue
        shown = revision - 1 if mode == "stale" else revision
        write(directory, "display.json", {"backend": {"name": "catallax"}, "live_document": True,
              "request": sequence, "revision": shown, "pending": mode == "pending",
              "selectable_faces": 0 if mode == "early" else 6})
        write(directory, "display.kernel.json", {"calls": []})
        (directory / "viewport.png").write_bytes(b"png")
        reply(request, {"isError": False})
    elif name == "scenario_view":
        write(directory, "view.json", {"backend": {"name": "catallax"}, "live_document": True, "method": "mesh",
              "request": sequence, "revision": revision})
        write(directory, "view.kernel.json", {"calls": []})
        (directory / "view.svg").write_text("<svg><path d='M0 0'/></svg>")
        reply(request, {"isError": False})
    elif name == "scenario_measure":
        if mode == "slow-measure":
            time.sleep(4.3)
        write(directory, "measurement.json", {"backend": {"name": "catallax"}, "live_document": True,
              "request": sequence, "revision": revision - 1 if mode == "stale-measure" else revision,
              "volume_mm3": 1.0})
        write(directory, "measurement.kernel.json", {"calls": []})
        reply(request, {"isError": False})
    else:
        if mode == "slow-click":
            time.sleep(1.3)
        sequence += 1
        revision += 1
        directory = bundle / f"{sequence:04}"
        directory.mkdir()
        write(directory, "command.json", {"summary": {"features": []},
              "backend": {"name": "catallax", "version": revision_pin},
              "references": [], "output": "", "request": sequence, "revision": revision})
        write(directory, "kernel.json", {"calls": []})
        write(directory, "recipe.json", {})
        reply(request, {"isError": False})
