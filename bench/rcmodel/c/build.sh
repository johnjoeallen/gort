#!/bin/bash
# builds build/rcm_<variant>_<opt> for all variants; usage: build.sh [opts...]  (default: O2 O3)
D=$(cd "$(dirname "$0")" && pwd); mkdir -p "$D/build"
OPTS=${@:-O2 O3}
names=(C0-raw C1-local C2-flagged C3-atomic C4-elided)
for o in $OPTS; do for m in 0 1 2 3 4; do
  gcc -$o -march=native -fno-optimize-sibling-calls -Wall -Wno-unused-function \
      -DMODE=$m -DVARIANT="\"${names[$m]}\"" -DIMPL="\"c-$o\"" -o "$D/build/rcm_${names[$m]}_$o" "$D/rcm.c" -lpthread || exit 1
  gcc -$o -march=native -fno-optimize-sibling-calls -Wall -Wno-unused-function -DCOUNT \
      -DMODE=$m -DVARIANT="\"${names[$m]}\"" -DIMPL="\"c-$o\"" -o "$D/build/rcmcount_${names[$m]}_$o" "$D/rcm.c" -lpthread || exit 1
done; done
