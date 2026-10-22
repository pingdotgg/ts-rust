#!/usr/bin/env python3
"""The tsgo fixture runs as digests, to compare a lust-built tsc with a Cargo tsc.

make <fork> <cargo-tsgo> <goldens.json> [<work>]: run the Cargo tsc on every
fixture command (tsrust_parity.projects/commands/run_one) and record a digest
of each run. check <fork> <lust-tsgo> <goldens.json> [<work>]: run the lust tsc
the same way and compare the digests. Both bins need the lib files next to them
(noembed). Run make and check with the same <work> layout: some fixture outputs
hold paths relative to the work dir. Used by .github/workflows/lust-release.yml."""
import hashlib
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import tsrust_parity as par

def one(fork, bin_path, work, idx_cmd):
    # tsc prints its lib dir (the bin's dir) and the project work root
    # in some outputs; mask both so goldens made on one machine (or at
    # one work path, or in parallel) compare anywhere. Written files
    # are hashed by the parity tree digest as-is: their paths in the
    # digest are relative, and no fixture writes an absolute path into
    # a file (checked: parallel and serial digests agree, 216/216).
    idx, (case, src, cmd) = idx_cmd
    root = work / str(idx)
    (rc, so, se), tree = par.run_one(cmd, src, root, 600)
    for token in (str(bin_path.parent).encode(), str(root).encode()):
        so, se = so.replace(token, b"@P@"), se.replace(token, b"@P@")
    tag = f"{case} :: {' '.join(cmd[1:])}"
    return tag, hashlib.sha256(repr(((rc, so, se), tree)).encode()).hexdigest()[:16]


def digests(fork, bin_path, work):
    tasks = []
    for case, src in par.projects(fork):
        for cmd in par.commands(bin_path, case):
            tasks.append((case, src, cmd))
    jobs = int(os.environ.get("GOLDENS_JOBS", "8"))
    if jobs <= 1:
        return dict(one(fork, bin_path, work, t) for t in enumerate(tasks))
    from concurrent.futures import ThreadPoolExecutor
    with ThreadPoolExecutor(max_workers=jobs) as ex:
        return dict(ex.map(lambda t: one(fork, bin_path, work, t), enumerate(tasks)))

mode, fork, bin_path, gold = sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3]), Path(sys.argv[4])
work = Path(sys.argv[5]) if len(sys.argv) > 5 else Path("/tmp/gold-work")
got = digests(fork, bin_path, work)
if mode == "make":
    gold.write_text(json.dumps(got, indent=0, sort_keys=True))
    print(f"goldens: {len(got)} runs recorded")
else:
    want = json.loads(gold.read_text())
    bad = [t for t in sorted(set(want) | set(got)) if want.get(t) != got.get(t)]
    for t in bad:
        print(f"DIFF {t}")
    print(f"verify: {len(want) - len(bad)}/{len(want)} same, ran {len(got)}")
    if not got:
        print("verify: NO RUNS (bin name must end in tsgo for the parity command table)")
    sys.exit(1 if bad else 0)
