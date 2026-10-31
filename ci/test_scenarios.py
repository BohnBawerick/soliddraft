"""Checks must fail closed when evidence or comparison fields are missing."""

import argparse
import contextlib
import importlib.util
import io
import json
import os
import pathlib
import struct
import sys
import tempfile
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location("scenarios", pathlib.Path(__file__).with_name("run-scenarios.py"))
scenarios = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scenarios)


def step_checker():
    """Loads check-step.py over a stand-in OCP, whose reader gives one valid solid of 1000 mm3."""
    names = ["OCP"] + [f"OCP.{name}" for name in ("BRepCheck", "BRepGProp", "GProp", "IFSelect", "STEPControl",
                                                  "TopAbs", "TopExp", "TopTools")]
    with mock.patch.dict(sys.modules, {name: mock.MagicMock() for name in names}):
        spec = importlib.util.spec_from_file_location("check_step", pathlib.Path(__file__).with_name("check-step.py"))
        checker = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(checker)
    checker.STEPControl_Reader.return_value.ReadFile.return_value = checker.IFSelect_RetDone
    checker.BRepCheck_Analyzer.return_value.IsValid.return_value = True
    checker.BRepGProp.VolumeProperties_s.return_value = 0.0
    checker.GProp_GProps.return_value.Mass.return_value = 1000.0
    return checker


class CheckStepFit(unittest.TestCase):
    AREA = 600.0
    GAP = 1e-3

    def failures(self, *fit, gap=GAP):
        checker = step_checker()
        checker.GProp_GProps.return_value.Mass.return_value = 1000.0 + gap
        checker.count = lambda shape, kind: 6 if kind is checker.TopAbs_FACE else 1
        step = {"solids": 1, "faces": 6, "removed_faces": 0, "removed_edges": 0,
                "max_deviation_mm": 0.0, "max_deviation": "0", "within_tolerance": True}
        step.update({"fit_mm": value for value in fit})
        report = {"body": {"faces": 6, "edges": 12, "volume": 1000.0, "area": self.AREA}, "report": step}
        return checker.check("part.step", report)[0]

    def assert_rejected(self, fit):
        failures = self.failures(fit)
        self.assertTrue(any("fit_mm" in failure for failure in failures), f"{fit!r}: {failures}")
        self.assertTrue(any("volume" in failure for failure in failures), f"{fit!r}: {failures}")

    def test_a_valid_finite_fit_widens_the_allowance_by_its_area(self):
        self.assertEqual(self.failures(self.GAP / self.AREA * 1.01), [])
        self.assertTrue(any("volume" in failure for failure in self.failures(self.GAP / self.AREA * 0.5)))

    def test_an_absent_or_zero_fit_adds_nothing(self):
        self.assertEqual(self.failures(gap=0.0), [])
        for fit in ((), (0, ), (0.0, )):
            self.assertTrue(any("volume" in failure for failure in self.failures(*fit)), fit)

    def test_overflow_is_refused(self):
        self.assert_rejected(json.loads("1e999"))
        self.assert_rejected(10 ** 400)
        # Finite, but the area takes it past the largest float.
        failures = self.failures(1e308)
        self.assertTrue(any("allowance" in failure for failure in failures), failures)

    def test_non_finite_is_refused(self):
        for fit in (float("inf"), float("-inf"), float("nan")):
            self.assert_rejected(fit)

    def test_a_boolean_is_refused(self):
        for fit in (True, False):
            self.assert_rejected(fit)

    def test_a_negative_fit_is_refused(self):
        self.assert_rejected(-1e-6)

    def test_a_fit_that_is_not_a_number_is_refused(self):
        for fit in ("0.001", None, [0.001]):
            self.assert_rejected(fit)


class CheckStepFaces(unittest.TestCase):
    def failures(self, read, reported, exact, removed, removed_edges=0):
        checker = step_checker()
        checker.count = lambda shape, kind: read if kind is checker.TopAbs_FACE else 1
        report = {"body": {"faces": exact, "edges": 20, "volume": 1000.0, "area": 600.0},
                  "report": {"solids": 1, "faces": reported, "removed_faces": removed, "removed_edges": removed_edges,
                             "max_deviation_mm": 0.0, "max_deviation": "0", "within_tolerance": True}}
        return checker.check("part.step", report)[0]

    def test_removed_faces_reconcile_the_written_and_exact_counts(self):
        self.assertEqual(self.failures(9, 9, 11, 2), [])

    def test_disagreeing_counts_still_fail_with_all_three_named(self):
        for read, reported, exact, removed in ((10, 9, 11, 2), (9, 10, 11, 2), (9, 9, 12, 2), (9, 9, 11, 1)):
            self.assertEqual(self.failures(read, reported, exact, removed), [
                f"{read} faces in OpenCASCADE, {reported} in the report, {exact} on the exact body"])

    def test_no_regularizations_leave_the_three_way_equality(self):
        self.assertEqual(self.failures(11, 11, 11, 0), [])
        self.assertEqual(self.failures(9, 9, 11, 0), ["9 faces in OpenCASCADE, 9 in the report, 11 on the exact body"])

    def test_removed_counts_must_be_whole_numbers_within_the_exact_counts(self):
        for value in (-1, True, False, 0.5, 1.0, "1", None):
            self.assertEqual(self.failures(12, 12, 11, value), [
                f"the report's removed_faces is {value!r}, not a whole number from 0 to the exact body's 11"])
            self.assertEqual(self.failures(11, 11, 11, 0, value), [
                f"the report's removed_edges is {value!r}, not a whole number from 0 to the exact body's 20"])
        self.assertEqual(self.failures(0, 0, 11, 12), [
            "the report's removed_faces is 12, not a whole number from 0 to the exact body's 11"])
        self.assertEqual(self.failures(11, 11, 11, 0, 21), [
            "the report's removed_edges is 21, not a whole number from 0 to the exact body's 20"])
        self.assertEqual(self.failures(11, 11, 11, 0, 20), [])


class Checks(unittest.TestCase):
    def test_edit_work_ceiling_accepts_equality_and_rejects_one_unit_more(self):
        step = {"max_edit_work": 3571497}
        equal = scenarios.edit_work_check(step, {"work": {"edit": 3571497}})
        self.assertTrue(equal["pass"])
        above = scenarios.edit_work_check(step, {"work": {"edit": 3571498}})
        self.assertFalse(above["pass"])
        self.assertEqual(above["status"], "edit-work-regression")

    def test_action_work_counts_click_and_background_work_against_the_ceiling(self):
        latency = {"work": {"edit": 5, "display": 4}, "background": {"work": {"display": 1}}}
        self.assertTrue(scenarios.action_work_check(10, latency)["pass"])
        self.assertFalse(scenarios.action_work_check(9, latency)["pass"])
        for ceiling in (-1, True, 1.5, None):
            with self.subTest(ceiling=ceiling), self.assertRaises(ValueError):
                scenarios.action_work_check(ceiling, latency)

    def test_graded_oracles_name_their_checker_and_exact_inputs(self):
        oracle = {"kind": "graded-gyroid", "box": [-1, -1, -1, 1, 1, 1], "tight": [-1, -1, -1, 1, 1, 1],
                  "period": 10, "grade": ["z", "-1", "1", "-1/10", "1/10"], "accuracy": "1/10",
                  "caps": [2000000, 1000000]}
        command = scenarios.oracle_command(oracle, "part.stl")
        self.assertTrue(command[0].endswith("check-graded-stl.py"))
        self.assertEqual(command[command.index("--kind") + 1], "gyroid")
        self.assertIn("--grade=z,-1,1,-1/10,1/10", command)
        lattice = dict(oracle, kind="graded-lattice", cell=100)
        self.assertIn("--cell", scenarios.oracle_command(lattice, "part.stl"))
        moments = scenarios.moments_command({"reference": "D1", "max_work": 19999999}, "report.json", "s.step")
        self.assertTrue(moments[0].endswith("check-field-moments.py"))
        self.assertEqual(moments[moments.index("--reference") + 1], "D1")
        self.assertEqual(moments[-2:], ["--step", "s.step"])

    def test_edit_work_ceiling_rejects_invalid_limits(self):
        for ceiling in (-1, True, 1.5, None):
            with self.subTest(ceiling=ceiling), self.assertRaises(ValueError):
                scenarios.edit_work_check({"max_edit_work": ceiling}, {"work": {"edit": 1}})

    def test_numeric_check_and_missing_evidence(self):
        rule = {"pointer": "/body/volume", "approx": 12000, "tolerance": 0.01}
        self.assertTrue(scenarios.check({"body": {"volume": 12000.005}}, rule)["pass"])
        self.assertFalse(scenarios.check({"body": {"volume": 11999}}, rule)["pass"])
        self.assertFalse(scenarios.check({"body": {}}, rule)["pass"])
        self.assertFalse(scenarios.check({"body": {"volume": None}}, rule)["pass"])

    def test_a_tally_counts_a_list_by_field(self):
        faces = [{"name": "Blend", "surface": "torus"}, {"name": "Blend", "surface": "spline"}]
        evidence = {"faces": faces}
        rule = {"pointer": "/faces", "tally": {"surface": {"torus": 1, "spline": 1}, "name": {"Blend": 2}}}
        self.assertTrue(scenarios.check(evidence, rule)["pass"])
        self.assertFalse(scenarios.check(evidence, {"pointer": "/faces", "tally": {"surface": {"torus": 2}}})["pass"])
        self.assertFalse(scenarios.check(evidence, {"pointer": "/faces", "tally": {"kind": {"torus": 2}}})["pass"])

    def test_invalid_or_ambiguous_comparisons_fail(self):
        for rule in ({"pointer": ""}, {"pointer": "", "equals": 3, "minimum": 1},
                     {"pointer": "bad", "equals": 3}):
            self.assertFalse(scenarios.check(3, rule)["pass"])

    def test_expected_repairs_and_the_features_waiting_for_them(self):
        features = [{"name": "Sketch4", "status": "needs repair: Boss · End cap no longer resolves"},
            {"name": "Cut-Extrude1", "status": "error: Sketch4 needs repair first"}, {"name": "Plate", "status": "ok"}]
        self.assertTrue(scenarios.rebuilds(features, ["Sketch4"])["pass"])
        self.assertFalse(scenarios.rebuilds(features, [])["pass"])
        self.assertFalse(scenarios.rebuilds(features[2:], ["Sketch4"])["pass"], "an expected prompt is missing")

    def test_references_keep_their_origins(self):
        ref = {"feature_id": "F5", "feature": "Sketch3", "slot": "sketch plane", "origin": "F2 End cap #1",
            "resolved": True, "kernel_origins": [{"tag": "F2.r0.sweep", "role": "end_cap"}], "matches_origin": True}
        known = {}
        self.assertTrue(scenarios.references_kept(known, [ref], [], False)["pass"])
        self.assertTrue(scenarios.references_kept(known, [dict(ref, feature="Boss sketch")], [], False)["pass"])
        self.assertFalse(scenarios.references_kept(known, [dict(ref, origin="F2 Start cap #1")], [], False)["pass"])
        self.assertFalse(scenarios.references_kept(known, [dict(ref, kernel_origins=[{"tag": "F8.r0.sweep"}])], [], False)["pass"])
        self.assertFalse(scenarios.references_kept(known, [dict(ref, matches_origin=False)], [], False)["pass"])
        self.assertFalse(scenarios.references_kept(known, [dict(ref, resolved=False)], [], False)["pass"])
        self.assertTrue(scenarios.references_kept(known, [dict(ref, resolved=False)], ["Sketch3"], False)["pass"])
        self.assertFalse(scenarios.references_kept(known, [dict(ref, resolved=True)], ["Sketch3"], False)["pass"])
        self.assertTrue(scenarios.references_kept(known, [dict(ref, resolved=None)], [], False)["pass"], "not built")
        self.assertFalse(scenarios.references_kept(known, [], [], False)["pass"], "a reference vanished")
        known["x"] = {"name": "y", "kernel": []}
        self.assertTrue(scenarios.references_kept(known, [], [], True)["pass"], "its feature was deleted")
        self.assertTrue(scenarios.references_kept(known, [], [], False)["pass"], "it stays deleted on later steps")
        undone = dict(ref, origin="F2 Start cap #1")
        self.assertFalse(scenarios.references_kept(known, [undone], [], False)["pass"], "an undo brings the same origin")
        self.assertTrue(scenarios.references_kept(known, [ref], [], False)["pass"])
        self.assertFalse(scenarios.references_kept(known, [], [], False)["pass"], "the undo cleared the deletion")

    def test_new_part_starts_its_own_reference_history(self):
        ref = {"feature_id": "F1", "feature": "Sketch1", "slot": "sketch plane", "origin": "Top plane",
            "resolved": True, "kernel_origins": [], "matches_origin": True}
        known = {}
        self.assertTrue(scenarios.references_kept(known, [ref], [], False)["pass"])
        # A refused replacement retains the old document and its reference checks.
        self.assertFalse(scenarios.references_kept(known, [], [], False, replaced=False)["pass"])
        self.assertTrue(scenarios.references_kept(known, [], [], False, replaced=True)["pass"])
        self.assertEqual(known, {})
        other = dict(ref, origin="Front plane")
        self.assertTrue(scenarios.references_kept(known, [other], [], False)["pass"])
        self.assertFalse(scenarios.references_kept(known, [ref], [], False)["pass"])
        self.assertFalse(scenarios.references_kept(known, [], [], False)["pass"])

    def test_an_opened_file_keeps_only_the_references_it_holds(self):
        ref = {"feature_id": "F1", "feature": "Sketch1", "slot": "sketch plane", "origin": "Top plane",
            "resolved": True, "kernel_origins": [], "matches_origin": True}
        fillet = dict(ref, feature_id="F5", feature="Fillet1", slot="edge 1", origin="F2 Edge 4")
        known = {}
        self.assertTrue(scenarios.references_kept(known, [ref, fillet], [], False)["pass"])
        # A refused open retains the document and its references.
        self.assertFalse(scenarios.references_kept(known, [ref], [], False, reopened=False)["pass"])
        self.assertTrue(scenarios.references_kept(known, [ref], [], False, reopened=True)["pass"])
        self.assertEqual(list(known), [("F1", "sketch plane")])
        # A reference the opened file holds must still name its origin.
        self.assertFalse(scenarios.references_kept(known, [dict(ref, origin="Front plane")], [], False, reopened=True)["pass"])

    def test_bundle_paths(self):
        self.assertEqual(scenarios.substitute({"path": "{bundle}/a.dwprt", "n": [1, "{bundle}"]}, "/run"),
            {"path": "/run/a.dwprt", "n": [1, "/run"]})

    def test_timings_list_labelled_steps_only(self):
        steps = [{"tool": "sketch", "pass": True, "timing": {"elapsed_ms": 3.0}},
                 {"tool": "hole", "label": "hole build", "pass": True, "timing": {"elapsed_ms": 812.5}}]
        self.assertEqual(scenarios.timings(steps),
                         [{"label": "hole build", "tool": "hole", "elapsed_ms": 812.5, "pass": True}])

    def test_json_pointer_escapes_and_arrays(self):
        self.assertEqual(scenarios.at({"a/b": [{"~": 2}]}, "/a~1b/0/~0"), 2)


class ReviewedArguments(unittest.TestCase):
    def test_token_is_copied_from_the_preview_evidence(self):
        step = {"arguments": {"path": "{bundle}/source.step"},
                "arguments_from": {"preview_token": {"step": 3, "pointer": "/result/token"}}}
        previous = {3: {"result": {"token": "reviewed source and questions"}}}
        self.assertEqual(scenarios.step_arguments(step, "/run", previous),
                         {"path": "/run/source.step", "preview_token": "reviewed source and questions"})
        with self.assertRaises(KeyError):
            scenarios.step_arguments(step, "/run", {})
        with self.assertRaises(KeyError):
            scenarios.step_arguments(step, "/run", {3: {"result": {}}})
        step["arguments"]["preview_token"] = "ambiguous literal"
        with self.assertRaises(ValueError):
            scenarios.step_arguments(step, "/run", previous)


class ExpectedFailedFeature(unittest.TestCase):
    def test_absent_derivative_and_present_digest(self):
        self.assertTrue(scenarios.check({}, {"pointer": "/derivative", "absent": True})["pass"])
        for value in ({"derivative": None}, {"derivative": {"sha256": "a" * 64}}):
            self.assertFalse(scenarios.check(value, {"pointer": "/derivative", "absent": True})["pass"])
        self.assertTrue(scenarios.check({"sha256": "a" * 64}, {"pointer": "/sha256", "absent": False})["pass"])
        self.assertFalse(scenarios.check({}, {"pointer": "/sha256", "absent": False})["pass"])

    def test_only_the_named_failure_with_the_expected_text_passes(self):
        expected = {"Import1": "not imported until the gap is resolved"}
        failure = {"name": "Import1", "status": "error: #178 not imported until the gap is resolved"}
        self.assertTrue(scenarios.rebuilds([failure], [], expected)["pass"])
        for features in ([dict(failure, status="ok")],
                         [dict(failure, status="error: another failure")],
                         [failure, {"name": "Other", "status": "error: broken"}], []):
            self.assertFalse(scenarios.rebuilds(features, [], expected)["pass"])
        self.assertFalse(scenarios.rebuilds([failure], [], {"Import1": ""})["pass"])


class SavedGeometry(unittest.TestCase):
    def test_recipe_hash_equality_and_missing_hash_fail_closed(self):
        with tempfile.TemporaryDirectory() as temp:
            folders = [pathlib.Path(temp) / name for name in ("before", "after")]
            for folder in folders:
                folder.mkdir()
            for hashes, expected in ((["a" * 64, "a" * 64], True),
                                     (["a" * 64, "b" * 64], False), ([None, None], False)):
                replies = [scenarios.subprocess.CompletedProcess([], 0,
                           json.dumps({"geometry_hash": value}), "") for value in hashes]
                with mock.patch.object(scenarios.subprocess, "run", side_effect=replies):
                    result = scenarios.same_geometry("soliddraft", folders[1], folders[0], 5)
                self.assertEqual(result["pass"], expected)

    def test_checked_step_fails_when_opencascade_or_validity_check_fails(self):
        evidence = {"summary": {"body": {"faces": 6, "edges": 12}},
                    "measurement": {"volume_mm3": 1000, "area_mm2": 600}}
        report = {"kind": "step", "solids": 1, "faces": 6, "within_tolerance": True}
        with tempfile.TemporaryDirectory() as temp:
            directory = pathlib.Path(temp)
            (directory / "part.step").write_bytes(b"STEP bytes")
            for error in ("ModuleNotFoundError: No module named 'OCP'", "FAIL: invalid solid"):
                replies = [scenarios.subprocess.CompletedProcess([], 0, json.dumps({"report": report}), ""),
                           scenarios.subprocess.CompletedProcess([], 1, "", error)]
                with mock.patch.object(scenarios.subprocess, "run", side_effect=replies) as run:
                    checks = scenarios.checked_step_export("soliddraft", "saved.dwprt", directory, evidence, 5)
                self.assertFalse(all(c["pass"] for c in checks))
                self.assertEqual(checks[-1]["check"], "check-step")
                self.assertEqual((directory / "check-step.stderr").read_text(), error)
                self.assertEqual(run.call_args_list[0].args[0][1:3], ["export", "saved.dwprt"])
                self.assertEqual(json.loads((directory / "step-report.json").read_text()), {
                    "report": report, "body": {"faces": 6, "edges": 12, "volume": 1000, "area": 600}})

    def test_checked_step_raw_check_fails_closed(self):
        evidence = {"summary": {"body": {"faces": 6, "edges": 12}},
                    "measurement": {"volume_mm3": 1000, "area_mm2": 600}}
        report = {"kind": "step", "solids": 1, "faces": 6, "within_tolerance": True}
        with tempfile.TemporaryDirectory() as temp:
            directory = pathlib.Path(temp)
            (directory / "part.step").write_bytes(b"STEP bytes")
            for code, error in ((0, ""), (1, "ModuleNotFoundError: No module named 'OCP'"),
                                (1, "raw_valid: false")):
                replies = [scenarios.subprocess.CompletedProcess([], 0, json.dumps({"report": report}), ""),
                           scenarios.subprocess.CompletedProcess([], 0, "standard reader valid", ""),
                           scenarios.subprocess.CompletedProcess([], code, "", error)]
                with mock.patch.object(scenarios.subprocess, "run", side_effect=replies) as run:
                    checks = scenarios.checked_step_export("soliddraft", "saved.dwprt", directory, evidence, 5)
                self.assertEqual(all(c["pass"] for c in checks), code == 0)
                self.assertEqual(checks[-1]["check"], "raw-step")
                self.assertEqual((directory / "raw-step.stderr").read_text(), error)
                command = run.call_args_list[-1].args[0]
                self.assertTrue(command[0].endswith("/.ci-ocp8/bin/python"))
                self.assertEqual(command[-2:], [scenarios.pinned_revision(), str(directory / "part.step")])
                self.assertIn('from raw_step import valid', command[2])
                self.assertIn('8.0.1.1.0', command[2])


    def test_checked_step_runs_on_the_scenarios_allowance(self):
        evidence = {"summary": {"body": {"faces": 6, "edges": 12}},
                    "measurement": {"volume_mm3": 1000, "area_mm2": 600}}
        report = {"kind": "step", "solids": 1, "faces": 6, "within_tolerance": True, "work": 5}
        for request, passed in (({"used": 7, "limit": 20, "exhausted": False}, True),
                                ({"used": 21, "limit": 20, "exhausted": False}, False),
                                ({"used": 7, "limit": 20, "exhausted": True}, False),
                                ({"used": 7, "limit": 19, "exhausted": False}, False), (None, False)):
            with tempfile.TemporaryDirectory() as temp:
                directory = pathlib.Path(temp)
                (directory / "part.step").write_bytes(b"STEP bytes")
                done = scenarios.subprocess.CompletedProcess([], 0, "", "")
                replies = [scenarios.subprocess.CompletedProcess(
                    [], 0, json.dumps({"report": report, "request_work": request}), ""), done, done]
                with mock.patch.object(scenarios.subprocess, "run", side_effect=replies) as run:
                    checks = scenarios.checked_step_export("soliddraft", "saved.dwprt", directory, evidence, 5, 20)
                self.assertEqual(all(c["pass"] for c in checks), passed)
                self.assertEqual(run.call_args_list[0].args[0][-2:], ["--max-work", "20"])

    def test_checked_step_oracle_runs_after_the_raw_check_and_fails_closed(self):
        evidence = {"summary": {"body": {"faces": 6, "edges": 12}},
                    "measurement": {"volume_mm3": 1000, "area_mm2": 600}}
        report = {"kind": "step", "solids": 1, "faces": 6, "within_tolerance": True}
        oracle = {"surfaces": {"plane": 6}, "volume": 1000, "inside": [[0, 0, 0]], "outside": [[9, 9, 9]]}
        for code in (0, 1):
            with tempfile.TemporaryDirectory() as temp:
                directory = pathlib.Path(temp)
                (directory / "part.step").write_bytes(b"STEP bytes")
                done = scenarios.subprocess.CompletedProcess([], 0, "", "")
                replies = [scenarios.subprocess.CompletedProcess([], 0, json.dumps({"report": report}), ""), done, done,
                           scenarios.subprocess.CompletedProcess([], code, "", "FAIL: raw: volume")]
                with mock.patch.object(scenarios.subprocess, "run", side_effect=replies) as run:
                    checks = scenarios.checked_step_export(
                        "soliddraft", "saved.dwprt", directory, evidence, 5, oracle=oracle)
                self.assertEqual(all(c["pass"] for c in checks), code == 0)
                self.assertEqual(checks[-1]["check"], "step-oracle")
                self.assertEqual(json.loads((directory / "step-oracle.json").read_text()), oracle)
                command = run.call_args_list[-1].args[0]
                self.assertTrue(command[0].endswith("/.ci-ocp8/bin/python"))
                self.assertTrue(command[1].endswith("/check-step-oracle.py"))


def step_oracle():
    spec = importlib.util.spec_from_file_location(
        "check_step_oracle", pathlib.Path(__file__).with_name("check-step-oracle.py"))
    oracle = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(oracle)
    return oracle


class CheckStepOracle(unittest.TestCase):
    SPEC = {"surfaces": {"plane": 6}, "volume": 1000.0, "fixed": 0.01, "inside": [[0, 0, 0]], "outside": [[9, 9, 9]]}

    def failures(self, *, kinds=None, values=(1000.002, 1000.0005, 1000.0005), valid=True, states=None, spec=None):
        oracle = step_oracle()
        states = states or {(0, 0, 0): "inside", (9, 9, 9): "outside"}
        names = {"OCP": mock.MagicMock(), "OCP.BRepCheck": mock.MagicMock()}
        names["OCP.BRepCheck"].BRepCheck_Analyzer.return_value.IsValid.return_value = valid
        with mock.patch.dict(sys.modules, names), \
                mock.patch.object(oracle, "surfaces", return_value=kinds or {"plane": 6}), \
                mock.patch.object(oracle, "volumes", return_value=list(values)), \
                mock.patch.object(oracle, "area", return_value=600.0), \
                mock.patch.object(oracle, "state", side_effect=lambda _, point: states[tuple(point)]), \
                contextlib.redirect_stdout(io.StringIO()):
            return oracle.failures("raw", object(), spec or self.SPEC)

    def test_the_expected_solid_passes(self):
        self.assertEqual(self.failures(), [])

    def test_an_invalid_solid_or_other_surfaces_fail(self):
        self.assertIn("invalid", self.failures(valid=False)[0])
        self.assertIn("surfaces", self.failures(kinds={"plane": 5, "spline": 1})[0])

    def test_the_allowance_is_the_fixed_part_plus_the_file_bound_and_the_spread(self):
        # 0.01 + 600 * 1e-7 = 0.01006, and the spread adds 0.0005.
        self.assertEqual(self.failures(values=(1000.0, 1000.0005, 1000.0005)), [])
        self.assertEqual(self.failures(values=(1000.01, 1000.0105, 1000.0105)), [])
        self.assertIn("volume", self.failures(values=(1000.0101, 1000.0101, 1000.0101))[0])
        self.assertEqual(self.failures(values=(1000.0, 1000.0, 1000.0), spec={**self.SPEC, "floor": 1.0}), [])

    def test_an_unconverged_volume_fails(self):
        self.assertIn("converge", self.failures(values=(1000.0, 1000.0, 1000.001))[0])

    def test_a_point_on_the_wrong_side_or_undecided_fails(self):
        for state in ("outside", "boundary"):
            found = self.failures(states={(0, 0, 0): state, (9, 9, 9): "outside"})
            self.assertEqual(len(found), 1)
            self.assertIn("expected inside", found[0])


class Latency(unittest.TestCase):
    @staticmethod
    def call(name, elapsed, operations=(), *, nested=False, refused=False, work=None, foreground=True):
        return {"operation": name, "wall_ms": elapsed, "nested": nested, "on_capture_thread": foreground,
                "status": "refused" if refused else "answered", "work": work,
                "operation_log": {"format": "catallax-operation-log/1", "operations": list(operations)}}

    @staticmethod
    def operation(name, elapsed, work=1):
        return {"operation": name, "wall_ms": elapsed, "work": {"used": work}}

    @staticmethod
    def segment(elapsed, calls=(), **extra):
        return {"source": "kernel.json", "elapsed_ms": elapsed, "kernel": {"calls": list(calls)}, **extra}

    def test_display_and_requested_measurement_are_in_the_click_budget(self):
        command = self.segment(400, [
            self.call("extrude", 300, [
                self.operation("extrude", 200, 20),
                self.operation("mesh", 50, 5),
            ], work={"used": 25}),
            self.call("mass_properties", 60, [self.operation("mass_properties", 55, 6)]),
        ])
        result = scenarios.timing("extrude", [command, self.segment(650, phase="display")])
        self.assertFalse(result["within_budget"])
        self.assertTrue(result["pass"], "a timing is a record, not a gate")
        self.assertEqual(result["elapsed_ms"], 1050)
        self.assertEqual(result["split_ms"], {
            "construction": 290, "edit": 0, "display": 700, "measurement": 60, "refusal": 0,
        })
        self.assertEqual(result["work"], {
            "construction": 20, "edit": 0, "display": 5, "measurement": 6, "refusal": 0,
        })

    def test_nested_drawing_queries_are_not_charged_twice(self):
        result = scenarios.timing("edit_extrude", [self.segment(200, [
            self.call("tessellate", 70, nested=True),
            self.call("view_curves", 100, [self.operation("mesh", 70, 5)], work={"used": 5}),
        ])])
        self.assertEqual(result["split_ms"]["display"], 100)
        self.assertEqual(result["split_ms"]["edit"], 100)
        self.assertEqual(sum(result["work"].values()), 5)

    def test_concurrent_background_mesh_keeps_external_wait_and_its_work(self):
        for tool in ("edit_extrude", "drive", "set_value"):
            with self.subTest(tool=tool):
                result = scenarios.timing(tool, [self.segment(1100, [
                    self.call("tessellate", 3000, [self.operation("mesh", 2900, 15)],
                              work={"used": 15}, foreground=False),
                    self.call("tessellate_edges", 500, nested=True, foreground=False),
                    self.call("extrude", 500, work={"used": 25}),
                ])])
                self.assertFalse(result["within_budget"])
                self.assertEqual(result["elapsed_ms"], 1100)
                self.assertEqual(result["split_ms"]["edit"], 1100)
                self.assertEqual(sum(result["split_ms"].values()), 1100)
                self.assertEqual(result["work"]["edit"], 25)
                self.assertEqual(result["background"]["call_ms"]["display"], 3000)
                self.assertEqual(result["background"]["work"]["display"], 15)
                self.assertEqual(scenarios.edit_work_check({"max_edit_work": 24}, result)["status"],
                                 "edit-work-regression")

    def test_refusal_keeps_its_whole_wait_and_work(self):
        result = scenarios.timing("cut", [self.segment(600, [
            self.call("boolean", 500, [self.operation("mesh", 50, 3)],
                      refused=True, work={"used": 7}),
        ], phase="refusal")])
        self.assertEqual(result["split_ms"]["refusal"], 600)
        self.assertEqual(result["work"]["refusal"], 7)
        self.assertEqual(result["split_ms"]["display"], 0)

    def test_drawing_reconstruction_is_not_hidden_as_rendering(self):
        result = scenarios.timing("rollback", [self.segment(400, [
            self.call("extrude", 200),
            self.call("view_curves", 100),
        ], phase="construction", overhead_phase="display")])
        self.assertEqual(result["split_ms"]["construction"], 200)
        self.assertEqual(result["split_ms"]["display"], 200)
        self.assertEqual(result["split_ms"]["edit"], 0)

    def test_exact_budget_boundary_and_one_microsecond_over(self):
        for tool, limit in (("box", 100), ("boolean", 500), ("edit_extrude", 1000), ("compare", 2000)):
            with self.subTest(tool=tool):
                self.assertTrue(scenarios.timing(tool, [self.segment(limit)])["within_budget"])
                over = scenarios.timing(tool, [self.segment(limit + 0.001)])
                self.assertFalse(over["within_budget"])
                self.assertTrue(over["pass"])

    def test_invalid_or_overlapping_evidence_fails_closed(self):
        for elapsed in (None, True, -1, float("nan"), float("inf")):
            with self.subTest(elapsed=elapsed), self.assertRaises(ValueError):
                scenarios.timing("extrude", [self.segment(elapsed)])
        with self.assertRaises(KeyError):
            scenarios.timing("extrude", [{"source": "missing", "elapsed_ms": 1}])
        with self.assertRaises(ValueError):
            scenarios.timing("extrude", [self.segment(10, [self.call("extrude", 11)])])
        with self.assertRaises(ValueError):
            scenarios.timing("extrude", [self.segment(10, [
                self.call("extrude", 5, [self.operation("mesh", 6)]),
            ])])
        call = self.call("extrude", 1)
        del call["on_capture_thread"]
        with self.assertRaises(KeyError):
            scenarios.timing("extrude", [self.segment(10, [call])])


class DrawingViewBudgets(unittest.TestCase):
    def test_view_has_its_own_budget_boundary(self):
        kernel = {"calls": []}
        self.assertTrue(scenarios.drawing_view_timing(scenarios.DRAWING_VIEW_BUDGET_MS, kernel)["within_budget"])
        over = scenarios.drawing_view_timing(scenarios.DRAWING_VIEW_BUDGET_MS + 0.001, kernel)
        self.assertFalse(over["within_budget"])
        self.assertTrue(over["pass"], "over budget is a warning")
        self.assertEqual(scenarios.verdict(over), "over-budget")
        self.assertEqual(over["class"], "drawing-view")
        self.assertEqual(scenarios.BUDGET_MS["feature"], 1000)

    def test_view_keeps_its_numeric_work_without_charging_nested_calls_twice(self):
        view = scenarios.drawing_view_timing(450, {"calls": [
            Latency.call("tessellate", 100, nested=True),
            Latency.call("view_curves", 400, [
                Latency.operation("mesh", 100, 5), Latency.operation("face_area", 30, 2),
            ], work={"used": 7}),
        ]})
        self.assertEqual(view["split_ms"]["display"], 420)
        self.assertEqual(view["split_ms"]["measurement"], 30)
        self.assertEqual(view["work"]["display"], 5)
        self.assertEqual(view["work"]["measurement"], 2)

    def test_each_view_an_edit_regenerates_keeps_the_ceiling(self):
        limit = scenarios.DRAWING_VIEW_BUDGET_MS
        calls = [Latency.call("extrude", 300), Latency.call("view_curves", limit),
                 Latency.call("section_view", 200), Latency.call("view_curves", 50, nested=True),
                 Latency.call("view_curves", 5000, foreground=False)]
        result = scenarios.drawing_view_calls({"calls": calls})
        self.assertTrue(result["within_budget"])
        self.assertEqual([v["operation"] for v in result["views"]], ["view_curves", "section_view"])
        self.assertEqual(result["slowest_ms"], limit)
        over = scenarios.drawing_view_calls({"calls": [Latency.call("section_view", limit + 0.001)]})
        self.assertFalse(over["within_budget"])
        self.assertTrue(over["pass"], "over the ceiling is a warning")
        self.assertEqual(over["class"], "drawing-view")
        recorded = scenarios.drawing_view_calls({"calls": [Latency.call("section_view", limit + 0.001)]}, gated=False)
        self.assertIsNone(recorded["within_budget"], "an output's certified views have no ceiling")
        self.assertIsNone(recorded["budget_ms"])
        self.assertEqual(recorded["slowest_ms"], limit + 0.001)
        self.assertTrue(scenarios.drawing_view_calls({"calls": []})["within_budget"], "no view regenerated")
        with self.assertRaises(ValueError):
            scenarios.drawing_view_calls({"calls": [Latency.call("view_curves", None)]})
        call = Latency.call("view_curves", 1)
        del call["on_capture_thread"]
        with self.assertRaises(KeyError):
            scenarios.drawing_view_calls({"calls": [call]})


class Outputs(unittest.TestCase):
    def test_a_drawing_export_records_its_wait_without_a_click_budget(self):
        for tool in ("drawing_export_svg", "drawing_export_pdf", "drawing_export_dxf"):
            with self.subTest(tool=tool):
                self.assertEqual(scenarios.budget(tool), "output")
                latency = scenarios.timing(tool, [Latency.segment(12_345)])
                self.assertTrue(latency["pass"])
                self.assertIsNone(latency["budget_ms"])
                self.assertEqual(latency["elapsed_ms"], 12_345)
                verdict = scenarios.accept_latency("drawing", 3, tool, latency, {})
                self.assertEqual((verdict["pass"], verdict["within_budget"], verdict["status"]), (True, None, "output"))
                self.assertEqual(verdict["verdict"], "output", "neither a strict pass nor a known gap")
        self.assertEqual(scenarios.budget("drawing_save"), "feature", "a save keeps the feature click budget")
        self.assertEqual(scenarios.budget("make_drawing"), "feature")

    def test_an_output_cannot_be_listed_as_a_known_gap(self):
        latency = scenarios.timing("drawing_export_svg", [Latency.segment(10)])
        known = {("drawing", 3): {"scenario": "drawing", "step": 3, "tool": "drawing_export_svg",
                                  "last_measured_ms": 10}}
        verdict = scenarios.accept_latency("drawing", 3, "drawing_export_svg", latency, known)
        self.assertTrue(verdict["pass"], "a stale listing is a warning")
        self.assertEqual(verdict["status"], "invalid-known-over-budget")
        self.assertIn("remove its listing", verdict["message"])
        self.assertEqual(verdict["verdict"], "output")

    def test_output_files_must_exist_and_hold_bytes(self):
        import tempfile
        with tempfile.TemporaryDirectory() as folder:
            full = pathlib.Path(folder, "sheet.svg")
            full.write_text("<svg/>")
            empty = pathlib.Path(folder, "empty.dxf")
            empty.write_text("")
            missing = str(pathlib.Path(folder, "missing.pdf"))
            checks = scenarios.output_files([str(full), str(empty), missing], absent=[missing, str(empty)])
            self.assertEqual([c["pass"] for c in checks], [True, False, False, True, False])
            self.assertEqual(checks[0]["bytes"], 6)
            self.assertIsNone(checks[2]["bytes"])


class KnownBudgets(unittest.TestCase):
    known = {("bracket", 6): {"scenario": "bracket", "step": 6, "tool": "extrude",
                            "last_measured_ms": 2000}}

    def verdict(self, elapsed, scenario="bracket", number=6, tool="extrude"):
        latency = scenarios.timing(tool, [Latency.segment(elapsed)])
        return scenarios.accept_latency(scenario, number, tool, latency, self.known)

    def test_a_listing_names_its_exact_step_and_tool(self):
        listed = self.verdict(2240)
        self.assertTrue(listed["pass"])
        self.assertFalse(listed["within_budget"])
        self.assertEqual(listed["status"], "known-over-budget")
        self.assertEqual(self.verdict(2240, scenario="another-bracket")["status"], "over-budget")
        self.assertEqual(self.verdict(2240, number=7)["status"], "over-budget")
        changed = self.verdict(2240, tool="cut")
        self.assertEqual(changed["status"], "invalid-known-over-budget")
        self.assertIn("tool differs", changed["message"])

    def test_listed_step_warns_above_20_percent_regression(self):
        boundary = self.verdict(2400)
        self.assertEqual(boundary["status"], "known-over-budget")
        self.assertEqual(boundary["budget_ms"], 1000)
        over = self.verdict(2400.001)
        self.assertTrue(over["pass"], "a regression is a warning")
        self.assertFalse(over["within_budget"])
        self.assertEqual(over["status"], "known-gap-regression")
        self.assertIn("regressed to 2400.001 ms", over["message"])
        self.assertEqual(over["budget_ms"], 1000)

    def test_listed_step_can_improve_without_changing_the_budget(self):
        for elapsed in (999.999, 1000):
            with self.subTest(elapsed=elapsed):
                improved = self.verdict(elapsed)
                self.assertTrue(improved["pass"])
                self.assertTrue(improved["within_budget"])
                self.assertEqual(improved["status"], "pass")
                self.assertNotIn("message", improved)

    def test_unlisted_step_over_budget_warns_and_passes(self):
        self.assertEqual(self.verdict(1000, number=7)["status"], "pass")
        over = self.verdict(1000.001, number=7)
        self.assertTrue(over["pass"])
        self.assertFalse(over["within_budget"])
        self.assertEqual(over["status"], "over-budget")
        self.assertIn("budget 1000 ms", over["message"])

    def test_malformed_known_over_budget_file_fails(self):
        entry = {"scenario": "bracket", "step": 6, "tool": "extrude", "last_measured_ms": 2000}
        for label, policy in (("wrong format", {"format": "other/1", "steps": [entry]}),
                              ("duplicate step", {"format": "soliddraft-known-over-budget/1",
                                                  "steps": [entry, dict(entry, tool="cut")]}),
                              ("bad time", {"format": "soliddraft-known-over-budget/1",
                                            "steps": [dict(entry, last_measured_ms=-1)]})):
            with self.subTest(label), tempfile.TemporaryDirectory() as folder:
                path = pathlib.Path(folder, "known-over-budget.json")
                path.write_text(json.dumps(policy))
                with self.assertRaises(ValueError):
                    scenarios.load_known_over_budget(path)

    def test_the_checked_in_file_loads(self):
        self.assertTrue(scenarios.load_known_over_budget(scenarios.KNOWN_OVER_BUDGET))


class Replies(unittest.TestCase):
    command = {"request": 3, "revision": 7}
    good = {"request": 3, "revision": 7, "selectable": True}

    def test_a_reply_for_this_request_and_revision_answers(self):
        self.assertIsNone(scenarios.answers(3, self.command, self.good, True))

    def test_early_stale_pending_and_unlabelled_replies_never_answer(self):
        cases = {
            "not selectable": dict(self.good, selectable=False),
            "selectable not a bool": dict(self.good, selectable=1),
            "no selection count": {"request": 3, "revision": 7},
            "older revision": dict(self.good, revision=6),
            "older request": dict(self.good, request=2),
            "pending": dict(self.good, pending=True),
            "no request": {"revision": 7, "selectable": True},
            "no revision": {"request": 3, "selectable": True},
            "boolean revision": dict(self.good, revision=True),
        }
        for label, reply in cases.items():
            with self.subTest(label):
                self.assertIsNotNone(scenarios.answers(3, self.command, reply, True))
        self.assertIsNotNone(scenarios.answers(3, self.command, None))
        self.assertIsNotNone(scenarios.answers(3, {"request": 3}, self.good), "the command has no revision")

    def test_a_proof_needs_no_selection_but_still_the_right_revision(self):
        self.assertIsNone(scenarios.answers(3, self.command, {"request": 3, "revision": 7}))
        self.assertIn("stale", scenarios.answers(3, self.command, {"request": 3, "revision": 6}))


class Pinning(unittest.TestCase):
    sha = "a" * 40

    def test_pin_is_the_adapters_full_commit_sha(self):
        self.assertRegex(scenarios.pinned_revision(), r"^[0-9a-f]{40}$")

    def test_only_the_pinned_revision_passes(self):
        good = {"backend": {"name": "catallax", "version": self.sha}}
        self.assertTrue(scenarios.backend_pinned(good, self.sha)["pass"])
        for bad in ({"backend": {"name": "catallax", "version": "b" * 40}},
                    {"backend": {"name": "catallax"}},
                    {"backend": {"name": "other", "version": self.sha}},
                    {"backend": None}, {}):
            with self.subTest(bad=bad):
                self.assertFalse(scenarios.backend_pinned(bad, self.sha)["pass"])


class Verdicts(unittest.TestCase):
    known = KnownBudgets.known

    def test_over_budget_is_never_a_strict_pass(self):
        latency = scenarios.timing("extrude", [Latency.segment(2240)])
        for number in (6, 7):
            accepted = scenarios.accept_latency("bracket", number, "extrude", latency, self.known)
            self.assertTrue(accepted["pass"])
            self.assertEqual(accepted["verdict"], "over-budget")
        within = scenarios.timing("extrude", [Latency.segment(1000)])
        self.assertEqual(scenarios.accept_latency("bracket", 7, "extrude", within, self.known)["verdict"], "strict-pass")
        self.assertEqual(scenarios.accept_latency("bracket", 6, "extrude", within, self.known)["verdict"], "strict-pass")
        regressed = scenarios.timing("extrude", [Latency.segment(2400.001)])
        self.assertEqual(scenarios.accept_latency("bracket", 6, "extrude", regressed, self.known)["verdict"], "over-budget")

    def test_measurement_of_4245_ms_misses_both_its_budget_and_the_2000_ms_gate(self):
        slow = scenarios.measurement_timing(4245, {"calls": []})
        self.assertTrue(slow["pass"])
        self.assertFalse(slow["within_budget"])
        self.assertFalse(slow["within_gate"])
        self.assertEqual((slow["budget_ms"], slow["gate_ms"]), (1000, 2000))
        self.assertEqual(scenarios.verdict(slow), "over-budget")

    def test_measurement_budget_and_gate_boundaries(self):
        self.assertTrue(scenarios.measurement_timing(1000, {"calls": []})["within_budget"])
        between = scenarios.measurement_timing(1500, {"calls": []})
        self.assertFalse(between["within_budget"], "the unchanged 1,000 ms budget still applies")
        self.assertTrue(between["within_gate"])
        self.assertTrue(scenarios.measurement_timing(2000, {"calls": []})["within_gate"])
        self.assertFalse(scenarios.measurement_timing(2000.001, {"calls": []})["within_gate"])


FAKE = pathlib.Path(__file__).with_name("fake_mcp_server.py")


class RunnerEndToEnd(unittest.TestCase):
    """The real run() against a synthetic MCP server, so the click ends where a user sees it."""

    def run_step(self, mode, measure=False, timeout=scenarios.HANG_TIMEOUT_S, strict=False, known=None, extra=None,
                 known_file=None):
        step = {"tool": "extrude", "arguments": {}}
        step.update(extra or {})
        if measure:
            step["checks"] = [{"pointer": "/measurement/volume_mm3", "approx": 1.0}]
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            scenario = root / "scenario.json"
            scenario.write_text(json.dumps({"format": "soliddraft-scenario/1", "name": "synthetic-gate",
                                            "steps": [step]}))
            args = argparse.Namespace(scenario=scenario, out=root / "run", soliddraft=str(FAKE),
                                      catallax_verify="unused", timeout=timeout, strict=strict)
            os.environ["FAKE_MODE"] = mode
            os.environ["FAKE_PIN"] = scenarios.pinned_revision()
            if known_file is None:
                listings = mock.patch.object(scenarios, "load_known_over_budget", return_value=known or {})
            else:
                (root / "known-over-budget.json").write_text(json.dumps(known_file))
                listings = mock.patch.object(scenarios, "KNOWN_OVER_BUDGET", root / "known-over-budget.json")
            output = io.StringIO()
            try:
                with contextlib.redirect_stdout(output), listings:
                    code = scenarios.run(args)
            finally:
                del os.environ["FAKE_MODE"], os.environ["FAKE_PIN"]
            report = json.loads((args.out / "result.json").read_text())
        self.output = output.getvalue()
        checks = {c["check"]: c for c in (report["steps"][0]["checks"] if report["steps"] else [])
                  if isinstance(c["check"], str)}
        self.code = code
        return report, checks

    def run_fresh(self, expected):
        helper = ["check-configuration-grid.py", "grid", "{bundle}/fixtures/g1.csv", expected]
        steps = [{"tool": "extrude", "arguments": {}},
                 {"tool": "extrude", "arguments": {}, "fresh_process": True, "helpers": [helper]}]
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            scenario = root / "scenario.json"
            scenario.write_text(json.dumps({
                "format": "soliddraft-scenario/1", "name": "synthetic-fresh", "steps": steps,
                "setup": [["check-configuration-grid.py", "fixtures", "{bundle}/fixtures"]]}))
            args = argparse.Namespace(scenario=scenario, out=root / "run", soliddraft=str(FAKE),
                                      catallax_verify="unused", timeout=scenarios.HANG_TIMEOUT_S, strict=False)
            os.environ["FAKE_MODE"] = "ok"
            os.environ["FAKE_PIN"] = scenarios.pinned_revision()
            try:
                with contextlib.redirect_stdout(io.StringIO()), mock.patch.object(
                        scenarios, "load_known_over_budget", return_value={}):
                    scenarios.run(args)
            finally:
                del os.environ["FAKE_MODE"], os.environ["FAKE_PIN"]
            report = json.loads((args.out / "result.json").read_text())
            return report, (args.out / "commands-1/0001/command.json").exists()

    def test_a_fresh_process_has_its_own_evidence_and_a_helper_can_fail_the_step(self):
        report, separate = self.run_fresh("g1")
        self.assertTrue(report["pass"], report)
        self.assertTrue(separate)
        report, _ = self.run_fresh("g0")
        self.assertFalse(report["pass"])

    def test_a_fresh_process_step_runs_after_the_old_process_exits(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            log = root / "log"
            scenario = root / "scenario.json"
            scenario.write_text(json.dumps({"format": "soliddraft-scenario/1", "name": "synthetic-gate", "steps": [
                {"tool": "save", "arguments": {}},
                {"tool": "open", "arguments": {}, "fresh_process": True},
                {"tool": "feature", "arguments": {}, "same_geometry_as_step": 1}]}))
            args = argparse.Namespace(scenario=scenario, out=root / "run", soliddraft=str(FAKE),
                                      catallax_verify="unused", timeout=30, strict=False)
            info = {"pass": True, "geometry_hash": "same"}
            with mock.patch.dict(os.environ, {"FAKE_MODE": "ok", "FAKE_PIN": scenarios.pinned_revision(),
                                              "FAKE_LOG": str(log)}), \
                    mock.patch.object(scenarios, "load_known_over_budget", return_value={}), \
                    mock.patch.object(scenarios, "same_geometry", return_value=info) as same, \
                    contextlib.redirect_stdout(io.StringIO()):
                code = scenarios.run(args)
            report = json.loads((args.out / "result.json").read_text())
            events = [line.split() for line in log.read_text().splitlines()]
            self.assertEqual(code, 0, report)
            self.assertEqual([event for _, event in events], ["save", "exit", "open", "feature", "exit"])
            self.assertEqual(events[0][0], events[1][0])
            self.assertEqual(events[2][0], events[4][0])
            self.assertNotEqual(events[0][0], events[2][0])
            self.assertTrue((args.out / "commands/0001/command.json").exists())
            self.assertTrue((args.out / "commands-1/0001/command.json").exists())
            self.assertEqual(same.call_args.args[1:3], (args.out / "commands-1/0002", args.out / "commands/0001"))
            methods = [entry["request"]["method"] for entry in json.loads((args.out / "protocol.json").read_text())]
            self.assertEqual(methods.count("initialize"), 2)

    def test_only_the_declared_field_view_refusal_is_accepted(self):
        for mode, declared, expected in [("field-view", True, True), ("field-view", False, False),
                                         ("wrong-view-error", True, False), ("ok", True, False)]:
            with self.subTest(mode=mode, declared=declared):
                report, _ = self.run_step(mode, extra={"view": "unavailable"} if declared else {})
                self.assertEqual(report["pass"], expected)

    def test_export_report_checks_and_refusals_run_through_the_cli(self):
        exports = [{"format": "stl", "checks": [{"pointer": "/report/deviation_bound_mm", "maximum": 0.1}]}]
        report, _ = self.run_step("ok", extra={"exports": exports})
        self.assertTrue(report["pass"])
        exports[0]["checks"][0]["maximum"] = 0.01
        report, _ = self.run_step("ok", extra={"exports": exports})
        self.assertFalse(report["pass"])
        for mode, expected in [("export-refused", True), ("export-refused-written", False),
                               ("export-refused-wrong", False), ("ok", False)]:
            report, _ = self.run_step(mode, extra={"exports": [{"format": "step", "refused": "field parts export as meshes"}]})
            self.assertEqual(report["pass"], expected, mode)

    def test_a_visible_selectable_edit_ends_the_click(self):
        report, checks = self.run_step("ok")
        self.assertTrue(checks["click ends at a visible selectable edit"]["pass"])
        interval = report["steps"][0]["intervals"]["preview"]
        self.assertEqual(interval["ends"], "visible selectable edit")
        self.assertEqual(interval["verdict"], "strict-pass")

    def test_edit_work_regression_fails_the_runner_even_with_a_fast_click(self):
        report, checks = self.run_step("ok", extra={"max_edit_work": 0})
        self.assertTrue(report["pass"])
        self.assertTrue(checks["edit work ceiling"]["pass"])
        original = scenarios.timing

        def one_more(*args, **kwargs):
            result = original(*args, **kwargs)
            result["work"]["edit"] = 1
            return result

        with mock.patch.object(scenarios, "timing", side_effect=one_more):
            report, checks = self.run_step("ok", extra={"max_edit_work": 0})
        self.assertTrue(checks["click budget"]["pass"])
        self.assertEqual(checks["edit work ceiling"]["status"], "edit-work-regression")
        self.assertFalse(report["pass"])
        self.assertEqual(self.code, 1)

    def test_a_display_without_its_evidence_fails_the_step(self):
        report, checks = self.run_step("lost-display")
        self.assertFalse(checks["display evidence written"]["pass"])
        self.assertIn("display.json", checks["display evidence written"]["reason"])
        self.assertNotEqual(report["steps"][0]["intervals"]["preview"]["ends"], "command reply, no solid to show")
        self.assertFalse(report["steps"][0]["pass"])
        self.assertFalse(report["pass"])

    def test_a_display_that_reports_no_solid_ends_the_click_at_the_reply(self):
        report, checks = self.run_step("no-solid", strict=True)
        self.assertNotIn("click ends at a visible selectable edit", checks)
        self.assertNotIn("live hidden-line view", checks)
        self.assertEqual(report["steps"][0]["intervals"]["preview"]["ends"], "command reply, no solid to show")
        self.assertTrue(report["pass"])
        self.assertEqual(self.code, 0)

    def test_a_stale_or_failed_no_solid_reply_cannot_skip_the_display(self):
        for mode, reason in (("stale-no-solid", "stale"), ("failed-no-solid", "failed")):
            with self.subTest(mode=mode):
                report, checks = self.run_step(mode)
                self.assertFalse(checks["no-solid reply answers this request"]["pass"])
                self.assertIn(reason, checks["no-solid reply answers this request"]["reason"])
                self.assertNotEqual(report["steps"][0]["intervals"]["preview"]["ends"],
                                    "command reply, no solid to show")
                self.assertFalse(report["pass"])

    def test_early_command_reply_cannot_pass(self):
        report, checks = self.run_step("early")
        self.assertFalse(checks["click ends at a visible selectable edit"]["pass"])
        self.assertFalse(report["steps"][0]["pass"])
        self.assertFalse(report["pass"])

    def test_stale_display_cannot_end_a_newer_click(self):
        report, checks = self.run_step("stale")
        self.assertIn("stale", checks["click ends at a visible selectable edit"]["reason"])
        self.assertFalse(report["pass"])

    def test_pending_display_stays_incomplete(self):
        report, checks = self.run_step("pending")
        self.assertIn("pending", checks["click ends at a visible selectable edit"]["reason"])
        self.assertFalse(report["pass"])

    def test_timeout_leaves_the_step_incomplete(self):
        report, _ = self.run_step("silent", timeout=1)
        self.assertFalse(report["pass"])
        self.assertIn("timed out", report["steps"][0]["incomplete"]["reason"])
        self.assertEqual(report["steps"][0]["incomplete"]["request"], "scenario_display")

    def test_stale_measurement_cannot_satisfy_the_request(self):
        report, checks = self.run_step("stale-measure", measure=True)
        self.assertFalse(checks["measurement answers this request"]["pass"])
        self.assertFalse(report["pass"])

    def test_slow_measurement_of_over_four_seconds_warns_and_passes(self):
        report, checks = self.run_step("slow-measure", measure=True)
        budget = checks["requested measurement budget"]
        self.assertGreater(budget["elapsed_ms"], 4245)
        self.assertFalse(budget["within_budget"])
        self.assertFalse(budget["within_gate"])
        self.assertTrue(report["pass"])
        self.assertEqual(self.code, 0)
        self.assertFalse(report["strict_pass"])
        self.assertIn("::warning::synthetic-gate step 1: requested measurement took", self.output)

    gap = {("synthetic-gate", 1): {"scenario": "synthetic-gate", "step": 1, "tool": "extrude",
                                   "last_measured_ms": 2000}}

    def test_known_slow_click_passes_with_a_warning_but_not_a_strict_pass(self):
        report, _ = self.run_step("slow-click", known=self.gap)
        self.assertTrue(report["pass"])
        self.assertEqual(self.code, 0)
        self.assertFalse(report["strict_pass"])
        self.assertEqual(report["steps"][0]["intervals"]["preview"]["verdict"], "over-budget")
        self.assertEqual(report["steps"][0]["timing"]["status"], "known-over-budget")
        self.assertEqual(len(report["over_budget"]), 1)
        self.assertIn("::warning::Known gap: synthetic-gate step 1 (extrude)", self.output)
        self.assertNotIn("::error::", self.output)

    def test_strict_release_path_rejects_the_known_gap(self):
        report, _ = self.run_step("slow-click", known=self.gap, strict=True)
        self.assertTrue(report["pass"])
        self.assertEqual(self.code, 1)
        self.assertIn("::error::Release path: Known gap: synthetic-gate step 1", self.output)

    def test_unlisted_slow_click_warns_and_passes_but_fails_the_strict_path(self):
        for strict, code in ((False, 0), (True, 1)):
            with self.subTest(strict=strict):
                report, checks = self.run_step("slow-click", strict=strict)
                self.assertTrue(report["pass"])
                self.assertEqual(self.code, code)
                self.assertEqual(checks["click budget"]["status"], "over-budget")
                self.assertIn("::warning::synthetic-gate step 1 (extrude) took", self.output)
                timing = report["steps"][0]["timing"]
                self.assertGreater(timing["elapsed_ms"], 1000, "the step's timing is in the result")

    def test_regressed_listed_step_warns_and_passes(self):
        recorded = {("synthetic-gate", 1): dict(self.gap[("synthetic-gate", 1)], last_measured_ms=1001)}
        report, checks = self.run_step("slow-click", known=recorded)
        self.assertTrue(report["pass"])
        self.assertEqual(self.code, 0)
        self.assertEqual(checks["click budget"]["status"], "known-gap-regression")
        self.assertIn("::warning::synthetic-gate step 1 (extrude) regressed to", self.output)
        self.assertNotIn("::error::", self.output)

    def test_a_stale_listing_warns_and_passes(self):
        for label, listing in (("missing step", {("synthetic-gate", 5): dict(self.gap[("synthetic-gate", 1)], step=5)}),
                               ("changed tool", {("synthetic-gate", 1): dict(self.gap[("synthetic-gate", 1)], tool="cut")})):
            with self.subTest(label):
                report, _ = self.run_step("ok", known=listing)
                self.assertTrue(report["pass"])
                self.assertEqual(self.code, 0)
                self.assertEqual(len(report["warnings"]), 1)
                self.assertEqual(report["over_budget"], [], "a stale listing is not a missed budget")
                self.assertTrue(report["strict_pass"])
                self.assertIn("::warning::", self.output)

    def test_a_malformed_known_over_budget_file_fails_the_run(self):
        entry = {"scenario": "synthetic-gate", "step": 1, "tool": "extrude", "last_measured_ms": 2000}
        for label, policy in (("wrong format", {"format": "other/1", "steps": [entry]}),
                              ("duplicate step", {"format": "soliddraft-known-over-budget/1", "steps": [entry, entry]})):
            with self.subTest(label):
                report, _ = self.run_step("ok", known_file=policy)
                self.assertFalse(report["pass"])
                self.assertEqual(self.code, 1)
                self.assertTrue(report["error"])

    def test_summary_lists_over_budget_steps_and_listed_steps_against_their_record(self):
        recorded = {("synthetic-gate", 1): dict(self.gap[("synthetic-gate", 1)], last_measured_ms=1001)}
        regressed, _ = self.run_step("slow-click", known=recorded)
        unlisted, _ = self.run_step("slow-click")
        clean, _ = self.run_step("ok")
        text = scenarios.summary([regressed, unlisted, clean], "test machine")
        over = text.split("### Over budget")[1].split("###")[0]
        self.assertEqual(over.count("| synthetic-gate | 1 | extrude | click budget |"), 2)
        listed = text.split("### Listed in ci/known-over-budget.json")[1]
        self.assertEqual(listed.count("| synthetic-gate | 1 | extrude |"), 1)
        self.assertIn("| 1001.000 |", listed)
        self.assertIn("| known-gap-regression |", listed)
        self.assertIn("test machine", text)
        self.assertIn("None.", scenarios.summary([clean], "test machine"))

    def test_writing_the_command_evidence_is_not_part_of_the_click(self):
        report, checks = self.run_step("slow-evidence", strict=True)
        self.assertLess(report["steps"][0]["intervals"]["preview"]["elapsed_ms"], 1000)
        self.assertTrue(checks["click ends at a visible selectable edit"]["pass"])
        self.assertTrue(report["strict_pass"])
        self.assertEqual(self.code, 0)

    def test_strict_release_path_accepts_a_run_that_met_every_budget(self):
        report, _ = self.run_step("ok", strict=True)
        self.assertTrue(report["strict_pass"])
        self.assertEqual(self.code, 0)

    def test_early_reply_fails_the_strict_path_too(self):
        _, _ = self.run_step("early", strict=True)
        self.assertEqual(self.code, 1)

    def test_evidence_from_another_catallax_revision_cannot_pass(self):
        report, checks = self.run_step("other-revision")
        self.assertFalse(checks["Catallax backend is the pinned revision"]["pass"])
        self.assertFalse(report["pass"])

    def test_result_names_the_app_revision(self):
        report, _ = self.run_step("ok")
        import subprocess
        expected = subprocess.check_output(["git", "-C", str(FAKE.parent.parent), "rev-parse", "HEAD"], text=True).strip()
        self.assertEqual(report["soliddraft_revision"], expected)

    def test_result_names_the_pinned_revision(self):
        report, checks = self.run_step("ok")
        self.assertEqual(report["catallax_revision"], scenarios.pinned_revision())
        self.assertTrue(checks["Catallax backend is the pinned revision"]["pass"])

    def test_current_measurement_passes_with_its_interval_recorded(self):
        report, checks = self.run_step("ok", measure=True)
        self.assertTrue(checks["measurement answers this request"]["pass"])
        self.assertEqual(report["steps"][0]["intervals"]["measurement"]["gate_ms"], 2000)


class FieldExportChecks(unittest.TestCase):
    def test_same_export_requires_a_known_matching_digest(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = pathlib.Path(tmp)
            args = argparse.Namespace(soliddraft=str(FAKE), timeout=scenarios.HANG_TIMEOUT_S)
            spec = {"format": "stl", "same_export_as": "density edit"}
            first = scenarios.export_check(args, directory, {"format": "stl"}, {})
            for digest, expected in [(first["sha256"], True), ("wrong", False), (None, False)]:
                result = scenarios.export_check(args, directory, spec, {("density edit", "stl"): digest})
                self.assertEqual(result["pass"], expected)

    def test_oracle_failure_and_report_mismatch_fail_the_export(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = pathlib.Path(tmp)
            args = argparse.Namespace(soliddraft=str(FAKE), timeout=scenarios.HANG_TIMEOUT_S)
            real_cli = scenarios.cli
            def fake_oracle(binary, arguments, output, label, timeout):
                if label.endswith("-oracle"):
                    data = bytearray((directory / "part.stl").read_bytes())
                    struct.pack_into("<d", data, 68, self.bound)
                    (directory / "part.stl").write_bytes(data)
                    (directory / f"{label}.stdout").write_text(json.dumps({"bound_mm": self.bound}))
                    return {"check": label, "pass": self.oracle_pass}
                return real_cli(binary, arguments, output, label, timeout)
            spec = {"format": "stl", "oracle": {"box": [0, 0, 0, 1, 1, 1], "cell": 5,
                    "radius": 1, "accuracy": "1/10", "euler": 2}}
            for self.oracle_pass, self.bound, expected in [(True, 0.1, True), (False, 0.1, False), (True, 0.2, False)]:
                with mock.patch.object(scenarios, "cli", side_effect=fake_oracle):
                    self.assertEqual(scenarios.export_check(args, directory, spec, {})["pass"], expected)

    def test_oracle_receives_the_caps_the_header_must_record(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = pathlib.Path(tmp)
            args = argparse.Namespace(soliddraft=str(FAKE), timeout=scenarios.HANG_TIMEOUT_S)
            real_cli, seen = scenarios.cli, []
            def record(binary, arguments, output, label, timeout):
                if label.endswith("-oracle"):
                    seen.append([str(a) for a in arguments])
                    return {"check": label, "pass": True}
                return real_cli(binary, arguments, output, label, timeout)
            oracle = {"box": [0, 0, 0, 1, 1, 1], "cell": 5, "radius": 1, "accuracy": "1/10", "euler": 2}
            for spec_oracle, expected in [(oracle, None), ({**oracle, "caps": [2000000, 1000000]}, ["2000000", "1000000"])]:
                seen.clear()
                with mock.patch.object(scenarios, "cli", side_effect=record):
                    scenarios.export_check(args, directory, {"format": "stl", "oracle": spec_oracle}, {})
                command = seen[0]
                actual = command[command.index("--caps") + 1:command.index("--caps") + 3] if "--caps" in command else None
                self.assertEqual(actual, expected)

    def test_length_and_maximum_fail_closed(self):
        self.assertTrue(scenarios.check([1, 2], {"pointer": "", "length": 2})["pass"])
        self.assertFalse(scenarios.check([1], {"pointer": "", "length": 2})["pass"])
        self.assertFalse(scenarios.check("ab", {"pointer": "", "length": 2})["pass"])
        self.assertFalse(scenarios.check(None, {"pointer": "", "maximum": 0.1})["pass"])


class CrossStepChecks(unittest.TestCase):
    def test_same_as_step_requires_an_equal_existing_prior_value(self):
        spec = {"pointer": "/revision", "same_as_step": 25}
        self.assertTrue(scenarios.check({"revision": 7}, spec, {25: {"revision": 7}})["pass"])
        self.assertFalse(scenarios.check({"revision": 8}, spec, {25: {"revision": 7}})["pass"])
        self.assertFalse(scenarios.check({"revision": 7}, spec, {})["pass"])
        self.assertFalse(scenarios.check({"revision": 7}, {**spec, "same_as_step": 26}, {25: {"revision": 7}})["pass"])


class InspectionChecks(unittest.TestCase):
    def test_comparisons_use_this_step_or_a_prior_step_and_fail_closed(self):
        for kind in ("same_as", "differs_from"):
            for earlier in (False, True):
                for other in (7, 8):
                    target = {"pointer": "/other", **({"step": 3} if earlier else {})}
                    rule = {"pointer": "/value", kind: target}
                    result = scenarios.check({"value": 7, "other": other}, rule, {3: {"other": other}})
                    self.assertEqual(result["pass"], (other == 7) == (kind == "same_as"))
                for current, previous in (({"value": 7}, {}), ({}, {3: {"other": 7}})):
                    self.assertFalse(scenarios.check(current, rule, previous)["pass"])

    def test_inspect_parses_json_and_fails_on_a_nonzero_exit_or_bad_json(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = pathlib.Path(temp)
            with mock.patch.object(scenarios.subprocess, "run", return_value=
                                   scenarios.subprocess.CompletedProcess([], 0, '{"revision": "R0"}', '')) as run:
                result = scenarios.inspect("soliddraft", {"part": ["doc", "info", "{step}/part.dwprt", "{bundle}"]},
                                           directory, directory, 5)
                self.assertEqual(result, {"part": {"revision": "R0"}})
                self.assertEqual(run.call_args.args[0], ["soliddraft", "doc", "info", str(directory / "part.dwprt"), str(directory)])
            for code, stdout, error in ((1, '{}', RuntimeError), (0, 'not JSON', ValueError)):
                with mock.patch.object(scenarios.subprocess, "run", return_value=
                                       scenarios.subprocess.CompletedProcess([], code, stdout, 'failure')):
                    with self.assertRaises(error):
                        scenarios.inspect("soliddraft", {"part": []}, directory, directory, 5)

    def test_folders_exist_before_steps_and_cannot_escape_the_bundle(self):
        with tempfile.TemporaryDirectory() as temp:
            bundle = pathlib.Path(temp)
            scenarios.scenario_folders(bundle, ["work", "work/nested"])
            self.assertTrue((bundle / "work/nested").is_dir())
            with self.assertRaises(ValueError):
                scenarios.scenario_folders(bundle, ["../escape"])


class Trends(unittest.TestCase):
    @staticmethod
    def samples(times):
        return [{"number": 3 + 4 * i, "tool": "sketch", "feature_count": 3 + 2 * i,
                 "timing": {"elapsed_ms": elapsed}} for i, elapsed in enumerate(times)]

    def verdict(self, rows):
        return scenarios.trend_checks([{"tool": "sketch", "from_step": 3}], rows)[0]

    def test_growth_warns_even_when_every_step_meets_its_click_budget(self):
        rows = self.samples([100 + 2 * i for i in range(30)])
        self.assertTrue(all(scenarios.timing("sketch", [Latency.segment(row["timing"]["elapsed_ms"])])["within_budget"]
                            for row in rows))
        result = self.verdict(rows)
        self.assertFalse(result["within_budget"])
        self.assertTrue(result["pass"], "growth is a warning")
        self.assertAlmostEqual(result["growth_ms"], 58)

    def test_flat_series_tolerates_jitter_and_a_single_stall(self):
        times = [300 + (-8, -2, 3, 7)[i % 4] for i in range(30)]
        times[12] = 900
        self.assertTrue(self.verdict(self.samples(times))["within_budget"])

    def test_trend_noise_boundary_does_not_change_click_budgets(self):
        self.assertTrue(self.verdict(self.samples([300 + 25 * i / 29 for i in range(30)]))["within_budget"])
        self.assertFalse(self.verdict(self.samples([300 + 25.1 * i / 29 for i in range(30)]))["within_budget"])
        self.assertFalse(scenarios.timing("sketch", [Latency.segment(1000.001)])["within_budget"])

    def test_missing_or_nonincreasing_feature_count_cannot_pass(self):
        for damage in ("missing", "repeated", "missing_time", "truncated"):
            rows = self.samples([300] * 30)
            if damage == "missing":
                del rows[5]["feature_count"]
            elif damage == "repeated":
                rows[5]["feature_count"] = rows[4]["feature_count"]
            elif damage == "missing_time":
                del rows[5]["timing"]
            else:
                rows = rows[:9]
            with self.subTest(damage=damage):
                self.assertFalse(self.verdict(rows)["pass"])


if __name__ == "__main__":
    unittest.main()
