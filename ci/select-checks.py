#!/usr/bin/env python3
"""Select conservative crate consumers and platform checks from both sides of a diff."""
import argparse
from collections import Counter
import json
import pathlib
import re
import subprocess
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parent.parent
LINUX = {'lint', 'kernel', 'step-opencascade', 'test-linux', 'web', 'msrv', 'interactive-linux', 'application-loop'}
PLATFORM = {'test-macos', 'test-windows', 'interactive-macos'}
PACKAGES = {'package-linux', 'package-windows', 'package-macos', 'package-web'}
UNITS = LINUX | PLATFORM
CATALLAX_GIT = 'https://github.com/BohnBawerick/catallax'
CATALLAX_SOURCE = re.compile(r'git\+https://github\.com/BohnBawerick/catallax\?rev=([0-9a-f]{40})#\1')
OWNERS = {
    'kernel': {'soliddraft-kernel-catallax', 'soliddraft-recipe'},
    'step-opencascade': {'soliddraft-cli', 'soliddraft-exchange'},
    'web': {'soliddraft-web'},
    'msrv': {'soliddraft-kernel-api', 'soliddraft-recipe', 'soliddraft-drawing', 'soliddraft-sketch',
             'soliddraft-assistant', 'soliddraft-materials', 'soliddraft-assembly'},
    'interactive-linux': {'soliddraft-app'},
    'application-loop': {'soliddraft-cli'},
}
POLICY = 'ci/check-consumers.json'


class PolicyError(Exception):
    """The consumer map cannot judge a path, so no selection may be published."""


def git(root, *args):
    return subprocess.check_output(['git', '-C', str(root), *args], stderr=subprocess.DEVNULL)


def content(root, path, revision=None):
    if revision:
        try:
            return git(root, 'show', f'{revision}:{path}').decode(errors='replace')
        except subprocess.CalledProcessError:
            return ''
    file = root / path
    return file.read_text(errors='replace') if file.is_file() else ''


def unique_keys(pairs):
    repeated = [key for key, count in Counter(key for key, _ in pairs).items() if count > 1]
    if repeated:
        raise PolicyError(f'{POLICY}: duplicate key {repeated[0]}')
    return dict(pairs)


def consumer_units(key, value):
    if value == 'all':
        return UNITS | PACKAGES
    if isinstance(value, list) and all(isinstance(u, str) and u in UNITS | PACKAGES for u in value):
        return set(value)
    raise PolicyError(f'{POLICY}: {key}: invalid units {value!r}')


def parse_consumers(text):
    try:
        policy = json.loads(text, object_pairs_hook=unique_keys)
    except ValueError as error:
        raise PolicyError(f'{POLICY}: missing or invalid JSON: {error}') from error
    if (not isinstance(policy, dict) or set(policy) != {'format', 'files', 'patterns'}
            or policy['format'] != 'soliddraft-check-consumers/1'
            or not isinstance(policy['files'], dict) or not isinstance(policy['patterns'], dict)):
        raise PolicyError(f'{POLICY}: invalid schema')
    files = {path: consumer_units(path, value) for path, value in policy['files'].items()}
    patterns = []
    for pattern, value in policy['patterns'].items():
        try:
            patterns.append((re.compile(pattern), consumer_units(pattern, value)))
        except re.error as error:
            raise PolicyError(f'{POLICY}: {pattern}: {error}') from error
    for path in files:
        mapped_units(path, (files, patterns))
    return files, patterns


def mapped_units(path, consumers):
    files, patterns = consumers
    matches = [units for pattern, units in patterns if pattern.fullmatch(path)]
    if path in files:
        matches.append(files[path])
    if len(matches) > 1:
        raise PolicyError(f'{POLICY}: {path}: more than one entry matches')
    return matches[0] if matches else None


def ci_consumers(path, removed, consumers, base_map):
    units = mapped_units(path, consumers)
    if units is None and removed and base_map:
        units = mapped_units(path, parse_consumers(base_map))
    if units is None:
        raise PolicyError(f'{path}: no entry in {POLICY}')
    return units


def check_inventory(root, revision, consumers):
    # The whole tree counts: a diff must not hide an unmapped file, and neither must a full selection.
    tracked = git(root, 'ls-tree', '-r', '-z', '--name-only', revision, '--', 'ci/').decode().split('\0')
    unmapped = [path for path in tracked if path and mapped_units(path, consumers) is None]
    if unmapped:
        raise PolicyError(f'no entry in {POLICY} for {", ".join(unmapped)}')


def platform_path(path, sources):
    if pathlib.PurePosixPath(path).name in {'Cargo.toml', 'Cargo.lock', 'rust-toolchain', 'rust-toolchain.toml'}:
        return True
    if path.startswith(('packaging/', '.github/', 'crates/soliddraft-app/src/viewport/')):
        return True
    # Include multiline and nested cfg expressions; wasm-only code does not add desktop legs.
    return path.endswith('.rs') and any(
        re.search(r'\bcfg!?\s*\([^;{}]*?\b(target_os|target_family|target_vendor|target_env|windows|unix)\b', s, re.S)
        for s in sources)


def graph(root, base=None):
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--format-version', '1', '--locked', '--no-deps'], cwd=root))
    packages = {p['name']: set(d['name'] for d in p['dependencies']) for p in metadata['packages']}
    # Cargo metadata includes optional, target and dev dependencies. Keep removed base edges too.
    if base:
        for path in git(root, 'ls-tree', '-r', '--name-only', base).decode().splitlines():
            if path.startswith('crates/') and path.endswith('/Cargo.toml'):
                manifest = tomllib.loads(content(root, path, base))
                tables = [manifest, *manifest.get('target', {}).values()]
                deps = packages.setdefault(manifest['package']['name'], set())
                for table in tables:
                    for kind in ('dependencies', 'dev-dependencies', 'build-dependencies'):
                        for name, spec in table.get(kind, {}).items():
                            deps.add(spec.get('package', name) if isinstance(spec, dict) else name)
    return packages


def catallax_lock_move(sources):
    base, head = [tomllib.loads(text) for text in sources]
    if {k: v for k, v in base.items() if k != 'package'} != {
            k: v for k, v in head.items() if k != 'package'}:
        return False
    grouped = []
    for manifest in (base, head):
        names, revisions, other = set(), set(), []
        for package in manifest['package']:
            source = CATALLAX_SOURCE.fullmatch(package.get('source', ''))
            if source:
                if package['name'] in names:
                    return False
                names.add(package['name'])
                revisions.add(source[1])
            else:
                other.append(json.dumps(package, sort_keys=True))
        grouped.append((revisions, Counter(other)))
    (old, old_other), (new, new_other) = grouped
    # Package source decides ownership: Catallax tables may change freely, nothing else may.
    return bool(old and new) and old != new and old_other == new_other


def catallax_manifest_move(sources):
    manifests = [tomllib.loads(text) for text in sources]
    revisions = []
    for manifest in manifests:
        pins = {}
        tables = [('', manifest), ('workspace', manifest.get('workspace', {}))]
        tables.extend((f'target.{key}', table) for key, table in manifest.get('target', {}).items())
        for prefix, table in tables:
            for kind in ('dependencies', 'dev-dependencies', 'build-dependencies'):
                for name, dependency in table.get(kind, {}).items():
                    if isinstance(dependency, dict) and dependency.get('git') == CATALLAX_GIT:
                        pins[prefix, kind, name] = dependency.pop('rev', None)
        revisions.append(pins)
    return manifests[0] == manifests[1] and revisions[0] != revisions[1]


def workflow_jobs(text):
    lines = text.splitlines(keepends=True)
    markers = [i for i, line in enumerate(lines) if line.rstrip('\r\n') == 'jobs:']
    if len(markers) != 1:
        return None
    index = markers[0]
    jobs = {}
    current = None
    for line in lines[index + 1:]:
        if not line.strip() or line.lstrip().startswith('#'):
            if current is not None:
                jobs[current] += line
            continue
        if not line.startswith((' ', '\t')):
            return None
        match = re.fullmatch(r'  ([A-Za-z0-9_-]+):[ \t]*\r?\n?', line)
        if match:
            current = match[1]
            if current in jobs:
                return None
            jobs[current] = line
        elif current is None or re.match(r'^  \S', line):
            return None
        else:
            jobs[current] += line
    return ''.join(lines[:index]), jobs


def shared_yaml_node(body):
    scalar_column = None
    for line in body.splitlines():
        if not line.strip() or line.lstrip().startswith('#'):
            continue
        indent = len(line) - len(line.lstrip())
        if scalar_column is not None and indent > scalar_column:
            continue
        scalar_column = None
        value = line.lstrip()
        column = indent
        while value.startswith('- '):
            value = value[2:].lstrip()
            column = len(line) - len(value)
        key = re.match(r'([^:]+):(?:[ \t]+|$)(.*)', value)
        if key:
            if key[1].strip(' \t\'"') == '<<':
                return True
            value = key[2].lstrip()
            if re.fullmatch(r'[|>][+\-1-9]*(?:\s+#.*)?\s*', value):
                scalar_column = column
                continue
        if value.startswith(('&', '*', '!')):
            return True
        if value.startswith(('[', '{')) and re.search(r'(?:[\[{,]\s*|:\s+)[&*]', value):
            return True
    return False


def one_linux_job(sources):
    parsed = [workflow_jobs(text) for text in sources]
    if any(part is None for part in parsed):
        return None
    (old_preamble, old), (new_preamble, new) = parsed
    if old_preamble != new_preamble or list(old) != list(new):
        return None
    changed = [name for name in old if old[name] != new[name]]
    if len(changed) != 1 or changed[0] not in LINUX:
        return None
    name = changed[0]
    runners = []
    for body in (old[name], new[name]):
        lines = body.splitlines()
        runner_lines = [i for i, line in enumerate(lines) if line.startswith('    runs-on:')]
        if len(runner_lines) != 1:
            return None
        index = runner_lines[0]
        runner = lines[index]
        # Only single-line labels and expressions have an unambiguous runner value here.
        if not re.fullmatch(r'    runs-on: (?:[A-Za-z0-9_-]+|\$\{\{.*\}\})[ \t]*(?:#.*)?', runner):
            return None
        following = next((line for line in lines[index + 1:]
                          if line.strip() and not line.lstrip().startswith('#')), '')
        if following and len(following) - len(following.lstrip()) > 4:
            return None
        runners.append(runner)
        if ('matrix' in body
                or re.search(r'^\s*strategy:', body, re.M) or shared_yaml_node(body)):
            return None
    return name if runners[0] == runners[1] else None


def narrow_units(path, sources):
    try:
        name = pathlib.PurePosixPath(path).name
        if ((name == 'Cargo.lock' and catallax_lock_move(sources))
                or (name == 'Cargo.toml' and catallax_manifest_move(sources))):
            return LINUX
        if path == '.github/workflows/hosted.yml':
            job = one_linux_job(sources)
            if job:
                return {job}
    except (ValueError, KeyError, TypeError, AttributeError):
        # Malformed or unfamiliar data keeps the broad rules.
        pass
    return None


def select(changes, event, ref, packages, read_sources):
    reasons = {'lint': ['always: formatting, lint and CI unit tests']}

    def add(units, reason):
        for unit in sorted(units):
            reasons.setdefault(unit, []).append(reason)

    base_map, head_map = read_sources(POLICY)
    mapped = parse_consumers(head_map)
    if changes is None or event == 'workflow_dispatch' or ref.startswith('refs/heads/release/'):
        add(UNITS | PACKAGES, 'full qualification or unavailable diff')
        return reasons
    if event == 'push' and ref == 'refs/heads/main':
        add(LINUX, 'main Linux selection audit')
    for change in changes:
        for path in change[1:]:
            if path.startswith('ci/'):
                removed = change[0][0] in 'DR' and path == change[1]
                add(ci_consumers(path, removed, mapped, base_map), f'CI consumers: {path}')
            sources = read_sources(path)
            narrow = narrow_units(path, sources) if change[0] == 'M' else None
            if narrow:
                add(narrow, f'revision-only Catallax move or one Linux job: {path}')
                continue
            if platform_path(path, sources):
                add(PLATFORM | {'interactive-linux', 'test-linux'}, f'platform: {path}')
            if path.startswith('packaging/') or path in {'.github/workflows/release.yml', 'docs/RELEASING.md'}:
                add(PACKAGES, f'packaging: {path}')
            if path.startswith('.github/'):
                add(UNITS | PACKAGES, f'check policy: {path}')
            elif pathlib.PurePosixPath(path).name in {'Cargo.toml', 'Cargo.lock', 'rust-toolchain', 'rust-toolchain.toml', 'build.rs'}:
                add(UNITS, f'compiler/dependencies/build: {path}')
            elif path.startswith('crates/') and len(path.split('/')) > 2 and path.split('/')[1] in packages:
                affected = {path.split('/')[1]}
                while True:
                    consumers = {p for p, deps in packages.items() if deps & affected}
                    if consumers <= affected:
                        break
                    affected |= consumers
                add({'test-linux'}, f'workspace: {path}')
                for unit, owners in OWNERS.items():
                    if owners & affected:
                        add({unit}, f'crate consumers of {path}: {", ".join(sorted(owners & affected))}')
            elif path.startswith('recipes/'):
                add(LINUX, f'compiled recipe fixture: {path}')
            elif (path.startswith('ci/') or (path.startswith('docs/') and path.endswith('.md'))
                  or path in {'README.md', 'LICENSE', 'NOTICE'}):
                pass
            else:
                add(UNITS | PACKAGES, f'unknown path: {path}')
    return reasons


def changes_between(root, base, head):
    if not base or set(base) == {'0'}:
        return None
    try:
        fields = git(root, 'diff', '--name-status', '-z', '-M', base, head).decode().split('\0')
        changes = []
        index = 0
        while fields[index]:
            status = fields[index]
            count = 2 if status.startswith(('R', 'C')) else 1
            changes.append([status, *fields[index + 1:index + 1 + count]])
            index += count + 1
        return changes
    except subprocess.CalledProcessError:
        return None


def with_base_rules(result, base_script, changes, event, ref, packages, sources):
    # Execute the accepted selector as well. A change cannot narrow its own obligations.
    if base_script:
        namespace = {'__name__': 'base_selector', '__file__': str(ROOT / 'ci/select-checks.py')}
        exec(compile(base_script, '<base selector>', 'exec'), namespace)
        # The accepted selector reads its own map, so a candidate map edit cannot erase a consumer.
        accepted = sources(POLICY)[0]
        bound = lambda path: [accepted, accepted] if path == POLICY else sources(path)
        try:
            selected = namespace['select'](changes, event, ref, packages, bound)
        except namespace.get('PolicyError', ()) as error:
            # A path new to the accepted map arrives with a map edit, which selects everything anyway.
            selected = {unit: [f'accepted map cannot judge this change: {error}'] for unit in UNITS | PACKAGES}
        for unit, reasons in selected.items():
            result.setdefault(unit, []).extend(f'base: {reason}' for reason in reasons)
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--root', type=pathlib.Path, default=ROOT)
    parser.add_argument('--base', required=True)
    parser.add_argument('--head', default='HEAD')
    parser.add_argument('--event', required=True)
    parser.add_argument('--ref', default='')
    parser.add_argument('--output', type=pathlib.Path, required=True)
    args = parser.parse_args()
    # The running checkout's selector and map are the candidate policy; --head names the tree they judge.
    policy = content(args.root, POLICY)
    sources = lambda path: [content(args.root, path, args.base),
                            policy if path == POLICY else content(args.root, path, args.head)]
    try:
        check_inventory(args.root, args.head, parse_consumers(policy))
        changes = changes_between(args.root, args.base, args.head)
        packages = graph(args.root, args.base if changes is not None else None)
        result = select(changes, args.event, args.ref, packages, sources)
        base_script = content(args.root, 'ci/select-checks.py', args.base)
        result = with_base_rules(result, base_script, changes, args.event, args.ref, packages, sources)
    except PolicyError as error:
        sys.exit(f'select-checks: {error}')
    args.output.write_text(json.dumps({'selected': sorted(result), 'reasons': result}, indent=2) + '\n')


if __name__ == '__main__':
    main()
