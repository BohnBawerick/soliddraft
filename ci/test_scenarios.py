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
    def call(name, elapsed, operations=(), *, nested=False, refused=False, work=None):
        return {"operation": name, "wall_ms": elapsed, "nested": nested,
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

    def test_refusal_keeps_its_whole_wait_and_work(self):
        result = scenarios.timing("cut", [self.segment(600, [
            self.call("boolean", 500, [self.operation("mesh", 50, 3)],
                      refused=True, work={"used": 7}),
        ], phase="refusal")])
        self.assertEqual(result["split_ms"]["refusal"], 600)
        self.assertEqual(result["work"]["refusal"], 7)
        self.assertEqual(result["split_ms"]["display"], 0)

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


if __name__ == "__main__":
    unittest.main()
