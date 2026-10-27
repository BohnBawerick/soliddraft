"""Every workflow cancels its branch's superseded runs and never cancels a run on main."""

import pathlib
import re
import unittest

WORKFLOWS = sorted(pathlib.Path(__file__).resolve().parent.parent.joinpath(".github", "workflows").glob("*.yml"))
MAIN = "refs/heads/main"


def top_level(text, key):
    """The value of a top-level `key:` or of its indented lines, without a YAML parser."""
    match = re.search(rf"^{key}:[ \t]*(.*)\n((?:[ \t]+.*\n|[ \t]*\n)*)", text, re.MULTILINE)
    return match and (match.group(1), match.group(2))


def evaluate(expr, **github):
    """Evaluates the subset of the expression language the concurrency settings use."""
    python = re.sub(r"github\.(\w+)", r"g['\1']", expr).replace("&&", " and ").replace("||", " or ")
    python = re.sub(r"!(?!=)", " not ", python)
    context = {"workflow": "Hosted CI", "head_ref": "", "run_id": "1", **github}
    return eval(python, {"__builtins__": {}}, {
        "g": context,
        "startsWith": lambda value, prefix: value.lower().startswith(prefix.lower()),
    })


def render(template, **github):
    """A setting's value with each `${{ ... }}` replaced by its result; booleans as GitHub prints them."""
    def value(match):
        result = evaluate(match.group(1).strip(), **github)
        return str(result).lower() if isinstance(result, bool) else str(result)

    return re.sub(r"\$\{\{(.*?)\}\}", value, template)


def settings(path):
    """The workflow's group and cancel-in-progress templates, and its text."""
    text = path.read_text()
    block = top_level(text, "concurrency")
    assert block, f"{path.name} has no concurrency group"
    fields = dict(re.findall(r"^[ \t]+([\w-]+):[ \t]*(.*)$", block[1], re.MULTILINE))
    return fields["group"], fields["cancel-in-progress"], text


class Concurrency(unittest.TestCase):
    def test_there_are_workflows(self):
        self.assertTrue(WORKFLOWS)

    def test_a_push_to_a_pull_request_branch_cancels_its_older_runs(self):
        for path in WORKFLOWS:
            group, cancel, _ = settings(path)
            first = dict(ref="refs/pull/7/merge", head_ref="feature", run_id="100")
            second = dict(first, run_id="101")
            self.assertEqual(render(group, **first), render(group, **second), path.name)
            self.assertEqual(render(cancel, **second), "true", path.name)

    def test_other_branches_do_not_share_a_group(self):
        for path in WORKFLOWS:
            group = settings(path)[0]
            one = render(group, ref="refs/pull/7/merge", head_ref="feature-one")
            two = render(group, ref="refs/pull/8/merge", head_ref="feature-two")
            self.assertNotEqual(one, two, path.name)

    def test_a_manual_run_on_a_branch_is_grouped_by_the_branch(self):
        for path in WORKFLOWS:
            group, cancel, _ = settings(path)
            run = dict(ref="refs/heads/feature", run_id="5")
            self.assertEqual(render(group, **run), render(group, **dict(run, run_id="6")), path.name)
            self.assertEqual(render(cancel, **run), "true", path.name)

    def test_a_run_on_main_is_never_cancelled_nor_shares_a_group(self):
        for path in WORKFLOWS:
            group, cancel, _ = settings(path)
            pushes = [render(group, ref=MAIN, run_id=str(n)) for n in (200, 201, 202)]
            self.assertEqual(len(set(pushes)), 3, f"{path.name}: main pushes share a group, so a waiting one is replaced")
            self.assertEqual(render(cancel, ref=MAIN, run_id="200"), "false", path.name)

    def test_a_pull_request_from_a_branch_called_main_cannot_reach_main_runs(self):
        for path in WORKFLOWS:
            group = settings(path)[0]
            fork = render(group, ref="refs/pull/9/merge", head_ref="main", run_id="300")
            self.assertNotEqual(fork, render(group, ref=MAIN, run_id="300"), path.name)
            self.assertNotEqual(fork, render(group, ref=MAIN, run_id="301"), path.name)

    def test_release_branch_runs_keep_independent_evidence(self):
        for path in WORKFLOWS:
            group, cancel, _ = settings(path)
            run = dict(ref="refs/heads/release/0.1.0", run_id="350")
            self.assertNotEqual(render(group, **run), render(group, **dict(run, run_id="351")), path.name)
            self.assertEqual(render(cancel, **run), "false", path.name)
            fork = dict(ref="refs/pull/10/merge", head_ref="release/0.1.0", run_id="350")
            self.assertNotEqual(render(group, **run), render(group, **fork), path.name)
            self.assertEqual(render(cancel, **fork), "true", path.name)

    def test_tag_releases_cannot_cancel_or_replace_one_another(self):
        path = pathlib.Path(__file__).resolve().parent.parent / ".github/workflows/release.yml"
        group, cancel, _ = settings(path)
        first = dict(ref="refs/tags/v0.1.0", run_id="400")
        second = dict(first, run_id="401")
        self.assertNotEqual(render(group, **first), render(group, **second))
        self.assertEqual(render(cancel, **first), "false")
        self.assertEqual(render(cancel, **second), "false")

    def test_the_setting_is_not_overridden_per_job(self):
        for path in WORKFLOWS:
            text = settings(path)[2]
            self.assertEqual(len(re.findall(r"^\s*concurrency:", text, re.MULTILINE)), 1, path.name)


if __name__ == "__main__":
    unittest.main()
