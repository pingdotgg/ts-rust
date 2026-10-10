#!/usr/bin/env python3
"""Oracle recorders for an upstream pin. Each writes only under the pin root
(<pinRoot>/<key>/..., see pin.py) and never replaces a file that exists there.
rerecord.sh calls them; they also run alone.

usage: record.py STEP KEY [--jobs N] [--main FILE]
  projects  saved oracle outputs of the gate project checks: oracle/ (Query, Hono, zod, ts-pattern,
            rhf and their .files.txt), errcopies-oracle/, the project-inputs-extra and
            project-inputs-wide oracle files. Same cwd and flags as the recorded runs.
  f1        continuation-r104-conformance-sample results/<id>.oracle.{out,err} (the gate f1 stage) on
            the pin's case files (corpus step). Run it under pin.py exec, as rerecord.sh does.
  typesyms  builds typesymdump-go from the pin checkout (go build -overlay, the checkout is not
            changed) and dumps query, hono and effect into typesyms/go/<name>. The overlay main is
            --main, else typesyms/tools/typesymdump-main-<k>.go of the latest pin k that is not
            newer than KEY (k = KEY first), else typesymdump-main.go. A microsoft/TypeScript pin
            gets a copy with its module path.
  corpus    corpus-full cases, list.json and shards from the pin checkout's test cases (a copy of
            prepare_full.py with the pin commits), and corpus-int3/shards/shard-0.json: the same
            1,500-case sample, matched by source path. Also the f1 sample case files and list.json
            (a copy of the sample's prepare.py with the pin commits; same fixed 512-variant list).
KEY is a pin key from UPSTREAM.json. For the current pin this writes a fresh copy under the pin
root, so the output can be compared with the default caches.

A microsoft/TypeScript pin (layout "typescript") has no TypeScript submodule: the old
_submodules/TypeScript/tests/cases are in testdata/tests/cases, and a case that collided with a Go
case has the new name that testdata/promotedTestCollisions.txt gives. corpus reads only
testdata/tests/cases there and maps the old sample paths (corpus-int3 shard, f1 list) to the new ones.
From #64457 (pin fed0bf24149f) the option declarations are generated: a pin without
internal/tsoptions/declscompiler.go gets a prepare_full.py copy that reads the same two arrays
(commonOptionsWithBuild, optionsForCompiler) from declarations_generated.go.
"""
import argparse
import concurrent.futures
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pin  # noqa: E402

REPO = pin.REPO
T = REPO / 'target'
R = T / 'continuation-r97-goport'
R104 = T / 'continuation-r104-conformance-sample'


OLD_CASES = '_submodules/TypeScript/tests/cases/'


class Pin:
    def __init__(self, key):
        self.cfg = pin.load()
        self.key = pin.resolve(self.cfg, key)
        self.rec = self.cfg['pins'][self.key]
        self.oracle = self.rec['oracle']['path']
        self.checkout = Path(self.rec['goCheckout'])
        self.new_layout = self.rec.get('layout') == 'typescript'
        self.dumper_main = None

    def case_path(self, source):
        """Path of an old-layout test case (Go-root-relative) at this pin."""
        if not self.new_layout or not source.startswith(OLD_CASES):
            return source
        if not hasattr(self, '_renamed'):
            # "renamed-promoted[-by-basename] <old> -> <new>" lines; "identical" cases keep the path.
            text = (self.checkout / 'testdata/promotedTestCollisions.txt').read_text()
            self._renamed = dict(re.findall(r'^renamed-promoted\S* (\S+) -> (\S+)$', text, re.M))
        rel = source[len(OLD_CASES):]
        return 'testdata/tests/cases/' + self._renamed.get(rel, rel)

    def at(self, path):
        """Pin-keyed location of a default path."""
        return pin.cache_src(self.cfg, self.key, Path(path))


def run_all(jobs, tasks):
    """tasks: [(label, fn)]. Runs them in a pool; prints one line each; returns the failures."""
    bad = []
    with concurrent.futures.ThreadPoolExecutor(jobs) as pool:
        futures = {pool.submit(fn): label for label, fn in tasks}
        for f in concurrent.futures.as_completed(futures):
            try:
                print(f'{futures[f]} {f.result()}', flush=True)
            except Exception as e:  # noqa: BLE001 - report and continue
                bad.append(futures[f])
                print(f'{futures[f]} ERROR {e}', flush=True)
    return bad


def oracle_check(p, cwd, config, out, extra=(), timeout=900):
    """`tsgo-oracle -p <config> --noEmit --pretty false` in cwd, stdout and stderr to out."""
    out = Path(out)
    if out.exists():
        return 'exists'
    out.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='rerecord-') as tmp:
        cmd = [p.oracle, '-p', config, *extra]
        if '--listFilesOnly' not in extra:
            cmd += ['--noEmit', '--pretty', 'false', '--tsBuildInfoFile', f'{tmp}/out.tsbuildinfo']
        part = out.with_name(out.name + '.part')
        with part.open('wb') as f:
            rc = subprocess.run(cmd, cwd=cwd, stdout=f, stderr=subprocess.STDOUT, timeout=timeout).returncode
        part.rename(out)
    return f'rc={rc}'


def entries(script):
    """label|project|cwd|config|oracle file rows of sweep-extra2.sh or sweep-wide.sh."""
    return [line.split('|') for line in re.findall(r'^\s*"([^"|]+\|[^"|]+\|[^"|]+\|[^"|]+\|[^"|]+)"', script.read_text(), re.M)]


def step_projects(p, jobs):
    tasks = []
    core = {'query': 'target/project-inputs/query/source/packages/query-core/tsconfig.prod.json',
            'hono': 'target/project-inputs/hono/source/tsconfig.build.json',
            'zod': 'target/project-inputs/zod/source/packages/zod/tsconfig.json',
            'ts-pattern': 'target/project-inputs/ts-pattern/source/tsconfig.json',
            'rhf': 'target/project-inputs/react-hook-form/source/tsconfig.json'}
    for name, config in core.items():
        tasks.append((f'oracle/{name}', lambda n=name, c=config: oracle_check(p, REPO, c, p.at(R / f'oracle/{n}.txt'))))
        tasks.append((f'oracle/{name}.files', lambda n=name, c=config: oracle_check(
            p, REPO, c, p.at(R / f'oracle/{n}.files.txt'), extra=('--listFilesOnly',))))
    for eid in ('Q-E1', 'Q-E2', 'Q-E3', 'Q-E4', 'Q-E5', 'H-E1'):
        config = f'errcopies/{eid}/' + ('packages/query-core/tsconfig.prod.json' if eid[0] == 'Q' else 'tsconfig.build.json')
        tasks.append((f'errcopies/{eid}', lambda c=config, e=eid: oracle_check(p, R, c, p.at(R / f'errcopies-oracle/{e}.txt'))))
    for base, script in ((T / 'project-inputs-extra', T / 'project-inputs-extra/sweep-extra2.sh'),
                         (T / 'project-inputs-wide', REPO / 'scripts/goport/sweep-wide.sh')):
        for label, proj, cwd, config, ofile in entries(script):
            tasks.append((f'{base.name}/{label}', lambda b=base, pr=proj, cw=cwd, c=config, o=ofile: oracle_check(
                p, b / pr / cw, c, p.at(b / pr / o))))
    return run_all(jobs, tasks)


def step_f1(p, jobs):
    listing = json.loads((R104 / 'list.json').read_text())

    def one(case):
        config = R104 / case['tsconfig']
        out = p.at(R104 / 'results' / f"{case['id']}.oracle.out")
        if out.exists():
            return 'exists'
        out.parent.mkdir(parents=True, exist_ok=True)
        err = out.with_suffix('.err')
        # Same command as the sample's run.py: tsgo-oracle -p <tsconfig> --pretty false, 60 s.
        with out.with_suffix('.part').open('wb') as so, err.open('wb') as se:
            try:
                rc = subprocess.run([p.oracle, '-p', config.name, '--pretty', 'false'], cwd=config.parent,
                                    stdout=so, stderr=se, timeout=60).returncode
            except subprocess.TimeoutExpired:
                rc = 'timeout'
        out.with_suffix('.part').rename(out)
        return f'rc={rc}'
    return run_all(jobs, [(f"f1/{c['id']}", lambda c=c: one(c)) for c in listing['cases'] if c['status'] == 'GENERATED'])


TYPESYMS = {'query': (T / 'project-inputs/query/source/packages/query-core', 'tsconfig.prod.json'),
            'hono': (T / 'project-inputs/hono/source', 'tsconfig.build.json'),
            'effect': (T / 'project-inputs/effect/source/packages/effect', 'tsconfig.json')}


def step_typesyms(p, jobs):
    dumper = p.at(R / 'typesyms/typesymdump-go')
    if not dumper.exists():
        dumper.parent.mkdir(parents=True, exist_ok=True)
        # A microsoft/TypeScript pin needs Go 1.27 (tsc/go.mod), as its oracle build.
        go = (p.rec.get('typesymDumper') or {}).get('go') or (p.rec['oracle']['go'] if p.new_layout else 'go1.26.8')
        # Go API changes need a new main (16c25522e123: NewCompilerHost took a contentmapper argument).
        mains = sorted((p.cfg['pins'][k]['date'], k == p.key, f) for f in (R / 'typesyms/tools').glob('typesymdump-main-*.go')
                       if (k := f.stem.rsplit('-', 1)[1]) in p.cfg['pins'] and p.cfg['pins'][k]['date'] <= p.rec['date'])
        main = Path(p.dumper_main) if p.dumper_main else mains[-1][2] if mains else R / 'typesyms/tools/typesymdump-main.go'
        print(f'typesymdump main {main} sha256 {pin.sha256(main)[:12]}, {go}', flush=True)
        with tempfile.TemporaryDirectory() as tmp:
            if p.new_layout:
                # Imports and go:linkname targets name the Go module.
                text = main.read_text().replace('github.com/microsoft/typescript-go/', 'github.com/microsoft/TypeScript/tsc/')
                main = Path(tmp, 'main.go')
                main.write_text(text)
            overlay = Path(tmp, 'overlay.json')
            overlay.write_text(json.dumps({'Replace': {str(p.checkout / 'cmd/typesymdump/main.go'): str(main)}}))
            env = dict(os.environ, GOTOOLCHAIN=go, CGO_ENABLED='0')
            subprocess.run(['go', 'build', '-buildvcs=false', f'-overlay={overlay}', '-o', str(dumper),
                            './cmd/typesymdump'], cwd=p.checkout, env=env, check=True)
        print(f'built {dumper} sha256 {pin.sha256(dumper)}', flush=True)

    def one(name):
        cwd, config = TYPESYMS[name]
        out = p.at(R / 'typesyms/go' / name)
        if out.exists():
            return 'exists'
        part = out.with_name(name + '.part')
        shutil.rmtree(part, ignore_errors=True)
        part.mkdir(parents=True)
        with open(out.with_name(name + '.stderr'), 'wb') as log:
            rc = subprocess.run([str(dumper), '-p', config, '-o', str(part)], cwd=cwd, stdout=log, stderr=log).returncode
        if rc != 0:
            return f'rc={rc} (kept {part})'
        part.rename(out)
        return 'rc=0'
    # One dump at a time: the effect dump is large.
    return run_all(1, [(f'typesyms/{n}', lambda n=n: one(n)) for n in TYPESYMS])


def pinned_copy(src, dest, p, extra=()):
    """Writes a copy of a prepare script with the pin's checkout and commits in place of the old pin's."""
    text = src.read_text()
    pinned = {"go = Path('/home/theo/.explore/repos/microsoft__typescript-go')": f"go = Path({str(p.checkout)!r})",
              "GO_COMMIT = 'dc37b5249ab60e2bbce936f71b883e6c8136167e'": f"GO_COMMIT = {p.rec['commit']!r}",
              "TS_COMMIT = 'c3bd12d888b86f676718b16e64d7d2abcb423514'": f"TS_COMMIT = {p.rec.get('typescriptSubmodule')!r}",
              **dict(extra)}
    for old, new in pinned.items():
        if text.count(old) != 1:
            sys.exit(f'{src.name} changed; cannot pin {old}')
        text = text.replace(old, new)
    dest.write_text(text)


# prepare_full.py option_table at a pin with generated option declarations (#64457): the compiler options are
# the two arrays that declscompiler.go held, so the text is cut to them (the file also declares build and
# typeAcquisition options).
GENERATED_OPTIONS = {
    "    text = (go / 'internal/tsoptions/declscompiler.go').read_text()\n":
        "    text = (go / 'internal/tsoptions/declarations_generated.go').read_text()\n"
        "    text = text[text.index('\\nvar commonOptionsWithBuild = '):]\n"
        "    text = text[:text.index('\\nvar ', text.index('\\nvar optionsForCompiler = ') + 1)]\n",
}


def step_corpus(p, jobs):
    full = p.at(R / 'corpus-full')
    if not (full / 'list.json').exists():
        full.mkdir(parents=True, exist_ok=True)
        extra = {
            ",\n          ('ts', ts / 'tests/cases', 'compiler'), ('ts', ts / 'tests/cases', 'conformance')]": ']',
            "    assert git(ts, 'rev-parse', 'HEAD') == TS_COMMIT and not git(ts, 'status', '--porcelain=v1')\n": '',
        } if p.new_layout else {}
        if not (p.checkout / 'internal/tsoptions/declscompiler.go').exists():
            extra.update(GENERATED_OPTIONS)
        pinned_copy(R / 'corpus-full/prepare_full.py', full / 'prepare_full.py', p, extra)
        for d in (full / 'cases', full / 'shards'):  # pin.py exec makes empty cache dirs; prepare_full wants none
            if d.is_dir() and not any(d.iterdir()):
                d.rmdir()
        subprocess.run([sys.executable, str(full / 'prepare_full.py')], check=True, stdout=subprocess.DEVNULL)
    shard = p.at(R / 'corpus-int3/shards/shard-0.json')
    if not shard.exists():
        old = json.loads((R / 'corpus-int3/shards/shard-0.json').read_text())
        rows = {c['source']: c for c in json.loads((full / 'list.json').read_text())['cases'] if c['status'] == 'GENERATED'}
        keep, seen = [], set()
        for c in old['cases']:
            row = rows.get(p.case_path(c['source']))
            if row and row['id'] not in seen:  # two old paths can name one deduplicated case
                seen.add(row['id'])
                keep.append({'id': row['id'], 'source': row['source'], 'tsconfig': row['tsconfig']})
        shard.parent.mkdir(parents=True, exist_ok=True)
        shard.write_text(json.dumps({**old, 'count': len(keep), 'cases': keep}, indent=1))
        print(f'corpus-int3 shard-0: {len(keep)} of {len(old["cases"])} sampled cases still generated at {p.key}')
    f1 = p.at(R104)
    if not (f1 / 'list.json').exists():
        f1.mkdir(parents=True, exist_ok=True)
        # prepare.py writes cases/ and list.json next to `here`; pin.py exec may have made cases/ empty.
        extra = {"here = root / 'target/continuation-r104-conformance-sample'": f"here = Path({str(f1)!r})",
                 "cases_dir.mkdir()  # Fails if a previous generation exists.":
                     "cases_dir.mkdir(exist_ok=True)\nassert not any(cases_dir.iterdir()), cases_dir"}
        rel = 'tools/ts_fixture/manifests/checker-milestone-v1.json'
        manifest = T / 'worktrees/checker-port' / rel
        if not manifest.exists():
            # A later commit deleted the manifest; prepare.py still checks its sha256.
            gone = subprocess.run(['git', '-C', str(REPO), 'log', '-1', '--format=%H', '--diff-filter=D', '--', f':(top){rel}'],
                                  check=True, capture_output=True, text=True).stdout.strip()
            manifest = f1 / 'checker-milestone-v1.json'
            manifest.write_bytes(subprocess.run(['git', '-C', str(REPO), 'show', f'{gone}^:{rel}'], check=True,
                                                capture_output=True).stdout)
            extra[f"manifest = root / 'target/worktrees/checker-port/{rel}'"] = f"manifest = Path({str(manifest)!r})"
        if p.new_layout:
            # Rows keep the manifest's old case path; the file is read from its path at this pin.
            moved = {v['case']: p.case_path(v['case']) for v in json.loads(manifest.read_text())['variants']}
            moved = {old: new for old, new in moved.items() if old != new}
            extra.update({
                "assert git(go / '_submodules/TypeScript', 'rev-parse', 'HEAD') == TS_COMMIT\n": '',
                "assert not git(go / '_submodules/TypeScript', 'status', '--porcelain=v1')\n": '',
                "    source = go / variant['case']\n": f"    source = go / {moved!r}.get(variant['case'], variant['case'])\n",
            })
        pinned_copy(R104 / 'prepare.py', f1 / 'prepare.py', p, extra)
        subprocess.run([sys.executable, str(f1 / 'prepare.py')], check=True)
    return []


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('step', choices=['projects', 'f1', 'typesyms', 'corpus'])
    ap.add_argument('key')
    ap.add_argument('--jobs', type=int, default=8)
    ap.add_argument('--main', help='typesyms: the typesymdump main.go to overlay')
    args = ap.parse_args()
    p = Pin(args.key)
    p.dumper_main = args.main
    bad = globals()[f'step_{args.step}'](p, args.jobs)
    print(f'{args.step} {p.key}: {"all done" if not bad else f"{len(bad)} failed: " + " ".join(bad[:10])}')
    sys.exit(1 if bad else 0)


if __name__ == '__main__':
    main()
