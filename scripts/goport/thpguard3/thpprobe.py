"""thpguard3 micro probe: the kernel cost of first touch of anonymous memory, huge pages against 4 KiB pages,
back to back and after an idle gap. No project input or binary needed (for CI runners).

usage: python3 -I thpprobe.py [MiB] [gap seconds] [rounds]
Each sample runs in a child process: mmap MiB anonymous, madvise HUGEPAGE or NOHUGEPAGE, write one byte per
4 KiB page with 4 threads (as a 4 checker run faults in parallel), and report wall and the child's sys time.
Order per round: huge, small (back to back), then sleep gap, huge, sleep gap, small.
"""
import mmap
import os
import resource
import sys
import threading
import time

def touch(mib, huge):
    """Maps `mib` MiB, advises, touches it with 4 threads, returns wall seconds."""
    size = mib << 20
    m = mmap.mmap(-1, size, flags=mmap.MAP_PRIVATE | mmap.MAP_ANONYMOUS)
    m.madvise(mmap.MADV_HUGEPAGE if huge else mmap.MADV_NOHUGEPAGE)
    mv = memoryview(m)
    t0 = time.perf_counter()
    parts = 4
    step = size // parts
    def run(i):
        v = mv[i * step:(i + 1) * step]
        for off in range(0, step, 4096):
            v[off] = 1
    ts = [threading.Thread(target=run, args=(i,)) for i in range(parts)]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    wall = time.perf_counter() - t0
    mv.release()
    return wall

def sample(mib, huge):
    """Runs `touch` in a child, returns (wall, child sys, child minflt)."""
    r, w = os.pipe()
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    pid = os.fork()
    if pid == 0:
        os.close(r)
        wall = touch(mib, huge)
        os.write(w, f"{wall}".encode())
        os._exit(0)
    os.close(w)
    data = os.read(r, 64)
    os.close(r)
    os.waitpid(pid, 0)
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    return float(data), after.ru_stime - before.ru_stime, after.ru_minflt - before.ru_minflt

def vm(key):
    with open("/proc/vmstat") as f:
        for line in f:
            k, v = line.split()
            if k == key:
                return int(v)
    return 0

def main():
    mib = int(sys.argv[1]) if len(sys.argv) > 1 else 512
    gap = float(sys.argv[2]) if len(sys.argv) > 2 else 10
    rounds = int(sys.argv[3]) if len(sys.argv) > 3 else 3
    print(f"thpprobe mib={mib} gap={gap}s rounds={rounds}")
    for rnd in range(1, rounds + 1):
        for label, huge, wait in (("b2b-huge", True, 0), ("b2b-small", False, 0), ("b2b-huge2", True, 0),
                                  ("gap-huge", True, gap), ("gap-small", False, gap)):
            if wait:
                time.sleep(wait)
            thp0 = vm("thp_fault_alloc")
            wall, stime, flt = sample(mib, huge)
            thp = vm("thp_fault_alloc") - thp0
            print(f"round={rnd} case={label} wall={wall:.3f} sys={stime:.3f} minflt={flt} thp_faults={thp}", flush=True)

main()
