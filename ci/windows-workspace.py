#!/usr/bin/env python3
"""Split the Windows workspace tests across hosted runners, and require all of them at the gate."""
import argparse
from collections import Counter
from datetime import datetime, timezone
import hashlib
import json
import math
import os
import pathlib
import re
import subprocess
import sys
import threading
import tomllib

RECORD_FORMAT = 'soliddraft-windows-record/1'
WEIGHTS_FORMAT = 'soliddraft-windows-weights/1'
RECORD = 'record.json'
ARTIFACT = 'windows-workspace-{}'
UNITS_GROUP = '@units-and-docs'
# A target with no timing yet is placed as a slow one. Weights only place targets, never omit them.
DEFAULT_WEIGHT = 600.0
# A test's surviving child can hold the output pipe after Cargo exits; it must not hold the shard.
DRAIN_SECONDS = 30
CARGO_TEST = ['cargo', 'test', '--workspace', '--all-features', '--locked', '--no-fail-fast']
COMPILE = [*CARGO_TEST, '--no-run', '--message-format=json-render-diagnostics']
METADATA = ['cargo', 'metadata', '--format-version', '1', '--locked', '--all-features', '--no-deps']
LIB_KINDS = {'lib', 'rlib', 'dylib', 'cdylib', 'staticlib', 'proc-macro'}
DOCTESTED_KINDS = {'lib', 'rlib', 'proc-macro'}
# Everything these keys can do is visible in Cargo metadata. Any other key needs a reviewed rule here.
TARGET_KEYS = {'name', 'path', 'crate-type', 'crate_type', 'required-features', 'test', 'doctest', 'doc', 'bench',
               'edition', 'proc-macro', 'proc_macro', 'harness'}
ANSI = re.compile(r'\x1b\[[0-9;]*[A-Za-z]')
STARTED = re.compile(r'^\s*Running\s.*\(([^()]*deps[\\/][^()\\/]+)\)\s*$')
DOCTEST = re.compile(r'^\s*Doc-tests\s+(\S+)\s*$')
RESULT = re.compile(r'^test result: (\S+?)\.\s+(\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; '
                    r'(\d+) filtered out;(?:.*?(?:finished in|ran in) ([\d.]+)s)?')
CASE = re.compile(r'^test (.+?) \.\.\. (ok|FAILED|ignored(?:, .*)?)\s*$')


class PlanError(Exception):
    """The workspace or the weights cannot be planned without risking an omitted test."""


def now():
    return datetime.now(timezone.utc).isoformat()


def unique_keys(pairs):
    repeated = [key for key, count in Counter(key for key, _ in pairs).items() if count > 1]
    if repeated:
        raise ValueError(f'duplicate key {repeated[0]}')
    return dict(pairs)


def refuse_constant(name):
    raise ValueError(f'{name} is not a number')


def strict_json(text):
    return json.loads(text, object_pairs_hook=unique_keys, parse_constant=refuse_constant)


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def check_weights(seconds):
    if not isinstance(seconds, dict):
        raise PlanError('weights: seconds must be an object')
    for name, value in seconds.items():
        if (not name or isinstance(value, bool) or not isinstance(value, (int, float))
                or not math.isfinite(value) or value < 0):
            raise PlanError(f'weights: {name!r}: {value!r} is not a finite, non-negative number of seconds')
    return seconds


def parse_weights(text):
    try:
        data = strict_json(text)
    except ValueError as error:
        raise PlanError(f'weights: invalid JSON: {error}') from error
    if not isinstance(data, dict) or set(data) != {'format', 'seconds'} or data['format'] != WEIGHTS_FORMAT:
        raise PlanError(f'weights: expected {{"format": "{WEIGHTS_FORMAT}", "seconds": {{...}}}}')
    return check_weights(data['seconds'])


def manifest_problems(manifest):
    sections = [manifest.get('lib', {})]
    for kind in ('bin', 'test', 'example', 'bench'):
        sections.extend(manifest.get(kind, []))
    problems = []
    for section in sections:
        name = section.get('name', 'lib')
        if section.get('harness', True) is not True:
            problems.append(f'{name}: harness = false reports no libtest result to reconcile')
        problems.extend(f'{name}: unknown target setting {key}' for key in sorted(set(section) - TARGET_KEYS))
    return problems


def unit_kind(kinds):
    if kinds == ['test']:
        return 'test'
    if kinds == ['bin']:
        return 'bin'
    if kinds and set(kinds) <= LIB_KINDS:
        return 'lib'
    return kinds[0] if len(kinds) == 1 else '+'.join(kinds)


def entry(package, target, kind, group):
    return {'id': f'{package}/{kind}/{target}', 'package': package, 'target': target, 'kind': kind, 'group': group}


def target_entries(package, target):
    """What today's unsplit `cargo test --workspace` runs for one target, or a refusal."""
    where = f"{package['name']}/{target['name']}"
    kind = unit_kind(target['kind'])
    if kind not in {'test', 'bin', 'lib', 'example', 'bench', 'custom-build'}:
        raise PlanError(f'{where}: unknown target kind {target["kind"]}; teach ci/windows-workspace.py to run it')
    unmet = sorted(set(target.get('required-features') or []) - set(package.get('features') or {}))
    if unmet:
        raise PlanError(f'{where}: --all-features does not enable required features {unmet}; '
                        'teach ci/windows-workspace.py to run it')
    if kind in {'example', 'bench', 'custom-build'}:
        if target['test']:
            raise PlanError(f'{where}: test = true on a {kind} runs under plain cargo test but under no shard; '
                            'teach ci/windows-workspace.py to run it')
        return []
    if not target['test']:
        raise PlanError(f'{where}: test = false differs from the default; teach ci/windows-workspace.py to skip it')
    if kind == 'test':
        return [entry(package['name'], target['name'], 'test', target['name'])]
    entries = [entry(package['name'], target['name'], kind, UNITS_GROUP)]
    if kind == 'lib' and target['doctest'] and set(target['kind']) & DOCTESTED_KINDS:
        entries.append(entry(package['name'], target['name'], 'doc', UNITS_GROUP))
    return entries


def inventory(metadata):
    """Every test execution of the workspace, by logical package and target, from current metadata."""
    members = set(metadata['workspace_members'])
    entries = []
    problems = []
    for package in metadata['packages']:
        if package['id'] not in members:
            continue
        manifest = tomllib.loads(pathlib.Path(package['manifest_path']).read_text(encoding='utf-8'))
        problems.extend(f"{package['name']}: {problem}" for problem in manifest_problems(manifest))
        for target in package['targets']:
            entries.extend(target_entries(package, target))
    if problems:
        raise PlanError('; '.join(problems) + '; teach ci/windows-workspace.py this configuration')
    repeated = [name for name, count in Counter(e['id'] for e in entries).items() if count > 1]
    if repeated:
        raise PlanError(f'{repeated[0]}: two targets share one logical identity')
    return sorted(entries, key=lambda e: e['id'])


def partition(groups, weights, count):
    """Heaviest group first, each onto the least-loaded shard; ties go to the lower index."""
    weight = lambda group: weights.get(group, DEFAULT_WEIGHT)
    loads = [0.0] * count
    shards = [[] for _ in range(count)]
    for group in sorted(set(groups), key=lambda g: (-weight(g), g)):
        index = min(range(count), key=lambda i: (loads[i], i))
        shards[index].append(group)
        loads[index] += weight(group)
    return [sorted(shard) for shard in shards]


def make_plan(entries, weights, count):
    return {'count': count, 'inventory': entries, 'weights': weights,
            'assignment': partition((e['group'] for e in entries), weights, count)}


def shard_commands(groups):
    """The exact Cargo invocations of one shard. No integration group means no unfiltered command."""
    names = sorted(set(groups) - {UNITS_GROUP})
    commands = [{'role': 'compile', 'argv': COMPILE}]
    if names:
        commands.append({'role': 'integration',
                         'argv': [*CARGO_TEST, *(flag for name in names for flag in ('--test', name))]})
    if UNITS_GROUP in groups:
        commands.append({'role': 'units', 'argv': [*CARGO_TEST, '--lib', '--bins']})
        commands.append({'role': 'docs', 'argv': [*CARGO_TEST, '--doc']})
    return commands


def role_ids(entries, groups, role):
    kinds = {'integration': {'test'}, 'units': {'lib', 'bin'}, 'docs': {'doc'}}[role]
    return sorted(e['id'] for e in entries if e['kind'] in kinds and e['group'] in groups)


def compiled_binaries(stream, packages):
    """Test executables by file name, as logical identities, from Cargo's artifact messages."""
    binaries = {}
    for line in stream.splitlines():
        if not line.startswith('{'):
            continue
        message = json.loads(line)
        if message.get('reason') != 'compiler-artifact' or not message.get('executable'):
            continue
        if not message['profile']['test']:
            continue
        package = packages.get(message['package_id'], message['package_id'])
        kind = unit_kind(message['target']['kind'])
        name = re.split(r'[\\/]', message['executable'])[-1]
        binaries[name] = f"{package}/{kind}/{message['target']['name']}"
    return binaries


def scan(text, binaries, docs):
    """Match Cargo's start lines to libtest's results, by logical identity.

    Results pair with the oldest start still open, so stdout arriving after the next binary's stderr line, as in
    the runner's own log, still lands on the right binary. A rustdoc target can print a result per merged edition.
    """
    executions = []
    waiting = []

    def current():
        return waiting[0] if waiting else executions[-1] if executions else None

    for raw in text.splitlines():
        line = ANSI.sub('', raw)
        started = STARTED.match(line)
        doctest = DOCTEST.match(line)
        if started or doctest:
            name = re.split(r'[\\/]', started[1])[-1] if started else doctest[1]
            execution = {'id': (binaries if started else docs).get(name), 'name': name, 'results': [], 'tests': {}}
            executions.append(execution)
            waiting.append(execution)
            continue
        result = RESULT.match(line)
        case = CASE.match(line)
        if result and current() is not None:
            current()['results'].append({
                'outcome': result[1], 'passed': int(result[2]), 'failed': int(result[3]), 'ignored': int(result[4]),
                'measured': int(result[5]), 'filtered': int(result[6]),
                'seconds': float(result[7]) if result[7] else None})
            if waiting:
                waiting.pop(0)
        elif case and current() is not None:
            current()['tests'][case[1]] = case[2].split(',')[0]
    return executions


def tee(argv, root, log, stdout=None):
    """Run a command, keeping all of its output in `log` and on the console.

    Execution merges stdout and stderr into one pipe, so libtest's lines stay in order with Cargo's.
    """
    process = subprocess.Popen(argv, cwd=root, stdout=stdout or subprocess.PIPE,
                               stderr=subprocess.PIPE if stdout else subprocess.STDOUT)
    stream = process.stderr if stdout else process.stdout

    def copy():
        with log.open('wb') as file:
            for line in stream:
                file.write(line)
                file.flush()
                sys.stdout.buffer.write(line)
                sys.stdout.buffer.flush()

    reader = threading.Thread(target=copy, daemon=True)
    reader.start()
    code = process.wait()
    reader.join(DRAIN_SECONDS)
    return code


def capture(argv, root):
    return subprocess.run(argv, cwd=root, check=True, stdout=subprocess.PIPE, text=True, encoding='utf-8').stdout


def profile():
    prefixes = ('CARGO_PROFILE_', 'CARGO_BUILD_')
    names = {'CARGO_INCREMENTAL', 'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER', 'RUSTDOCFLAGS',
             'WGPU_BACKEND', 'SOLIDDRAFT_REQUIRE_GPU'}
    return {key: value for key, value in sorted(os.environ.items()) if key in names or key.startswith(prefixes)}


def file_hash(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def save(record, out):
    # Written after every step, so a shard that dies leaves an incomplete record for the gate to refuse.
    temporary = out / (RECORD + '.partial')
    temporary.write_text(json.dumps(record, indent=2, sort_keys=True) + '\n', encoding='utf-8')
    temporary.replace(out / RECORD)


def execution_failures(label, execution):
    failures = []
    if not execution['results']:
        failures.append(f"{label}: {execution['id']}: no libtest result")
    for result in execution['results']:
        if result['outcome'] != 'ok' or result['failed']:
            failures.append(f"{label}: {execution['id']}: libtest {result['outcome']}, {result['failed']} failed")
        if result['filtered']:
            failures.append(f"{label}: {execution['id']}: {result['filtered']} tests filtered out")
    return failures


def record_failures(record):
    """What keeps one shard's record from proving its assigned targets ran and passed."""
    label = f"shard {record['shard']}"
    failures = [f"{label}: {record['error']}"] if record['error'] else []
    if record['status'] != 'complete':
        failures.append(f"{label}: status {record['status']}")
    entries = record['inventory']
    groups = record['assignment'][record['shard']]
    expected = shard_commands(groups)
    commands = record['commands']
    if [c['argv'] for c in commands] != [c['argv'] for c in expected]:
        failures.append(f'{label}: commands differ from the plan')
    for command in commands:
        if command['exit'] != 0:
            failures.append(f"{label}: {command['role']} exited {command['exit']}")
    compiled = sorted(e['id'] for e in entries if e['kind'] != 'doc')
    if sorted(record['compiled']) != compiled:
        failures.append(f'{label}: compiled test targets differ from the inventory')
    for command in commands[1:]:
        observed = [e['id'] for e in command['executions']]
        if command['role'] not in {'integration', 'units', 'docs'}:
            failures.append(f"{label}: unknown command role {command['role']}")
            continue
        wanted = role_ids(entries, groups, command['role'])
        if sorted(str(i) for i in observed) != wanted:
            missing = sorted(set(wanted) - set(observed))
            extra = sorted(str(i) for i in set(observed) - set(wanted))
            repeated = sorted(str(i) for i, n in Counter(observed).items() if n > 1)
            failures.append(f"{label}: {command['role']}: missing {missing}, extra {extra}, repeated {repeated}")
        for execution in command['executions']:
            failures.extend(execution_failures(label, execution))
    return failures


def run(args):
    out = args.out
    out.mkdir(parents=True, exist_ok=True)
    root = args.root.resolve()
    record = {'format': RECORD_FORMAT, 'run_id': args.run_id, 'attempt': args.attempt, 'shard': args.shard,
              'count': args.count, 'requested_sha': args.sha, 'sha': None, 'status': 'running', 'error': None,
              'rustc': None, 'profile': profile(), 'inputs': None, 'plan_sha256': None, 'inventory': [],
              'assignment': [], 'weights': {}, 'compiled': [], 'commands': [], 'started': now(), 'ended': None}
    save(record, out)

    def finish(error=None):
        record['ended'] = now()
        record['error'] = error
        record['status'] = 'failed'
        if not error:
            record['status'] = 'complete'
            if record_failures(record):
                record['status'] = 'failed'
        save(record, out)
        for failure in record_failures(record):
            print(f'windows-workspace: {failure}', file=sys.stderr)
        return 0 if record['status'] == 'complete' else 1

    try:
        record['sha'] = capture(['git', 'rev-parse', 'HEAD'], root).strip()
        if record['sha'] != args.sha:
            return finish(f"checkout {record['sha']} is not the requested {args.sha}")
        record['rustc'] = capture(['rustc', '-vV'], root)
        record['inputs'] = {'Cargo.lock': file_hash(root / 'Cargo.lock'), 'weights': file_hash(args.weights)}
        metadata = json.loads(capture(METADATA, root))
        weights = parse_weights(args.weights.read_text(encoding='utf-8'))
        plan = make_plan(inventory(metadata), weights, args.count)
    except (PlanError, OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError) as error:
        return finish(f'cannot plan: {error}')
    record.update(inventory=plan['inventory'], assignment=plan['assignment'], weights=plan['weights'],
                  plan_sha256=digest(plan))
    save(record, out)

    commands = shard_commands(plan['assignment'][args.shard])
    compile_start = now()
    stream = out / 'compile.jsonl'
    with stream.open('wb') as stdout:
        code = tee(commands[0]['argv'], root, out / 'compile.log', stdout)
    record['commands'].append({**commands[0], 'log': 'compile.log', 'start': compile_start, 'end': now(), 'exit': code,
                               'executions': []})
    save(record, out)
    if code:
        return finish(f'compile exited {code}')
    packages = {p['id']: p['name'] for p in metadata['packages']}
    try:
        binaries = compiled_binaries(stream.read_text(encoding='utf-8', errors='replace'), packages)
    except (ValueError, KeyError, TypeError) as error:
        return finish(f'cannot read the compiler artifacts: {error}')
    record['compiled'] = sorted(binaries.values())
    planned = sorted(e['id'] for e in plan['inventory'] if e['kind'] != 'doc')
    if record['compiled'] != planned:
        missing = sorted(set(planned) - set(record['compiled']))
        extra = sorted(set(record['compiled']) - set(planned))
        return finish(f'compiled test targets differ from the plan: missing {missing}, extra {extra}')

    docs = {e['target']: e['id'] for e in plan['inventory'] if e['kind'] == 'doc'}
    for command in commands[1:]:
        # Keep going after a failure, as --no-fail-fast does across one unsplit command.
        start = now()
        log = out / f"{command['role']}.log"
        code = tee(command['argv'], root, log)
        text = log.read_text(encoding='utf-8', errors='replace') if log.exists() else ''
        record['commands'].append({**command, 'log': log.name, 'start': start, 'end': now(), 'exit': code,
                                   'executions': scan(text, binaries, docs)})
        save(record, out)
    return finish()


def load_records(directory):
    records = []
    failures = []
    folders = sorted(p for p in directory.iterdir() if p.is_dir()) if directory.is_dir() else []
    for folder in folders:
        try:
            record = strict_json((folder / RECORD).read_text(encoding='utf-8'))
            if not isinstance(record, dict) or not isinstance(record.get('shard'), int):
                raise ValueError('no shard number')
            if folder.name != ARTIFACT.format(record['shard']):
                raise ValueError(f"holds shard {record['shard']}")
            records.append(record)
        except (OSError, ValueError) as error:
            failures.append(f'{folder.name}: unusable record: {error}')
    return records, failures


def plan_failures(record, count):
    """The record's plan must be its own hash, and the partition its inputs deterministically produce."""
    label = f"shard {record['shard']}"
    plan = {key: record[key] for key in ('count', 'inventory', 'weights', 'assignment')}
    failures = []
    if digest(plan) != record['plan_sha256']:
        failures.append(f'{label}: plan hash does not match the plan')
    check_weights(record['weights'])
    entries = record['inventory']
    if [e['id'] for e in entries] != sorted({e['id'] for e in entries}):
        failures.append(f'{label}: inventory is unsorted or repeats a target')
    for item in entries:
        if item['id'] != f"{item['package']}/{item['kind']}/{item['target']}":
            failures.append(f"{label}: {item['id']}: identity differs from its package and target")
        if item['group'] != (item['target'] if item['kind'] == 'test' else UNITS_GROUP):
            failures.append(f"{label}: {item['id']}: not in its group")
    if record['assignment'] != partition((e['group'] for e in entries), record['weights'], count):
        failures.append(f'{label}: assignment is not the deterministic partition of its inventory')
    return failures


def verify(records, count, sha, run_id):
    """Everything the gate requires of the shards' records, as a list of failures."""
    shards = Counter(record.get('shard') for record in records)
    failures = [f'shard {shard}: repeated' for shard, n in shards.items() if n > 1]
    failures.extend(f'shard {shard}: no record' for shard in range(count) if shard not in shards)
    failures.extend(f'shard {shard}: not in the plan' for shard in shards if shard not in range(count))
    if failures:
        return failures
    records = sorted(records, key=lambda r: r['shard'])
    try:
        for record in records:
            label = f"shard {record['shard']}"
            if record['format'] != RECORD_FORMAT:
                failures.append(f"{label}: unknown format {record['format']!r}")
            if record['sha'] != sha:
                failures.append(f"{label}: checkout {record['sha']}, not {sha}")
            if str(record['run_id']) != str(run_id):
                failures.append(f"{label}: run {record['run_id']}, not {run_id}")
            if record['count'] != count:
                failures.append(f"{label}: planned {record['count']} shards, not {count}")
            failures.extend(plan_failures(record, count))
            failures.extend(record_failures(record))
        # A rerun child may keep earlier siblings only under the same compiler, profile and plan.
        for key in ('rustc', 'profile', 'inputs', 'plan_sha256', 'inventory', 'assignment', 'weights'):
            if len({json.dumps(record[key], sort_keys=True) for record in records}) != 1:
                failures.append(f'{key}: differs between shards')
        expected = Counter(e['id'] for e in records[0]['inventory'])
        observed = Counter(execution['id'] for record in records for command in record['commands'][1:]
                           for execution in command['executions'])
        if observed != expected:
            failures.append(f'coverage: missing {sorted(set(expected - observed))}, '
                            f'extra {sorted(str(i) for i in observed - expected)}')
    except (KeyError, TypeError, IndexError, ValueError, AttributeError, PlanError) as error:
        failures.append(f'malformed record: {error!r}')
    return failures


def verify_command(args):
    records, failures = load_records(args.records)
    failures.extend(verify(records, args.count, args.sha, args.run_id))
    for failure in failures:
        print(f'windows-workspace: {failure}')
    if not failures:
        total = len(records[0]['inventory'])
        print(f'windows-workspace: {args.count} shards ran all {total} test targets of {args.sha}')
    return 1 if failures else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    runner = commands.add_parser('run', help='compile the workspace and run one shard')
    runner.add_argument('--root', type=pathlib.Path, required=True)
    runner.add_argument('--shard', type=int, required=True)
    runner.add_argument('--count', type=int, required=True)
    runner.add_argument('--weights', type=pathlib.Path, required=True)
    runner.add_argument('--out', type=pathlib.Path, required=True)
    runner.add_argument('--sha', required=True)
    runner.add_argument('--run-id', required=True)
    runner.add_argument('--attempt', required=True)
    runner.set_defaults(function=run)
    checker = commands.add_parser('verify', help='require a complete record from every shard')
    checker.add_argument('--records', type=pathlib.Path, required=True)
    checker.add_argument('--count', type=int, required=True)
    checker.add_argument('--sha', required=True)
    checker.add_argument('--run-id', required=True)
    checker.set_defaults(function=verify_command)
    args = parser.parse_args()
    if args.command == 'run' and not 0 <= args.shard < args.count:
        parser.error(f'shard {args.shard} is not one of {args.count}')
    return args.function(args)


if __name__ == '__main__':
    raise SystemExit(main())
