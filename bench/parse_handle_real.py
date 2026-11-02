#!/usr/bin/env python3
"""Turn handle_real output (lines: R size lock B W mode ns) into overhead tables against the raw-pointer variant."""
import sys, collections
d = collections.defaultdict(dict)
for line in open(sys.argv[1]):
    t = line.split()
    if t and t[0] == 'R':
        _, size, lock, B, W, mode, ns = t
        d[(size, lock, int(B))][(int(W), mode)] = float(ns)
modes = ['raw2', 'rl16', 'rlelem', 'rlacc']
Ws = sorted({w for v in d.values() for (w, _) in v})
print("Each cell: overhead over `raw` in ns per element (percent). `raw2` is the same code as `raw`: its column is the noise floor.\n")
for (size, lock, B), v in sorted(d.items(), key=lambda kv: (['4KB','256KB','64MB'].index(kv[0][0]), ['none','spin','mutex'].index(kv[0][1]), kv[0][2])):
    print(f"### array {size}, lock {lock}, batch {B}\n")
    print("| W | raw ns/elem | " + " | ".join(modes) + " |")
    print("|---|---|" + "---|" * len(modes))
    for w in Ws:
        r = v[(w, 'raw')]
        cells = []
        for m in modes:
            x = v[(w, m)] - r
            cells.append(f"{x:+.3f} ({100*x/r:+.1f}%)")
        print(f"| {w} | {r:.3f} | " + " | ".join(cells) + " |")
    print()
# summary: worst and median overhead per mode over cells whose raw time is >= a given per-element cost
print("## Summary over all configurations\n")
print("| mode | noise-floor-corrected overhead, median % | worst % | worst ns |")
print("|---|---|---|---|")
for m in modes:
    pcts, nss = [], []
    for v in d.values():
        for w in Ws:
            r = v[(w, 'raw')]; x = v[(w, m)] - r
            pcts.append(100 * x / r); nss.append(x)
    pcts.sort()
    print(f"| {m} | {pcts[len(pcts)//2]:+.1f} | {max(pcts):+.1f} | {max(nss):+.3f} |")
