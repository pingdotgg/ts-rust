#!/usr/bin/env python3
"""Prints the README tables from run.sh and t3code.sh results: median times, the speedup over tsc 6,
and the geometric mean speedup over tsc 7. For T3 Code, the sum of the five projects for each mode.
A limited run (TOOLS=tsc-rs, <app>.tsc-rs.json) replaces those tools' times of the full run.
usage: scripts/bench-apps/summary.py <work-dir>"""
import json, math, sys
from pathlib import Path

R = Path(sys.argv[1]) / 'results'
APPS = ['vscode', 'sentry', 'playwright', 'typeorm', 'excalidraw', 'trpc-server']
T3 = ['apps-server', 'apps-web', 'apps-mobile', 'packages-client-runtime', 'packages-shared']
TOOLS = ['tsc6', 'tsc7', 'tsc-rs', 'bun']


def medians(stem: str) -> dict[str, float] | None:
    """Median time per tool for one result stem: the full run, then each limited run over it."""
    if not (R / f'{stem}.json').exists():
        return None
    med = {}
    for f in [R / f'{stem}.json', *sorted(R.glob(f'{stem}.*.json'))]:
        med |= {r['command']: r['median'] for r in json.load(open(f))['results']}
    return med


rows, vs7 = [], {t: [] for t in TOOLS}
for app in APPS:
    if (med := medians(app)) is None:
        continue
    cells = [f'{med[t]:.2f}s' + ('' if t == 'tsc6' else f' ({med["tsc6"] / med[t]:.1f}×)') for t in TOOLS]
    rows.append(f'| {app} | ' + ' | '.join(cells) + ' |')
    for t in TOOLS:
        vs7[t].append(med['tsc7'] / med[t])
    for f in [R / f'{app}.errors', *sorted(R.glob(f'{app}.*.errors'))]:
        print(f.name, f.read_text().replace('\n', ' '))

if rows:
    print('\n| App | ' + ' | '.join(TOOLS) + ' |\n| --- |' + ' ---: |' * len(TOOLS))
    print('\n'.join(rows) + '\n')
    for t in TOOLS:
        print(f'{t}: {math.exp(sum(map(math.log, vs7[t])) / len(vs7[t])):.2f}× the speed of tsc7 (geometric mean)')

for mode in ['noeffect', 'effect']:
    meds = [medians(f't3-{p}-{mode}') for p in T3]
    if None in meds:
        continue
    total = {t: sum(m[t] for m in meds) for t in TOOLS}
    print(f'\nT3 Code {mode} (sum of {len(T3)} projects):')
    for t in sorted(TOOLS, key=total.get):
        print(f'  {t:7} {total[t]:7.2f}s  {total["tsc6"] / total[t]:5.1f}× tsc6  {total["tsc7"] / total[t]:5.2f}× tsc7')
