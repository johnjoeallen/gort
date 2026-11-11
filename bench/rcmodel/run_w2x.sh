#!/bin/bash
# Comparative W2 hot-path runs: C (Gort model schemes), Go, Java, Rust; lock-based and lock-free tables; with and without real work.
# Same protocol in every driver: 10k-entry table, 5% writes, read = lookup + 3-frame walk, then hash= bytes of FNV-1a over a
# thread-local buffer (1500 ~ 1 us, 15000 ~ 10 us); op-count delta over a DUR ms window after WARM ms of warm-up.
# usage: [REPS=5] [WARM=500] [DUR=1000] [PHASES="env build hot diff analyze"] ./run_w2x.sh   -> results_w2x/
set -u
D=$(cd "$(dirname "$0")" && pwd); R=$D/results_w2x; mkdir -p "$R"
REPS=${REPS:-5}; WARM=${WARM:-500}; DUR=${DUR:-1000}; PHASES=${PHASES:-"env build hot diff analyze"}
RAW=$R/raw.csv; ERR=$R/stderr.log
CB=$D/c/build/w2sync; GO=$D/go/w2x/w2x_go; JAR=$D/java/target/rcm.jar; RS=$D/rust/target/release/w2x
has() { [[ " $PHASES " == *" $1 "* ]]; }
[ -f "$RAW" ] || echo "run,workload,impl,variant,params,metric,value" > "$RAW"
cpus() { [ "$1" -le 1 ] && echo 2 || echo "0-$(($1-1))"; }
say() { echo "[$(date +%H:%M:%S)] $*" | tee -a "$R/progress.log"; }
# implementation id -> command prefix (thread count, mode, hash appended)
IMPLS=(c:spin c:qsbr_clone c:qsbr_borrow c:hp_asym c:hp_asym_p100 go:rwmutex go:atomic java-g1:chm java-zgc:chm java-g1:ara java-zgc:ara rust:rwlock rust:arcswap rust:epoch)
cmd() {
  case $1 in
    c:hp_asym)      echo "$CB scheme=hp_asym hpt=1024" ;;
    c:hp_asym_p100) echo "$CB scheme=hp_asym hpt=1024 ppct=100" ;;
    c:*)            echo "$CB scheme=${1#c:}" ;;
    go:*)           echo "$GO impl=${1#go:}" ;;
    java-g1:*)      echo "java -XX:+UseG1GC -Drcm.variant=g1 -cp $JAR rcm.W2X impl=${1#*:}" ;;
    java-zgc:*)     echo "java -XX:+UseZGC -Drcm.variant=zgc -cp $JAR rcm.W2X impl=${1#*:}" ;;
    rust:*)         echo "$RS impl=${1#rust:}" ;;
  esac
}
rot() { local r=$1; shift; local a=("$@") n=$#; for ((i=0;i<n;i++)); do echo "${a[$(((i+r)%n))]}"; done; }
one() { local r=$1 im=$2 T=$3 m=$4 h=$5
  taskset -c $(cpus $T) $(cmd $im) threads=$T mode=$m hash=$h warm_ms=$WARM dur_ms=$DUR 2>>"$ERR" | sed "s/^/$r,/" >> "$RAW"; }

if has env; then
  { echo "date: $(date -Iseconds)"; uname -a; lscpu | grep -E "Model name|^CPU\(s\)|Thread|L2|L3"
    echo "governor: $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor)  driver: $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_driver)  boost: $(cat /sys/devices/system/cpu/cpufreq/boost 2>/dev/null)"
    gcc --version | head -1; go version; java -version 2>&1 | head -1; rustc --version
    (cd "$D/rust" && cargo tree -e normal --depth 1 2>/dev/null | grep -E "arc-swap|crossbeam-epoch")
    echo "REPS=$REPS WARM=$WARM DUR=$DUR"; } > "$R/env.txt" 2>&1
fi
if has build; then say build
  gcc -O2 -march=native -fno-optimize-sibling-calls -Wall -Wno-unused-function -Wno-unused-variable -DMODE=2 -DIMPL='"c-O2"' -o "$CB" "$D/c/w2sync.c" -lpthread || exit 1
  (cd "$D/go" && go build -o w2x/w2x_go ./w2x) || exit 1
  (cd "$D/java" && mvn -q -B package -DskipTests 2>&1 | grep -E "ERROR"); (cd "$D/rust" && cargo build --release --bin w2x 2>&1 | tail -1)
fi
if has hot; then say "hot_same x threads x work"
  for r in $(seq $REPS); do for h in 0 1500 15000; do for T in 1 4 8 16; do for im in $(rot $r "${IMPLS[@]}"); do
    one $r $im $T hot_same $h; done; done; done; say "rep $r"; done
fi
if has diff; then say "hot_diff reference, T=16"
  for r in $(seq $REPS); do for im in $(rot $r "${IMPLS[@]}"); do one $r $im 16 hot_diff 0; done; done
fi
if has analyze; then python3 -I "$D/tools/analyze_w2x.py" "$RAW" > "$R/tables.md"; fi
say done
