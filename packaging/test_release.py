"""Reject release notes that would misidentify the packaged kernel."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("release", Path(__file__).with_name("release.py"))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class SourceRevisionsTests(unittest.TestCase):
    def test_rejects_inconsistent_kernel_sources(self):
        kernel, other = "a" * 40, "b" * 40
        cases = (
            (other, kernel, kernel, "dependencies"),
            (kernel, other, kernel, "backend identity"),
            (kernel, kernel, other, "Locked"),
        )
        for dependency, declared, locked, error in cases:
            with self.subTest(error=error), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                adapter = root / "crates/soliddraft-kernel-catallax"
                (adapter / "src").mkdir(parents=True)
                (adapter / "Cargo.toml").write_text(
                    f'[dependencies]\ncatallax-app = {{ rev = "{kernel}" }}\n'
                    f'catallax-mesh = {{ rev = "{dependency}" }}\n')
                (adapter / "src/lib.rs").write_text(
                    f'pub const CATALLAX_REVISION: &str = "{declared}";\n')
                (root / "Cargo.lock").write_text(
                    '[[package]]\nname = "catallax-app"\n'
                    f'source = "git+https://github.com/BohnBawerick/catallax?rev={kernel}#{locked}"\n')
                with patch.object(release, "ROOT", root), patch.object(
                        release.subprocess, "check_output", return_value="c" * 40 + "\n"):
                    with self.assertRaisesRegex(ValueError, error):
                        release.source_revisions()


if __name__ == "__main__":
    unittest.main()
