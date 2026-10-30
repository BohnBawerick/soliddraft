"""Bundle comparison must preserve answers and raw STEP bytes."""
import importlib.util
import json
import pathlib
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("bundles", pathlib.Path(__file__).with_name("compare-scenario-bundles.py"))
bundles = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bundles)


class BundleIdentity(unittest.TestCase):
    def test_only_pins_and_document_uuids_are_normalised(self):
        pins = "a" * 40, "b" * 40
        base = b'{"pin":"' + pins[0].encode() + b'","id":"11111111-1111-1111-1111-111111111111","volume":1}'
        candidate = base.replace(pins[0].encode(), pins[1].encode()).replace(b"11111111", b"22222222")
        self.assertNotEqual(bundles.normalise(base, None), bundles.normalise(candidate, None))
        self.assertEqual(bundles.normalise(base, pins), bundles.normalise(candidate, pins))
        self.assertNotEqual(bundles.normalise(base, pins), bundles.normalise(candidate.replace(b'"volume":1', b'"volume":2'), pins))

    def test_step_files_are_compared_without_normalisation(self):
        with tempfile.TemporaryDirectory() as temp:
            a, b = [pathlib.Path(temp) / side for side in ("base", "candidate")]
            for root in (a, b):
                (root / "commands/0001").mkdir(parents=True)
                (root / "scenario.json").write_text(json.dumps({"steps": [{}]}))
                (root / "result.json").write_text(json.dumps({"steps": [{}]}))
                (root / "protocol.json").write_text("{}")
                (root / "commands/0001/part.step").write_bytes(b"STEP original")
            self.assertEqual(bundles.problems(a, b), [])
            (b / "commands/0001/part.step").write_bytes(b"STEP changed")
            self.assertEqual(bundles.problems(a, b), ["commands/0001/part.step: STEP bytes differ"])
            (b / "commands/0001/part.step").unlink()
            self.assertIn("commands/0001/part.step: STEP bytes differ", bundles.problems(a, b))


if __name__ == "__main__":
    unittest.main()
