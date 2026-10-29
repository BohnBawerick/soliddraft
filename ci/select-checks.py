#!/usr/bin/env python3
"""Select conservative crate consumers and platform checks from both sides of a diff."""
import argparse
import json
import pathlib
import re
import subprocess
import tomllib

ROOT = pathlib.Path(__file__).resolve().parent.parent
LINUX = {'lint', 'kernel', 'step-opencascade', 'test-linux', 'web', 'msrv', 'interactive-linux', 'application-loop'}
PLATFORM = {'test-macos', 'test-windows', 'interactive-macos'}
PACKAGES = {'package-linux', 'package-windows', 'package-macos', 'package-web'}
UNITS = LINUX | PLATFORM
OWNERS = {
    'kernel': {'soliddraft-kernel-catallax', 'soliddraft-recipe'},
    'step-opencascade': {'soliddraft-cli', 'soliddraft-exchange'},
    'web': {'soliddraft-web'},
    'msrv': {'soliddraft-kernel-api', 'soliddraft-recipe', 'soliddraft-drawing', 'soliddraft-sketch',
             'soliddraft-assistant', 'soliddraft-materials', 'soliddraft-assembly'},
    'interactive-linux': {'soliddraft-app'},
    'application-loop': {'soliddraft-cli'},
}


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


def select(changes, event, ref, packages, read_sources):
    reasons = {'lint': ['always: formatting, lint and CI unit tests']}

    def add(units, reason):
        for unit in sorted(units):
            reasons.setdefault(unit, []).append(reason)

    if changes is None or event == 'workflow_dispatch' or ref.startswith('refs/heads/release/'):
        add(UNITS | PACKAGES, 'full qualification or unavailable diff')
        return reasons
    if event == 'push' and ref == 'refs/heads/main':
        add(LINUX, 'main Linux selection audit')
    for change in changes:
        for path in change[1:]:
            sources = read_sources(path)
            if platform_path(path, sources):
                add(PLATFORM | {'interactive-linux', 'test-linux'}, f'platform: {path}')
            if path.startswith('packaging/') or path in {'.github/workflows/release.yml', 'docs/RELEASING.md'}:
                add(PACKAGES, f'packaging: {path}')
            if path.startswith(('ci/', '.github/')):
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
            elif (path.startswith('docs/') and path.endswith('.md')) or path in {'README.md', 'LICENSE', 'NOTICE'}:
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


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--root', type=pathlib.Path, default=ROOT)
    parser.add_argument('--base', required=True)
    parser.add_argument('--head', default='HEAD')
    parser.add_argument('--event', required=True)
    parser.add_argument('--ref', default='')
    parser.add_argument('--output', type=pathlib.Path, required=True)
    args = parser.parse_args()
    changes = changes_between(args.root, args.base, args.head)
    packages = graph(args.root, args.base if changes is not None else None)
    sources = lambda path: [content(args.root, path, args.base), content(args.root, path, args.head)]
    result = select(changes, args.event, args.ref, packages, sources)
    # Execute the accepted selector as well. A change cannot narrow its own obligations.
    base_script = content(args.root, 'ci/select-checks.py', args.base)
    if base_script:
        namespace = {'__name__': 'base_selector', '__file__': str(args.root / 'ci/select-checks.py')}
        exec(compile(base_script, '<base selector>', 'exec'), namespace)
        for unit, reasons in namespace['select'](changes, args.event, args.ref, packages, sources).items():
            result.setdefault(unit, []).extend(f'base: {reason}' for reason in reasons)
    args.output.write_text(json.dumps({'selected': sorted(result), 'reasons': result}, indent=2) + '\n')


if __name__ == '__main__':
    main()
