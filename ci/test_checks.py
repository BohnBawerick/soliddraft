"""Independent R2 fixtures, scenario inventory and fail-closed workflow wiring."""
import hashlib
import importlib.util
import json
import pathlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent


def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'ci' / f'{name}.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


selector = load('select-checks')
gate = load('check-gate')


def read(name):
    return json.loads((ROOT / 'ci' / name).read_text())


def jobs(text):
    starts = list(re.finditer(r'^  ([\w-]+):\n', text[text.index('jobs:\n') + 6:], re.M))
    body = text[text.index('jobs:\n') + 6:]
    return {m[1]: body[m.end():starts[i + 1].start() if i + 1 < len(starts) else len(body)]
            for i, m in enumerate(starts)}


class Inventory(unittest.TestCase):
    def test_inventory_matches_files_and_has_owners(self):
        entries = read('scenario-inventory.json')['scenarios']
        self.assertEqual(len(entries), len({e['file'] for e in entries}))
        self.assertEqual({e['file'] for e in entries},
                         {str(p.relative_to(ROOT)) for p in (ROOT / 'ci/scenarios').glob('*.json')})
        self.assertTrue(all(e['owners'] for e in entries))

    def test_acceptance_has_numeric_output_and_error_path(self):
        entries = read('scenario-inventory.json')['scenarios']
        self.assertTrue(next(e for e in entries if e['id'] == 'bracket')['acceptance'])
        for entry in entries:
            if entry['acceptance']:
                steps = json.loads((ROOT / entry['file']).read_text())['steps']
                self.assertTrue(any('approx' in c or 'equals' in c for s in steps for c in s.get('checks', [])))
                self.assertTrue(any('refused' in s or 'refusal' in s for s in steps))


class Routing(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.packages = selector.graph(ROOT)
        cls.sources = staticmethod(lambda p: [selector.content(ROOT, p, 'HEAD'), selector.content(ROOT, p)])

    def test_frozen_fixtures(self):
        for name, sha in [('routing-cases.json', '6fcf684d2dcae3ad47ae1ed7901c55056fdaa99e616b4903b4e2c0e639765f75'),
                          ('gate-cases.json', 'c66b54349c735d237663849692c9df802caabf8b9f33d6e03940134d89d23c05')]:
            self.assertEqual(hashlib.sha256((ROOT / 'ci' / name).read_bytes()).hexdigest(), sha)

    def test_routing_fixture(self):
        fixture = read('routing-cases.json')
        for case in fixture['cases']:
            with self.subTest(case['id']):
                selected = selector.select(case['changes'], case['event'], case.get('ref', ''), self.packages, self.sources)
                must = fixture['units'] if case['must'] == 'units' else case['must']
                self.assertLessEqual(set(must), set(selected))
                self.assertFalse(set(case['must_not']) & set(selected))

    def test_every_platform_file_selects_platform_checks(self):
        tracked = selector.git(ROOT, 'ls-files', '-z').decode().split('\0')
        platform = [p for p in tracked if p and selector.platform_path(p, self.sources(p))]
        required = ['crates/soliddraft-app/src/file.rs', 'crates/soliddraft-assistant/src/key.rs',
                    'crates/soliddraft-document/src/journal.rs', 'crates/soliddraft-document/src/save.rs',
                    'crates/soliddraft-cli/tests/compatibility.rs', 'crates/soliddraft-cli/tests/fuzz_arguments.rs',
                    'crates/soliddraft-assistant/Cargo.toml']
        self.assertLessEqual(set(required), set(platform))
        for path in platform:
            with self.subTest(path):
                selected = selector.select([['M', path]], 'pull_request', '', self.packages, self.sources)
                self.assertLessEqual(selector.PLATFORM, set(selected))


class Gate(unittest.TestCase):
    def test_gate_fixture(self):
        fixture = read('gate-cases.json')
        for case in fixture['cases']:
            with self.subTest(case['id']):
                result = gate.verdict(fixture['registry'], case['base_registry'] or fixture['registry'],
                                      case['retired'], case['selected'], case['needs'])
                self.assertEqual(result['pass'], case['pass'])
                for name in case['names']:
                    self.assertIn(name, '\n'.join(result['messages'] + result['failures']))


class Wiring(unittest.TestCase):
    def test_gate_always_needs_every_job(self):
        workflow = (ROOT / '.github/workflows/hosted.yml').read_text()
        parsed = jobs(workflow)
        self.assertRegex(parsed['gate'], r'(?m)^    if: (?:\$\{\{ )?always\(\)')
        needs = re.search(r'^    needs: \[(.*?)\]', parsed['gate'], re.M)[1]
        self.assertEqual({n.strip() for n in needs.split(',')}, set(parsed) - {'gate'})
        self.assertEqual(set(read('check-registry.json')['jobs'].values()), set(parsed) - {'gate'})

    def test_no_silent_failure(self):
        self.assertNotIn('continue-on-error', (ROOT / '.github/workflows/hosted.yml').read_text())

    def test_release_requires_exact_successful_hosted_run(self):
        parsed = jobs((ROOT / '.github/workflows/release.yml').read_text())
        self.assertRegex(parsed['publish'], r'needs: \[plan, assemble\]')
        self.assertIn('needs: plan', parsed['assemble'])
        self.assertIn('uses: ./.github/workflows/packages.yml', parsed['assemble'])
        plan = parsed['plan']
        self.assertIn('branch=$(python3 packaging/release.py check-source "$GITHUB_REF_NAME")', plan)
        self.assertIn('actions/workflows/hosted.yml/runs?head_sha=$GITHUB_SHA&event=push&branch=$branch', plan)
        self.assertIn("any(.conclusion == \"success\")' | grep -x true", plan)
        self.assertIn('"$GITHUB_REF" == refs/tags/v*', plan)


if __name__ == '__main__':
    unittest.main()
