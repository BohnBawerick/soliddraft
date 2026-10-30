"""The S0 ownership acceptance cases."""

import pathlib
import shutil
import subprocess
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[1]


class OwnershipTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = pathlib.Path(self.directory.name)
        for crate in ('soliddraft-app', 'soliddraft-model'):
            shutil.copytree(ROOT / 'crates' / crate / 'src', self.root / 'crates' / crate / 'src')
        (self.root / 'ci').mkdir()
        shutil.copy(ROOT / 'ci/ownership.toml', self.root / 'ci/ownership.toml')

    def run_check(self):
        return subprocess.run(['python3', str(ROOT / 'ci/check-ownership.py'), '--root', str(self.root)],
                              capture_output=True, text=True)

    def test_clean(self):
        result = self.run_check()
        self.assertEqual((result.returncode, result.stdout), (0, ''))

    def test_persistence_cannot_import_commands(self):
        file = self.root / 'crates/soliddraft-model/src/file.rs'
        original = file.read_text()
        for imported in ('crate::command::Command', 'crate::Command'):
            with self.subTest(imported=imported):
                file.write_text(original + f'\nuse {imported};\n')
                result = self.run_check()
                line = original.count('\n') + 2
                self.assertEqual(result.returncode, 1)
                self.assertEqual(result.stdout,
                                 f'crates/soliddraft-model/src/file.rs:{line}: O2: '
                                 'model.persistence references model.commands\n')

    def test_slices_cannot_import_other_slices(self):
        for name, source in (('grid', 'use crate::measure::Track;\n'), ('measure', 'pub struct Track;\n')):
            directory = self.root / 'crates/soliddraft-app/src' / name
            directory.mkdir(exist_ok=True)
            (directory / 'mod.rs').write_text(source)
        result = self.run_check()
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout,
                         'crates/soliddraft-app/src/grid/mod.rs:1: O1: v3.grid references v2.measure\n')

    def test_unowned_file(self):
        (self.root / 'crates/soliddraft-app/src/pick_measure.rs').write_text('pub struct Probe;\n')
        result = self.run_check()
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout,
                         'crates/soliddraft-app/src/pick_measure.rs:1: O3: '
                         'file belongs to 0 areas; expected exactly one\n')

    def test_cross_crate_slices_cannot_import_other_slices(self):
        for crate, module, source in (
            ('soliddraft-app', 'measure', 'use soliddraft_model::configs::Variant;\n'),
            ('soliddraft-model', 'configs', 'pub struct Variant;\n'),
        ):
            directory = self.root / 'crates' / crate / 'src' / module
            directory.mkdir(exist_ok=True)
            (directory / 'mod.rs').write_text(source)
        result = self.run_check()
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout,
                         'crates/soliddraft-app/src/measure/mod.rs:1: O1: v2.measure references v3.configs\n')
