#!/bin/bash
# D17/D2: Refs held for good. After a cache hit, stash=PERMILLE of the hits keep their cloned Ref for the rest of the run, so the objects stay pinned. Flush scenario, split, 16 locks.
D=$(dirname "$0"); N=${1:-300000}
gcc -O2 -DSPLIT=1 -o "$D/c/service_split_1" "$D/c/service_split_bench.c" -lpthread || exit 1
OUT="$D/results/stash.txt"; : > "$OUT"
for rep in 1 2 3; do for st in 0 1 10 100; do for mv in 0 1; do
  taskset -c 2-7 "$D/c/service_split_1" 50 200 256 4 $N flush=1 shards=16 clone=1 stash=$st mover=$mv >> "$OUT" || echo "FAILED $st $mv" >> "$OUT"
done; done; done
python3 - "$OUT" > "$D/results/stash.tables.md" <<'PY'
import sys, statistics as st
from collections import defaultdict
runs = defaultdict(list)
for line in open(sys.argv[1]):
    if not line.startswith("RUN"): continue
    kv = dict(p.split("=", 1) for p in line.split()[1:]); runs[(kv["stash"], kv["mover"])].append({k: float(v) for k, v in kv.items() if k not in ("layout", "csum")})
med = lambda k, f: st.median(r[f] for r in runs[k])
print("| Hits that keep their Ref for good | Mover | Resident at end (MB) | Mapped (MB) | Mover attempts | Chunks freed | Skipped (pinned) | Skip share | Work per request (ns) |\n|---|---|---|---|---|---|---|---|---|")
for sx in ("0", "1", "10", "100"):
    for mv in ("0", "1"):
        k = (sx, mv)
        if k not in runs: continue
        g = lambda f: med(k, f)
        print(f"| {int(sx)/10:g}% | {'on' if mv=='1' else 'off'} | {g('rss_end_mb'):.0f} | {g('mapped_end_mb'):.0f} | " + (f"{g('attempts'):,.0f} | {g('chunks_freed'):,.0f} | {g('pinned_skips'):,.0f} | {100*g('pinned_skips')/max(1,g('attempts')):.0f}% | " if mv=="1" else "- | - | - | - | ") + f"{g('busy_ns_per_req'):.0f} |")
PY
