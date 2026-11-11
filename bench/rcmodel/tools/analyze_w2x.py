#!/usr/bin/env python3
"""Tables for results_w2x/raw.csv: comparative W2 hot path, median (min-max) Mops/s."""
import csv, sys, statistics
from collections import defaultdict

ORDER = [  # (label, impl column, variant column, ppct)
    ("C spin lock, counted (Gort today)", "c-O2", "spin", "0"),
    ("C lock-free, counted (QSBR)", "c-O2", "qsbr_clone", "0"),
    ("C borrow, QSBR", "c-O2", "qsbr_borrow", "0"),
    ("C borrow, hazard ptr (L4')", "c-O2", "hp_asym", "0"),
    ("C borrow, hazard ptr, 100% promoted", "c-O2", "hp_asym", "100"),
    ("Go RWMutex stripes", "go", "rwmutex", "0"),
    ("Go atomic.Pointer", "go", "atomic", "0"),
    ("Java G1 ConcurrentHashMap", "java-g1", "chm", "0"),
    ("Java ZGC ConcurrentHashMap", "java-zgc", "chm", "0"),
    ("Java G1 AtomicReferenceArray", "java-g1", "ara", "0"),
    ("Java ZGC AtomicReferenceArray", "java-zgc", "ara", "0"),
    ("Rust RwLock<Arc> stripes", "rust", "rwlock", "0"),
    ("Rust arc-swap", "rust", "arcswap", "0"),
    ("Rust crossbeam-epoch", "rust", "epoch", "0"),
]
data = defaultdict(list)
for row in csv.DictReader(open(sys.argv[1])):
    p = dict(kv.split("=") for kv in row["params"].split(";"))
    k = (row["impl"], row["variant"], p.get("ppct", "0"), p["mode"], p["threads"], p.get("hash", "0"), row["metric"])
    data[k].append(float(row["value"]))

def med(impl, var, pp, mode, t, h, metric="mops_total"):
    v = data.get((impl, var, pp, mode, str(t), str(h), metric))
    return statistics.median(v) if v else None

def cell(impl, var, pp, mode, t, h, metric="mops_total"):
    v = data.get((impl, var, pp, mode, str(t), str(h), metric))
    if not v: return "-"
    m = statistics.median(v)
    f = "{:.2f}" if m < 10 else "{:.1f}" if m < 100 else "{:.0f}"
    return f.format(m) if len(v) == 1 else f"{f.format(m)} ({f.format(min(v))}-{f.format(max(v))})"

def table(title, cols, fn):
    print(f"\n### {title}\n")
    print("| | " + " | ".join(c for c, _ in cols) + " |")
    print("|---|" + "---|" * len(cols))
    for lab, im, va, pp in ORDER:
        print(f"| {lab} | " + " | ".join(fn(im, va, pp, c) for _, c in cols) + " |")

print("# Comparative W2 hot path: Mops/s total, median (min-max)")
for h, desc in [(0, "no work"), (1500, "~1 us real work per read"), (15000, "~10 us real work per read")]:
    table(f"hot_same, hash={h} ({desc})", [(f"T={t}", t) for t in [1, 4, 8, 16]],
          lambda im, va, pp, t, h=h: cell(im, va, pp, "hot_same", t, h))
# efficiency vs ideal with work: ideal = T / (single-thread time per op) of the same implementation
for h in [1500, 15000]:
    def eff(im, va, pp, t, h=h):
        one, m = med(im, va, pp, "hot_same", 1, h), med(im, va, pp, "hot_same", t, h)
        return "-" if not one or not m else f"{100 * m / (t * one):.0f}%"
    table(f"hot_same, hash={h}: scaling efficiency vs own single thread (100% = perfect)", [(f"T={t}", t) for t in [4, 8, 16]], eff)
table("hot_diff reference, T=16, no work", [("Mops/s", 16)], lambda im, va, pp, t: cell(im, va, pp, "hot_diff", t, 0))
table("peak RSS MB, hot_same T=16", [("hash=0", 0), ("hash=15000", 15000)],
      lambda im, va, pp, h: cell(im, va, pp, "hot_same", 16, h, "rss_mb"))
