#!/bin/bash
# W2 shared-table contention under reader-friendly synchronisation schemes (c/w2sync.c). Writes results_w2sync/raw.csv + env.txt + tables.md.
# usage: [REPS=5] [DUR=1000] [PHASES="env build main wsweep promote work stall q hpt analyze"] ./run_w2sync.sh
# Pinning as run_all.sh: cores 0..T-1 are distinct physical cores; cpu 2 for single-thread. Each run is fixed-duration (DUR ms).
set -u
D=$(cd "$(dirname "$0")" && pwd); R=$D/results_w2sync; mkdir -p "$R"
REPS=${REPS:-5}; DUR=${DUR:-1000}; PHASES=${PHASES:-"env build main wsweep promote work stall q hpt analyze"}
B=$D/c/build/w2sync; RAW=$R/raw.csv; ERR=$R/stderr.log
SCHEMES=(spin rw qsbr_clone qsbr_borrow hp hp_asym)
has() { [[ " $PHASES " == *" $1 "* ]]; }
[ -f "$RAW" ] || echo "run,workload,impl,variant,params,metric,value" > "$RAW"
emit() { local r=$1; shift; "$@" 2>>"$ERR" | sed "s/^/$r,/" >> "$RAW"; }
rot() { local r=$1; shift; local a=("$@") n=$#; for ((i=0;i<n;i++)); do echo "${a[$(((i+r)%n))]}"; done; }
cpus() { [ "$1" -le 1 ] && echo 2 || echo "0-$(($1-1))"; }
say() { echo "[$(date +%H:%M:%S)] $*" | tee -a "$R/progress.log"; }
run() { local r=$1 T=$2; shift 2; emit $r taskset -c $(cpus $T) $B threads=$T dur_ms=$DUR "$@"; }

if has env; then
  { echo "date: $(date -Iseconds)"; uname -a; lscpu | grep -E "Model name|^CPU\(s\)|Thread|L2|L3"
    echo "governor: $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor)  driver: $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_driver)  boost: $(cat /sys/devices/system/cpu/cpufreq/boost 2>/dev/null)"
    gcc --version | head -1; echo "REPS=$REPS DUR=$DUR"; } > "$R/env.txt" 2>&1
fi
if has build; then
  say build
  gcc -O2 -march=native -fno-optimize-sibling-calls -Wall -Wno-unused-function -Wno-unused-variable -DMODE=2 -DIMPL='"c-O2"' -o "$B" "$D/c/w2sync.c" -lpthread || exit 1
fi
# L0-L4 x mode x threads at the W2 default 5% writes
if has main; then say main
  for r in $(seq $REPS); do for T in 1 2 4 8 16; do for m in uniform hot_same hot_diff; do for s in $(rot $r "${SCHEMES[@]}"); do
    run $r $T scheme=$s mode=$m; done; done; done; done
fi
# write ratio x count-line layout, hot_same 16 threads: wpct=0 isolates the count line from the slot line
if has wsweep; then say wsweep
  for r in $(seq $REPS); do for w in 0 1 5 20; do for sp in 0 1; do for s in $(rot $r "${SCHEMES[@]}"); do
    run $r 16 scheme=$s mode=hot_same wpct=$w split=$sp; done; done; done; done
fi
# promotion rate: fraction of borrowed reads whose borrow escapes (compiler-inserted clone)
if has promote; then say promote
  for r in $(seq $REPS); do for m in hot_same uniform; do for p in 0 10 50 100; do for s in qsbr_borrow hp hp_asym; do
    run $r 16 scheme=$s mode=$m ppct=$p; done; done; done; done
fi
# handler work after the read (1 us, 10 us), with and without promotion of every borrow
if has work; then say work
  for r in $(seq $REPS); do for w in 1000 10000; do
    for s in spin rw qsbr_clone; do run $r 16 scheme=$s mode=hot_same work=$w; done
    for s in qsbr_borrow hp_asym; do for p in 0 100; do run $r 16 scheme=$s mode=hot_same work=$w ppct=$p; done; done
  done; done
fi
# reclamation under a reader that parks mid-read for stall_ms
if has stall; then say stall
  for r in $(seq 3); do for st in 0 10 100; do for s in qsbr_borrow hp hp_asym; do
    run $r 16 scheme=$s mode=hot_same stall_ms=$st; done; done; done
fi
# QSBR quiescent-state interval (reads per announcement; each announcement is two mfences)
if has q; then say q
  for r in $(seq $REPS); do for q in 1 4 16 256; do for s in qsbr_clone qsbr_borrow; do
    run $r 16 scheme=$s mode=hot_same q=$q; done; done; done
fi
# hazard-pointer scan threshold: amortises the scan (and hp_asym's membarrier IPIs) against bounded garbage
if has hpt; then say hpt
  for r in $(seq $REPS); do for t in 96 1024 8192; do for m in hot_same hot_diff; do for s in hp hp_asym; do
    run $r 16 scheme=$s mode=$m hpt=$t; done; done; done; done
fi
if has analyze; then python3 -I "$D/tools/analyze_w2sync.py" "$RAW" > "$R/tables.md"; fi
say done
