"""The committed icon catalogue is what its generator writes."""

import pathlib
import subprocess
import sys
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[1]


class IconCatalogueTests(unittest.TestCase):
    def test_assets_match_the_generator(self):
        script = ROOT / 'scripts' / 'generate_cad_icons.py'
        result = subprocess.run([sys.executable, '-B', str(script), '--check'], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
