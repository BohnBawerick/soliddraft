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
        known["x"] = "y"
        self.assertTrue(scenarios.references_kept(known, [], [], True)["pass"], "its feature was deleted")

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


class DrawingViewBudgets(unittest.TestCase):
    def test_view_has_its_own_strict_budget_boundary(self):
        kernel = {"calls": []}
        self.assertTrue(scenarios.drawing_view_timing(scenarios.DRAWING_VIEW_BUDGET_MS, kernel)["pass"])
        over = scenarios.drawing_view_timing(scenarios.DRAWING_VIEW_BUDGET_MS + 0.001, kernel)
        self.assertFalse(over["pass"])
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


class KnownBudgets(unittest.TestCase):
    known = {("bracket", 6): {"scenario": "bracket", "step": 6, "tool": "extrude",
                            "last_measured_ms": 2000}}

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

    def test_listed_step_still_fails_above_20_percent_regression(self):
        boundary = self.verdict(2400)
        self.assertTrue(boundary["pass"])
        self.assertFalse(boundary["within_budget"])
        self.assertEqual(boundary["budget_ms"], 1000)
        over = self.verdict(2400.001)
        self.assertFalse(over["pass"])
        self.assertFalse(over["within_budget"])
        self.assertEqual(over["status"], "known-gap-regression")
        self.assertEqual(over["budget_ms"], 1000)

    def test_listed_step_can_improve_without_changing_the_strict_budget(self):
        for elapsed in (999.999, 1000):
            with self.subTest(elapsed=elapsed):
                improved = self.verdict(elapsed)
                self.assertTrue(improved["pass"])
                self.assertTrue(improved["within_budget"])
                self.assertEqual(improved["status"], "pass")

    def test_unlisted_step_keeps_its_strict_budget_boundary(self):
        self.assertTrue(self.verdict(1000, number=7)["pass"])
        over = self.verdict(1000.001, number=7)
        self.assertFalse(over["pass"])
        self.assertEqual(over["status"], "over-budget")


class Trends(unittest.TestCase):
    @staticmethod
    def samples(times):
        return [{"number": 3 + 4 * i, "tool": "sketch", "feature_count": 3 + 2 * i,
                 "timing": {"elapsed_ms": elapsed}} for i, elapsed in enumerate(times)]

    def verdict(self, rows):
        return scenarios.trend_checks([{"tool": "sketch", "from_step": 3}], rows)[0]

    def test_growth_fails_even_when_every_step_meets_its_click_budget(self):
        rows = self.samples([100 + 2 * i for i in range(30)])
        self.assertTrue(all(scenarios.timing("sketch", [Latency.segment(row["timing"]["elapsed_ms"])])["pass"]
                            for row in rows))
        result = self.verdict(rows)
        self.assertFalse(result["pass"])
        self.assertAlmostEqual(result["growth_ms"], 58)

    def test_flat_series_tolerates_jitter_and_a_single_stall(self):
        times = [300 + (-8, -2, 3, 7)[i % 4] for i in range(30)]
        times[12] = 900
        self.assertTrue(self.verdict(self.samples(times))["pass"])

    def test_trend_noise_boundary_does_not_change_click_budgets(self):
        self.assertTrue(self.verdict(self.samples([300 + 25 * i / 29 for i in range(30)]))["pass"])
        self.assertFalse(self.verdict(self.samples([300 + 25.1 * i / 29 for i in range(30)]))["pass"])
        self.assertFalse(scenarios.timing("sketch", [Latency.segment(1000.001)])["pass"])

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
