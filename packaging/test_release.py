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



class DeferredWindowsTests(unittest.TestCase):
    NAMES = ("linux-x86_64.AppImage", "linux-x86_64.deb", "linux-x86_64.tar.gz", "macos-universal.dmg", "web.zip")
    WINDOWS = ("windows-x64.msi", "windows-x64.zip")

    def inventory(self, names):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        (root / "Cargo.toml").write_text('[workspace.package]\nversion = "0.1.0"\n')
        (root / "dist").mkdir()
        for name in ("release-notes.md", "soliddraft-0.1.0.spdx.json", *(f"soliddraft-0.1.0-{n}" for n in names)):
            (root / "dist" / name).write_text(name)
        return root

    def test_a_package_run_has_no_windows_cell_unless_asked(self):
        self.assertEqual([cell["platform"] for cell in release.desktop(False)], ["linux", "macos"])
        self.assertEqual(release.desktop(True), [
            {"platform": "linux", "os": "ubuntu-22.04", "target": "x86_64-unknown-linux-gnu", "formats": "appimage,deb"},
            {"platform": "windows", "os": "windows-2022", "target": "x86_64-pc-windows-msvc", "formats": "wix"},
            {"platform": "macos", "os": "macos-26", "target": "universal-apple-darwin", "formats": "dmg"}])

    def test_inventory_needs_windows_files_exactly_when_windows_ran(self):
        for names, windows, words in ((self.NAMES, False, None), (self.NAMES + self.WINDOWS, True, None),
                                      (self.NAMES, True, "missing"), (self.NAMES + self.WINDOWS[:1], True, "missing"),
                                      (self.NAMES + self.WINDOWS, False, "extra")):
            with self.subTest(names=names, windows=windows):
                root = self.inventory(names)
                with patch.object(release, "ROOT", root):
                    if words:
                        with self.assertRaisesRegex(ValueError, words):
                            release.checksums(windows)
                        self.assertFalse((root / "dist/SHA256SUMS").exists())
                    else:
                        release.checksums(windows)
                        self.assertEqual("windows" in (root / "dist/SHA256SUMS").read_text(), windows)

    def test_notes_say_when_a_release_has_no_windows_packages(self):
        for windows, present, absent in ((False, "no Windows packages", "SmartScreen"),
                                         (True, "SmartScreen", "no Windows packages")):
            with self.subTest(windows=windows):
                root = self.inventory(())
                with patch.object(release, "ROOT", root), patch.object(release, "formats", return_value={}), \
                        patch.object(release, "source_revisions", return_value=("a", "b")), \
                        patch.object(release, "known_gap_notes", return_value=[]), \
                        patch.object(release, "pull_request_notes", return_value=""):
                    release.notes(root / "pulls.json", None, windows)
                text = (root / "dist/release-notes.md").read_text()
                self.assertIn(present, text)
                self.assertNotIn(absent, text)
                self.assertIn("macOS Gatekeeper", text)


class KnownGapNotesTests(unittest.TestCase):
    def test_lists_every_known_gap_by_scenario(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "ci").mkdir()
            (root / "ci/known-over-budget.json").write_text(
                '{"format": "soliddraft-known-over-budget/1", "follow_up": "https://example.invalid/27", '
                '"steps": [{"scenario": "bracket", "step": 19, "tool": "edit_extrude", "last_measured_ms": 1}, '
                '{"scenario": "drawing", "step": 24, "tool": "edit_extrude", "last_measured_ms": 1}, '
                '{"scenario": "drawing", "step": 35, "tool": "edit_extrude", "last_measured_ms": 1}]}')
            with patch.object(release, "ROOT", root):
                text = "\n".join(release.known_gap_notes())
        self.assertIn("## Known gaps", text)
        self.assertIn("- bracket: steps 19 (edit_extrude).", text)
        self.assertIn("- drawing: steps 24 (edit_extrude), 35 (edit_extrude).", text)
        self.assertIn("https://example.invalid/27", text)

if __name__ == "__main__":
    unittest.main()
