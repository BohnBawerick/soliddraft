#!/usr/bin/env python3
"""Drive the installed application's MCP and CLI, retaining evidence on failure."""

import argparse
import collections
import hashlib
import json
import math
import pathlib
import queue
import re
import statistics
import struct
import subprocess
import sys
import threading
import time


# Click budgets. CI records each click against them and warns, never fails: clock time on
# shared runners is not evidence; only --strict, the release verdict, requires them. Not
# process timeouts. An output has no click limit: its wait is the caller's wait for the
# certified file, and is recorded, as the CLI export and drawing checks are.
BUDGET_MS = {"stock": 100, "boolean": 500, "feature": 1000, "comparison": 2000, "output": None}
# Calibrated independently from click latency; see docs/application-loop.md.
DRAWING_VIEW_BUDGET_MS = 1000
# A requested measurement keeps its 1,000 ms click-class budget; no wait above this gate is accepted.
MEASUREMENT_GATE_MS = 2000
PHASES = ("construction", "edit", "display", "measurement", "refusal")
DISPLAY = {"mesh", "display", "tessellate", "tessellate_edges", "preview", "preview_edges", "view_curves",
           "section_view"}
MEASUREMENT = {"mass_properties", "measured_topology", "face_area", "face_centroid", "face_bounding_box",
               "bounding_box", "edge_length", "edge_midpoint", "inertia", "field_mass", "face_support_plane"}
TREND_NOISE_MS = 25
# The one hang guard for every MCP request and CLI command: generous, never a speed limit.
HANG_TIMEOUT_S = 600
KNOWN_OVER_BUDGET = pathlib.Path(__file__).with_name("known-over-budget.json")


def budget(tool):
    if tool in {"box", "cylinder", "sphere", "cone", "torus"}:
        return "stock"
    if tool in {"boolean", "union", "subtract", "intersect"}:
        return "boolean"
    if tool == "compare":
        return "comparison"
    if tool.startswith("drawing_export_"):
        return "output"
    return "feature"


def phase(operation, default):
    if operation.get("refusal") or operation.get("status") == "refused":
        return "refusal"
    if operation["operation"] in DISPLAY:
        return "display"
    if operation["operation"] in MEASUREMENT:
        return "measurement"
    return default


def pinned_revision():
    """The Catallax commit the adapter is built against; the evidence must come from that sha."""
    source = pathlib.Path(__file__).resolve().parent.parent / "crates/soliddraft-kernel-catallax/src/lib.rs"
    found = re.search(r'^pub const CATALLAX_REVISION: &str = "([0-9a-f]{40})";', source.read_text(), re.M)
    if found is None:
        raise ValueError("CATALLAX_REVISION is not a full commit sha")
    return found.group(1)


def backend_pinned(command, pin):
    """A command run on any other Catallax revision is not evidence for this one."""
    backend = command.get("backend")
    version = backend.get("version") if isinstance(backend, dict) else None
    return {"check": "Catallax backend is the pinned revision", "expected": pin, "actual": version,
            "pass": isinstance(backend, dict) and backend.get("name") == "catallax" and version == pin}


def milliseconds(value):
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or value < 0:
        raise ValueError(f"invalid elapsed milliseconds: {value!r}")
    return value


def command_phase(tool):
    if tool in ("measure", "field_mass"):
        return "measurement"
    if tool.startswith("edit_") or tool in {
        "undo", "redo", "rollback", "suppress", "delete_feature", "rename", "drive", "set_value",
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
    limit = BUDGET_MS[kind]
    # A timing is a record: it always passes, and within_budget says whether it met its budget.
    return {"check": "click budget", "class": kind, "budget_ms": limit,
            "elapsed_ms": elapsed, "split_ms": split, "work": work, "segments": sources,
            "background": {"call_ms": background_split, "work": background_work},
            "within_budget": None if limit is None else elapsed <= limit, "pass": True}


def drawing_view_timing(elapsed_ms, kernel):
    result = timing("view", [{"source": "view.kernel.json", "elapsed_ms": elapsed_ms,
                             "phase": "display", "kernel": kernel}])
    return {**result, "check": "drawing-view budget", "class": "drawing-view",
            "budget_ms": DRAWING_VIEW_BUDGET_MS, "within_budget": elapsed_ms <= DRAWING_VIEW_BUDGET_MS}


def measurement_timing(elapsed_ms, kernel):
    """The explicit measurement request, recorded against its own budget and the 2,000 ms gate."""
    result = timing("measure", [{"source": "measurement.kernel.json", "elapsed_ms": elapsed_ms,
                                "phase": "measurement", "kernel": kernel}])
    return {**result, "check": "requested measurement budget", "gate_ms": MEASUREMENT_GATE_MS,
            "within_gate": elapsed_ms <= MEASUREMENT_GATE_MS}


def verdict(result):
    """strict-pass met its budget; over-budget did not, and is a warning, not a failure.

    An output has no click budget: its wait is recorded as "output", neither a strict pass
    nor over budget."""
    if result["within_budget"] is None:
        return "output"
    return "strict-pass" if result["within_budget"] else "over-budget"


def action_work_check(ceiling, latency):
    """One application action's whole deterministic work: its click and background calls."""
    if isinstance(ceiling, bool) or not isinstance(ceiling, int) or ceiling < 0:
        raise ValueError("max_action_work must be a nonnegative integer")
    used = sum(latency["work"].values()) + sum(latency["background"]["work"].values())
    return {"check": "action work ceiling", "used": used, "ceiling": ceiling, "pass": used <= ceiling}


def edit_work_check(step, latency):
    """A deterministic rebuild guard, independent of click-time acceptance."""
    ceiling = step["max_edit_work"]
    if isinstance(ceiling, bool) or not isinstance(ceiling, int) or ceiling < 0:
        raise ValueError("max_edit_work must be a nonnegative integer")
    used = latency["work"]["edit"]
    return {"check": "edit work ceiling", "used": used, "ceiling": ceiling,
            "status": "pass" if used <= ceiling else "edit-work-regression", "pass": used <= ceiling}


def answers(request, command, evidence, selectable=False):
    """Why this reply cannot answer the request, or None when it does.

    A reply that carries no request and revision, or the wrong ones, is an early or stale
    answer and never ends an interval. An answer still pending or not selectable does not either."""
    if not isinstance(evidence, dict):
        return "no reply"
    if evidence.get("pending"):
        return "reply is still pending"
    for key, want in (("request", request), ("revision", command.get("revision"))):
        got = evidence.get(key)
        if isinstance(got, bool) or not isinstance(got, int) or isinstance(want, bool) or not isinstance(want, int):
            return f"reply does not say which {key} it answers"
        if got != want:
            return f"stale: answers {key} {got}, not {want}"
    if selectable:
        if evidence.get("selectable") is not True:
            return "the edit is not selectable in the viewport"
    return None


DRAWING_VIEW_CALLS = {"view_curves", "section_view"}


def drawing_view_calls(kernel, gated=True):
    """Every drawing view a command regenerated on its thread, against the drawing-view ceiling.

    An open drawing follows the part inside the command (automatic update), so its views'
    kernel calls are in the click; this records the slowest against the same ceiling the
    live hidden-line view has, and fails closed on a call without a wall time. An output
    draws its views again from the certified mesh: those calls have no ceiling, as the
    rest of its wait has none.
    """
    views = []
    for call in kernel["calls"]:
        if call.get("nested", False) or call["operation"] not in DRAWING_VIEW_CALLS:
            continue
        if not isinstance(call["on_capture_thread"], bool):
            raise ValueError("missing capture-thread ownership")
        if call["on_capture_thread"]:
            views.append({"operation": call["operation"], "wall_ms": milliseconds(call["wall_ms"])})
    slowest = max((v["wall_ms"] for v in views), default=0.0)
    return {"check": "drawing view calls", "class": "drawing-view",
            "budget_ms": DRAWING_VIEW_BUDGET_MS if gated else None, "views": views, "slowest_ms": slowest,
            "within_budget": slowest <= DRAWING_VIEW_BUDGET_MS if gated else None, "pass": True}


def output_files(paths, absent=()):
    """Each file a step says it wrote exists and is not empty; a refused output left no file."""
    checks = []
    for path in paths:
        p = pathlib.Path(path)
        size = p.stat().st_size if p.is_file() else None
        checks.append({"check": "output file", "path": str(p), "bytes": size, "pass": bool(size)})
    for path in absent:
        checks.append({"check": "no partial output", "path": str(path), "pass": not pathlib.Path(path).exists()})
    return checks


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


# Listed steps record where faster booleans when rebuilding after an edit are still owed:
# https://github.com/BohnBawerick/soliddraft/issues/27.
# A step over its budget, a listed step over 120% of its recorded time and a listing that
# no longer matches its step are each a warning; none fails the run.
def accept_latency(scenario, number, tool, latency, known):
    result = {**latency, "status": "over-budget" if latency["within_budget"] is False else "pass"}
    label = f"{scenario} step {number} ({tool})"
    approved = known.get((scenario, number))
    if latency["budget_ms"] is None:
        result["status"] = "output"
        result["message"] = f"{label} wrote its output in {latency['elapsed_ms']:.3f} ms"
        if approved is not None:
            result["status"] = "invalid-known-over-budget"
            result["message"] = f"{label} is an output with no click budget; remove its listing"
        result["verdict"] = verdict(result)
        return result
    if result["status"] == "over-budget":
        result["message"] = (f"{label} took {latency['elapsed_ms']:.3f} ms; "
                             f"budget {latency['budget_ms']} ms")
    if approved is None:
        result["verdict"] = verdict(result)
        return result
    result["known_over_budget"] = approved
    result["regression_limit_ms"] = approved["last_measured_ms"] * 1.2
    if approved["tool"] != tool:
        result["status"] = "invalid-known-over-budget"
        result["message"] = f"{label}: tool differs from ci/known-over-budget.json; remove or review the listing"
    elif latency["elapsed_ms"] > result["regression_limit_ms"]:
        result["status"] = "known-gap-regression"
        result["message"] = (f"{label} regressed to {latency['elapsed_ms']:.3f} ms; "
                             f"recorded {approved['last_measured_ms']:.3f} ms, "
                             f"20% regression limit {result['regression_limit_ms']:.3f} ms; "
                             f"budget remains {latency['budget_ms']} ms")
    elif result["status"] == "over-budget":
        result["status"] = "known-over-budget"
        result["message"] = (f"Known gap: {label} took {latency['elapsed_ms']:.3f} ms; "
                             f"recorded {approved['last_measured_ms']:.3f} ms, "
                             f"20% regression limit {result['regression_limit_ms']:.3f} ms; "
                             f"budget remains {latency['budget_ms']} ms")
    result["verdict"] = verdict(result)
    return result


def trend_checks(rules, steps):
    """Theil–Sen growth across the feature-count range; single stalls cannot hide a slope.

    Growth over the noise allowance is a warning; missing or unusable evidence fails."""
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
                           "within_budget": growth <= TREND_NOISE_MS, "pass": True})
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
            status = self.process.wait(timeout=self.timeout)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
            raise RuntimeError("MCP did not exit after EOF")
        self.process.stdout.close()
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


def check(value, spec, previous=None):
    try:
        if isinstance(spec.get("absent"), bool) and len(spec) == 2:
            try:
                actual = at(value, spec["pointer"])
            except KeyError:
                return {"check": spec, "pass": spec["absent"]}
            return {"check": spec, "actual": actual, "pass": not spec["absent"]}
        actual = at(value, spec["pointer"])
        comparisons = [key for key in ("equals", "approx", "minimum", "maximum", "length", "tally", "contains", "same_as_step", "same_as", "differs_from") if key in spec]
        if len(comparisons) != 1:
            raise ValueError("a check needs exactly one comparison")
        kind = comparisons[0]
        expected = spec[kind]
        if kind == "same_as_step":
            expected = at(previous[expected], spec["pointer"])
            passed = actual == expected
        elif kind in {"same_as", "differs_from"}:
            other = previous[expected["step"]] if "step" in expected else value
            expected = at(other, expected["pointer"])
            passed = (actual == expected) if kind == "same_as" else (actual != expected)
        elif kind == "equals":
            passed = actual == expected
        elif kind == "approx":
            passed = math.isclose(actual, expected, rel_tol=0, abs_tol=spec.get("tolerance", 0.001))
        elif kind == "minimum":
            passed = actual >= expected
        elif kind == "maximum":
            passed = actual <= expected
        elif kind == "length":
            passed = isinstance(actual, list) and len(actual) == expected
        elif kind == "tally":
            passed = all(dict(collections.Counter(item[key] for item in actual)) == counts
                         for key, counts in expected.items())
        else:
            passed = expected in actual
        return {"check": spec, "actual": actual, "pass": passed}
    except (KeyError, IndexError, TypeError, ValueError) as error:
        return {"check": spec, "error": str(error), "pass": False}


def rebuilds(features, repair, failed=None):
    """Every feature builds, except the expected repairs and the features waiting for them."""
    failed = {} if failed is None else failed
    expected_failed = {f["name"] for f in features if f["status"].startswith("error:")
                       and isinstance(failed.get(f["name"]), str) and failed[f["name"]]
                       and failed[f["name"]] in f["status"]}
    waiting = tuple(f"error: {name} needs repair first" for name in repair)
    broken = [f for f in features if f["status"].startswith(("error", "needs repair"))
        and not (f["name"] in repair and f["status"].startswith("needs repair"))
        and not f["status"].startswith(waiting)
        and f["name"] not in expected_failed]
    prompted = {f["name"] for f in features if f["status"].startswith("needs repair")}
    missing = [name for name in repair if name not in prompted]
    missing_failed = sorted(set(failed) - expected_failed)
    return {"check": "feature rebuilds", "actual": broken, "missing_repairs": missing,
        "missing_failures": missing_failed, "pass": not broken and not missing and not missing_failed}


def references_kept(known, references, repair, deleted, *, replaced=False, reopened=False):
    """Each reference names the origin it named before, and resolves or raises the repair prompt.

    An opened file replaces the document: a reference it does not hold left with the old
    document, while one it does hold must still name the same origin."""
    if replaced:
        known.clear()
    now = {(r["feature_id"], r["slot"]): r for r in references}
    if reopened:
        for key in [key for key in known if key not in now]:
            del known[key]
    changed = [{"reference": list(key), "was": identity, "now": now[key]["origin"]}
        for key, identity in known.items() if key in now
        and (now[key]["origin"] != identity["name"]
            or (all(o is not None for o in now[key]["kernel_origins"])
                and now[key]["kernel_origins"] != identity["kernel"]))]
    lost = [list(key) for key, identity in known.items()
        if key not in now and not deleted and not identity.get("deleted")]
    # A delete took these on purpose. They stay known so an undo must bring back the same origin.
    for key, identity in known.items():
        identity["deleted"] = key not in now and (deleted or identity.get("deleted", False))
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


def step_arguments(step, bundle, previous):
    """Bind arguments to evidence returned by an earlier step."""
    arguments = substitute(step.get("arguments", {}), bundle)
    for name, source in step.get("arguments_from", {}).items():
        if name in arguments:
            raise ValueError(f"argument {name} has both a literal and an evidence source")
        arguments[name] = at(previous[source["step"]], source["pointer"])
    return arguments


def cli(binary, arguments, directory, label, timeout):
    start = time.perf_counter_ns()
    result = subprocess.run([binary, *map(str, arguments)], capture_output=True, text=True, timeout=timeout)
    (directory / f"{label}.stdout").write_text(result.stdout)
    (directory / f"{label}.stderr").write_text(result.stderr)
    return {"check": label, "command": [binary, *map(str, arguments)], "exit": result.returncode,
            "elapsed_ms": (time.perf_counter_ns() - start) / 1_000_000,
            "pass": result.returncode == 0}


def oracle_command(oracle, path):
    """The independent checker for one exported STL. Only a named kind or the cubic lattice runs."""
    kind = oracle.get("kind")
    if kind in ("graded-lattice", "graded-gyroid"):
        checker = pathlib.Path(__file__).with_name("check-graded-stl.py")
        field = (["--kind", "lattice", "--cell", str(oracle["cell"])] if kind == "graded-lattice"
                 else ["--kind", "gyroid", "--period", str(oracle["period"])])
        return [str(checker), str(path), *field, "--box", *map(str, oracle["box"]),
                "--tight", *map(str, oracle["tight"]), f"--grade={','.join(map(str, oracle['grade']))}",
                "--accuracy", oracle["accuracy"], "--caps", *map(str, oracle["caps"])]
    if kind == "gyroid":
        checker = pathlib.Path(__file__).with_name("check-gyroid-stl.py")
        return [str(checker), str(path), "--box", *map(str, oracle["box"]), "--period", str(oracle["period"]),
                f"--level={oracle['level']}", "--accuracy", oracle["accuracy"],
                "--caps", *map(str, oracle["caps"])]
    if kind is not None:
        raise ValueError(f"unknown oracle kind {kind}")
    checker = pathlib.Path(__file__).with_name("check-lattice-stl.py")
    command = [str(checker), str(path), "--box", *map(str, oracle["box"]),
               "--cell", str(oracle["cell"]), "--radius", str(oracle["radius"]),
               "--accuracy", oracle["accuracy"], "--euler", str(oracle["euler"])]
    if "caps" in oracle:
        command += ["--caps", *map(str, oracle["caps"])]
    return command


def moments_command(oracle, report, step=None):
    """The independent moment check of a field mass answer or a mass stand-in export."""
    checker = pathlib.Path(__file__).with_name("check-field-moments.py")
    command = [str(checker), str(report), "--reference", oracle["reference"],
               "--permutation", str(oracle.get("permutation", 0)), "--max-work", str(oracle["max_work"])]
    return command + (["--step", str(step)] if step is not None else [])


def replay_stand_in(args, directory, path, spec):
    """The same stand-in asked of the pinned kernel's public request: the field's native
    recipe from this step's evidence, the design's widths and the application allowance.
    The application's file must be the request's answer byte for byte."""
    question = {"format": "catallax-question/1", "source": "recipe", "max_work": spec["replay"]["max_work"],
                "problem": {"format": "catallax-stand-in/1",
                            "recipe": json.loads((directory / "recipe.json").read_text()),
                            "accuracy": {"volume": {"relative": "0", "absolute": "1/100"},
                                         "centroid": {"relative": "0", "absolute": "1/1000"},
                                         "inertia": {"relative": "0", "absolute": "1/10"}},
                            "max_cells": 2000000}}
    write(directory / "stand-in-question.json", question)
    result = cli(args.catallax_verify, ["replay", directory / "stand-in-question.json", "--out",
                 directory / "stand-in-replay"], directory, "stand-in-replay", args.timeout)
    result["check"] = "public stand-in request answers the same bytes"
    if result["pass"]:
        response = json.loads((directory / "stand-in-replay.stdout").read_text())
        answer = directory / "stand-in-replay" / "answer.step"
        result["status"] = response["manifest"]["status"]
        result["work"] = response["work"]
        result["pass"] = (result["status"] == "answered" and answer.is_file()
                          and answer.read_bytes() == path.read_bytes())
    return result


def export_check(args, directory, spec, previous):
    extension = spec["format"]
    stand_in = spec.get("stand_in", False)
    path = directory / ("stand-in.step" if stand_in else f"part.{extension}")
    label = "export-stand-in" if stand_in else f"export-{extension}"
    # A ceiling runs the fresh process's whole request, opening and rebuilding the part
    # included, on one allowance, and the CLI reports what it used.
    ceiling = ["--max-work", spec["max_work"]] if "max_work" in spec else []
    exported = cli(args.soliddraft, ["export", directory / "part.dwprt", "--json",
                                     "--stand-in" if stand_in else "--out", path, *ceiling],
                   directory, label, args.timeout)
    if "refused" in spec:
        exported["pass"] = (exported["exit"] != 0 and not path.exists() and
                            spec["refused"] in (directory / f"{label}.stderr").read_text())
        return exported
    if not exported["pass"]:
        return exported
    try:
        value = json.loads((directory / f"{label}.stdout").read_text())
        exported["report"] = value["report"]
        exported["checks"] = [check(value, rule) for rule in spec.get("checks", [])]
        exported["sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
        if "max_work" in spec:
            # A B-rep report states no work of its own; its request's allowance is the evidence.
            if value["report"].get("kind") not in ("step", "mesh"):
                exported["checks"].append({"check": "export work within the application allowance",
                                           "work": value["report"].get("work"), "ceiling": spec["max_work"],
                                           "pass": 0 < value["report"].get("work", 0) <= spec["max_work"]})
            exported["checks"].append(request_work_check(value, spec["max_work"]))
        if stand_in:
            exported["checks"].append({"check": "stand-in report describes the written bytes",
                                       "pass": value["report"].get("sha256") == exported["sha256"]})
            if "oracle" in spec:
                command = moments_command(spec["oracle"], directory / f"{label}.stdout", path)
                exported["checks"].append(cli(sys.executable, command, directory, f"{label}-oracle", args.timeout))
            if "replay" in spec:
                exported["checks"].append(replay_stand_in(args, directory, path, spec))
            if spec.get("raw_valid"):
                exported["checks"].append(raw_step_check(path, directory, f"{label}-raw-step", args.timeout))
            exported["pass"] = all(c["pass"] for c in exported["checks"])
            return exported
        if "same_export_as" in spec:
            expected = previous.get((spec["same_export_as"], extension))
            exported["checks"].append({"check": "same exported bytes", "expected": expected,
                                       "pass": expected is not None and expected == exported["sha256"]})
        if "oracle" in spec:
            oracle = spec["oracle"]
            command = oracle_command(oracle, path)
            result = cli(sys.executable, command, directory, f"{label}-oracle", args.timeout)
            exported["checks"].append(result)
            if "kind" in oracle:
                # Report counts stay claims until they match the bytes the oracle read.
                decoded = json.loads((directory / f"{label}-oracle.stdout").read_text())
                exported["checks"].append({"check": "export report counts match labelled file",
                    "pass": value["report"].get("triangles") == decoded["triangles"] and
                            value["report"].get("vertices") == decoded["vertices"]})
            # The report must describe precisely the bytes the oracle checked.
            bound = struct.unpack_from("<d", path.read_bytes(), 68)[0]
            exported["checks"].append({"check": "export report matches labelled file",
                "pass": value["report"].get("deviation_bound_mm") == bound and
                        value["report"].get("sha256") == exported["sha256"]})
        exported["pass"] = all(c["pass"] for c in exported["checks"])
    except (KeyError, TypeError, ValueError, OSError, struct.error) as error:
        exported.update({"pass": False, "error": str(error)})
    return exported


def replay_question(verifier, question, directory, timeout):
    result = cli(verifier, ["replay", question, "--out", directory / "replay"], directory, "replay", timeout)
    # The verifier returns a complete response for refusals too. Infrastructure errors fail this check.
    if result["pass"]:
        response = json.loads((directory / "replay.stdout").read_text())
        result["status"] = response["manifest"]["status"]
        result["refusal"] = (response["manifest"].get("refusal") or {}).get("code")
        result["pass"] = result["status"] not in ("internal-error", "invalid-question")
    return result


def same_geometry(binary, directory, previous_directory, timeout):
    result = {"check": "same geometry as step", "pass": False, "info": []}
    hashes = []
    for folder in (previous_directory, directory):
        info = cli(binary, ["doc", "info", folder / "part.dwprt", "--json"],
                   folder, "geometry-info", timeout)
        result["info"].append(info)
        if not info["pass"]:
            return result
        hashes.append(json.loads((folder / "geometry-info.stdout").read_text()).get("geometry_hash"))
    result.update({"expected": hashes[0], "actual": hashes[1],
                   "pass": isinstance(hashes[0], str) and bool(hashes[0]) and hashes[0] == hashes[1]})
    return result


def request_work_check(value, ceiling):
    """A fresh export ran its whole request, opening and rebuilding included, on one allowance."""
    request = value.get("request_work") or {}
    return {"check": "fresh open, rebuild and export within one allowance", "request_work": request,
            "ceiling": ceiling, "pass": request.get("limit") == ceiling and request.get("exhausted") is False
                                        and value["report"].get("work", 0) <= request.get("used", 0) <= ceiling}


def checked_step_export(binary, part, directory, evidence, timeout, max_work=None, oracle=None):
    ceiling = [] if max_work is None else ["--max-work", max_work]
    exported = cli(binary, ["export", part, "--json", "--out", directory / "part.step", *ceiling],
                   directory, "checked-step-export", timeout)
    checks = [exported]
    if not exported["pass"]:
        return checks
    value = json.loads((directory / "checked-step-export.stdout").read_text())
    report = value["report"]
    if max_work is not None:
        checks.append(request_work_check(value, max_work))
    checks.append({"check": "one within-tolerance STEP solid", "report": report,
                   "pass": report.get("kind") == "step" and report.get("within_tolerance") is True
                           and report.get("solids") == 1})
    write(directory / "step-report.json", {"report": report, "body": {
        "faces": evidence["summary"]["body"]["faces"],
        "edges": evidence["summary"]["body"]["edges"],
        "volume": evidence["measurement"]["volume_mm3"],
        "area": evidence["measurement"]["area_mm2"],
    }})
    checks.extend(output_files([directory / "part.step"]))
    checks.append(cli(sys.executable, [pathlib.Path(__file__).with_name("check-step.py"),
        directory / "part.step", directory / "step-report.json"], directory, "check-step", timeout))
    if not checks[-1]["pass"]:
        return checks
    checks.append(raw_step_check(directory / "part.step", directory, "raw-step", timeout))
    if oracle is not None and checks[-1]["pass"]:
        write(directory / "step-oracle.json", oracle)
        checks.append(step_oracle_check(directory / "part.step", directory, timeout))
    return checks


def raw_step_check(step, directory, label, timeout):
    """The kernel gate's direct no-healing validity check of every solid in a STEP file."""
    root = pathlib.Path(__file__).resolve().parent.parent
    # Use the kernel gate's direct translator, with its own OCP 8 binding.
    program = """import importlib.metadata, json, pathlib, subprocess, sys
root, revision, step = sys.argv[1:]
assert importlib.metadata.version("cadquery-ocp") == "8.0.1.1.0", "raw STEP check requires OCP 8.0.1.1.0"
actual = subprocess.check_output(["git", "-C", root, "rev-parse", "HEAD"], text=True).strip()
assert actual == revision, "raw STEP checker checkout differs from the adapter pin"
sys.path.insert(0, str(pathlib.Path(root) / "tools/checks"))
from raw_step import valid
passed = valid(step)
print(json.dumps({"catallax_revision": actual, "raw_valid": passed}))
sys.exit(0 if passed else 1)
"""
    return cli(str(root / ".ci-ocp8/bin/python"), ["-c", program, root / ".ci-catallax", pinned_revision(), step],
               directory, label, timeout)


def step_oracle_check(step, directory, timeout):
    """A design's independent solid, volume and material expectations, on the direct and ordinary imports."""
    root = pathlib.Path(__file__).resolve().parent.parent
    return cli(str(root / ".ci-ocp8/bin/python"), [pathlib.Path(__file__).with_name("check-step-oracle.py"),
               root / ".ci-catallax", pinned_revision(), step, directory / "step-oracle.json"],
               directory, "step-oracle", timeout)


def scenario_folders(bundle, folders):
    for folder in folders:
        path = (bundle / folder).resolve()
        if not path.is_relative_to(bundle.resolve()):
            raise ValueError("scenario folder must stay inside the bundle")
        path.mkdir(parents=True, exist_ok=True)


def inspect(binary, requests, bundle, directory, timeout):
    result = {}
    for label, arguments in requests.items():
        arguments = [
            argument.replace("{bundle}", str(bundle)).replace("{step}", str(directory)) for argument in arguments]
        process = subprocess.run([binary, *arguments], capture_output=True, text=True, timeout=timeout)
        (directory / f"inspect-{label}.stdout").write_text(process.stdout)
        (directory / f"inspect-{label}.stderr").write_text(process.stderr)
        if process.returncode:
            raise RuntimeError(f"inspect {label} exited {process.returncode}: {process.stderr}")
        result[label] = json.loads(process.stdout)
    return result


def timings(steps):
    """Labelled steps' click times, such as a feature's build and edit."""
    return [{"label": s["label"], "tool": s["tool"], "elapsed_ms": s.get("timing", {}).get("elapsed_ms"),
             "pass": s["pass"]} for s in steps if "label" in s]


def no_solid(request, command, displayed):
    """Whether the display reply shows no solid, and why a reply that claims none cannot.

    Only a successful reply for this request and command revision may say `solid: false`.
    Every other reply owes the step its display, whatever files exist."""
    reply = displayed.get("structuredContent") or {}
    if reply.get("solid") is not False:
        return False, None
    if displayed.get("isError"):
        return False, "a failed reply cannot report no solid"
    reason = answers(request, command, reply)
    return reason is None, reason


def wait(client, entry, name, arguments):
    """One timed MCP call. A timeout or closed pipe leaves the step incomplete, never passed."""
    try:
        return client.request("tools/call", {"name": name, "arguments": arguments})
    except RuntimeError as error:
        entry["incomplete"] = {"request": name, "reason": str(error)}
        raise


def advise(report, message, missed=True):
    """A timing finding: a warning annotation kept in the result, never a failure.

    A missed budget also keeps the run from a strict pass."""
    report["warnings"].append(message)
    if missed:
        report["over_budget"].append(message)
    print(f"::warning::{message}", flush=True)


def summary(reports, machine):
    """The job summary: every timing over its budget, every listed step against its record."""
    over, listed, trends, errors = [], [], [], []
    for report in reports:
        for step in report["steps"]:
            for c in step["checks"]:
                if c.get("within_budget") is False:
                    over.append(f"| {report['name']} | {step['number']} | {step['tool']} | {c['check']} | "
                                f"{c.get('elapsed_ms', c.get('slowest_ms')):.3f} | {c['budget_ms']} |")
            timing = step.get("timing", {})
            if "known_over_budget" in timing:
                recorded = timing["known_over_budget"]["last_measured_ms"]
                listed.append(f"| {report['name']} | {step['number']} | {step['tool']} | "
                              f"{timing['elapsed_ms']:.3f} | {recorded:.3f} | "
                              f"{100 * timing['elapsed_ms'] / recorded:.0f}% | {timing['status']} |")
        for trend in report.get("trends", []):
            result = "error" if "error" in trend else "within" if trend["within_budget"] else "over"
            growth = trend.get("growth_ms")
            trends.append(f"| {report['name']} | {trend['tool']} | {trend.get('samples', 0)} | "
                          f"{'unavailable' if growth is None else f'{growth:.3f}'} | {trend['noise_ms']} | {result} |")
        errors.extend(f"- {report['name']}: {w}" for w in report.get("warnings", [])
                      if w not in report.get("over_budget", []))
        if report.get("error"):
            errors.append(f"- {report['name']}: error: {report['error']}")
    rows = ["## Application timings", "",
            f"Release build on {machine}; raw click-to-display timing on a shared runner. "
            "Every step's timing is in the uploaded result.json; these are warnings, not failures.", "",
            "### Over budget", ""]
    rows += (["| Scenario | Step | Tool | Check | ms | Budget ms |", "|---|---:|---|---|---:|---:|", *over]
             if over else ["None."])
    rows += ["", "### Listed in ci/known-over-budget.json", ""]
    rows += (["| Scenario | Step | Tool | Wait ms | Recorded ms | Of recorded | Status |",
              "|---|---:|---|---:|---:|---:|---|", *listed] if listed else ["None ran."])
    if trends:
        rows += ["", "### Feature-count latency trends", "",
                 "| Scenario | Tool | Samples | Fitted growth ms | Noise ms | Growth |",
                 "|---|---|---:|---:|---:|---|", *trends]
    if errors:
        rows += ["", "### Other findings", "", *errors]
    return "\n".join(rows) + "\n"


def start_client(args, scenario, folder):
    """A new application process, initialized and prepared outside every click."""
    client = Client(args.soliddraft, folder, scenario.get("max_work", 100_000_000), args.timeout)
    initialized = client.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "soliddraft-scenario", "version": "1"}})
    if initialized["protocolVersion"] != "2025-06-18":
        raise RuntimeError("MCP initialization version mismatch")
    client.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
    # GPU preparation runs once, outside every click.
    response = client.request("tools/call", {"name": "scenario_prepare", "arguments": {}})
    if response.get("isError"):
        raise RuntimeError(f"renderer preparation failed: {response}")
    return client


def helper(arguments, bundle, directory, label, timeout):
    """A check script beside this runner, given bundle paths."""
    script, *rest = substitute(arguments, bundle)
    return cli(sys.executable, [pathlib.Path(__file__).with_name(script), *rest], directory, label, timeout)


def run(args):
    scenario = json.loads(args.scenario.read_text())
    if scenario["format"] != "soliddraft-scenario/1" or not scenario.get("steps"):
        raise ValueError("expected soliddraft-scenario/1 with steps")
    args.out.mkdir(parents=True, exist_ok=False)
    write(args.out / "scenario.json", scenario)
    scenario_folders(args.out, scenario.get("folders", []))
    pin = pinned_revision()
    report = {"format": "soliddraft-scenario-result/1", "name": scenario["name"], "catallax_revision": pin,
              "soliddraft_revision": subprocess.check_output(
                  ["git", "-C", str(pathlib.Path(__file__).resolve().parent.parent), "rev-parse", "HEAD"], text=True).strip(),
              "steps": [], "warnings": [], "over_budget": [], "pass": False,
              "strict_pass": False}
    client = None
    transcript = []
    try:
        # A malformed file still fails the run: the listings are data, even though timing is not gated.
        known_latency = load_known_over_budget(KNOWN_OVER_BUDGET)
        for name, number in known_latency:
            if name == scenario["name"] and not 1 <= number <= len(scenario["steps"]):
                advise(report, f"remove missing {name} step {number} from ci/known-over-budget.json", missed=False)
        for index, arguments in enumerate(scenario.get("setup", [])):
            result = helper(arguments, args.out, args.out, f"setup-{index}", args.timeout)
            if not result["pass"]:
                raise RuntimeError(f"scenario setup failed: {result['command']}")
        commands = args.out / "commands"
        client = start_client(args, scenario, commands)
        known = {}
        previous = {}
        exports = {}
        folders = {}
        session = offset = 0
        for number, step in enumerate(scenario["steps"], 1):
            entry = {"number": number, "tool": step["tool"], "checks": [], "pass": False}
            if "label" in step:
                entry["label"] = step["label"]
            report["steps"].append(entry)
            if step.get("fresh_process"):
                # The application exits cleanly and nothing it held survives but its files.
                # Each process counts its own requests, so its evidence has its own folder.
                client.close()
                transcript += client.transcript
                session, offset = session + 1, number - 1
                commands = args.out / f"commands-{session}"
                client = start_client(args, scenario, commands)
            request = number - offset
            arguments = step_arguments(step, args.out, previous)
            started = time.perf_counter_ns()
            result = wait(client, entry, step["tool"], arguments)
            command_ms = (time.perf_counter_ns() - started) / 1_000_000
            directory = commands / f"{request:04}"
            folders[number] = directory
            # The click ends at the viewport, before the command's evidence is written. A step
            # with no solid to show ends at the command's reply; its display request does not count.
            started = time.perf_counter_ns()
            displayed = wait(client, entry, "scenario_display", {})
            display_ms = (time.perf_counter_ns() - started) / 1_000_000
            written = wait(client, entry, "scenario_evidence", {})
            if written.get("isError"):
                raise RuntimeError(f"bundle evidence was not written: {written}")
            command = json.loads((directory / "command.json").read_text())
            kernel = json.loads((directory / "kernel.json").read_text())
            segments = [{"source": "kernel.json", "elapsed_ms": command_ms, "kernel": kernel,
                         "phase": "refusal" if result.get("isError") else command_phase(step["tool"])}]
            entry["feature_count"] = len(command["summary"]["features"])
            checks = entry["checks"]
            none, reason = no_solid(request, command, displayed)
            if reason is not None:
                checks.append({"check": "no-solid reply answers this request", "reason": reason, "pass": False})
            solid = not none
            expected_error = bool(step.get("refusal") or step.get("refused"))
            checks.append({"check": "command outcome", "pass": bool(result.get("isError")) == expected_error})
            if command.get("request") != request:
                checks.append({"check": "command evidence is this request", "actual": command.get("request"),
                               "pass": False})
            checks.append({"check": "Catallax backend", "pass": command["backend"]["name"] == "catallax"})
            checks.append(backend_pinned(command, pin))
            if step.get("refused") and number > 1 and step["tool"] == "sketch":
                before = previous[number - 1]["summary"]
                checks.append({"check": "refused CAD preserves features and undo", "pass":
                    before["features"] == command["summary"]["features"] and
                    before.get("undo") == command["summary"].get("undo")})
            if step.get("refused"):
                # An application refusal, such as an export the drawing's previews cannot
                # certify: the tool refuses with this text and no kernel replay applies.
                checks.append({"check": "application refusal", "actual": command["output"],
                               "pass": step["refused"] in command["output"]})
            if "drawing" in command["summary"]:
                # The drawing followed the part inside this command: each regenerated view
                # is recorded against the drawing-view ceiling, on top of the click budget below.
                views = drawing_view_calls(kernel, gated=budget(step["tool"]) != "output")
                checks.append(views)
                if views["within_budget"] is False:
                    advise(report, f"{scenario['name']} step {number}: a regenerated drawing view took "
                                   f"{views['slowest_ms']:.3f} ms; ceiling is {DRAWING_VIEW_BUDGET_MS} ms")
            try:
                # The tool's JSON answer, for checks on what it reported.
                command["result"] = json.loads(command["output"])
            except (TypeError, ValueError):
                command["result"] = None
            repair = step.get("repair", [])
            checks.append(rebuilds(command["summary"]["features"], repair, step.get("failed")))
            checks.append(references_kept(known, command["references"], repair,
                step["tool"] == "delete_feature" and not result.get("isError"),
                replaced=step["tool"] == "new_part" and not result.get("isError"),
                reopened=step["tool"] == "open" and not result.get("isError")))
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
            if solid:
                checks.append({"check": "live display", "response": displayed, "pass": not displayed.get("isError")})
                lost = [] if displayed.get("isError") else [
                    name for name in ("display.json", "display.kernel.json") if not (directory / name).exists()]
                if lost:
                    checks.append({"check": "display evidence written", "pass": False,
                                   "reason": f"scenario_display wrote no {' or '.join(lost)} for this step's solid"})
                # Without its capture calls the display still keeps its whole wait in the click.
                log = directory / "display.kernel.json"
                segments.append({"source": "display.kernel.json", "elapsed_ms": display_ms, "phase": "display",
                                 "kernel": json.loads(log.read_text()) if log.exists() else {"calls": []}})
                display = None
                if not displayed.get("isError") and "display.json" not in lost:
                    display = json.loads((directory / "display.json").read_text())
                    checks.append({"check": "live Catallax display", "actual": display,
                                   "pass": display["backend"]["name"] == "catallax" and display["live_document"]})
                # The click ends only when the viewport shows this revision with pickable faces.
                reason = ("display request failed" if displayed.get("isError") else "display evidence missing"
                          if display is None else answers(request, command, display, True))
                checks.append({"check": "click ends at a visible selectable edit", "reason": reason,
                               "pass": reason is None})
                checks.append({"check": "viewport PNG", "pass": (directory / "viewport.png").exists()})
            entry["waits_ms"] = [{"source": s["source"], "elapsed_ms": s["elapsed_ms"]} for s in segments]
            write(directory / "waits.json", entry["waits_ms"])
            latency = accept_latency(scenario["name"], number, step["tool"], timing(step["tool"], segments), known_latency)
            entry["timing"] = latency
            entry["intervals"] = {"preview": {
                "starts": "command request sent",
                "ends": "visible selectable edit" if solid else "command reply, no solid to show",
                "elapsed_ms": latency["elapsed_ms"], "budget_ms": latency["budget_ms"],
                "verdict": latency["verdict"]}}
            checks.append(latency)
            if "max_edit_work" in step:
                checks.append(edit_work_check(step, latency))
            if "max_action_work" in scenario:
                checks.append(action_work_check(scenario["max_action_work"], latency))
            if "moments" in step and command.get("result") is not None:
                # The field_mass answer, checked against the design's references.
                write(directory / "field-mass.json", command["result"])
                checks.append(cli(sys.executable, moments_command(step["moments"], directory / "field-mass.json"),
                                  directory, "field-mass-oracle", args.timeout))
            write(directory / "timing.json", latency)
            print(json.dumps({"step": number, "tool": step["tool"], "timing": latency}), flush=True)
            if latency["status"] == "output":
                print(f"::notice::{latency['message']}", flush=True)
            elif "message" in latency:
                advise(report, latency["message"], missed=latency["within_budget"] is False)
            if solid:
                started = time.perf_counter_ns()
                viewed = wait(client, entry, "scenario_view", {})
                view_ms = (time.perf_counter_ns() - started) / 1_000_000
                if step.get("view") == "unavailable":
                    checks.append({"check": "field drawing unavailable", "response": viewed,
                                   "pass": bool(viewed.get("isError")) and
                                   "drawings of field parts are coming soon" in json.dumps(viewed)})
                else:
                    checks.append({"check": "live hidden-line view", "response": viewed,
                                   "pass": not viewed.get("isError")})
                    view_latency = drawing_view_timing(
                        view_ms, json.loads((directory / "view.kernel.json").read_text()))
                    entry["drawing_view"] = view_latency
                    checks.append(view_latency)
                    write(directory / "view.timing.json", view_latency)
                    print(json.dumps({"step": number, "tool": "scenario_view", "timing": view_latency}), flush=True)
                    if not view_latency["within_budget"]:
                        advise(report, f"{scenario['name']} drawing-view step {number} took {view_ms:.3f} ms; "
                                       f"budget is {DRAWING_VIEW_BUDGET_MS} ms")
                    entry["intervals"]["view"] = {"elapsed_ms": view_ms, "budget_ms": DRAWING_VIEW_BUDGET_MS,
                                                  "verdict": verdict(view_latency)}
                    if not viewed.get("isError"):
                        view = json.loads((directory / "view.json").read_text())
                        reason = answers(request, command, view)
                        checks.append({"check": "view answers this request", "reason": reason, "pass": reason is None})
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
                measured = wait(client, entry, "scenario_measure", {})
                measured_ms = (time.perf_counter_ns() - started) / 1_000_000
                checks.append({"check": "requested measurement", "pass": not measured.get("isError"),
                               "response": measured})
                measurement = None
                measure_timing = measurement_timing(
                    measured_ms, json.loads((directory / "measurement.kernel.json").read_text()))
                measure_timing["verdict"] = verdict(measure_timing)
                if not measure_timing["within_budget"]:
                    advise(report, f"{scenario['name']} step {number}: requested measurement took {measured_ms:.3f} ms; "
                                   f"budget {measure_timing['budget_ms']} ms, gate {MEASUREMENT_GATE_MS} ms")
                entry["measurement"] = measure_timing
                entry["intervals"]["measurement"] = {
                    "elapsed_ms": measured_ms, "budget_ms": measure_timing["budget_ms"],
                    "gate_ms": MEASUREMENT_GATE_MS, "verdict": measure_timing["verdict"]}
                checks.append(measure_timing)
                if not measured.get("isError"):
                    measurement = json.loads((directory / "measurement.json").read_text())
                    # A proof for an older request or revision cannot stand in for this one.
                    reason = answers(request, command, measurement)
                    checks.append({"check": "measurement answers this request", "reason": reason,
                                   "pass": reason is None})
                    if reason is not None:
                        measurement = None
                evidence = {**command, "measurement": measurement}
            if step.get("inspect"):
                evidence["inspect"] = inspect(args.soliddraft, step["inspect"], args.out, directory, args.timeout)
            checks.extend(check(evidence, spec, previous) for spec in step.get("checks", []))
            previous[number] = evidence
            if "same_geometry_as_step" in step:
                earlier = step["same_geometry_as_step"]
                if isinstance(earlier, bool) or not isinstance(earlier, int) or not 1 <= earlier < number:
                    raise ValueError("same_geometry_as_step must name an earlier step")
                checks.append(same_geometry(args.soliddraft, directory, folders[earlier], args.timeout))
            checks.extend(output_files(substitute(step.get("files", []), args.out),
                                       substitute(step.get("absent", []), args.out)))
            # Exports run after the click is timed, as a user's separate request would.
            if "checked_step_export" in step:
                checks.extend(checked_step_export(args.soliddraft,
                    substitute(step["checked_step_export"], args.out), directory, evidence, args.timeout,
                    step.get("checked_step_max_work"), step.get("step_oracle")))
            for index, arguments in enumerate(step.get("helpers", [])):
                checks.append(helper(arguments, args.out, directory, f"helper-{index}", args.timeout))
            specifications = [{"format": f} for f in step.get("export", [])] + step.get("exports", [])
            for export_spec in specifications:
                extension = export_spec["format"]
                exported = export_check(args, directory, export_spec, exports)
                entry["intervals"].setdefault("export", {})[extension] = {"elapsed_ms": exported["elapsed_ms"]}
                checks.append(exported)
                if exported["pass"] and "label" in step and "sha256" in exported:
                    exports[(step["label"], extension)] = exported["sha256"]
            entry["pass"] = all(c["pass"] for c in checks)
            write(directory / "checks.json", entry)
            write(args.out / "result.json", report)
            if not entry["pass"]:
                # Retain every candidate input, including rejected scratch builds, for kernel diagnosis.
                entry["questions"] = [str(p.relative_to(args.out)) for p in directory.glob("question-*.json")]
                break
        report["trends"] = trend_checks(scenario.get("trends", []), report["steps"])
        for trend in report["trends"]:
            if trend.get("within_budget") is False:
                advise(report, f"{scenario['name']} {trend['tool']} latency grew {trend['growth_ms']:.3f} ms over "
                               f"its feature-count range; noise allowance {TREND_NOISE_MS} ms")
        report["pass"] = (len(report["steps"]) == len(scenario["steps"])
            and all(s["pass"] for s in report["steps"]) and all(t["pass"] for t in report["trends"]))
        report["timings"] = timings(report["steps"])
        report["strict_pass"] = report["pass"] and not report["over_budget"]
    except Exception as error:
        report["error"] = str(error)
    finally:
        try:
            if client is not None:
                client.close()
        except Exception as error:
            report["error"] = str(error)
            report["pass"] = False
        write(args.out / "protocol.json", transcript + (client.transcript if client else []))
        write(args.out / "result.json", report)
    print(json.dumps({"bundle": str(args.out), "pass": report["pass"],
                      "strict_pass": report.get("strict_pass", False),
                      "over_budget": len(report["over_budget"]), "error": report.get("error"),
                      "timings": report.get("timings", [])}))
    if getattr(args, "strict", False):
        # The release path accepts no timing over budget: only a run that met every budget exits zero.
        for message in report["over_budget"]:
            print(f"::error::Release path: {message}; over budget is not a strict pass", flush=True)
        return 0 if report["strict_pass"] else 1
    return 0 if report["pass"] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("scenario", type=pathlib.Path)
    parser.add_argument("--out", type=pathlib.Path, required=True, help="new bundle directory, never overwritten")
    parser.add_argument("--soliddraft", required=True, help="built soliddraft executable")
    parser.add_argument("--catallax-verify", required=True, help="verifier built from the pinned Catallax revision")
    parser.add_argument("--strict", action="store_true",
                        help="release-path verdict: exit nonzero unless every timing budget was met")
    parser.add_argument("--timeout", type=int, default=HANG_TIMEOUT_S,
                        help="hang guard: seconds per MCP request or CLI command")
    args = parser.parse_args()
    args.out = args.out.resolve()
    args.soliddraft = str(pathlib.Path(args.soliddraft).resolve())
    args.catallax_verify = str(pathlib.Path(args.catallax_verify).resolve())
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
