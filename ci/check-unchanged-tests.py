#!/usr/bin/env python3
"""Reject edits to existing acceptance inputs and lost test names during S0."""

import collections
import re
import sys

from refactor_baseline import at_base, cargo, git


def names():
    listed = cargo('test', '--workspace', '--all-features', '--locked', '--', '--list')
    return collections.Counter(re.findall(r'^(\S+): test$', listed, re.MULTILINE))


def protected(path):
    return (path == 'ci/run-scenarios.py' or path.startswith(('ci/scenarios/', 'recipes/'))
            or bool(re.match(r'crates/[^/]+/(?:tests|fixtures)/', path)))


if __name__ == '__main__':
    base = sys.argv[1]
    errors = []
    for line in git('diff', '--name-status', '--no-renames', f'{base}...HEAD').splitlines():
        status, path = line.split('\t', 1)
        if status != 'A' and protected(path):
            errors.append(f'existing acceptance input changed: {status} {path}')
    with at_base(base):
        before = names()
    after = names()
    # This unit test moves with plane_on_face; its assertions are unchanged.
    moved = {'document::tests::face_planes_have_upright_frames':
             'evaluate::tests::face_planes_have_upright_frames'}
    for old, new in moved.items():
        if old in before:
            before[new] += before.pop(old)
    errors.extend(f'missing test: {name} ({count})' for name, count in (before - after).items())
    for error in errors:
        print(error)
    print(f'{sum(before.values())} base tests; {sum(after.values())} candidate tests')
    raise SystemExit(bool(errors))
