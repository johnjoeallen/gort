#!/bin/bash
# Confirms the counted variants really contain the count updates (not optimised away) and raw contains none.
D=$(cd "$(dirname "$0")/.." && pwd); B=$D/c/build
for o in O2 O3; do for v in C0-raw C1-local C2-flagged C3-atomic C4-elided; do
  f=$B/rcm_${v}_$o; [ -x $f ] || continue
  echo "== $v $o : W1 walk() instructions touching the count at offset 0 of the object"
  objdump -d --no-show-raw-insn -M intel $f | awk '/<walk>:/,/^$/' | grep -E "(inc|dec|add|sub|xadd).*(DWORD PTR \[r[a-z0-9]+\]|DWORD PTR \[r[a-z0-9]+\+0x0\])" | sed 's/^/   /'
  n=$(objdump -d --no-show-raw-insn -M intel $f | awk '/<layer>:/,/^$/' | grep -cE "lock (xadd|add|sub|dec|inc)|(inc|dec)[ ]+DWORD PTR \[r"); echo "   W3 layer(): $n count-updating instructions (static)"
done; done
