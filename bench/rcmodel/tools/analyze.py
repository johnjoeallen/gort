#!/usr/bin/env python3 -I
"""raw.csv -> summary.csv + tables.md (tables, bar charts, pre-registered verdict, gap decomposition).
usage: analyze.py <results_dir>"""
import csv, os, statistics as st, sys
from collections import defaultdict
R = sys.argv[1]
rows = list(csv.DictReader(open(os.path.join(R, "raw.csv"))))
G = defaultdict(list)
for r in rows:
    try: G[(r["workload"], r["impl"], r["variant"], r["params"], r["metric"])].append(float(r["value"]))
    except ValueError: pass
def stat(k):
    v = G.get(k)
    return None if not v else (st.median(v), min(v), max(v), len(v))
with open(os.path.join(R, "summary.csv"), "w", newline="") as f:
    w = csv.writer(f); w.writerow(["workload", "impl", "variant", "params", "metric", "n", "median", "min", "max"])
    for k in sorted(G): s = stat(k); w.writerow(list(k) + [s[3], s[0], s[1], s[2]])

def fmt(s, scale=1.0, d=3):
    if s is None: return "–"
    f = lambda x: f"{x*scale:.{d}g}"
    return f"{f(s[0])} ({f(s[1])}–{f(s[2])})"
def med(k, scale=1.0): s = stat(k); return None if s is None else s[0] * scale
def P(**kw): return ";".join(f"{k}={v}" for k, v in kw.items())
out = []
def table(head, body):
    out.append("| " + " | ".join(head) + " |"); out.append("|" + "|".join(["---"] * len(head)) + "|")
    for b in body: out.append("| " + " | ".join(str(x) for x in b) + " |")
    out.append("")
def bars(items, unit, width=44, lower_better=False):
    items = [(n, v) for n, v in items if v is not None]
    if not items: return
    m = max(v for _, v in items); out.append("```")
    for n, v in items: out.append(f"{n:<26} {'█' * max(1, int(width * v / m))} {v:.3g} {unit}")
    out.append("```"); out.append("")

CV = ["C0-raw", "C1-local", "C2-flagged", "C3-atomic", "C4-elided"]
impls = lambda w, metric: sorted({(k[1], k[2]) for k in G if k[0] == w and k[4] == metric})
def label(i, v): return f"{v} ({i})" if i.startswith("c-") else v

# ---------------------------------------------------------------- W1
out.append("## W1 pass-down micro: ns per pass (median (min–max) over runs)\n")
cols = [1, 4, 16]
body = []
for i, v in impls("W1", "ns_per_pass"):
    if "shared=1" in "".join(k[3] for k in G if k[1] == i and k[2] == v and k[0] == "W1" and "shared=1" in k[3]) and False: pass
    for sh in (0, 1):
        cells = [fmt(stat(("W1", i, v, P(depth=d, shared=sh), "ns_per_pass"))) for d in cols]
        if all(c == "–" for c in cells): continue
        body.append([label(i, v) + (" **shared flag set (atomic path)**" if sh else "")] + cells)
table(["variant", "depth 1", "depth 4", "depth 16"], body)
bars([(label(i, v), med(("W1", i, v, P(depth=16, shared=0), "ns_per_pass"))) for i, v in impls("W1", "ns_per_pass")], "ns/pass @depth16")

# ---------------------------------------------------------------- W2
out.append("## W2 shared-table read contention: total Mops/s (median (min–max)); 95/5 read/write, 10k entries\n")
Ts = [1, 2, 4, 8, 16]
for mode in ["uniform", "hot_same", "hot_diff"]:
    out.append(f"### mode = {mode}\n")
    body = []
    for i, v in impls("W2", "mops_total"):
        if i == "c-O3": continue
        body.append([label(i, v)] + [fmt(stat(("W2", i, v, P(threads=T, mode=mode), "mops_total"))) for T in Ts])
    table(["variant"] + [f"{T} thr" for T in Ts], body)
    bars([(label(i, v), med(("W2", i, v, P(threads=16, mode=mode), "mops_total"))) for i, v in impls("W2", "mops_total") if i != "c-O3"], "Mops/s @16 thr")
out.append("Per-op slowdown of hot_same vs hot_diff at 16 threads (H4 asks >= 10x for C2/C3):\n")
body = []
for i, v in impls("W2", "mops_total"):
    a, b = med(("W2", i, v, P(threads=16, mode="hot_diff"), "mops_total")), med(("W2", i, v, P(threads=16, mode="hot_same"), "mops_total"))
    if a and b: body.append([label(i, v), f"{a/b:.1f}x"])
table(["variant", "hot_diff / hot_same throughput"], body)

# ---------------------------------------------------------------- W3
out.append("## W3 request path (5M requests, CPU only): throughput, latency, RSS\n")
def w3k(i, v, T, tb="shared", work=1, m="rps"): return ("W3", i, v, f"workers={T};table={tb};work={work}", m)
allv = [(i, v) for i, v in impls("W3", "rps")]
for T in [1, 4, 8]:
    out.append(f"### {T} worker(s), shared session table\n")
    body = []
    goref = med(w3k("go", "Go-gc", T))
    for i, v in allv:
        s = stat(w3k(i, v, T))
        if s is None: continue
        body.append([label(i, v), fmt(s, 1e-6), f"{goref/s[0]:.2f}" if goref else "–", fmt(stat(w3k(i, v, T, m="p50_ns"))), fmt(stat(w3k(i, v, T, m="p99_ns"))), fmt(stat(w3k(i, v, T, m="rss_mb")), d=3)])
    table(["variant", "Mreq/s", "Go ÷ this (R)", "p50 ns", "p99 ns", "peak RSS MB"], body)
    bars([(label(i, v), med(w3k(i, v, T), 1e-6)) for i, v in allv], f"Mreq/s @{T}w")
out.append("### C variants with private session tables (no sharing; used for the gap decomposition)\n")
body = []
for i, v in allv:
    if not i.startswith("c-"): continue
    row = [label(i, v)]
    for T in [1, 4, 8]: row.append(fmt(stat(w3k(i, v, T, "private")), 1e-6))
    if any(c != "–" for c in row[1:]): body.append(row)
table(["variant", "1 w Mreq/s", "4 w", "8 w"], body)
out.append("### Sensitivity: heavier per-request work (work=8: body/response hash loops x8), 8 workers, shared table\n")
body = []
for i, v in allv:
    s = stat(w3k(i, v, 8, work=8))
    if s: body.append([label(i, v), fmt(s, 1e-6), f"{med(w3k('go','Go-gc',8,work=8))/s[0]:.2f}" if med(w3k('go','Go-gc',8,work=8)) else "–"])
table(["variant", "Mreq/s", "Go ÷ this"], body)
body = []
for i, v in allv:
    s = med(w3k(i, v, 1, m="incs_per_req"))
    if s: body.append([label(i, v), f"{s:.1f}", f"{med(w3k(i,v,1,m='atomic_incs_per_req')):.1f}", f"{med(w3k(i,v,1,m='decs_per_req')):.1f}", f"{med(w3k(i,v,1,m='allocs_per_req') or 0):.1f}"])
body or None
if body:
    out.append("### Executed counting operations per request (COUNT build, 1 worker)\n"); table(["variant", "incs", "of which atomic", "decs", "mallocs"], body)

# ---------------------------------------------------------------- verdict
out.append("## Pre-registered verdict (W3, 8 workers, shared table)\n")
def pick(opt):
    return {v: med(w3k(f"c-{opt}", v, 8)) for v in CV}
verdict = []
for opt in ["O2", "O3"]:
    c = pick(opt); go = med(w3k("go", "Go-gc", 8))
    if not go or not c.get("C2-flagged"): continue
    R2 = go / c["C2-flagged"]; R4 = go / c["C4-elided"] if c.get("C4-elided") else None
    if R2 <= 1.5: v = "**C2 within 1.5x of Go: model is viable. Continue.**"
    elif R2 <= 2.0: v = ("**C2 is 1.5-2x off Go; C4 within 1.3x: viable only if elisions are realistic.**" if R4 and R4 <= 1.3 else "**C2 is 1.5-2x off Go and C4 is NOT within 1.3x: not viable by the threshold.**")
    else: v = "**C2 slower than 2x; " + ("C4 still slower than 1.5x: confirmed not viable.**" if R4 and R4 > 1.5 else "C4 is within 1.5x of Go, so 'confirmed not viable' does not apply; read the C4 elisions.**")
    verdict.append([opt, f"{go/1e6:.2f}", f"{c['C2-flagged']/1e6:.2f}", f"{R2:.2f}", f"{R4:.2f}" if R4 else "–", v])
table(["C opt", "Go Mreq/s", "C2 Mreq/s", "R = Go/C2", "Go/C4", "outcome"], verdict)
def ratio(a, b): return f"{a/b:.2f}" if a and b else "–"
c2 = med(w3k("c-O2", "C2-flagged", 8)); c0 = med(w3k("c-O2", "C0-raw", 8)); c4 = med(w3k("c-O2", "C4-elided", 8))
table(["comparison at 8 workers (c-O2)", "ratio"], [
    ["C2 ÷ Go (throughput)", ratio(c2, med(w3k("go", "Go-gc", 8)))],
    ["C2 ÷ Rust pessimal (Arc per pass)", ratio(c2, med(w3k("rust", "rust-arc", 8)))],
    ["C2 ÷ Rust idiomatic", ratio(c2, med(w3k("rust", "rust-idiomatic", 8)))],
    ["C2 ÷ Java G1", ratio(c2, med(w3k("java", "Java-g1", 8)))],
    ["C2 ÷ Java ZGC", ratio(c2, med(w3k("java", "Java-zgc", 8)))],
    ["C2 ÷ C0 raw", ratio(c2, c0)], ["C4 ÷ C0 raw", ratio(c4, c0)], ["C3 ÷ C2", ratio(med(w3k("c-O2", "C3-atomic", 8)), c2)]])

# ---------------------------------------------------------------- gap split
out.append("## Decomposing the C2 vs C0 gap in W3 (ns per request per core = workers / rps)\n")
body = []
for T in [1, 8]:
    def cost(v, tb): m = med(w3k("c-O2", v, T, tb)); return T / m * 1e9 if m else None
    c0s, c0p, c1p, c2p, c2s = cost("C0-raw", "shared"), cost("C0-raw", "private"), cost("C1-local", "private"), cost("C2-flagged", "private"), cost("C2-flagged", "shared")
    if None in (c0s, c0p, c1p, c2p, c2s): continue
    gap = c2s - c0s; traffic = c1p - c0p; flag = c2p - c1p; sharing = c0s - c0p; atom = (c2s - c2p) - sharing
    body.append([T, f"{c0s:.0f}", f"{c2s:.0f}", f"{gap:.0f}", f"{traffic:.0f} ({100*traffic/gap:.0f}%)" if gap else "–", f"{flag:.0f} ({100*flag/gap:.0f}%)" if gap else "–", f"{atom:.0f} ({100*atom/gap:.0f}%)" if gap else "–", f"{sharing:.0f}"])
table(["workers", "C0 ns/req", "C2 ns/req", "gap", "inc/dec traffic (C1p-C0p)", "flag test (C2p-C1p)", "atomics on shared objs ((C2s-C2p)-(C0s-C0p))", "table sharing cost in raw (C0s-C0p, not Gort-specific)"], body)

# ---------------------------------------------------------------- W4
out.append("## W4 graph alloc/free: 1M-node 4-ary tree, ns per node (median over runs of the median of cycles 2-6)\n")
for T in [1, 8]:
    for wk in [0, 1]:
        out.append(f"### {T} thread(s), weak back-references = {wk}\n")
        body = []
        for i, v in impls("W4", "cycle_ns_per_node"):
            if i == "c-O3": continue
            k = lambda m: ("W4", i, v, f"threads={T};weak={wk}", m)
            if stat(k("cycle_ns_per_node")): body.append([label(i, v)] + [fmt(stat(k(m))) for m in ["build_ns_per_node", "traverse_ns_per_node", "free_ns_per_node", "cycle_ns_per_node", "cpu_ns_per_node_cycle", "rss_mb"]])
        table(["variant", "build", "traverse (passes Refs)", "free (cascade)", "cycle wall", "cpu ns/node/cycle", "RSS MB"], body)
        bars([(label(i, v), med(("W4", i, v, f"threads={T};weak={wk}", "cycle_ns_per_node"))) for i, v in impls("W4", "cycle_ns_per_node") if i != "c-O3"], "ns/node/cycle (wall)")

# ---------------------------------------------------------------- W5
if any(k[0] == "W5" for k in G):
    out.append("## W5 loopback HTTP/1.1 (server on cores 0-7, oha + idle holder on cores 8-15)\n")
    for active, idle in [(10, 0), (100, 0), (1000, 0), (100, 10000)]:
        out.append(f"### {active} active connections, {idle} idle\n"); body = []
        for i, v in impls("W5", "rps"):
            k = lambda m: ("W5", i, v, f"active={active};idle={idle}", m)
            if stat(k("rps")): body.append([label(i, v), fmt(stat(k("rps")), 1e-3), fmt(stat(k("p50_us"))), fmt(stat(k("p99_us"))), fmt(stat(k("rss_mb_peak"))), fmt(stat(k("rss_per_idle_conn_bytes"))) if idle else "–", fmt(stat(k("success_rate")), 100)])
        table(["server", "kreq/s", "p50 µs", "p99 µs", "server peak RSS MB", "RSS bytes / idle conn", "success %"], body)
        bars([(label(i, v), med(("W5", i, v, f"active={active};idle={idle}", "rps"), 1e-3)) for i, v in impls("W5", "rps")], "kreq/s")

open(os.path.join(R, "tables.md"), "w").write("\n".join(out))
print("wrote", os.path.join(R, "tables.md"))
