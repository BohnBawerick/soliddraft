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
            "resolved": True}
        known = {}
        self.assertTrue(scenarios.references_kept(known, [ref], [], False)["pass"])
        self.assertTrue(scenarios.references_kept(known, [dict(ref, feature="Boss sketch")], [], False)["pass"])
        self.assertFalse(scenarios.references_kept(known, [dict(ref, origin="F2 Start cap #1")], [], False)["pass"])
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

    def test_json_pointer_escapes_and_arrays(self):
        self.assertEqual(scenarios.at({"a/b": [{"~": 2}]}, "/a~1b/0/~0"), 2)


if __name__ == "__main__":
    unittest.main()
