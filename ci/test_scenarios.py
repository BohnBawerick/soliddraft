"""Checks must fail closed when evidence or comparison fields are missing."""

import importlib.util
import pathlib
import unittest

spec = importlib.util.spec_from_file_location("scenarios", pathlib.Path(__file__).with_name("run-scenarios.py"))
scenarios = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scenarios)


class Checks(unittest.TestCase):
    def test_numeric_check_and_missing_evidence(self):
        rule = {"pointer": "/body/volume", "approx": 12000, "tolerance": 0.01}
        self.assertTrue(scenarios.check({"body": {"volume": 12000.005}}, rule)["pass"])
        self.assertFalse(scenarios.check({"body": {"volume": 11999}}, rule)["pass"])
        self.assertFalse(scenarios.check({"body": {}}, rule)["pass"])
        self.assertFalse(scenarios.check({"body": {"volume": None}}, rule)["pass"])

    def test_invalid_or_ambiguous_comparisons_fail(self):
        for rule in ({"pointer": ""}, {"pointer": "", "equals": 3, "minimum": 1},
                     {"pointer": "bad", "equals": 3}):
            self.assertFalse(scenarios.check(3, rule)["pass"])

    def test_json_pointer_escapes_and_arrays(self):
        self.assertEqual(scenarios.at({"a/b": [{"~": 2}]}, "/a~1b/0/~0"), 2)


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
        display = self.segment(650, phase="display")
        result = scenarios.timing("extrude", [command, display])
        self.assertFalse(result["pass"])
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
        result = scenarios.timing("edit_extrude", [self.segment(1100, [
            self.call("tessellate", 3000, [self.operation("mesh", 2900, 15)],
                      work={"used": 15}, foreground=False),
            self.call("tessellate_edges", 500, nested=True, foreground=False),
            self.call("extrude", 500, work={"used": 25}),
        ])])
        self.assertFalse(result["pass"])
        self.assertEqual(result["elapsed_ms"], 1100)
        self.assertEqual(result["split_ms"]["edit"], 1100)
        self.assertEqual(sum(result["split_ms"].values()), 1100)
        self.assertEqual(result["work"]["edit"], 25)
        self.assertEqual(result["background"]["call_ms"]["display"], 3000)
        self.assertEqual(result["background"]["work"]["display"], 15)

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
                self.assertTrue(scenarios.timing(tool, [self.segment(limit)])["pass"])
                self.assertFalse(scenarios.timing(tool, [self.segment(limit + 0.001)])["pass"])

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


class KnownBudgets(unittest.TestCase):
    known = {("bracket", 6): {"scenario": "bracket", "step": 6, "tool": "extrude",
                            "last_measured_ms": 2240.892348}}

    def verdict(self, elapsed, scenario="bracket", number=6, tool="extrude"):
        latency = scenarios.timing(tool, [Latency.segment(elapsed)])
        return scenarios.accept_latency(scenario, number, tool, latency, self.known)

    def test_approved_overrun_is_accepted_only_for_its_exact_step_and_tool(self):
        accepted = self.verdict(2240)
        self.assertTrue(accepted["pass"])
        self.assertFalse(accepted["within_budget"])
        self.assertEqual(accepted["status"], "known-over-budget")
        self.assertFalse(self.verdict(2240, scenario="another-bracket")["pass"])
        self.assertFalse(self.verdict(2240, number=7)["pass"])
        self.assertFalse(self.verdict(2240, tool="cut")["pass"])

    def test_listed_step_that_meets_the_budget_fails_as_stale(self):
        for elapsed in (999.999, 1000):
            with self.subTest(elapsed=elapsed):
                stale = self.verdict(elapsed)
                self.assertFalse(stale["pass"])
                self.assertTrue(stale["within_budget"])
                self.assertEqual(stale["status"], "stale-known-over-budget")

    def test_unlisted_step_keeps_its_strict_budget_boundary(self):
        self.assertTrue(self.verdict(1000, number=7)["pass"])
        over = self.verdict(1000.001, number=7)
        self.assertFalse(over["pass"])
        self.assertEqual(over["status"], "over-budget")


if __name__ == "__main__":
    unittest.main()
