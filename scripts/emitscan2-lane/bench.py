#!/usr/bin/env python3
"""perfscan7 bench (copy of perfscan6 bench.py; int100 6cd0e4321 PGO+BOLT build int100-1 as RS, Go 3e9236361fa9 as GO): every side of a project runs interleaved
(one warmup round, then ROUNDS rounds) on one host. perfscan6 change: each round runs the sides in a new random
order (random.Random(f'{project}/{round}'), so a rerun repeats it), not a turned fixed order: the perfscan5 skeptic found a
port default run after a Go st run 4 to 6% slower, and a fixed cyclic order gives a side the same predecessor in most
rounds. runs.tsv gets an 11th column: the side that ran just before (- for the first run of a project).

usage: bench.py OUT ROUNDS PROJECTS SIDES
  PROJECTS, SIDES: comma lists of the names below, or 'all'.
Env: CPUS (taskset list; empty = no taskset), W (bench work dir, default /tmp/bench.noindex), RS (port tsgo),
GO (Go oracle), TSC7 (npm tsc 7), BUN (bun), PROG (name=tsgo,...: more port builds as <name>-def and <name>-st). Every run: env -i HOME PATH LC_ALL=C GOPORT_LAUNCH=0, stdin /dev/null.
Writes OUT/host.txt, OUT/runs.tsv (project, side, round, wall s, user s, sys s, max RSS KiB, exit, diag count,
diag sha12, previous side; round -1 = warmup), OUT/out/<project>.<side>.txt (the warmup output). One run at a time.
"""
import hashlib, os, random, re, socket, subprocess, sys, tempfile, time
from pathlib import Path

R = '/home/theo/Code/sandbox/ts-rust'
C = R + '/target/continuation-r97-goport'
W = os.environ.get('W', '/tmp/bench.noindex')
RS = os.environ.get('RS', C + '/int100/timing/rel/int100-1/bin/tsgo')
GO = os.environ.get('GO', '/home/theo/.local/bin/tsgo-oracle-3e9236361fa9')
TSC7 = os.environ.get('TSC7', W + '/node_modules/@typescript/typescript-linux-x64/lib/tsc')
BUN = os.environ.get('BUN', W + '/bun-linux-x64/bun')
PI = R + '/target/project-inputs'
# PROG="name=tsgo,...": more port builds, each as sides <name>-def and <name>-st (progress runs).
PROG = [tuple(x.split('=', 1)) for x in os.environ.get('PROG', '').split(',') if x]
F = '--noEmit --incremental false'
T3F = F + ' --composite false'
# name, cwd, tsconfig, flags (the fastmode1 projects; effect without the Effect plugin)
PROJECTS = [
    ('query', R, PI + '/query/source/packages/query-core/tsconfig.prod.json', F),
    ('hono', R, PI + '/hono/source/tsconfig.build.json', T3F),
    ('zod', R, PI + '/zod/source/packages/zod/tsconfig.json', F),
    ('effect', R, C + '/perfscan7/cfg/effnoplug/tsconfig.json', T3F + ' --declarationMap false'),
    ('trpc', W + '/repos/trpc', 'packages/server/tsconfig.json', F),
    ('typeorm', W + '/repos/typeorm', 'packages/typeorm/tsconfig.bench.json', F),
    ('playwright', W + '/repos/playwright', 'tsconfig.json', F),
    ('excalidraw', W + '/repos/excalidraw', 'tsconfig.bench.json', F),
    ('t3server', W + '/repos/t3code/apps/server', 'tsconfig.noeffect.json', T3F),
    ('sentry', W + '/repos/sentry', 'tsconfig.json', F),
    ('vscode', W + '/repos/vscode', 'src/tsconfig.json', F),
]


def sides(cfg, flags):
    rs = f'{RS} -p {cfg} {flags} --pretty false'
    go = f'{GO} -p {cfg} {flags} --pretty false'
    t7 = f'{TSC7} -p {cfg} {flags} --pretty false'
    return [
        ('port-def', rs), ('port-st', rs + ' --singleThreaded'), ('port-fast', rs + ' --fast'),
        ('go-def', go), ('go-st', go + ' --singleThreaded'), ('go-c16', go + ' --checkers 16'),
        ('tsc7-def', t7), ('tsc7-st', t7 + ' --singleThreaded'),
        ('bun', f'{BUN} check -p {cfg} {flags} --no-pretty --all'),
        ('bun-st', f'{BUN} check -p {cfg} {flags} --no-pretty --all --threads=1'),
    ] + [(f'{n}-{m}', f'{b} -p {cfg} {flags} --pretty false' + a) for n, b in PROG for m, a in (('def', ''), ('st', ' --singleThreaded'))]


DIAG = re.compile(rb'(error|warning|message|suggestion) TS[0-9]+')
ENV = {'HOME': os.environ.get('HOME', '/home/theo'), 'PATH': '/usr/local/bin:/usr/bin:/bin', 'LC_ALL': 'C', 'GOPORT_LAUNCH': '0'}


def run(cmd, cwd, cpus, keep):
    full = (['taskset', '-c', cpus] if cpus else []) + cmd.split()
    with tempfile.TemporaryFile() as out:
        t0 = time.perf_counter()
        p = subprocess.Popen(full, cwd=cwd, env=ENV, stdout=out, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL)
        _, status, ru = os.wait4(p.pid, 0)
        wall = time.perf_counter() - t0
        rc = os.waitstatus_to_exitcode(status)
        out.seek(0)
        text = out.read()
    lines = sorted(l for l in text.splitlines() if DIAG.search(l))
    sha = hashlib.sha256(b'\n'.join(lines) + b'\n').hexdigest()
    if keep is not None:
        keep.write_bytes(text)
    return wall, ru.ru_utime, ru.ru_stime, ru.ru_maxrss, rc, len(lines), sha


def main():
    out = Path(sys.argv[1]); rounds = int(sys.argv[2])
    pnames = sys.argv[3].split(','); snames = sys.argv[4].split(',')
    cpus = os.environ.get('CPUS', '0-15')
    (out / 'out').mkdir(parents=True, exist_ok=True)
    with open(out / 'host.txt', 'a') as h:
        h.write(f'date {time.strftime("%FT%T%z")} host {socket.gethostname()} cpus {cpus or "all"} rounds {rounds}\n')
        for k, v in (('RS', RS), ('GO', GO), ('TSC7', TSC7), ('BUN', BUN)):
            h.write(f'{k} {v}\n')
    tsv = open(out / 'runs.tsv', 'a')
    for name, cwd, cfg, flags in PROJECTS:
        if pnames != ['all'] and name not in pnames:
            continue
        vs = [v for v in sides(cfg, flags) if snames == ['all'] or v[0] in snames]
        prev = '-'
        for r in range(-1, rounds):
            order = list(vs)
            random.Random(f'{name}/{r}').shuffle(order)
            for sname, cmd in order:
                keep = out / 'out' / f'{name}.{sname}.txt' if r == -1 else None
                wall, ut, st, rss, rc, n, sha = run(cmd, cwd, cpus, keep)
                tsv.write(f'{name}\t{sname}\t{r}\t{wall:.4f}\t{ut:.3f}\t{st:.3f}\t{rss}\t{rc}\t{n}\t{sha[:12]}\t{prev}\n')
                tsv.flush()
                prev = sname
        print(f'{name} done {time.strftime("%T")}', flush=True)


if __name__ == '__main__':
    main()
