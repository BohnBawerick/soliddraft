#!/usr/bin/env python3
"""Drive the installed application's MCP and CLI, retaining evidence on failure."""

import argparse
import json
import math
import pathlib
import queue
import statistics
import subprocess
import sys
import threading
import time


# Shared with the scenario latency gate. Acceptance limits, not process timeouts.
BUDGET_MS = {"stock": 100, "boolean": 500, "feature": 1000, "comparison": 2000}
# Calibrated independently from click latency; see docs/application-loop.md.
DRAWING_VIEW_BUDGET_MS = 1000
PHASES = ("construction", "edit", "display", "measurement", "refusal")
DISPLAY = {"mesh", "display", "tessellate", "tessellate_edges", "view_curves", "section_view"}
MEASUREMENT = {"mass_properties", "face_area", "face_centroid", "face_bounding_box",
               "bounding_box", "edge_length", "edge_midpoint", "inertia"}
TREND_NOISE_MS = 25


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


def drawing_view_timing(elapsed_ms, kernel):
    result = timing("view", [{"source": "view.kernel.json", "elapsed_ms": elapsed_ms,
                             "phase": "display", "kernel": kernel}])
    return {**result, "check": "drawing-view budget", "class": "drawing-view",
            "budget_ms": DRAWING_VIEW_BUDGET_MS, "pass": elapsed_ms <= DRAWING_VIEW_BUDGET_MS}


def load_known_over_budget(path):
    policy = json.loads(path.read_text())
    if policy["format"] != "soliddraft-known-over-budget/1":
        raise ValueError("expected soliddraft-known-over-budget/1")
    known = {}
    for entry in policy["steps"]:
        key = (entry["scenario"], entry["step"])
        if key in known:
            raise ValueError(f"duplicate known-over-budget step: {key}")
        milliseconds(entry["last_measured_ms"])
        known[key] = entry
    return known


# Approved gaps track faster booleans when rebuilding after an edit:
# https://github.com/BohnBawerick/soliddraft/issues/27.
# The click budget stays strict; only these recorded gaps get a 20% regression guard.
def accept_latency(scenario, number, tool, latency, known):
    result = {**latency, "within_budget": latency["pass"],
              "status": "pass" if latency["pass"] else "over-budget"}
    approved = known.get((scenario, number))
    if approved is None:
        return result
    result["known_over_budget"] = approved
    label = f"{scenario} step {number} ({tool})"
    result["regression_limit_ms"] = milliseconds(approved["last_measured_ms"]) * 1.2
    if approved["tool"] != tool:
        result["pass"] = False
        result["status"] = "invalid-known-over-budget"
        result["message"] = f"{label}: tool differs from ci/known-over-budget.json; remove or review the listing"
    elif latency["elapsed_ms"] > result["regression_limit_ms"]:
        result["pass"] = False
        result["status"] = "known-gap-regression"
        result["message"] = (f"{label} regressed to {latency['elapsed_ms']:.3f} ms; "
                             f"recorded {approved['last_measured_ms']:.3f} ms, "
                             f"20% regression limit {result['regression_limit_ms']:.3f} ms; "
                             f"budget remains {latency['budget_ms']} ms")
    elif not latency["pass"]:
        result["pass"] = True
        result["status"] = "known-over-budget"
        result["message"] = (f"Known gap: {label} took {latency['elapsed_ms']:.3f} ms; "
                             f"recorded {approved['last_measured_ms']:.3f} ms, "
                             f"20% regression limit {result['regression_limit_ms']:.3f} ms; "
                             f"budget remains {latency['budget_ms']} ms")
    return result


def trend_checks(rules, steps):
    """Theil–Sen growth across the feature-count range; single stalls cannot hide a slope."""
    results = []
    for rule in rules:
        result = {"check": "feature-count latency trend", "tool": rule["tool"],
                  "noise_ms": TREND_NOISE_MS, "pass": False}
        results.append(result)
        try:
            samples = [(step["feature_count"], milliseconds(step["timing"]["elapsed_ms"]))
                for step in steps if step["tool"] == rule["tool"]
                and step["number"] >= rule.get("from_step", 1)]
            result["samples"] = len(samples)
            if len(samples) < 10:
                raise ValueError("trend needs at least ten comparable steps")
            counts = [count for count, _ in samples]
            if any(isinstance(n, bool) or not isinstance(n, int) or n < 1 for n in counts):
                raise ValueError("missing positive feature count")
            if any(b <= a for a, b in zip(counts, counts[1:])):
                raise ValueError("trend feature counts must increase")
            slope = statistics.median((b_ms - a_ms) / (b_n - a_n)
                for i, (a_n, a_ms) in enumerate(samples) for b_n, b_ms in samples[i + 1:])
            growth = slope * (counts[-1] - counts[0])
            result.update({"first_feature_count": counts[0], "last_feature_count": counts[-1],
                           "slope_ms_per_feature": slope, "growth_ms": growth,
                           "pass": growth <= TREND_NOISE_MS})
        except (KeyError, TypeError, ValueError) as error:
            result["error"] = str(error)
    return results


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


def rebuilds(features, repair):
    """Every feature builds, except the expected repairs and the features waiting for them."""
    waiting = tuple(f"error: {name} needs repair first" for name in repair)
    broken = [f for f in features if f["status"].startswith(("error", "needs repair"))
        and not (f["name"] in repair and f["status"].startswith("needs repair"))
        and not f["status"].startswith(waiting)]
    prompted = {f["name"] for f in features if f["status"].startswith("needs repair")}
    missing = [name for name in repair if name not in prompted]
    return {"check": "feature rebuilds", "actual": broken, "missing_repairs": missing,
        "pass": not broken and not missing}


def references_kept(known, references, repair, deleted):
    """Each reference names the origin it named before, and resolves or raises the repair prompt."""
    now = {(r["feature_id"], r["slot"]): r for r in references}
    changed = [{"reference": list(key), "was": identity, "now": now[key]["origin"]}
        for key, identity in known.items() if key in now
        and (now[key]["origin"] != identity["name"]
            or (all(o is not None for o in now[key]["kernel_origins"])
                and now[key]["kernel_origins"] != identity["kernel"]))]
    lost = [list(key) for key in known if key not in now and not deleted]
    wrong = [r for r in references
        if (r["resolved"] is not None and r["resolved"] != (r["feature"] not in repair))
        or (r["resolved"] is True and r["matches_origin"] is not True)]
    for key, r in now.items():
        known.setdefault(key, {"name": r["origin"], "kernel": r["kernel_origins"]})
    return {"check": "references kept", "changed": changed, "lost": lost, "unexpected": wrong,
        "pass": not changed and not lost and not wrong}


def substitute(value, bundle):
    """Replaces {bundle} in string arguments with the run's bundle directory."""
    if isinstance(value, str):
        return value.replace("{bundle}", str(bundle))
    if isinstance(value, list):
        return [substitute(v, bundle) for v in value]
    if isinstance(value, dict):
        return {k: substitute(v, bundle) for k, v in value.items()}
    return value


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
        known_latency = load_known_over_budget(pathlib.Path(__file__).with_name("known-over-budget.json"))
        for name, number in known_latency:
            if name == scenario["name"] and not 1 <= number <= len(scenario["steps"]):
                raise ValueError(f"remove missing {name} step {number} from ci/known-over-budget.json")
        client = Client(args.soliddraft, args.out / "commands", scenario.get("max_work", 100_000_000), args.timeout)
        initialized = client.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "soliddraft-scenario", "version": "1"}})
        if initialized["protocolVersion"] != "2025-06-18":
            raise RuntimeError("MCP initialization version mismatch")
        client.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        known = {}
        prepared = False
        for number, step in enumerate(scenario["steps"], 1):
            entry = {"number": number, "tool": step["tool"], "checks": [], "pass": False}
            report["steps"].append(entry)
            arguments = substitute(step.get("arguments", {}), args.out)
            started = time.perf_counter_ns()
            result = client.request("tools/call", {"name": step["tool"], "arguments": arguments})
            command_ms = (time.perf_counter_ns() - started) / 1_000_000
            directory = args.out / "commands" / f"{number:04}"
            command = json.loads((directory / "command.json").read_text())
            kernel = json.loads((directory / "kernel.json").read_text())
            segments = [{"source": "kernel.json", "elapsed_ms": command_ms, "kernel": kernel,
                         "phase": "refusal" if result.get("isError") else command_phase(step["tool"])}]
            entry["feature_count"] = len(command["summary"]["features"])
            checks = entry["checks"]
            checks.append({"check": "command outcome", "pass": bool(result.get("isError")) == bool(step.get("refusal"))})
            checks.append({"check": "Catallax backend", "pass": command["backend"]["name"] == "catallax"})
            repair = step.get("repair", [])
            checks.append(rebuilds(command["summary"]["features"], repair))
            checks.append(references_kept(known, command["references"], repair, step["tool"] == "delete_feature"))
            if step.get("refusal"):
                code = step["refusal"]
                checks.append({"check": "application refusal", "actual": command["output"], "pass": code in command["output"]})
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
                segments.append({"source": "display.kernel.json", "elapsed_ms": display_ms, "phase": "display",
                                 "kernel": json.loads((directory / "display.kernel.json").read_text())})
                if not displayed.get("isError"):
                    display = json.loads((directory / "display.json").read_text())
                    checks.append({"check": "live Catallax display", "actual": display,
                                   "pass": display["backend"]["name"] == "catallax" and display["live_document"]})
                checks.append({"check": "viewport PNG", "pass": (directory / "viewport.png").exists()})
            entry["waits_ms"] = [{"source": s["source"], "elapsed_ms": s["elapsed_ms"]} for s in segments]
            write(directory / "waits.json", entry["waits_ms"])
            latency = accept_latency(scenario["name"], number, step["tool"], timing(step["tool"], segments), known_latency)
            entry["timing"] = latency
            checks.append(latency)
            write(directory / "timing.json", latency)
            print(json.dumps({"step": number, "tool": step["tool"], "timing": latency}), flush=True)
            if latency["status"] == "known-over-budget":
                print(f"::warning::{latency['message']}", flush=True)
            elif latency["status"] in {"known-gap-regression", "invalid-known-over-budget"}:
                print(f"::error::{latency['message']}", flush=True)
            view_latency = None
            if (directory / "recipe.json").exists():
                started = time.perf_counter_ns()
                viewed = client.request("tools/call", {"name": "scenario_view", "arguments": {}})
                view_ms = (time.perf_counter_ns() - started) / 1_000_000
                checks.append({"check": "live hidden-line view", "response": viewed,
                               "pass": not viewed.get("isError")})
                view_latency = drawing_view_timing(
                    view_ms, json.loads((directory / "view.kernel.json").read_text()))
                entry["drawing_view"] = view_latency
                checks.append(view_latency)
                write(directory / "view.timing.json", view_latency)
                print(json.dumps({"step": number, "tool": "scenario_view", "timing": view_latency}), flush=True)
                if not view_latency["pass"]:
                    print(f"::error::Drawing-view step {number} took {view_ms:.3f} ms; "
                          f"budget is {DRAWING_VIEW_BUDGET_MS} ms", flush=True)
                if not viewed.get("isError"):
                    view = json.loads((directory / "view.json").read_text())
                    checks.append({"check": "live Catallax hidden-line view", "actual": view,
                                   "pass": view["backend"]["name"] == "catallax" and view["live_document"]
                                           and view["method"] == "mesh"})
                checks.append({"check": "SVG contains curves",
                               "pass": (directory / "view.svg").exists() and "<path " in (directory / "view.svg").read_text()})
            if step.get("drawing"):
                drawing = cli(args.soliddraft, ["draw", directory / "part.dwprt", "--template", "iso-a3",
                    "--out", directory / "drawing.dwdrw", "--out", directory / "drawing.svg", "--kernel-log", directory / "drawing.kernel.json", "--json"], directory, "drawing", args.timeout)
                if drawing["pass"]:
                    rendered = json.loads((directory / "drawing.stdout").read_text())
                    drawing["backend"] = rendered["backend"]
                    drawing["pass"] &= rendered["backend"].startswith("catallax ")
                checks.append(drawing)
            # Volume/area acceptance is an explicit request after the click's display,
            # not automatic work on the normal edit path or a pre-warming step.
            evidence = command
            if any(spec["pointer"].startswith("/measurement/") for spec in step.get("checks", [])):
                started = time.perf_counter_ns()
                measured = client.request("tools/call", {"name": "scenario_measure", "arguments": {}})
                measured_ms = (time.perf_counter_ns() - started) / 1_000_000
                checks.append({"check": "requested measurement", "pass": not measured.get("isError"),
                               "response": measured})
                measurement = None
                measure_timing = timing("measure", [{
                    "source": "measurement.kernel.json", "elapsed_ms": measured_ms, "phase": "measurement",
                    "kernel": json.loads((directory / "measurement.kernel.json").read_text())}])
                measure_timing["check"] = "requested measurement budget"
                entry["measurement"] = measure_timing
                checks.append(measure_timing)
                if not measured.get("isError"):
                    measurement = json.loads((directory / "measurement.json").read_text())
                evidence = {**command, "measurement": measurement}
            checks.extend(check(evidence, spec) for spec in step.get("checks", []))
            entry["pass"] = all(c["pass"] for c in checks)
            write(directory / "checks.json", entry)
            write(args.out / "result.json", report)
            if not entry["pass"]:
                # Retain every candidate input, including rejected scratch builds, for kernel diagnosis.
                entry["questions"] = [str(p.relative_to(args.out)) for p in directory.glob("question-*.json")]
                if any(not c["pass"] for c in checks if c is not latency and c is not view_latency):
                    break
        report["trends"] = trend_checks(scenario.get("trends", []), report["steps"])
        report["pass"] = (len(report["steps"]) == len(scenario["steps"])
            and all(s["pass"] for s in report["steps"]) and all(t["pass"] for t in report["trends"]))
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
