#!/usr/bin/env python3
"""Tables for results_w2sync/raw.csv: median (min-max) over runs, one table per sweep."""
import csv, sys, statistics
from collections import defaultdict

SCHEMES = ["spin", "rw", "qsbr_clone", "qsbr_borrow", "hp", "hp_asym"]
LABEL = {"spin": "L0 spin", "rw": "L1 rw", "qsbr_clone": "L2 qsbr+clone", "qsbr_borrow": "L3 qsbr+borrow",
         "hp": "L4 hp", "hp_asym": "L4' hp+membarrier"}
DEF = {"wpct": "5", "ppct": "0", "work": "0", "q": "16", "split": "0", "stall_ms": "0"}

data = defaultdict(list)   # (scheme, frozenset params, metric) -> values
for row in csv.DictReader(open(sys.argv[1])):
    p = dict(kv.split("=") for kv in row["params"].split(";"))
    data[(row["variant"], tuple(sorted(p.items())), row["metric"])].append(float(row["value"]))

def key(scheme, metric, **kw):
    p = dict(DEF); p.update({k: str(v) for k, v in kw.items()})
    return (scheme, tuple(sorted(p.items())), metric)

def cell(scheme, metric="mops_total", fmt="{:.1f}", **kw):
    v = data.get(key(scheme, metric, **kw))
    if not v: return "-"
    m = statistics.median(v)
    return fmt.format(m) if len(v) == 1 else f"{fmt.format(m)} ({fmt.format(min(v))}-{fmt.format(max(v))})"

def table(title, cols, rows):
    print(f"\n### {title}\n")
    print("| | " + " | ".join(c for c, _ in cols) + " |")
    print("|---|" + "---|" * len(cols))
    for label, f in rows:
        print(f"| {label} | " + " | ".join(f(c) for _, c in cols) + " |")

print("# W2 sync schemes: Mops/s total, median (min-max)")
for mode in ["hot_same", "uniform", "hot_diff"]:
    table(f"main, mode={mode}, wpct=5", [(f"T={t}", t) for t in [1, 2, 4, 8, 16]],
          [(LABEL[s], lambda t, s=s: cell(s, threads=t, mode=mode)) for s in SCHEMES])
for sp in ["0", "1"]:
    table(f"write ratio, hot_same, T=16, split={sp} ({'count on data line' if sp == '0' else 'count on its own line'})",
          [(f"wpct={w}", w) for w in [0, 1, 5, 20]],
          [(LABEL[s], lambda w, s=s: cell(s, threads=16, mode="hot_same", wpct=w, split=sp)) for s in SCHEMES])
for mode in ["hot_same", "uniform"]:
    table(f"promotion rate, {mode}, T=16", [(f"ppct={p}", p) for p in [0, 10, 50, 100]],
          [(LABEL[s], lambda p, s=s: cell(s, threads=16, mode=mode, ppct=p)) for s in ["qsbr_borrow", "hp", "hp_asym"]])
rows = [(LABEL[s], lambda w, s=s: cell(s, threads=16, mode="hot_same", work=w)) for s in ["spin", "rw", "qsbr_clone"]]
for s in ["qsbr_borrow", "hp_asym"]:
    for p in [0, 100]:
        rows.append((f"{LABEL[s]} ppct={p}", lambda w, s=s, p=p: cell(s, threads=16, mode="hot_same", work=w, ppct=p)))
table("handler work after the read, hot_same, T=16 (work=1000 ~ 1 us)", [("work=1000", 1000), ("work=10000", 10000)], rows)
for metric, fmt in [("mops_total", "{:.1f}"), ("garbage_max_objs", "{:.0f}"), ("rss_mb", "{:.0f}")]:
    table(f"stalled reader, hot_same, T=16: {metric}", [(f"stall_ms={s}", s) for s in [0, 10, 100]],
          [(LABEL[s], lambda st, s=s: cell(s, metric, fmt, threads=16, mode="hot_same", stall_ms=st)) for s in ["qsbr_borrow", "hp", "hp_asym"]])
table("QSBR quiescent interval, hot_same, T=16", [(f"q={q}", q) for q in [1, 4, 16, 256]],
      [(LABEL[s], lambda q, s=s: cell(s, threads=16, mode="hot_same", q=q)) for s in ["qsbr_clone", "qsbr_borrow"]])
for metric, fmt in [("mops_total", "{:.1f}"), ("garbage_max_objs", "{:.0f}")]:
    rows = [(f"{LABEL[s]} {m}", lambda t, s=s, m=m: cell(s, metric, fmt, threads=16, mode=m, hpt=t))
            for m in ["hot_same", "hot_diff"] for s in ["hp", "hp_asym"]]
    table(f"hazard-pointer scan threshold, T=16: {metric}", [(f"hpt={t}", t) for t in [96, 1024, 8192]], rows)
leaks = sum(v for (s, p, m), vs in data.items() if m == "leaked_objs" for v in vs)
print(f"\nleaked objects over all runs: {leaks:.0f}")
