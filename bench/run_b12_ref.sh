#!/bin/bash
# B12 on the D17 model: the mixed-request service with Ref<User> cache objects, sharded cache locks and clone-on-hit.
# Builds combined (SPLIT=0) and split (SPLIT=1) from service_split_bench.c. Steady scenarios run every config in a rotating order; flush scenarios run the mover.
# Writes results/b12_ref.txt and results/b12_ref.tables.md.
D=$(dirname "$0"); R=${1:-5}; N=${2:-300000}
for m in 0 1; do gcc -O2 -DSPLIT=$m -o "$D/c/service_split_$m" "$D/c/service_split_bench.c" -lpthread || exit 1; done
OUT="$D/results/b12_ref.txt"; : > "$OUT"
run() { taskset -c 2-7 "$D/c/service_split_$1" "${@:2}" >> "$OUT" || echo "FAILED $*" >> "$OUT"; }
CFG=("0 shards=1 clone=0" "0 shards=16 clone=0" "0 shards=16 clone=1" "1 shards=1 clone=0" "1 shards=16 clone=0" "1 shards=16 clone=1")
for sc in "100 10 256 4|" "50 200 256 4|" "20 500 128 4|" "100 10 256 1|" "100 10 256 4|skew=50"; do
  for rep in $(seq $R); do
    for i in $(seq 0 5); do c=${CFG[$(( (i + rep) % 6 ))]}; set -- $c; l=$1; shift; run $l ${sc%%|*} $N ${sc#*|} "$@"; done
  done
done
for rep in 1 2 3; do
  for v in "0 shards=16 clone=1 mover=0" "1 shards=16 clone=0 mover=0" "1 shards=16 clone=0 mover=1" "1 shards=16 clone=1 mover=0" "1 shards=16 clone=1 mover=1"; do
    set -- $v; l=$1; shift; run $l 50 200 256 4 $N flush=1 "$@"
  done
done
python3 "$D/parse_b12_ref.py" "$OUT" > "$D/results/b12_ref.tables.md"
