import sys, statistics as st
from collections import defaultdict
runs = defaultdict(list)
for line in open(sys.argv[1]):
    if not line.startswith("RUN"): continue
    kv = dict(p.split("=", 1) for p in line.split()[1:])
    key = (kv["hit"], kv["db_us"], kv["conc"], kv["workers"], kv["flush"], kv["layout"], kv["mover"])
    runs[key].append({k: (float(v) if k not in ("layout", "csum") else v) for k, v in kv.items()})
def med(k, f): return st.median(r[f] for r in runs[k])
def spread(k, f): v = [r[f] for r in runs[k]]; return (max(v) - min(v)) / st.median(v) * 100
scs = []
for k in runs:
    s = k[:5]
    if s not in scs: scs.append(s)
print("## Steady state (no flush, no mover)\n")
for s in [x for x in scs if x[4] == "0"]:
    hit, db, conc, wk, _ = s
    c, p = s + ("combined", "0"), s + ("split", "0")
    if c not in runs or p not in runs: continue
    same = {r["csum"] for r in runs[c]} == {r["csum"] for r in runs[p]} and len({r["csum"] for r in runs[c]}) == 1
    print(f"### cache target {hit}%, database wait {db} us, {conc} requests in flight per worker, {wk} worker(s)\n")
    print(f"Actual hit rate {med(c,'hit_actual'):.0f}%. Same checksums in both layouts: {'yes' if same else 'NO'}.\n")
    print("| Layout | Work per request (ns) | Difference | Cache-lock retries per request | Requests/s | p50 (us) | p99 (us) | p99.9 (us) | Resident at end (MB) |")
    print("|---|---|---|---|---|---|---|---|---|")
    b = med(c, "busy_ns_per_req")
    for name, k in (("combined", c), ("split", p)):
        x = med(k, "busy_ns_per_req"); d = "(baseline)" if k == c else f"{x-b:+.0f} ns ({100*(x-b)/b:+.1f}%)"
        print(f"| {name} | {x:.0f} | {d} | {med(k,'spins_per_req'):.2f} | {med(k,'rps'):,.0f} | {med(k,'p50_us'):.1f} | {med(k,'p99_us'):.1f} | {med(k,'p999_us'):.1f} | {med(k,'rss_end_mb'):.0f} |")
    print(f"\nRun-to-run spread of the combined layout's work per request: {spread(c,'busy_ns_per_req'):.1f}% (min to max over {len(runs[c])} runs); of the split layout's: {spread(p,'busy_ns_per_req'):.1f}%.\n")
print("## After a flush: 90% of the cache evicted at random, then traffic on a hot 1/20 of the users\n")
for s in [x for x in scs if x[4] == "1"]:
    hit, db, conc, wk, _ = s
    print(f"### cache target {hit}%, database wait {db} us, {conc} requests in flight per worker, {wk} worker(s)\n")
    print("| Layout | Resident before flush (MB) | after flush | at end | Mapped arenas at end (MB) | Work per request (ns) | Requests/s | p99 (us) | p99.9 (us) | Objects moved | Chunks freed | Mover CPU (ms) | Longest cache-lock hold by the mover (us) |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    for name, k in (("combined", s + ("combined", "0")), ("split, no mover", s + ("split", "0")), ("split, mover running", s + ("split", "1"))):
        if k not in runs: continue
        g = lambda f: med(k, f)
        mv = k[6] == "1"
        print(f"| {name} | {g('rss_preflush_mb'):.0f} | {g('rss_postflush_mb'):.0f} | {g('rss_end_mb'):.0f} | {g('mapped_end_mb'):.0f} | {g('busy_ns_per_req'):.0f} | {g('rps'):,.0f} | {g('p99_us'):.1f} | {g('p999_us'):.1f} | "
              + (f"{g('moved'):,.0f} | {g('chunks_freed'):,.0f} | {g('mover_cpu_ms'):.0f} | {g('max_hold_us'):.0f} |" if mv else "- | - | - | - |"))
    print()
