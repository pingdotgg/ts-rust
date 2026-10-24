"""thpguard3: tables from runs.tsv.

usage: python3 -I table.py <runs.tsv> [--rows]
Per phase, project and config: run count, median and range of wall and sys, median minor faults, the
guard decision counts. b2b drops run 1 of each block (it follows another config).
"""
import csv
import statistics
import sys

def med(xs):
    return statistics.median(xs) if xs else float("nan")

def main():
    path = sys.argv[1]
    rows = list(csv.DictReader(open(path), delimiter="\t"))
    # measure.sh wrote the /proc/vmstat deltas in /proc/vmstat order: the columns named thp_alloc,
    # thp_fallback and compact_stall hold the deltas of pgfault (system wide), compact_stall and thp_fault_alloc.
    for r in rows:
        r["pgfault_sys"], r["compact"], r["thp"] = r["thp_alloc"], r["thp_fallback"], r["compact_stall"]
    if "--summary" in sys.argv:
        return summary(rows)
    if "--paired" in sys.argv:
        return paired(rows)
    if "--rows" in sys.argv:
        for r in rows:
            print(r["phase"], r["round"], r["project"], r["config"], r["wall"], r["sys"], r["minflt"], r["free2m_mib"], "thp", r["thp"], "compact", r["compact"], r["guard"][:90])
        return
    groups = {}
    for r in rows:
        if r["phase"] == "warm" or (r["phase"] == "b2b" and r["round"] == "1"):
            continue
        groups.setdefault((r["phase"], r["project"], r["config"]), []).append(r)
    print("| phase | project | config | n | wall med (min-max) | sys med (min-max) | minflt med | guard |")
    print("|---|---|---|---|---|---|---|---|")
    order = {"b2b": 0, "gap": 1, "idle": 2}
    porder = {"valibot": 0, "query": 1, "hono": 2, "zod": 3, "effect": 4}
    corder = {"unset": 0, "force": 1, "forcel": 2, "go": 3}
    for key in sorted(groups, key=lambda k: (order[k[0]], porder.get(k[1], 9), corder.get(k[2], 9))):
        g = groups[key]
        wall = [float(r["wall"]) for r in g]
        sys_ = [float(r["sys"]) for r in g]
        flt = [int(r["minflt"]) for r in g]
        guards = {}
        for r in g:
            first = r["guard"].split(";")[0]
            if key[2] == "go":
                d = "-"
            elif "watcher" in r["guard"] and "THP off by the watcher" in r["guard"]:
                d = "kept,watcher fired"
            elif first.startswith("THP kept"):
                d = "kept"
            elif first.startswith("THP off") or first.startswith("THP already off"):
                d = "off"
            else:
                d = "?"
            guards[d] = guards.get(d, 0) + 1
        gs = " ".join(f"{k} {v}" for k, v in sorted(guards.items()))
        print(f"| {key[0]} | {key[1]} | {key[2]} | {len(g)} | {med(wall):.2f} ({min(wall):.2f}-{max(wall):.2f}) | "
              f"{med(sys_):.2f} ({min(sys_):.2f}-{max(sys_):.2f}) | {med(flt):,.0f} | {gs} |")

def paired(rows):
    """gap and idle phases: per project the per-round ratio forcel/unset of wall (runs next to each other),
    and the sum of each config's walls over all rounds and projects."""
    for phase in ("gap", "idle"):
        by = {}
        for r in rows:
            if r["phase"] == phase:
                by.setdefault((r["project"], r["round"]), {})[r["config"]] = float(r["wall"])
        if not by:
            continue
        for other in ("forcel", "force"):
            tot = {}
            allr = []
            lines = []
            for proj in ("valibot", "query", "hono", "zod", "effect"):
                rs = []
                for (p, rnd), m in sorted(by.items()):
                    if p == proj and "unset" in m and other in m:
                        rs.append(m[other] / m["unset"])
                        for c in ("unset", other):
                            tot[c] = tot.get(c, 0) + m[c]
                if rs:
                    allr += rs
                    lines.append(f"  {proj}: " + " ".join(f"{x:.2f}" for x in rs) + f"  median {med(rs):.2f}")
            if not allr:
                continue
            print(f"\n{phase}: {other}/unset wall per round")
            print("\n".join(lines))
            print(f"  all {len(allr)} pairs: median {med(allr):.3f}, geomean {statistics.geometric_mean(allr):.3f}, "
                  f"{other} faster in {sum(1 for x in allr if x < 1)}")
            print("  wall sums: " + ", ".join(f"{c} {w:.2f}" for c, w in sorted(tot.items())))

def summary(rows):
    """Per phase: per project the median wall and sys of each config, and the geomean wall ratios."""
    cfgs = ["unset", "force", "forcel", "go"]
    for phase in ("b2b", "gap"):
        data = {}
        for r in rows:
            if r["phase"] != phase or (phase == "b2b" and r["round"] == "1"):
                continue
            data.setdefault(r["project"], {}).setdefault(r["config"], []).append(r)
        print(f"\n{phase}: median wall s (sys s)")
        print("| project | " + " | ".join(cfgs) + " | forcel/unset wall | force/unset sys |")
        print("|---|" + "---|" * (len(cfgs) + 2))
        ratios, sysr = [], []
        for proj in ("valibot", "query", "hono", "zod", "effect"):
            if proj not in data:
                continue
            cells = []
            m = {}
            for c in cfgs:
                g = data[proj].get(c, [])
                if not g:
                    cells.append("-")
                    continue
                w = med([float(r["wall"]) for r in g])
                sy = med([float(r["sys"]) for r in g])
                m[c] = (w, sy)
                cells.append(f"{w:.2f} ({sy:.2f})" if c != "forcel" else f"{w:.2f}")
            rw = m["forcel"][0] / m["unset"][0]
            rs = m["force"][1] / m["unset"][1] if m["unset"][1] else float("nan")
            ratios.append(rw)
            sysr.append(rs)
            print(f"| {proj} | " + " | ".join(cells) + f" | {rw:.2f} | {rs:.2f} |")
        gm = statistics.geometric_mean(ratios)
        print(f"geomean forcel/unset wall {gm:.3f}")

main()
