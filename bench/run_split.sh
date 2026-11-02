#!/bin/bash
# D17 benchmark: combined slot against split control block. Writes results/split.txt and results/split.tables.md.
D=$(dirname "$0"); R=${1:-5}
for m in 0 1; do gcc -O2 -Wall -DSPLIT=$m -o "$D/c/split_$m" "$D/c/split_bench.c" || exit 1; done
OUT="$D/results/split.txt"; : > "$OUT"
run() { # modes alternate order every repetition so neither always runs first
  local rep=$1; shift
  local order="0 1"; [ $((rep % 2)) -eq 0 ] && order="1 0"
  for m in $order; do taskset -c 2 "$D/c/split_$m" "$@" >> "$OUT" || echo "FAILED $m $*" >> "$OUT"; done
}
for rep in $(seq $R); do
  for b in 16 64 256; do run $rep work=alloc n=1500000 body=$b; done
  for n in 2000 100000 1500000 6000000; do run $rep work=deref n=$n body=64; done
done
for rep in 1 2 3; do
  run $rep work=retain n=2000000 body=64 keep=100 pattern=scatter
  run $rep work=retain n=2000000 body=64 keep=1000 pattern=scatter
  run $rep work=retain n=2000000 body=64 keep=100 pattern=cluster
  run $rep work=retain n=1000000 body=256 keep=100 pattern=scatter
  run $rep work=weak n=2000000 body=64
  run $rep work=weak n=1000000 body=256
done
python3 "$D/parse_split.py" "$OUT" > "$D/results/split.tables.md"
