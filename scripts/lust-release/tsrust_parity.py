#!/usr/bin/env python3
"""Parity of the ts-rust fork's binaries: built by lust vs built by Cargo.

Three checks, all on the fork's own inputs:
  tools  the codegen tools (ts_ast_codegen, ts_diagnostics_codegen) on the checked-in schema,
         catalog JSON and generated files (TOOL_RUNS), run in the fork root: stdout, stderr,
         exit code and the files written to one scratch dir must be byte-equal.
  runs   every bin on every fixture project of crates/ts_goport/tests (each dir with a
         tsconfig.json under fixtures/ and effect_fixtures/): stdout, stderr, exit code and
         every file the run writes must be byte-equal. Each run gets a fresh copy of the
         project at the same path, so paths in the output match, three dirs down in a work
         root that is wiped before each run and compared whole: a project may write outside
         itself (build_watch/coarse-mtime/lib writes ../../../out), and that must neither
         leak into the other side's run nor go unchecked.
  tests  the fork's integration test executables (built by `cargo test --release`) run once
         with the Cargo bins and once with the lust bins in their place. They find the bins
         through CARGO_BIN_EXE_<name>, a path fixed at build time in the cargo target dir,
         so the lust bins are copied over those paths and the Cargo ones put back after.
GOPORT_TRACE is removed from the environment of every run: without the fork's `backtrace`
feature (off by default and in lust dev builds) the bins print a fixed text in place of a
backtrace, and a backtrace's text cannot match Cargo's. The one test that sets GOPORT_TRACE (tsgo_panic_hook, stderr a closed
pipe) checks only the exit status, which is the same either way.

Usage: tsrust_parity.py --fork DIR --cargo-bins DIR --lust-bins DIR --out DIR [--tests DIR]
  --cargo-bins  <cargo target>/release (holds tsgo, goport, ...)
  --lust-bins   dir with the lust builds, named <package>-<bin> (tsrust_full.py writes them)
  --tests       <cargo target>/release/deps (the integration test executables); default from
                --cargo-bins
Last line: "tsrust-parity: <same>/<runs> runs same, <same>/<tests> test bins same, <same>/<tools> tool runs same"
"""
import argparse
import hashlib
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

BINS = ["tsgo", "goport", "goport_emit", "goport_typesyms", "astdump", "goport_build", "goport_multiprog", "goport_live_programs", "goport_watch"]
TESTS = ["early_emit", "emit_pool", "fswatch_linux", "multi_program", "source_map_cut", "tsgo_launch", "tsgo_panic_hook"]


# Codegen tool runs: args relative to the fork root; "{out}" is the scratch dir for written files.
TOOL_RUNS = {
    "ts_ast_codegen": [
        ["emit"],
        ["emit", "tools/ts_ast_codegen/spec/ast.json", "{out}/syntax_kind.rs"],
        ["emit-ast", "{out}/ast_generated.rs"],
        ["check", "crates/ts_goport/src/astdata/syntax_kind.rs"],
        ["check-ast", "crates/ts_goport/src/astdata/ast_generated.rs"],
        ["check-ast", "crates/ts_goport/src/astdata/syntax_kind.rs"],
        [],
    ],
    "ts_diagnostics_codegen": [
        ["--input", "crates/ts_goport/data/effect/effectDiagnosticMessages.json", "--output", "{out}/catalog.rs"],
        ["--input", "tools/ts_ast_codegen/spec/ast.json", "--output", "{out}/catalog.rs"],
        ["--names-output", "{out}/diag.rs", "--output", "{out}/catalog.rs"],
        ["--output"],
        [],
    ],
}


def rustfmt_ok():
    """True when rustfmt runs. ts_ast_codegen pipes its output through rustfmt; without it most
    of its runs fail on both sides and would count as same (tsrust-pr-2, 13:52). Boat boxes have
    only the rustup proxy, so the component is added once when it is missing."""
    def works():
        try:
            return subprocess.run(["rustfmt", "--version"], capture_output=True, timeout=60).returncode == 0
        except (OSError, subprocess.TimeoutExpired):
            return False
    if not works():
        subprocess.run(["rustup", "component", "add", "rustfmt"], capture_output=True, timeout=600)
    return works()


def parity_tools(fork, cargo_bins, lust_bins, out, timeout):
    """TOOL_RUNS with the Cargo and the lust build of each tool, in the fork root. No working
    rustfmt makes every row a failure, never a pass."""
    scratch = out / "parity-tools"
    rows = []
    if not rustfmt_ok():
        return [(f"{tool} {' '.join(args)}", "no rustfmt (rustup component add rustfmt failed)") for tool, runs in TOOL_RUNS.items() for args in runs]
    for tool, runs in TOOL_RUNS.items():
        cb, lb = cargo_bins / tool, lust_bins / f"{tool}-{tool}"
        for args in runs:
            argv = [x.replace("{out}", str(scratch)) for x in args]
            tag = f"{tool} {' '.join(args)}"
            if not lb.exists() or not cb.exists():
                rows.append((tag, "no lust bin" if not lb.exists() else "no cargo bin"))
                continue
            res = []
            for b in (cb, lb):
                shutil.rmtree(scratch, ignore_errors=True)
                scratch.mkdir(parents=True)
                try:
                    r = subprocess.run([str(b)] + argv, cwd=fork, capture_output=True, timeout=timeout)
                    res.append(((r.returncode, r.stdout, r.stderr), tree_digest(scratch)))
                except subprocess.TimeoutExpired:
                    res.append((("hang", b"", b""), []))
            rows.append((tag, compare(res[0], res[1])))
    shutil.rmtree(scratch, ignore_errors=True)
    return rows


def compare(a, b):
    """One row result for two (rc, stdout, stderr), files results: "same" or the first difference."""
    (a, ta), (b, tb) = a, b
    if a == b and ta == tb:
        return "same"
    if a[0] != b[0]:
        return f"exit cargo {a[0]} lust {b[0]}"
    if a[1] != b[1]:
        return "stdout " + first_diff(a[1], b[1])
    if a[2] != b[2]:
        return "stderr " + first_diff(a[2], b[2])
    da, db = dict(ta), dict(tb)
    diff = sorted(k for k in set(da) | set(db) if da.get(k) != db.get(k))
    return f"files differ: {', '.join(diff[:5])}"


def projects(fork):
    base = fork / "crates/ts_goport/tests"
    found = sorted({p.parent for d in ("fixtures", "effect_fixtures") for p in (base / d).rglob("tsconfig.json") if "node_modules" not in p.parts})
    return [(str(p.relative_to(base)), p) for p in found]


def commands(bin_path, case):
    """The command lines run per project. `out` is a dir the run may write, inside the copy."""
    b = str(bin_path)
    name = bin_path.name
    if name.endswith("tsgo"):
        return [
            [b, "-p", "tsconfig.json", "--noEmit", "--pretty", "false"],
            [b, "-p", "tsconfig.json", "--outDir", "out", "--pretty", "false", "--listEmittedFiles"],
            [b, "-p", "tsconfig.json", "--showConfig"],
            [b, "-b", "tsconfig.json", "--pretty", "false", "--verbose"],
        ]
    if name.endswith("goport_emit"):
        return [[b, "-p", "tsconfig.json", "--outDir", "out", "--pretty", "false"]]
    if name.endswith("goport_typesyms") or name.endswith("astdump"):
        return [[b, "-p", "tsconfig.json", "-o", "out"]]
    if name.endswith("goport_build"):
        return [[b, "-b", "tsconfig.json", "--pretty", "false", "--verbose"]]
    if name.endswith("goport_live_programs"):
        return [[b, "live", "out", "check:tsconfig.json", "emit:tsconfig.json"]]
    if name.endswith("goport"):
        return [[b, "-p", "tsconfig.json", "--pretty", "false"]]
    return []  # goport_multiprog, goport_watch: covered by the multi_program test bin


def tree_digest(d):
    """Every file under d (relative path and sha256), sorted: what a run wrote."""
    out = []
    for p in sorted(d.rglob("*")):
        if p.is_file():
            out.append((str(p.relative_to(d)), hashlib.sha256(p.read_bytes()).hexdigest()))
    return out


# `tsc -b --verbose` starts lines with the wall clock time; two runs differ there only.
CLOCK = re.compile(rb"^\d\d:\d\d:\d\d [AP]M - ", re.M)
# Rust 1.99 names the OS thread id in its thread messages (`thread 'main' (4764) panicked`).
TID = re.compile(rb"^(thread '[^']*') \(\d+\)", re.M)


def run_one(cmd, src, root, timeout):
    """One run of cmd in a fresh copy of src at root/a/b/c/case: (rc, stdout, stderr) with the
    clock and thread ids masked, and the digest of every file under root."""
    if root.exists():
        shutil.rmtree(root)
    work = root / "a/b/c/case"
    shutil.copytree(src, work, symlinks=True)
    try:
        r = subprocess.run(cmd, cwd=work, capture_output=True, timeout=timeout)
        res = (r.returncode, CLOCK.sub(b"HH:MM:SS - ", r.stdout), TID.sub(rb"\1 (TID)", r.stderr))
    except subprocess.TimeoutExpired:
        res = ("hang", b"", b"")
    return res, tree_digest(root)


def first_diff(a, b):
    la, lb = a.decode(errors="replace").splitlines(), b.decode(errors="replace").splitlines()
    for i, (x, y) in enumerate(zip(la, lb)):
        if x != y:
            return f"line {i + 1}: cargo {x[:120]!r} | lust {y[:120]!r}"
    return f"cargo {len(la)} lines, lust {len(lb)} lines"


def parity_runs(fork, cargo_bins, lust_bins, out, timeout, bins=BINS, keep=lambda case: True):
    """Every fixture run of each bin in `bins` (default all) on each project `keep` takes:
    Cargo build vs lust build."""
    root = out / "parity-work"  # one path for both sides
    rows = []
    for bin_name in bins:
        cb = cargo_bins / bin_name
        lb = lust_bins / f"ts_goport-{bin_name}"
        for case, src in filter(lambda p: keep(p[0]), projects(fork)):
            for cmd in commands(cb, case):
                tag = f"{bin_name} {' '.join(cmd[1:])} @ {case}"
                if not lb.exists():
                    rows.append((tag, "no lust bin"))
                    continue
                if not cb.exists():
                    rows.append((tag, "no cargo bin"))
                    continue
                rows.append((tag, compare(run_one(cmd, src, root, timeout), run_one([str(lb)] + cmd[1:], src, root, timeout))))
    shutil.rmtree(out / "parity-work", ignore_errors=True)
    return rows


def test_exe(deps, name):
    """The newest integration test executable for one test target (deps/<name>-<hash>)."""
    cands = [p for p in deps.glob(f"{name}-*") if p.is_file() and os.access(p, os.X_OK) and "." not in p.name]
    return max(cands, key=lambda p: p.stat().st_mtime) if cands else None


def run_tests(exe, cwd, timeout):
    try:
        r = subprocess.run([str(exe), "--test-threads", "1"], cwd=cwd, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return "hang", ""
    tail = [ln for ln in r.stdout.splitlines() if ln.startswith("test result:")]
    return r.returncode, (tail[-1] if tail else r.stderr[-300:])


def put_bin(src, dst):
    """Copy a bin over `dst` by rename: writing into a bin that just ran can fail with
    "Text file busy" (ETXTBSY), a rename never does."""
    tmp = dst.with_name(dst.name + ".parity-tmp")
    shutil.copy2(src, tmp)
    os.replace(tmp, dst)


def parity_tests(fork, cargo_bins, lust_bins, deps, timeout, swap=BINS, tests=TESTS):
    """Each test bin with Cargo bins, then with the lust builds of the bins in `swap` (default
    all) at the same paths. The Cargo bins are restored at the end, also on an error."""
    saved = cargo_bins / "parity-saved"
    saved.mkdir(exist_ok=True)
    rows = []
    try:
        for b in BINS:
            if (cargo_bins / b).exists():
                shutil.copy2(cargo_bins / b, saved / b)
        cwd = fork / "crates/ts_goport"  # cargo test runs a test bin in its package dir
        for t in tests:
            exe = test_exe(deps, t)
            if exe is None:
                rows.append((t, "no test bin"))
                continue
            for b in BINS:
                put_bin(saved / b, cargo_bins / b)
            a = run_tests(exe, cwd, timeout)
            missing = [b for b in swap if not (lust_bins / f"ts_goport-{b}").exists()]
            if missing:
                rows.append((t, f"no lust bin for {', '.join(missing)} (cargo bins: {a[1]})"))
                continue
            for b in swap:
                put_bin(lust_bins / f"ts_goport-{b}", cargo_bins / b)
            l = run_tests(exe, cwd, timeout)
            rows.append((t, "same" if a[0] == l[0] == 0 else f"cargo: {a[0]} {a[1]} | lust: {l[0]} {l[1]}"))
    finally:
        for b in BINS:
            if (saved / b).exists():
                put_bin(saved / b, cargo_bins / b)
        shutil.rmtree(saved, ignore_errors=True)
    return rows


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--fork", required=True, type=Path)
    ap.add_argument("--cargo-bins", required=True, type=Path)
    ap.add_argument("--lust-bins", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--tests", type=Path)
    ap.add_argument("--timeout", type=int, default=300)
    a = ap.parse_args()
    os.environ.pop("GOPORT_TRACE", None)  # the debug path that differs on purpose (docstring)
    fork, cb, lb, out = a.fork.resolve(), a.cargo_bins.resolve(), a.lust_bins.resolve(), a.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    if not cb.exists():
        sys.exit(f"tsrust-parity: no Cargo bins at {cb}")
    tools = parity_tools(fork, cb, lb, out, a.timeout)
    runs = parity_runs(fork, cb, lb, out, a.timeout)
    tests = parity_tests(fork, cb, lb, a.tests or cb / "deps", a.timeout * 4)
    with open(out / "parity.txt", "w") as f:
        for tag, res in tools + runs + tests:
            f.write(f"{res}\t{tag}\n")
    bad = [(t, r) for t, r in tools + runs + tests if r != "same" and not r.startswith("no lust bin")]
    nobin = sum(r.startswith("no lust bin") for _, r in tools + runs)
    if nobin:
        print(f"tsrust-parity: {nobin} runs have no lust bin yet")
    for t, r in bad[:40]:
        print(f"tsrust-parity: DIFF {t}: {r}")
    rs = sum(r == "same" for _, r in runs)
    ts = sum(r == "same" for _, r in tests)
    tl = sum(r == "same" for _, r in tools)
    print(f"tsrust-parity: {rs}/{len(runs)} runs same, {ts}/{len(tests)} test bins same, {tl}/{len(tools)} tool runs same"
          f" (all rows: {out / 'parity.txt'})")


if __name__ == "__main__":
    main()
