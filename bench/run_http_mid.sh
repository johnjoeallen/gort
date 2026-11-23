#!/bin/bash
# gort/http-mid built normally and with --atomic-refs, driven by ApacheBench (keep-alive). usage: run_http_mid.sh [reps]
# The server runs on cores 0-3 (four workers), ab on cores 8-15. Writes results/http_mid.txt and results/http_mid.md.
D=$(dirname "$0"); R=${1:-5}; BIN=${TMPDIR:-/tmp}/http_mid; mkdir -p "$BIN"
GC="$D/../gortc/target/release/gortc"; (cd "$D/../gortc" && cargo build --release -q) || exit 1
export GORTC_OPT=-O2; S="$D/../gort/runtime/*.gort $D/../gort/http-mid/*.gort"; L="$D/../gortc/rt/posix.c"
"$GC" build --link "$L" -o "$BIN/plain" $S || exit 1
"$GC" build --atomic-refs --link "$L" -o "$BIN/atomic" $S || exit 1
OUT="$D/results/http_mid.txt"; : > "$OUT"; PORT=18120
for rep in $(seq "$R"); do
  order="plain atomic"; [ $((rep % 2)) -eq 0 ] && order="atomic plain"
  for b in $order; do
    "$BIN/$b" --port=$PORT --workers=4 >/dev/null 2>&1 & pid=$!; sleep 1
    curl -s -X PUT -d value "http://127.0.0.1:$PORT/kv/a" >/dev/null
    for path in health kv/a; do
      r=$(taskset -c 8-15 timeout 60 ab -k -c 64 -n 300000 -q "http://127.0.0.1:$PORT/$path" 2>/dev/null | awk '/Requests per second/ {print $4}')
      echo "$b /$path ${r:-0}" >> "$OUT"
    done
    kill $pid; wait $pid 2>/dev/null; sleep 0.5
  done
done
python3 - "$OUT" > "$D/results/http_mid.md" <<'PY'
import sys, statistics, collections
d = collections.defaultdict(list)
for l in open(sys.argv[1]):
    b, p, r = l.split(); d[(b, p)].append(float(r))
print("Median requests per second, `ab -k -c 64`, four workers, `-O2`.\n")
print("| path | ref (plain) | --atomic-refs | plain / atomic |\n|---|---|---|---|")
for p in ["/health", "/kv/a"]:
    a = statistics.median(d[("plain", p)]); b = statistics.median(d[("atomic", p)])
    print(f"| {p} | {a:.0f} | {b:.0f} | {a / max(b, 1):.3f} |")
PY
cat "$D/results/http_mid.md"
