#!/usr/bin/env python3
"""Generate ci/windows-workspace-weights.json from the timing table beside it."""
from collections import defaultdict
import json
import pathlib
import statistics

CI = pathlib.Path(__file__).resolve().parent
UNITS_GROUP = '@units-and-docs'


def samples(table):
    """One `{group: seconds}` per timing column. Same-named targets of different packages add up."""
    columns = None
    for line in table.splitlines():
        cells = [cell.strip() for cell in line.strip().strip('|').split('|')]
        if cells[0] == 'Binary':
            columns = [defaultdict(float) for _ in cells[1:]]
        elif columns and '/' in cells[0]:
            name = cells[0].split('/')[1]
            key = UNITS_GROUP if name in {'src', 'doctests'} else name
            for column, value in zip(columns, cells[1:], strict=True):
                if value != 'absent':
                    column[key] += float(value)
    return columns


def weights(table):
    groups = samples(table)
    names = sorted({name for group in groups for name in group})
    # Libtest reports hundredths, so rounding drops only the noise of binary floating point.
    return {name: round(statistics.median(g[name] for g in groups if name in g), 2) for name in names}


def render(table):
    data = {'format': 'soliddraft-windows-weights/1', 'seconds': weights(table)}
    return json.dumps(data, indent=2) + '\n'


if __name__ == '__main__':
    table = (CI / 'windows-workspace-timings.md').read_text(encoding='utf-8')
    (CI / 'windows-workspace-weights.json').write_text(render(table), encoding='utf-8')
