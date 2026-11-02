#!/bin/bash
# D17 whole-service benchmark. Builds combined and split, runs steady-state scenarios (layouts alternate order) and the flush scenario
# (combined, split, split with the mover). Writes results/service_split.txt and results/service_split.tables.md.
D=$(dirname "$0"); R=${1:-5}; N=${2:-300000}
for m in 0 1; do gcc -O2 -DSPLIT=$m -o "$D/c/service_split_$m" "$D/c/service_split_bench.c" -lpthread || exit 1; done
OUT="$D/results/service_split.txt"; : > "$OUT"
run() { taskset -c 2-7 "$D/c/service_split_$1" "${@:2}" >> "$OUT" || echo "FAILED $*" >> "$OUT"; }
for sc in "50 200 256 1" "20 500 128 1" "100 100 256 1" "100 10 256 1" "50 200 256 4" "20 500 128 4" "100 10 256 4"; do
  for rep in $(seq $R); do
    if [ $((rep % 2)) -eq 1 ]; then o="0 1"; else o="1 0"; fi
    for m in $o; do run $m $sc $N; done
  done
done
for sc in "50 200 256 4" "50 200 256 1"; do
  for rep in 1 2 3; do
    for v in "0 mover=0" "1 mover=0" "1 mover=1"; do set -- $v; run $1 $sc $N flush=1 $2; done
  done
done
python3 "$D/parse_service_split.py" "$OUT" > "$D/results/service_split.tables.md"
