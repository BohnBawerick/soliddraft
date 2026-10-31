#!/usr/bin/env python3
"""A synthetic stand-in for `soliddraft mcp --bundle`, for runner tests only.

FAKE_MODE picks one misbehaviour: ok, early (display not selectable), stale (display of the
previous revision), pending, silent (no display reply), stale-measure (measurement of the previous
revision), slow-measure (measurement takes 4.3 s), slow-click (the command takes 1.3 s), slow-evidence
(writing the command's evidence takes 1.3 s), other-revision (the backend reports another Catallax sha),
no-solid (the display reports no solid), lost-display (the display succeeds but writes no evidence),
stale-no-solid (a no-solid reply for the previous revision), failed-no-solid (a failed reply claiming no solid).
FAKE_LOG names a file that records each process's commands and its exit."""

import json
import hashlib
import os
import pathlib
import struct
import sys
import time

mode = os.environ.get("FAKE_MODE", "ok")
revision_pin = os.environ.get("FAKE_PIN", "")
if mode == "other-revision":
    revision_pin = "0" * 40
if len(sys.argv) > 1 and sys.argv[1] == "export":
    path = pathlib.Path(sys.argv[sys.argv.index("--out") + 1])
    if mode.startswith("export-refused"):
        if mode == "export-refused-written":
            path.write_bytes(b"bad partial file")
        print("field parts export as meshes" if mode != "export-refused-wrong" else "other error", file=sys.stderr)
        sys.exit(1)
    data = bytearray(84)
    struct.pack_into("<d", data, 68, 0.1)
    path.write_bytes(data)
    print(json.dumps({"report": {"kind": "field_mesh", "deviation_bound_mm": 0.1,
                               "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}}))
    sys.exit(0)
bundle = pathlib.Path(sys.argv[sys.argv.index("--bundle") + 1])
bundle.mkdir(parents=True)
sequence = 0
revision = 1


def record(event):
    if "FAKE_LOG" in os.environ:
        with open(os.environ["FAKE_LOG"], "a") as log:
            log.write(f"{os.getpid()} {event}\n")


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
        if mode in {"no-solid", "stale-no-solid", "failed-no-solid"}:
            reply(request, {"isError": mode == "failed-no-solid", "structuredContent": {"solid": False,
                  "request": sequence, "revision": revision - 1 if mode == "stale-no-solid" else revision}})
            continue
        if mode == "lost-display":
            reply(request, {"isError": False, "structuredContent": {"request": sequence, "revision": revision}})
            continue
        shown = revision - 1 if mode == "stale" else revision
        write(directory, "display.json", {"backend": {"name": "catallax"}, "live_document": True,
              "request": sequence, "revision": shown, "pending": mode == "pending",
              "selectable": mode != "early"})
        write(directory, "display.kernel.json", {"calls": []})
        (directory / "viewport.png").write_bytes(b"png")
        reply(request, {"isError": False})
    elif name == "scenario_view":
        if mode in {"field-view", "wrong-view-error"}:
            reply(request, {"isError": True, "content": [{"type": "text", "text":
                  "drawings of field parts are coming soon" if mode == "field-view" else "unrelated view error"}]})
            continue
        write(directory, "view.json", {"backend": {"name": "catallax"}, "live_document": True, "method": "mesh",
              "request": sequence, "revision": revision})
        write(directory, "view.kernel.json", {"calls": []})
        (directory / "view.svg").write_text("<svg><path d='M0 0'/></svg>")
        reply(request, {"isError": False})
    elif name == "scenario_evidence":
        if mode == "slow-evidence":
            time.sleep(1.3)
        write(directory, "command.json", {"summary": {"features": []},
              "backend": {"name": "catallax", "version": revision_pin},
              "references": [], "output": "", "request": sequence, "revision": revision})
        write(directory, "kernel.json", {"calls": []})
        write(directory, "recipe.json", {})
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
        record(name)
        # Like the real bundle, the command's evidence waits for scenario_evidence.
        reply(request, {"isError": False})
record("exit")
