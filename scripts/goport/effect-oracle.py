#!/usr/bin/env python3
"""Makes the Effect oracle files of the sweep-wide gate stage (Effect gate rule, Theo 2026-10-10, issue #28;
state note theo-effect051-approval-2026-10-10).

Rule: for a sweep-wide config whose tsconfig lists @effect/language-service, the expected output is the plain
oracle lines plus the TS377xxx lines of effect-tsgo at the Effect pin. sweep-wide.sh compares a config with
oracle/<label>.effect.txt when that file is next to its plain oracle/<label>.txt. The plain lines come from the
plain file, so a TypeScript line that effect-tsgo gets wrong (the reference TS6133 bug) stays out.

usage: GOPORT_PIN=<key> scripts/upstream/pin.py exec -- python3 scripts/goport/effect-oracle.py
           --effect-tsc <effect-tsgo tsc> [--out DIR] [label ...]
  --effect-tsc  the tsc of the effect-tsgo package at the Effect pin (run through scripts/tsgo-oracle.sh with
                TS_GO_ORACLE_BIN, --pretty false, in the same cwd as the plain oracle run).
  --out DIR     write DIR/<project>/oracle/<label>.effect.txt. Default: next to the plain file (in a pin run,
                the pin's oracle cache).
  label         sweep-wide labels (<project>:<name>). Default: every sweep-wide config.
A config gets a file only when its resolved tsconfig (--showConfig) lists the plugin and effect-tsgo reports at
least one TS377xxx line there; otherwise the plain file is already the expected output. An existing file is not
replaced: remove it first. Lines are merged in Go's order (ast.CompareDiagnostics: path, then position), and a
plain line at the same path, line and column as an Effect line stops the run, since the text cannot order them.
"""
import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
X = Path('/home/theo/Code/sandbox/ts-rust/target/project-inputs-wide')  # as sweep-wide.sh (worktrees too)
PLUGIN = '@effect/language-service'
HEAD = re.compile(r'^(.*)\((\d+),(\d+)\): \w+ TS(\d+): ')


def entries():
    """label|project|cwd|config|oracle file rows of sweep-wide.sh (as scripts/upstream/record.py reads them)."""
    text = (REPO / 'scripts/goport/sweep-wide.sh').read_text()
    return [line.split('|') for line in re.findall(r'^\s*"([^"|]+\|[^"|]+\|[^"|]+\|[^"|]+\|[^"|]+)"', text, re.M)]


def records(text, cwd):
    """[(sort key, code, text)]: one record per diagnostic; an indented line belongs to the record before it."""
    out = []
    for line in text.splitlines(keepends=True):
        if out and line[:1].isspace():
            key, code, body = out[-1]
            out[-1] = (key, code, body + line)
            continue
        m = HEAD.match(line)
        key = (os.path.normpath(os.path.join(cwd, m[1])), int(m[2]), int(m[3])) if m else ('', 0, 0)
        out.append((key, m[4] if m else '', line))
    return out


def merge(plain, effect, label):
    for name, recs in (('plain', plain), ('effect', effect)):
        if any(a[0] > b[0] for a, b in zip(recs, recs[1:])):
            sys.exit(f'{label}: the {name} lines are not in Go order')
    ties = {r[0] for r in plain} & {r[0] for r in effect}
    if ties:
        sys.exit(f'{label}: plain and Effect lines at the same place, cannot order them: {sorted(ties)[:3]}')
    return ''.join(r[2] for r in sorted(plain + effect, key=lambda r: r[0]))  # stable: each side keeps its order


def main():
    p = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    p.add_argument('--effect-tsc', required=True)
    p.add_argument('--out')
    p.add_argument('labels', nargs='*')
    a = p.parse_args()
    oracle = Path.home() / '.local/bin/tsgo-oracle'  # in a pin run, the pin's oracle
    rows = [e for e in entries() if not a.labels or e[0] in a.labels]
    if a.labels and len(rows) != len(a.labels):
        sys.exit(f'unknown labels: {sorted(set(a.labels) - {e[0] for e in rows})}')
    version = subprocess.run([a.effect_tsc, '--version'], capture_output=True, text=True).stdout.strip()
    print(f'effect-tsc {a.effect_tsc} ({version}); plain oracle {oracle}; pin {os.environ.get("GOPORT_PIN_ACTIVE", "default")}')
    for label, proj, cwd, config, ofile in rows:
        cwd = X / proj / cwd
        shown = subprocess.run([oracle, '--showConfig', '-p', config], cwd=cwd, capture_output=True, text=True)
        if shown.returncode != 0:
            print(f'{label}: --showConfig rc={shown.returncode}, skipped')
            continue
        if not any(pl.get('name') == PLUGIN for pl in json.loads(shown.stdout).get('compilerOptions', {}).get('plugins', [])):
            continue
        run = subprocess.run([REPO / 'scripts/tsgo-oracle.sh', '-p', config, '--pretty', 'false'], cwd=cwd,
                             capture_output=True, text=True, env=dict(os.environ, TS_GO_ORACLE_BIN=a.effect_tsc))
        if run.returncode not in (0, 1, 2) or 'panic' in run.stderr:
            sys.exit(f'{label}: effect-tsgo rc={run.returncode}: {run.stderr[:300]}')
        ref = records(run.stdout, cwd)
        effect = [r for r in ref if r[1].startswith('377')]
        plain_file = X / proj / ofile
        plain = records(plain_file.read_text(), cwd)
        # Information for the reviewer: TypeScript lines where effect-tsgo and the plain oracle differ.
        ref_ts, plain_ts = {r[2] for r in ref if not r[1].startswith('377')}, {r[2] for r in plain}
        note = f'ts-lines: plain-only {len(plain_ts - ref_ts)}, effect-tsgo-only {len(ref_ts - plain_ts)}'
        if not effect:
            print(f'{label} plugin, 0 TS377xxx lines: no file (the plain oracle is the expected output); {note}')
            continue
        dest = (Path(a.out) / proj / ofile if a.out else plain_file).with_suffix('.effect.txt')
        if dest.exists():
            sys.exit(f'{label}: {dest} exists; remove it first')
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(merge(plain, effect, label))
        print(f'{label} rc={run.returncode} effect={len(effect)} plain={len(plain)} -> {dest}; {note}')


if __name__ == '__main__':
    main()
