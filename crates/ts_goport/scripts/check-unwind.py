#!/usr/bin/env python3
"""Checks that a tsc still unwinds a caught panic. release.yml runs it on the BOLT copy of the
static musl linux-x64 tsc (bolt-musl.sh). The port catches panics as Go recovers them, so a tsc
that aborts on a panic gives a crash where Go gives an answer.

Usage: check-unwind.py <tsc> [<llvm-dwarfdump>]

1. Two caught panics in the tsc itself, each in a new `tsc --api --async`. The API answers the
   panic as an error response (Go's request recover; the port's go_recover in
   ipc/conn_async.rs). The same process must then answer transpileModule, exit 0 when its stdin
   closes, and write no "libunwind:" line to stderr. A tsc that cannot unwind aborts at the panic.
   a. transpileDeclaration of a .d.ts file. Go's transpile.go asserts that the emit wrote a file
      ("Output generation failed").
   b. With --callbacks readFile:error, createSnapshot of a project. The read of its tsconfig
      panics (Go's callbackfs.go panicIfError) inside the config load. In the BOLT tsc of
      Release run 39967282137, this unwind crosses 3 tsoptions frames that BOLT moved, each with
      a landing pad. The unwind of a. crosses no moved frame there.
2. With <llvm-dwarfdump>: every LSDA (the exception table of a function, named by its FDE in
   .eh_frame) starts with 0xFF (DW_EH_PE_omit: no LPStart). llvm-bolt -split-eh writes LSDAs
   with an LPStart (2,180 in a relmusl1 test tsc). The musl target unwinds with LLVM libunwind,
   which logs "lsda at ... does not start with 0xFF" for such an LSDA. That line is only a log.
   In relmusl1's small test program, a caught panic through such LSDAs aborted ("failed to
   initiate panic"), but the -split-eh test tsc still caught the panic of 1b. So this check is a
   cautious guard: it can reject a tsc that would work, but it does not pass a tsc that has such
   an LSDA. It checks every function, not only the paths of 1.
"""
import json
import re
import signal
import struct
import subprocess
import sys
import tempfile


def fail(message):
    sys.exit(f"check-unwind: {message}")


# api_panic: one run of `tsc --api --async <args>` in <cwd>. <method> must give the caught panic
# <panic>; then transpileModule must answer, the tsc must exit 0, and stderr has no libunwind line.
def api_panic(tsc, cwd, args, method, params, panic):
    signal.alarm(120)
    with tempfile.TemporaryFile() as stderr:
        proc = subprocess.Popen([tsc, "--api", "--async", *args, "--cwd", cwd], stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, stderr=stderr)

        def stderr_text():
            stderr.seek(0)
            return stderr.read().decode(errors="replace")

        def ended(name):
            fail(f"{method}: {name}: the tsc ended (exit {proc.wait()}): {stderr_text()[-400:]!r}")

        # request: one JSON-RPC call (Content-Length framing), and its response.
        def request(id, name, params):
            body = json.dumps({"jsonrpc": "2.0", "id": id, "method": name, "params": params}).encode()
            proc.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
            proc.stdin.flush()
            while True:
                length = None
                while (line := proc.stdout.readline()) != b"\r\n" or length is None:
                    if not line:
                        ended(name)
                    if line.lower().startswith(b"content-length:"):
                        length = int(line.split(b":")[1])
                body = proc.stdout.read(length)
                if len(body) < length:
                    ended(name)
                message = json.loads(body)
                if message.get("id") == id and "method" not in message:
                    return message

        request(1, "initialize", None)
        answer = request(2, method, params)
        error = answer.get("error", {}).get("message", "")
        if not error.startswith("panic: ") or panic not in error:
            fail(f"{method}: no caught panic: {json.dumps(answer)[:400]}")
        answer = request(3, "transpileModule", {"input": "const a = 1;", "options": {}})
        if "const a = 1;" not in answer.get("result", {}).get("outputText", ""):
            fail(f"{method}: transpileModule after the panic: {json.dumps(answer)[:400]}")
        proc.stdin.close()
        code = proc.wait()
        if code != 0:
            fail(f"{method}: the tsc exited {code} after the panic: {stderr_text()[-400:]!r}")
        logs = [line for line in stderr_text().splitlines() if "libunwind:" in line]
        if logs:
            fail(f"{method}: {len(logs)} libunwind lines on stderr, for example {logs[0]!r}")
    signal.alarm(0)
    print(f"{method}: caught panic ({error.splitlines()[0]}); the next request answered, exit 0")


def caught_panics(tsc):
    with tempfile.TemporaryDirectory() as cwd:
        api_panic(tsc, cwd, [], "transpileDeclaration",
                  {"input": "declare const x: number;\n", "options": {"fileName": "a.d.ts"}},
                  "Output generation failed")
        with open(f"{cwd}/tsconfig.json", "w") as f:
            f.write('{"compilerOptions": {"strict": true, "noEmit": true}, "files": ["a.ts"]}\n')
        with open(f"{cwd}/a.ts", "w") as f:
            f.write("export const x: number = 1;\n")
        api_panic(tsc, cwd, ["--callbacks", "readFile:error"], "createSnapshot",
                  {"openProjects": [f"{cwd}/tsconfig.json"]},
                  "filesystem operation configured with serverFS.error: readFile")


def lsda_starts(tsc, dwarfdump):
    data = open(tsc, "rb").read()
    phoff, = struct.unpack_from("<Q", data, 0x20)
    phentsize, phnum = struct.unpack_from("<HH", data, 0x36)
    loads = []
    for i in range(phnum):
        p_type, _, p_offset, p_vaddr, _, p_filesz = struct.unpack_from("<IIQQQQ", data, phoff + i * phentsize)
        if p_type == 1:  # PT_LOAD
            loads.append((p_vaddr, p_offset, p_filesz))
    frames = subprocess.run([dwarfdump, "--eh-frame", tsc], capture_output=True, text=True, check=True).stdout
    lsdas = sorted({int(a, 16) for a in re.findall(r"LSDA Address: ([0-9a-fA-F]+)", frames)})
    if not lsdas:
        fail(f"{dwarfdump} --eh-frame names no LSDA in {tsc}")
    bad = []
    for address in lsdas:
        offset = next((off + address - va for va, off, size in loads if va <= address < va + size), None)
        if offset is None or data[offset] != 0xFF:
            bad.append(address)
    if bad:
        fail(f"{len(bad)} of {len(lsdas)} LSDAs do not start with 0xFF (llvm-bolt -split-eh?), "
             f"for example {', '.join(hex(a) for a in bad[:3])}")
    print(f"LSDAs: all {len(lsdas)} start with 0xFF")


if len(sys.argv) not in (2, 3):
    sys.exit(__doc__)
caught_panics(sys.argv[1])
if len(sys.argv) == 3:
    lsda_starts(sys.argv[1], sys.argv[2])
