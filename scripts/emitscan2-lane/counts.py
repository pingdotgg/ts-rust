#!/usr/bin/env python3
"""Count st emit instructions and check every output against the base.
Usage: python3 -I counts.py OUT BASE LANE PHASES_TSV PROJECTS ROUNDS [--noEmit]
The median of all measured rounds is the lane's two-cell acceptance bar.
"""
import csv
import glob
import hashlib
import json
import os
from pathlib import Path
import re
import runpy
import socket
import statistics
import subprocess
import sys

out, base, lane, phases, names, rounds = sys.argv[1:7]
assert sys.argv[7:] in ([], ['--noEmit'])
no_emit = bool(sys.argv[7:])
rounds = int(rounds)
out = Path(out)
out.mkdir(parents=True, exist_ok=False)
bench = runpy.run_path('/home/theo/Code/sandbox/ts-rust/scripts/emitscan2-lane/bench.py')
env = bench['ENV']
projects = {p[0]: p[1:] for p in bench['PROJECTS']}
known = {}
with open(phases) as f:
    for row in csv.reader(f, delimiter='\t'):
        if row[0] in ('vscode', 'typeorm') and row[1:4] == ['go', 'st', 'emit']:
            value = (int(row[6]), int(row[8]), row[9])
            if row[0] in known and known[row[0]] != value:
                raise SystemExit('STOP: inconsistent saved Go hashes')
            known[row[0]] = value
assert set(known) == {'vscode', 'typeorm'}
perfs = sorted(glob.glob('/usr/lib/linux-tools/*/perf'), key=lambda p: [int(v) if v.isdigit() else v for v in re.split(r'(\d+)', p)])
perf = perfs[-1]
probe = subprocess.run([perf, 'stat', '-e', 'instructions:u', 'true'], capture_output=True)
prefix = [] if probe.returncode == 0 else ['sudo', '-n']
cpus = os.environ.get('CPUS', '0-15')
meta = {'host': socket.gethostname(), 'perf': perf, 'prefix': prefix, 'env': env, 'cpus': cpus,
        'rounds': rounds, 'noEmit': no_emit, 'base': base, 'lane': lane, 'knownGo': known, 'commands': []}
for side, binary in [('base', base), ('lane', lane)]:
    meta[side + 'Sha256'] = hashlib.sha256(Path(binary).read_bytes()).hexdigest()
(out / 'host.json').write_text(json.dumps(meta, indent=2) + '\n')


def tree(directory, manifest):
    # Same path ordering and hash bytes as the requested xdemit.py tree().
    h = hashlib.sha256()
    n = 0
    with manifest.open('w') as m:
        for root, _, files in sorted(os.walk(directory)):
            for filename in sorted(files):
                p = os.path.join(root, filename)
                n += 1
                digest = hashlib.sha256(Path(p).read_bytes())
                rel = os.path.relpath(p, directory)
                h.update(rel.encode() + b'\0' + digest.digest())
                m.write(digest.hexdigest() + '  ' + rel + '\n')
    return n, h.hexdigest()


rows = []
summary = {}
with (out / 'runs.tsv').open('w') as tsv:
    writer = csv.DictWriter(tsv, delimiter='\t', fieldnames=['project', 'side', 'round', 'instructions', 'exit', 'diagnostics', 'stdoutSha256', 'files', 'treeSha256'])
    writer.writeheader()
    for name in names.split(','):
        cwd, cfg, flags = projects[name]
        flags = flags.replace('--noEmit', '').split()
        if no_emit:
            flags += ['--noEmit']
        elif name == 'playwright':
            flags += ['--noEmit', 'false']
        show = subprocess.run([base, '-p', cfg, *flags, '--showConfig'], cwd=cwd, env=env, capture_output=True, check=True)
        config = json.loads(show.stdout)
        assert not config['compilerOptions'].get('declarationDir'), name
        assert not config['compilerOptions'].get('outFile'), name
        (out / (name + '.config.json')).write_bytes(show.stdout)
        first = None
        for r in range(-1, rounds):
            order = [('base', base), ('lane', lane)]
            if r % 2:
                order.reverse()
            pair = []
            for side, binary in order:
                label = f'{name}.{side}.{r}'
                scratch = out / 'scratch' / label
                assert not scratch.exists()
                output = out / (label + '.txt')
                perfout = out / (label + '.perf')
                cleanenv = ['env', '-i', *[f'{k}={v}' for k, v in env.items()]]
                cmd = ['taskset', '-c', cpus, *cleanenv, *prefix, perf, 'stat', '-x,', '-e', 'instructions:u', '-o', str(perfout), binary, '-p', cfg, *flags, '--singleThreaded', '--pretty', 'false', '--outDir', str(scratch)]
                meta['commands'].append({'project': name, 'side': side, 'round': r, 'cwd': cwd, 'argv': cmd})
                with output.open('wb') as log:
                    rc = subprocess.run(cmd, cwd=cwd, stdout=log, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL).returncode
                ins = None
                for line in perfout.read_text().splitlines():
                    fields = line.split(',')
                    if len(fields) > 2 and fields[2] == 'instructions:u':
                        ins = int(fields[0])
                assert ins and ins > 0, perfout
                text = output.read_bytes()
                count = len(re.findall(rb'(?:error|warning|message|suggestion) TS[0-9]+', text))
                files, digest = tree(scratch, out / (label + '.sha256'))
                signature = (rc, text, files, digest)
                row = dict(project=name, side=side, round=r, instructions=ins, exit=rc, diagnostics=count, stdoutSha256=hashlib.sha256(text).hexdigest(), files=files, treeSha256=digest)
                rows.append(row)
                writer.writerow(row)
                tsv.flush()
                pair.append(signature)
                if first is None:
                    first = signature
                if signature != first:
                    raise SystemExit(f'STOP: output changed at {label}')
                if name in known and not no_emit:
                    expected_rc, expected_files, expected_hash = known[name]
                    if (rc, files, digest) != known[name]:
                        raise SystemExit(f'STOP: saved Go emit parity changed at {label}: {(rc, files, digest)} != {known[name]}')
                    assert count == (10 if name == 'vscode' else 0), (label, count)
                print(f'{label}: ins={ins} exit={rc} diagnostics={count} files={files} tree={digest}', flush=True)
            assert pair[0] == pair[1], name
        summary[name] = {'parity': 'PASS', 'exit': first[0], 'files': first[2], 'treeSha256': first[3], 'savedGoParity': name in known and not no_emit}
        for n in (3, rounds):
            med = {s: statistics.median(v['instructions'] for v in rows if v['project'] == name and v['side'] == s and 0 <= v['round'] < n) for s in ('base', 'lane')}
            summary[name][f'rounds{n}'] = {**med, 'deltaPercent': (med['lane'] / med['base'] - 1) * 100}
        (out / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
        (out / 'host.json').write_text(json.dumps(meta, indent=2) + '\n')
        print(name, json.dumps(summary[name]), flush=True)
bar = all(v[f'rounds{rounds}']['deltaPercent'] <= 0.1 for v in summary.values())
if not no_emit:
    bar = bar and all(summary[p][f'rounds{rounds}']['deltaPercent'] <= -0.3 for p in ('vscode', 'typeorm'))
print('KEEP' if bar else 'DROP', flush=True)
