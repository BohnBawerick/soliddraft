#!/usr/bin/env python3
"""Drive the installed application's MCP and CLI, retaining evidence on failure."""

import argparse
import json
import math
import pathlib
import queue
import subprocess
import sys
import threading
import time


# Catallax PLAN 7.3. These are acceptance limits, not process timeouts.
BUDGET_MS = {"stock": 100, "boolean": 500, "feature": 1000, "comparison": 2000}
PHASES = ("construction", "edit", "display", "measurement", "refusal")
DISPLAY = {"mesh", "display", "tessellate", "tessellate_edges", "view_curves", "section_view"}
MEASUREMENT = {"mass_properties", "face_area", "face_centroid", "face_bounding_box",
               "bounding_box", "edge_length", "edge_midpoint", "inertia"}


def budget(tool):
    if tool in {"box", "cylinder", "sphere", "cone", "torus"}:
        return "stock"
    if tool in {"boolean", "union", "subtract", "intersect"}:
        return "boolean"
    if tool == "compare":
        return "comparison"
    return "feature"


def phase(operation, default):
    if operation.get("refusal") or operation.get("status") == "refused":
        return "refusal"
    if operation["operation"] in DISPLAY:
        return "display"
    if operation["operation"] in MEASUREMENT:
        return "measurement"
    return default


def milliseconds(value):
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or value < 0:
        raise ValueError(f"invalid elapsed milliseconds: {value!r}")
    return value


def command_phase(tool):
    if tool == "measure":
        return "measurement"
    if tool.startswith("edit_") or tool in {
        "undo", "redo", "rollback", "suppress", "delete_feature", "rename",
    }:
        return "edit"
    return "construction"


def timing(tool, segments):
    """Partition external wait time using disjoint top-level calls and their operation logs."""
    kind = budget(tool)
    default = command_phase(tool)
    split = dict.fromkeys(PHASES, 0.0)
    work = dict.fromkeys(PHASES, 0)
    background_split = dict.fromkeys(PHASES, 0.0)
    background_work = dict.fromkeys(PHASES, 0)
    sources = []
    for segment in segments:
        elapsed = milliseconds(segment["elapsed_ms"])
        remaining = elapsed
        fallback = segment.get("phase", default)
        for call in segment["kernel"]["calls"]:
            if call.get("nested", False):
                continue  # Included by the enclosing log_scope; never charge it twice.
            on_capture_thread = call["on_capture_thread"]
            if not isinstance(on_capture_thread, bool):
                raise ValueError("missing capture-thread ownership")
            call_split = split if on_capture_thread else background_split
            call_work = work if on_capture_thread else background_work
            call_ms = milliseconds(call["wall_ms"])
            owner = phase(call, fallback)
            log = call["operation_log"]
            if log["format"] != "catallax-operation-log/1":
                raise ValueError("unsupported operation log")
            allocated = 0.0
            used = 0
            for operation in log["operations"]:
                op_ms = milliseconds(operation["wall_ms"])
                op_work = operation["work"]["used"]
                target = phase(operation, owner)
                # A refused outer call owns its entire wait, including successful attempts.
                if owner == "refusal":
                    target = owner
                if target != owner:
                    call_split[target] += op_ms
                    allocated += op_ms
                call_work[target] += op_work
                used += op_work
            if allocated > call_ms + 0.001:
                raise ValueError("overlapping operation timings exceed their enclosing call")
            call_split[owner] += max(0.0, call_ms - allocated)
            if call.get("work") is not None:
                call_work[owner] += max(0, call["work"]["used"] - used)
            if on_capture_thread:
                remaining -= call_ms
        if remaining < -0.001:
            raise ValueError("kernel call timings exceed external wait time")
        split[segment.get("overhead_phase", fallback)] += max(0.0, remaining)
        sources.append({"source": segment["source"], "elapsed_ms": elapsed})
    elapsed = sum(s["elapsed_ms"] for s in sources)
    return {"check": "click budget", "class": kind, "budget_ms": BUDGET_MS[kind],
            "elapsed_ms": elapsed, "split_ms": split, "work": work, "segments": sources,
            "background": {"call_ms": background_split, "work": background_work},
            "pass": elapsed <= BUDGET_MS[kind]}


def write(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


class Client:
    def __init__(self, binary, bundle, max_work, timeout):
        self.process = subprocess.Popen(
            [binary, "mcp", "--bundle", str(bundle), "--max-work", str(max_work)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
        )
        self.timeout = timeout
        self.sequence = 0
        self.replies = queue.Queue()
        self.transcript = []
        threading.Thread(target=self.read, daemon=True).start()

    def read(self):
        try:
            for line in self.process.stdout:
                self.replies.put(json.loads(line))
            self.replies.put(RuntimeError("MCP closed stdout"))
        except Exception as error:
            self.replies.put(error)

    def send(self, request):
        self.transcript.append({"request": request})
        self.process.stdin.write(json.dumps(request) + "\n")
        self.process.stdin.flush()

    def request(self, method, params):
        self.sequence += 1
        self.send({"jsonrpc": "2.0", "id": self.sequence, "method": method, "params": params})
        try:
            reply = self.replies.get(timeout=self.timeout)
        except queue.Empty as error:
            raise RuntimeError(f"MCP timed out on {method}") from error
        if isinstance(reply, Exception):
            raise reply
        self.transcript[-1]["response"] = reply
        if reply.get("id") != self.sequence or "error" in reply:
            raise RuntimeError(f"MCP protocol failure: {reply}")
        return reply["result"]

    def close(self):
        self.process.stdin.close()
        try:
            status = self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
            raise RuntimeError("MCP did not exit after EOF")
        if status:
            raise RuntimeError(f"MCP exited {status}")


def at(value, pointer):
    if pointer == "":
        return value
    if not pointer.startswith("/"):
        raise ValueError(f"JSON pointer must start with /: {pointer}")
    for part in pointer[1:].split("/"):
        part = part.replace("~1", "/").replace("~0", "~")
        value = value[int(part)] if isinstance(value, list) else value[part]
    return value


def check(value, spec):
    try:
        actual = at(value, spec["pointer"])
        comparisons = [key for key in ("equals", "approx", "minimum", "contains") if key in spec]
        if len(comparisons) != 1:
            raise ValueError("a check needs exactly one comparison")
        kind = comparisons[0]
        expected = spec[kind]
        if kind == "equals":
            passed = actual == expected
        elif kind == "approx":
            passed = math.isclose(actual, expected, rel_tol=0, abs_tol=spec.get("tolerance", 0.001))
        elif kind == "minimum":
            passed = actual >= expected
        else:
            passed = expected in actual
        return {"check": spec, "actual": actual, "pass": passed}
    except (KeyError, IndexError, TypeError, ValueError) as error:
        return {"check": spec, "error": str(error), "pass": False}


def cli(binary, arguments, directory, label, timeout):
    start = time.perf_counter_ns()
    result = subprocess.run([binary, *map(str, arguments)], capture_output=True, text=True, timeout=timeout)
    (directory / f"{label}.stdout").write_text(result.stdout)
    (directory / f"{label}.stderr").write_text(result.stderr)
    return {"check": label, "command": [binary, *map(str, arguments)], "exit": result.returncode,
            "elapsed_ms": (time.perf_counter_ns() - start) / 1_000_000,
            "pass": result.returncode == 0}


def replay_question(verifier, question, directory, timeout):
    result = cli(verifier, ["replay", question, "--out", directory / "replay"], directory, "replay", timeout)
    # The verifier returns a complete response for refusals too. Infrastructure errors fail this check.
    if result["pass"]:
        response = json.loads((directory / "replay.stdout").read_text())
        result["status"] = response["manifest"]["status"]
        result["refusal"] = (response["manifest"].get("refusal") or {}).get("code")
        result["pass"] = result["status"] not in ("internal-error", "invalid-question")
    return result


def run(args):
    scenario = json.loads(args.scenario.read_text())
    if scenario["format"] != "soliddraft-scenario/1" or not scenario.get("steps"):
        raise ValueError("expected soliddraft-scenario/1 with steps")
    args.out.mkdir(parents=True, exist_ok=False)
    write(args.out / "scenario.json", scenario)
    report = {"format": "soliddraft-scenario-result/1", "name": scenario["name"], "steps": [], "pass": False}
    client = None
    try:
        client = Client(args.soliddraft, args.out / "commands", scenario.get("max_work", 100_000_000), args.timeout)
        initialized = client.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "soliddraft-scenario", "version": "1"}})
        if initialized["protocolVersion"] != "2025-06-18":
            raise RuntimeError("MCP initialization version mismatch")
        client.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        prepared = False
        for number, step in enumerate(scenario["steps"], 1):
            entry = {"number": number, "tool": step["tool"], "checks": [], "pass": False}
            report["steps"].append(entry)
            started = time.perf_counter_ns()
            result = client.request("tools/call", {"name": step["tool"], "arguments": step.get("arguments", {})})
            command_ms = (time.perf_counter_ns() - started) / 1_000_000
            directory = args.out / "commands" / f"{number:04}"
            command = json.loads((directory / "command.json").read_text())
            kernel = json.loads((directory / "kernel.json").read_text())
            segments = [{"source": "kernel.json", "elapsed_ms": command_ms, "kernel": kernel,
                         "phase": "refusal" if result.get("isError") else command_phase(step["tool"])}]
            checks = entry["checks"]
            checks.append({"check": "command outcome", "pass": bool(result.get("isError")) == bool(step.get("refusal"))})
            checks.append({"check": "Catallax backend", "pass": command["backend"]["name"] == "catallax"})
            checks.extend(check(command, spec) for spec in step.get("checks", []))
            errors = [f for f in command["summary"]["features"] if f["status"].startswith(("error", "needs repair"))]
            checks.append({"check": "feature rebuilds", "actual": errors, "pass": not errors})
            if step.get("refusal"):
                code = step["refusal"]
                checks.append({"check": "application refusal", "actual": command["output"], "pass": code in command["output"]})
                # Read the captured refusal, never replay it inside the click interval.
                refusals = [operation["refusal"]["code"] for call in kernel["calls"]
                    for operation in call["operation_log"]["operations"] if operation.get("refusal")]
                checks.append({"check": "recorded kernel refusal", "actual": refusals, "pass": code in refusals})
                questions = sorted(directory.glob("question-*.json"))
                checks.append({"check": "kernel question exported", "pass": bool(questions)})
                if questions:
                    question = questions[-1]
                    replay = replay_question(args.catallax_verify, question, directory, args.timeout)
                    replay["expected_refusal"] = code
                    replay["pass"] &= replay.get("refusal") == code and replay.get("status") == "refused"
                    checks.append(replay)
            if (directory / "recipe.json").exists():
                if not prepared:
                    response = client.request("tools/call", {"name": "scenario_prepare", "arguments": {}})
                    if response.get("isError"):
                        raise RuntimeError(f"renderer preparation failed: {response}")
                    prepared = True
                started = time.perf_counter_ns()
                displayed = client.request("tools/call", {"name": "scenario_display", "arguments": {}})
                display_ms = (time.perf_counter_ns() - started) / 1_000_000
                checks.append({"check": "live display", "response": displayed, "pass": not displayed.get("isError")})
                if not displayed.get("isError"):
                    display = json.loads((directory / "display.json").read_text())
                    checks.append({"check": "live Catallax display", "actual": display,
                                   "pass": display["backend"]["name"] == "catallax" and display["live_document"]})
                    segments.append({"source": "display.kernel.json", "elapsed_ms": display_ms, "phase": "display",
                                     "kernel": json.loads((directory / "display.kernel.json").read_text())})
                checks.append({"check": "SVG contains curves",
                               "pass": (directory / "view.svg").exists() and "<path " in (directory / "view.svg").read_text()})
                checks.append({"check": "viewport PNG", "pass": (directory / "viewport.png").exists()})
            if step.get("drawing"):
                drawing = cli(args.soliddraft, ["draw", directory / "part.dwprt", "--template", "iso-a3",
                    "--out", directory / "drawing.dwdrw", "--out", directory / "drawing.svg", "--kernel-log", directory / "drawing.kernel.json", "--json"], directory, "drawing", args.timeout)
                if drawing["pass"]:
                    rendered = json.loads((directory / "drawing.stdout").read_text())
                    drawing["backend"] = rendered["backend"]
                    drawing["pass"] &= rendered["backend"].startswith("catallax ")
                checks.append(drawing)
                if drawing["pass"]:
                    segments.append({"source": "drawing.kernel.json", "elapsed_ms": drawing["elapsed_ms"],
                                     "phase": "construction", "overhead_phase": "display",
                                     "kernel": json.loads((directory / "drawing.kernel.json").read_text())})
            # Preserve external waits even if the log cannot support a phase split.
            entry["waits_ms"] = [{"source": s["source"], "elapsed_ms": s["elapsed_ms"]} for s in segments]
            write(directory / "waits.json", entry["waits_ms"])
            latency = timing(step["tool"], segments)
            entry["timing"] = latency
            checks.append(latency)
            write(directory / "timing.json", latency)
            print(json.dumps({"step": number, "tool": step["tool"], "timing": latency}), flush=True)
            entry["pass"] = all(c["pass"] for c in checks)
            write(directory / "checks.json", entry)
            write(args.out / "result.json", report)
            if not entry["pass"]:
                # Retain every candidate input, including rejected scratch builds, for kernel diagnosis.
                entry["questions"] = [str(p.relative_to(args.out)) for p in directory.glob("question-*.json")]
                if any(not c["pass"] for c in checks if c is not latency):
                    break
        report["pass"] = len(report["steps"]) == len(scenario["steps"]) and all(s["pass"] for s in report["steps"])
    except Exception as error:
        report["error"] = str(error)
    finally:
        try:
            if client is not None:
                client.close()
        except Exception as error:
            report["error"] = str(error)
            report["pass"] = False
        write(args.out / "protocol.json", client.transcript if client else [])
        write(args.out / "result.json", report)
    print(json.dumps({"bundle": str(args.out), "pass": report["pass"], "error": report.get("error")}))
    return 0 if report["pass"] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("scenario", type=pathlib.Path)
    parser.add_argument("--out", type=pathlib.Path, required=True, help="new bundle directory, never overwritten")
    parser.add_argument("--soliddraft", required=True, help="built soliddraft executable")
    parser.add_argument("--catallax-verify", required=True, help="verifier built from the pinned Catallax revision")
    parser.add_argument("--timeout", type=int, default=300, help="seconds per MCP request or CLI command")
    args = parser.parse_args()
    args.out = args.out.resolve()
    args.soliddraft = str(pathlib.Path(args.soliddraft).resolve())
    args.catallax_verify = str(pathlib.Path(args.catallax_verify).resolve())
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
