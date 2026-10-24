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
    changed = [{"reference": list(key), "was": origin, "now": now[key]["origin"]}
        for key, origin in known.items() if key in now and now[key]["origin"] != origin]
    lost = [list(key) for key in known if key not in now and not deleted]
    wrong = [r for r in references if r["resolved"] is not None and r["resolved"] != (r["feature"] not in repair)]
    known.clear()
    known.update({key: r["origin"] for key, r in now.items()})
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
    result = subprocess.run([binary, *map(str, arguments)], capture_output=True, text=True, timeout=timeout)
    (directory / f"{label}.stdout").write_text(result.stdout)
    (directory / f"{label}.stderr").write_text(result.stderr)
    return {"check": label, "command": [binary, *map(str, arguments)], "exit": result.returncode,
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
        known = {}
        for number, step in enumerate(scenario["steps"], 1):
            entry = {"number": number, "tool": step["tool"], "checks": [], "pass": False}
            report["steps"].append(entry)
            arguments = substitute(step.get("arguments", {}), args.out)
            result = client.request("tools/call", {"name": step["tool"], "arguments": arguments})
            directory = args.out / "commands" / f"{number:04}"
            command = json.loads((directory / "command.json").read_text())
            checks = entry["checks"]
            checks.append({"check": "command outcome", "pass": bool(result.get("isError")) == bool(step.get("refusal"))})
            checks.append({"check": "Catallax backend", "pass": command["backend"]["name"] == "catallax"})
            checks.extend(check(command, spec) for spec in step.get("checks", []))
            repair = step.get("repair", [])
            checks.append(rebuilds(command["summary"]["features"], repair))
            checks.append(references_kept(known, command["references"], repair, step["tool"] == "delete_feature"))
            if step.get("refusal"):
                code = step["refusal"]
                checks.append({"check": "application refusal", "actual": command["output"], "pass": code in command["output"]})
                kernel = json.loads((directory / "kernel.json").read_text())
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
            if step.get("render", True) and (directory / "recipe.json").exists():
                view = cli(args.soliddraft, ["view", directory / "recipe.json",
                    "--dir", "iso", "--svg", directory / "view.svg", "--json"], directory, "view", args.timeout)
                if view["pass"]:
                    rendered = json.loads((directory / "view.stdout").read_text())
                    view["backend"] = rendered["backend"]
                    view["method"] = rendered["method"]
                    view["pass"] &= rendered["backend"].startswith("catallax ")
                checks.append(view)
                checks.append(cli(args.soliddraft, ["screenshot", directory / "part.dwprt", "--png", directory / "viewport.png"],
                    directory, "screenshot", args.timeout))
                if (directory / "view.svg").exists():
                    checks.append({"check": "SVG contains curves", "pass": "<path " in (directory / "view.svg").read_text()})
            if step.get("drawing"):
                drawing = cli(args.soliddraft, ["draw", directory / "part.dwprt", "--template", "iso-a3",
                    "--out", directory / "drawing.dwdrw", "--out", directory / "drawing.svg", "--kernel-log", directory / "drawing.kernel.json", "--json"], directory, "drawing", args.timeout)
                if drawing["pass"]:
                    rendered = json.loads((directory / "drawing.stdout").read_text())
                    drawing["backend"] = rendered["backend"]
                    drawing["pass"] &= rendered["backend"].startswith("catallax ")
                checks.append(drawing)
            entry["pass"] = all(c["pass"] for c in checks)
            write(directory / "checks.json", entry)
            write(args.out / "result.json", report)
            if not entry["pass"]:
                # Retain every candidate input, including rejected scratch builds, for kernel diagnosis.
                entry["questions"] = [str(p.relative_to(args.out)) for p in directory.glob("question-*.json")]
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
