import sys, statistics as st
from collections import defaultdict
runs = defaultdict(list)
for line in open(sys.argv[1]):
    if not line.startswith("RUN"): continue
    kv = dict(p.split("=", 1) for p in line.split()[1:])
    key = (kv["flush"], kv["hold"], kv["layout"], kv["clone"], kv["mover"])
    runs[key].append({k: (float(v) if k not in ("layout", "csum") else v) for k, v in kv.items()})
def med(k, f): return st.median(r[f] for r in runs[k])
print("## Steady state: 50% hits, 200 us database, 4 workers, 16 locks, a 500 us hold after every hit\n")
ks = [("combined, copy held", ("0","500","combined","0","0")), ("combined, Ref held", ("0","500","combined","1","0")), ("split, copy held", ("0","500","split","0","0")), ("split, Ref held", ("0","500","split","1","0"))]
cs = {r["csum"] for _, k in ks for r in runs[k]}
print(f"Identical responses in all four configurations: {'yes' if len(cs)==1 else 'NO'}.\n")
print("| Configuration | Work per request (ns) | Lock retries per request | Requests/s | p50 (us) | p99 (us) | Resident at end (MB) |\n|---|---|---|---|---|---|---|")
for n, k in ks: print(f"| {n} | {med(k,'busy_ns_per_req'):.0f} | {med(k,'spins_per_req'):.2f} | {med(k,'rps'):,.0f} | {med(k,'p50_us'):.0f} | {med(k,'p99_us'):.0f} | {med(k,'rss_end_mb'):.0f} |")
print("\n## After a flush, by hold time (50% hits, 200 us database, 4 workers, 16 locks, median of 3)\n")
hs = sorted({k[1] for k in runs if k[0]=="1"}, key=int)
print("| Hold (us) | Configuration | Resident at end (MB) | Mapped (MB) | Work per request (ns) | Requests/s | Mover attempts | Chunks freed | Skipped (pinned) | Skip share | Bodies moved | Longest hold (us) |\n|---|---|---|---|---|---|---|---|---|---|---|---|")
for h in hs:
    for n, l, cl, mv in (("combined, Ref held", "combined", "1", "0"), ("split, copy held, no mover", "split", "0", "0"), ("split, copy held, mover", "split", "0", "1"), ("split, Ref held, no mover", "split", "1", "0"), ("split, Ref held, mover", "split", "1", "1")):
        k = ("1", h, l, cl, mv)
        if k not in runs: continue
        g = lambda f: med(k, f)
        m = mv == "1"
        print(f"| {h} | {n} | {g('rss_end_mb'):.0f} | {g('mapped_end_mb'):.0f} | {g('busy_ns_per_req'):.0f} | {g('rps'):,.0f} | "
              + (f"{g('attempts'):,.0f} | {g('chunks_freed'):,.0f} | {g('pinned_skips'):,.0f} | {100*g('pinned_skips')/max(1,g('attempts')):.0f}% | {g('moved'):,.0f} | {g('max_hold_us'):.0f} |" if m else "- | - | - | - | - | - |"))
