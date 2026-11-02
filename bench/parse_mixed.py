#!/usr/bin/env python3
"""Summarise mixed.txt: median over repetitions per scenario and variant, loss against mode 0 (raw pointer)."""
import sys, re, collections, statistics as st
runs = collections.defaultdict(list)
for line in open(sys.argv[1]):
    if not line.startswith('SC '): continue
    t = line.split(); sc = ' '.join(t[1:5]); mode = t[5]
    kv = dict(x.split('=') for x in t[6:] if '=' in x)
    runs[(sc, mode)].append({k: float(v) for k, v in kv.items()})
names = {'0': 'raw pointer', '0b': 'raw pointer, second copy (noise)', '1': 'handle', '2': 'handle, reload every access'}
scs = []
for (sc, m) in runs:
    if sc not in scs: scs.append(sc)
def med(sc, m, k): return st.median(r[k] for r in runs[(sc, m)])
def spread(sc, m, k):
    v = [r[k] for r in runs[(sc, m)]]; return (max(v) - min(v)) / st.median(v) * 100
for sc in scs:
    hit, db, conc, wk = sc.split()
    print(f"### cache target {hit}%, database wait {db} us, {conc} requests in flight per worker, {wk} worker(s)\n")
    print(f"Actual cache hit rate {med(sc,'0','hit_actual'):.0f}%.\n")
    print("| Variant | Work per request (ns) | Loss vs raw | Requests/s | p50 latency (us) | p99 latency (us) |")
    print("|---|---|---|---|---|---|")
    b = med(sc, '0', 'busy_ns_per_req'); r0 = med(sc, '0', 'rps')
    for m in ['0', '0b', '1', '2']:
        x = med(sc, m, 'busy_ns_per_req'); loss = '(baseline)' if m == '0' else f"{x-b:+.1f} ns ({100*(x-b)/b:+.1f}%)"
        print(f"| {names[m]} | {x:.0f} | {loss} | {med(sc,m,'rps'):,.0f} | {med(sc,m,'p50_us'):.1f} | {med(sc,m,'p99_us'):.1f} |")
    print(f"\nRun-to-run spread of the raw pointer's work per request: {spread(sc,'0','busy_ns_per_req'):.1f}% (min to max over {len(runs[(sc,'0')])} runs).\n")
