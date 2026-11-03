import sys, statistics as st
from collections import defaultdict
runs = defaultdict(list)
for line in open(sys.argv[1]):
    if not line.startswith("RUN"): continue
    kv = dict(p.split("=", 1) for p in line.split()[1:])
    key = (kv["hit"], kv["db_us"], kv["conc"], kv["workers"], kv["flush"], kv["skew"], kv["layout"], kv["shards"], kv["clone"], kv["mover"])
    runs[key].append({k: (float(v) if k not in ("layout", "csum") else v) for k, v in kv.items()})
def med(k, f): return st.median(r[f] for r in runs[k])
def spread(k, f): v = [r[f] for r in runs[k]]; return (max(v) - min(v)) / st.median(v) * 100
scs = []
for k in runs:
    if k[:6] not in scs: scs.append(k[:6])
cfgs = [("combined", "1", "0", "combined, 1 lock, copy under lock (previous run)"), ("combined", "16", "0", "combined, 16 locks, copy under lock"), ("combined", "16", "1", "combined, 16 locks, clone Ref on hit"),
        ("split", "1", "0", "split, 1 lock, copy under lock (previous run)"), ("split", "16", "0", "split, 16 locks, copy under lock"), ("split", "16", "1", "split, 16 locks, clone Ref on hit")]
print("## Steady state (no flush, no mover)\n")
for s in [x for x in scs if x[4] == "0"]:
    hit, db, conc, wk, _, sk = s
    ks = [(n, s + (l, sh, cl, "0")) for l, sh, cl, n in cfgs if s + (l, sh, cl, "0") in runs]
    cs = {r["csum"] for _, k in ks for r in runs[k]}
    print(f"### cache target {hit}%, database wait {db} us, {conc} in flight per worker, {wk} worker(s)" + (f", {sk}% of requests on the 1000 hottest users" if sk != "0" else "") + "\n")
    print(f"Actual hit rate {med(ks[0][1],'hit_actual'):.0f}%. All {len(ks)} configurations serve identical responses (one checksum): {'yes' if len(cs)==1 else 'NO'}.\n")
    print("| Configuration | Work per request (ns) | vs combined, 1 lock | Lock retries per request | Requests/s | p50 (us) | p99 (us) | p99.9 (us) | Resident at end (MB) | Spread |")
    print("|---|---|---|---|---|---|---|---|---|---|")
    b = med(ks[0][1], "busy_ns_per_req")
    for n, k in ks:
        x = med(k, "busy_ns_per_req"); d = "(baseline)" if k == ks[0][1] else f"{x-b:+.0f} ns ({100*(x-b)/b:+.1f}%)"
        print(f"| {n} | {x:.0f} | {d} | {med(k,'spins_per_req'):.2f} | {med(k,'rps'):,.0f} | {med(k,'p50_us'):.1f} | {med(k,'p99_us'):.1f} | {med(k,'p999_us'):.1f} | {med(k,'rss_end_mb'):.0f} | {spread(k,'busy_ns_per_req'):.0f}% |")
    print()
print("## After a flush (90% evicted at random, then a hot 1/20 of the users), 16 locks\n")
for s in [x for x in scs if x[4] == "1"]:
    hit, db, conc, wk, _, sk = s
    print(f"### cache target {hit}%, database wait {db} us, {conc} in flight per worker, {wk} worker(s)\n")
    print("| Configuration | Resident before flush (MB) | after flush | at end | Mapped arenas at end (MB) | Work per request (ns) | Requests/s | p99 (us) | p99.9 (us) | Objects moved | Chunks freed | Chunks skipped (pinned) | Mover CPU (ms) | Longest hold, all locks (us) |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    for l, cl, mv, n in (("combined", "1", "0", "combined, clone on hit"), ("split", "0", "0", "split, copy under lock, no mover"), ("split", "0", "1", "split, copy under lock, mover"),
                         ("split", "1", "0", "split, clone on hit, no mover"), ("split", "1", "1", "split, clone on hit, mover")):
        k = s + (l, "16", cl, mv)
        if k not in runs: continue
        g = lambda f: med(k, f)
        print(f"| {n} | {g('rss_preflush_mb'):.0f} | {g('rss_postflush_mb'):.0f} | {g('rss_end_mb'):.0f} | {g('mapped_end_mb'):.0f} | {g('busy_ns_per_req'):.0f} | {g('rps'):,.0f} | {g('p99_us'):.1f} | {g('p999_us'):.1f} | "
              + (f"{g('moved'):,.0f} | {g('chunks_freed'):,.0f} | {g('pinned_skips'):,.0f} | {g('mover_cpu_ms'):.0f} | {g('max_hold_us'):.0f} |" if mv == "1" else "- | - | - | - | - |"))
    print()
