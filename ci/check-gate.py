#!/usr/bin/env python3
"""Fail closed on absent results, unregistered jobs and silent check retirement."""
import argparse
import json
import pathlib


def verdict(registry, base_registry, retired, selected, needs):
    messages, failures = [], []
    retirement = {r['job']: r['reason'] for r in retired if r.get('reason', '').strip()}
    for unit in sorted(set(base_registry) - set(registry)):
        if unit in retirement:
            messages.append(f'{unit}: retired, {retirement[unit]}')
        else:
            failures.append(f'{unit}: removed without a retirement record')
    for unit in sorted(set(needs) - set(registry)):
        failures.append(f'{unit}: unregistered job')
    if selected is None or needs.get('select') != 'success':
        failures.append('select: selection failed or missing')
    for unit in sorted(set(selected or []) | {'select'}):
        result = needs.get(unit, 'absent')
        messages.append(f'{unit}: {result}')
        if unit not in registry or result != 'success':
            failures.append(f'{unit}: required success, got {result}')
    # An unexpected red result must not be hidden just because the selector omitted it.
    for unit, result in needs.items():
        if result not in {'success', 'skipped'} and unit not in set(selected or []):
            failures.append(f'{unit}: unexpected {result}')
    return {'pass': not failures, 'messages': messages, 'failures': failures}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--registry', type=pathlib.Path, required=True)
    parser.add_argument('--base-registry', type=pathlib.Path, required=True)
    parser.add_argument('--selection', type=pathlib.Path, required=True)
    parser.add_argument('--needs', type=pathlib.Path, required=True)
    args = parser.parse_args()
    registry = json.loads(args.registry.read_text())
    base = json.loads(args.base_registry.read_text())
    selected = json.loads(args.selection.read_text()).get('selected') if args.selection.exists() else None
    jobs = json.loads(args.needs.read_text())
    mappings = registry['jobs']
    results = {unit: jobs[job]['result'] for unit, job in mappings.items() if job in jobs}
    for job in set(jobs) - set(mappings.values()):
        results[job] = jobs[job]['result']
    result = verdict(mappings, base['jobs'], registry['retired'], selected, results)
    print(json.dumps(result, indent=2))
    return 0 if result['pass'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
