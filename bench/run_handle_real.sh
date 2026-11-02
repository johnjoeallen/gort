#!/bin/bash
# D15 realistic handle benchmark. Outputs: results/handle_real.txt (-O2), handle_real_O3.txt (-O3) and the overhead tables *.tables.md
D=$(dirname "$0")
for O in 2 3; do
  S=""; [ $O = 3 ] && S="_O3"
  gcc -O$O -o "$D/c/handle_real$S" "$D/c/handle_real.c" -lpthread || exit 1
  taskset -c 4 "$D/c/handle_real$S" 9 > "$D/results/handle_real$S.txt" || exit 1
  python3 "$D/parse_handle_real.py" "$D/results/handle_real$S.txt" > "$D/results/handle_real$S.tables.md"
done
