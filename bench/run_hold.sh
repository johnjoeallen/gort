#!/bin/bash
# D17/D2: a Ref held across a park. The mixed service with 16 cache locks; after a cache hit the request parks for hold=US microseconds before rendering.
# clone=1 holds the cloned Ref across the park (the object is pinned); clone=0 holds a copy (the control). Flush scenario with the mover, and a steady-state check.
# Writes results/hold.txt and results/hold.tables.md.
D=$(dirname "$0"); N=${1:-300000}
for m in 0 1; do gcc -O2 -DSPLIT=$m -o "$D/c/service_split_$m" "$D/c/service_split_bench.c" -lpthread || exit 1; done
OUT="$D/results/hold.txt"; : > "$OUT"
run() { taskset -c 2-7 "$D/c/service_split_$1" "${@:2}" >> "$OUT" || echo "FAILED $*" >> "$OUT"; }
for rep in 1 2 3 4 5; do
  for v in "0 clone=0" "0 clone=1" "1 clone=0" "1 clone=1"; do set -- $v; l=$1; shift; run $l 50 200 256 4 $N shards=16 hold=500 "$@"; done
done
for H in 100 500 2000 10000; do
  for rep in 1 2 3; do
    for v in "0 clone=1 mover=0" "1 clone=0 mover=0" "1 clone=0 mover=1" "1 clone=1 mover=0" "1 clone=1 mover=1"; do
      set -- $v; l=$1; shift; run $l 50 200 256 4 $N flush=1 shards=16 hold=$H "$@"
    done
  done
done
python3 "$D/parse_hold.py" "$OUT" > "$D/results/hold.tables.md"
