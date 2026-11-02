#!/bin/bash
# D15 mixed-request benchmark. Builds three variants, runs scenarios interleaved (so drift hits all variants alike), writes results/mixed.txt and results/mixed.tables.md.
D=$(dirname "$0"); R=${1:-7}; N=${2:-100000}
for m in 0 1 2; do gcc -O2 -DMODE=$m -o "$D/c/mixed_$m" "$D/c/mixed_bench.c" -lpthread || exit 1; done
cp "$D/c/mixed_0" "$D/c/mixed_0b"                               # the same binary again: its difference from mode 0 is the noise floor
: > "$D/results/mixed.txt"
# scenario: hit% db_us concurrency workers
ORDERS=("0 0b 1 2" "0b 1 2 0" "1 2 0 0b" "2 0 0b 1")          # the order rotates every repetition so no variant always runs first
for sc in "100 100 256 1" "50 200 256 1" "20 500 128 1" "100 100 256 4" "20 500 128 4"; do
  taskset -c 2-5 "$D/c/mixed_0" $sc $N x > /dev/null                  # warm-up, discarded
  for rep in $(seq $R); do
    for m in ${ORDERS[$(( (rep - 1) % 4 ))]}; do
      echo "SC $sc $m $(taskset -c 2-5 "$D/c/mixed_$m" $sc $N x)" >> "$D/results/mixed.txt"
    done
  done
done
python3 "$D/parse_mixed.py" "$D/results/mixed.txt" > "$D/results/mixed.tables.md"
