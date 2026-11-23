#!/bin/bash
# B18 benchmark: control block placement (H header in slot, D control section in chunk, C separate table, B external slab).
# Writes results/ctl.txt and results/ctl.tables.md. usage: run_ctl.sh [reps]
D=$(dirname "$0"); R=${1:-5}; BIN=${TMPDIR:-/tmp}/ctl_bin; mkdir -p "$BIN"
for L in 0 1 2 3; do for B in 16 48 64 72 256; do
  gcc -O2 -Wall -DLAYOUT=$L -DBODY=$B -o "$BIN/ctl_${L}_${B}" "$D/c/ctl_bench.c" || exit 1
done; gcc -O2 -Wall -DLAYOUT=$L -DBODY=64 -DATOMIC=1 -o "$BIN/ctl_${L}_64_atomic" "$D/c/ctl_bench.c" || exit 1; done
OUT="$D/results/ctl.txt"; : > "$OUT"
run() { # layouts alternate order every repetition so none always runs first
  local rep=$1 body=$2 suffix=$3; shift 3
  local order="0 1 2 3"; [ $((rep % 2)) -eq 0 ] && order="3 2 1 0"
  for L in $order; do taskset -c 2 "$BIN/ctl_${L}_${body}${suffix}" "$@" >> "$OUT" || echo "FAILED $L $body $*" >> "$OUT"; done
}
for rep in $(seq $R); do
  for b in 16 64 256; do run $rep $b "" work=alloc n=1500000; done
  for n in 2000 100000 1500000; do run $rep 64 "" work=access n=$n; done
  for b in 48 72; do for n in 100000 1500000; do run $rep $b "" work=access n=$n; done; done
  for n in 2000 100000 1500000; do run $rep 64 _atomic work=access n=$n; done
done
for rep in 1 2 3; do
  run $rep 64 "" work=access n=6000000
  run $rep 64 _atomic work=access n=6000000
  for b in 16 48 64 72 256; do run $rep $b "" work=mem n=2000000; done
  run $rep 64 "" work=weak n=2000000
  run $rep 256 "" work=weak n=1000000
  run $rep 64 "" work=retain n=2000000 keep=100
  run $rep 64 "" work=retain n=2000000 keep=1000
  run $rep 256 "" work=retain n=1000000 keep=100
done
python3 "$D/parse_ctl.py" "$OUT" > "$D/results/ctl.tables.md"
