"""Reject release notes that would misidentify the packaged kernel."""
import importlib.util
from pathlib import Path
import subprocess
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


class ReleaseSourceTests(unittest.TestCase):
    def test_publication_requires_matching_release_branch_ancestry(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text('[workspace.package]\nversion = "0.1.0"\n')

            def git(*args):
                subprocess.run(["git", *args], cwd=root, check=True,
                               stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)

            git("init")
            git("add", "Cargo.toml")
            git("-c", "user.name=Release test", "-c", "user.email=release@example.invalid",
                "commit", "-m", "candidate")
            git("update-ref", "refs/remotes/origin/main", "HEAD")
            git("update-ref", "refs/remotes/origin/release/0.2.0", "HEAD")
            with patch.object(release, "ROOT", root):
                with self.assertRaises(subprocess.CalledProcessError):
                    release.check_source("v0.1.0")
                git("update-ref", "refs/remotes/origin/release/0.1.0", "HEAD")
                self.assertEqual(release.check_source("v0.1.0"), "release/0.1.0")
                (root / "Cargo.toml").write_text('[workspace.package]\nversion = "0.1.0"\n# release-only pin\n')
                git("add", "Cargo.toml")
                git("-c", "user.name=Release test", "-c", "user.email=release@example.invalid",
                    "commit", "-m", "release-only metadata")
                git("update-ref", "refs/remotes/origin/release/0.1.0", "HEAD")
                self.assertEqual(release.check_source("v0.1.0"), "release/0.1.0")
                with self.assertRaisesRegex(ValueError, "workspace version"):
                    release.check_source("v0.2.0")
                (root / "Cargo.toml").write_text('[workspace.package]\nversion = "0.1.0"\n# later work\n')
                git("add", "Cargo.toml")
                git("-c", "user.name=Release test", "-c", "user.email=release@example.invalid",
                    "commit", "-m", "unreleased work")
                git("update-ref", "refs/remotes/origin/main", "HEAD")
                with self.assertRaises(subprocess.CalledProcessError):
                    release.check_source("v0.1.0")


if __name__ == "__main__":
    unittest.main()
