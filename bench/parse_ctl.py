#!/usr/bin/env python3
"""Summarises results/ctl.txt (B18): median of the repetitions per configuration, as Markdown tables."""
import sys, re, statistics
from collections import defaultdict

rows = defaultdict(list)
for line in open(sys.argv[1]):
    if not line.startswith("RESULT"):
        continue
    kv = dict(p.split("=", 1) for p in line.split()[1:])
    key = (kv["work"], kv["layout"], int(kv["body"]), int(kv["atomic"]), int(kv["n"]), kv.get("keep_bp", ""))
    rows[key].append({k: v for k, v in kv.items()})

def med(key, field):
    vals = [float(r[field]) for r in rows.get(key, []) if field in r]
    return statistics.median(vals) if vals else None

def rng(key, field):
    vals = [float(r[field]) for r in rows.get(key, []) if field in r]
    return (min(vals), max(vals)) if vals else None

L = ["H", "D", "C", "B"]
def f(v, d=2):
    return "-" if v is None else f"{v:.{d}f}"
def pct(v, base):
    if v is None or base is None or base == 0:
        return ""
    return f" ({(v / base - 1) * 100:+.0f}%)"

def table(title, header, keyfn, fields, ns, bodies, atomic=0, digits=2):
    print(f"\n### {title}\n")
    print("| " + " | ".join(header) + " |")
    print("|" + "---|" * len(header))
    for n in ns:
        for b in bodies:
            for fld, label in fields:
                cells = []
                base = med(keyfn("H", b, atomic, n), fld)
                for l in L:
                    v = med(keyfn(l, b, atomic, n), fld)
                    cells.append(f(v, digits) + (pct(v, base) if l != "H" else ""))
                print(f"| {n} | {b} B | {label} | " + " | ".join(cells) + " |")

def k(work):
    return lambda l, b, a, n: (work, l, b, a, n, "")

print("# B18 tables (median of the runs; percentages are against H, the header in the slot)")
hdr = ["objects", "body", "metric"] + L
acc = [("seq_ns", "read, sequential"), ("random_ns", "read, random"), ("chase_ns", "read, pointer chase"),
       ("cseq_ns", "count+read, sequential"), ("crandom_ns", "count+read, random"), ("cchase_ns", "count+read, chase"),
       ("count_only_ns", "count up/down only, random")]
table("Access, ns per operation, plain counts (64 B body)", hdr, k("access"), acc, [2000, 100000, 1500000, 6000000], [64])
table("Access, ns per operation, plain counts, bodies of 48 and 72 B (index arithmetic by reciprocal multiply)", hdr, k("access"), acc, [100000, 1500000], [48, 72])
table("Access, ns per operation, atomic counts (64 B body)", hdr, k("access"), acc, [2000, 100000, 1500000, 6000000], [64], atomic=1)
al = [("fill_ns", "allocate"), ("free_random_ns", "free, random order"), ("churn_pair_ns", "free + allocate")]
table("Allocation, ns per object", hdr, k("alloc"), al, [1500000], [16, 64, 256])

print("\n### Resident memory per object, all live (2M objects; 1M for 256 B)\n")
print("| body | " + " | ".join(L) + " |\n|---|---|---|---|---|")
for b in [16, 48, 64, 72, 256]:
    n = 1000000 if b == 256 else 2000000
    print(f"| {b} B | " + " | ".join(f(med(("mem", l, b, 0, 2000000 if b != 256 else 2000000, ""), "bytes_per_object"), 1) for l in L) + " |")

print("\n### Dead objects held by a WeakRef (resident MB)\n")
print("| body | state | " + " | ".join(L) + " |\n|---|---|---|---|---|---|")
for b, n in [(64, 2000000), (256, 1000000)]:
    for fld, lab in [("rss_full_kb", "all live"), ("rss_dead_with_weak_kb", "all dead, weak refs held"), ("rss_after_weak_dropped_kb", "weak refs dropped")]:
        print(f"| {b} B | {lab} | " + " | ".join(f(None if med(('weak', l, b, 0, n, ''), fld) is None else med(('weak', l, b, 0, n, ''), fld) / 1024, 1) for l in L) + " |")

print("\n### Scattered survivors, no mover (resident MB; 64 B x 2M, 256 B x 1M)\n")
print("| body | kept | state | " + " | ".join(L) + " |\n|---|---|---|---|---|---|")
for b, n, keep in [(64, 2000000, "100"), (64, 2000000, "1000"), (256, 1000000, "100")]:
    for fld, lab in [("rss_full_kb", "all live"), ("rss_after_free_kb", "after freeing the rest"), ("rss_after_pagerelease_kb", "after releasing free pages")]:
        print(f"| {b} B | {int(keep)/100:g}% | {lab} | " + " | ".join(f(None if med(('retain', l, b, 0, n, keep), fld) is None else med(('retain', l, b, 0, n, keep), fld) / 1024, 1) for l in L) + " |")
