#!/usr/bin/env python3
"""Checks that a tsc still unwinds a caught panic. release.yml runs it on the BOLT copy of the
static musl linux-x64 tsc (bolt-musl.sh). The port catches panics as Go recovers them, so a tsc
that aborts on a panic gives a crash where Go gives an answer.

Usage: check-unwind.py <tsc> [<llvm-dwarfdump>]

1. A caught panic in the tsc itself: `tsc --api --async` gets transpileDeclaration of a .d.ts
   file. Go's transpile.go asserts that the emit wrote a file ("Output generation failed"), and
   the API answers the panic as an error response (Go's request recover; the port's go_recover in
   ipc/conn_async.rs). The same process must then answer transpileModule, and exit 0 when its
   stdin closes. A tsc that cannot unwind aborts at the panic.
2. With <llvm-dwarfdump>: every LSDA (the exception table of a function, named by its FDE in
   .eh_frame) starts with 0xFF (DW_EH_PE_omit: no LPStart). The musl target unwinds with LLVM
   libunwind, which aborts on any other first byte ("lsda at ... does not start with 0xFF").
   llvm-bolt -split-eh writes such tables (2,180 in a relmusl1 test bin), so a panic through one
   of those functions aborts. This checks every function, not only the path of 1.
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


def caught_panic(tsc):
    signal.alarm(120)
    with tempfile.TemporaryDirectory() as cwd:
        proc = subprocess.Popen([tsc, "--api", "--async", "--cwd", cwd], stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE)

        def ended(method):
            fail(f"{method}: the tsc ended (exit {proc.wait()}): {proc.stderr.read()[-400:]!r}")

        # request: one JSON-RPC call (Content-Length framing), and its response.
        def request(id, method, params):
            body = json.dumps({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).encode()
            proc.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
            proc.stdin.flush()
            while True:
                length = None
                while (line := proc.stdout.readline()) != b"\r\n" or length is None:
                    if not line:
                        ended(method)
                    if line.lower().startswith(b"content-length:"):
                        length = int(line.split(b":")[1])
                body = proc.stdout.read(length)
                if len(body) < length:
                    ended(method)
                message = json.loads(body)
                if message.get("id") == id:
                    return message

        request(1, "initialize", None)
        answer = request(2, "transpileDeclaration",
                         {"input": "declare const x: number;\n", "options": {"fileName": "a.d.ts"}})
        error = answer.get("error", {}).get("message", "")
        if not error.startswith("panic: ") or "Output generation failed" not in error:
            fail(f"transpileDeclaration of a .d.ts: no caught panic: {json.dumps(answer)[:400]}")
        answer = request(3, "transpileModule", {"input": "const a = 1;", "options": {}})
        if "const a = 1;" not in answer.get("result", {}).get("outputText", ""):
            fail(f"transpileModule after the panic: {json.dumps(answer)[:400]}")
        proc.stdin.close()
        code = proc.wait()
        if code != 0:
            fail(f"the tsc exited {code} after the panic: {proc.stderr.read()[-400:]!r}")
    signal.alarm(0)
    print(f"caught panic: {error.splitlines()[0]}; the next request answered, exit 0")


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
caught_panic(sys.argv[1])
if len(sys.argv) == 3:
    lsda_starts(sys.argv[1], sys.argv[2])
