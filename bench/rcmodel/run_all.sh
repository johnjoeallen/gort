#!/bin/bash
# Runs every benchmark and writes results/raw.csv (run,workload,impl,variant,params,metric,value) + results/env.txt.
# usage: [REPS=10] [PHASES="env build w1 w2 w3 w4 w5 analyze"] ./run_all.sh
# Single benchmark process at a time, pinned with taskset (cores 0..N-1 are distinct physical cores on this CPU; cpu 2 for single-thread).
set -u
D=$(cd "$(dirname "$0")" && pwd); R=$D/results; mkdir -p "$R"
REPS=${REPS:-10}; PHASES=${PHASES:-"env build w1 w2 w3 w4 w5 analyze"}
COPTS=${COPTS:-"O2 O3"}            # C optimisation levels for W1 and W3
SREPS=${SREPS:-5}                  # repetitions for sensitivity runs
CV=(C0-raw C1-local C2-flagged C3-atomic C4-elided)
CB=$D/c/build; GO=$D/go/rcm_go; JAR=$D/java/target/rcm.jar; RS=$D/rust/target/release/rcm
G1="-XX:+UseG1GC"; ZGC="-XX:+UseZGC"
RAW=$R/raw.csv; ERR=$R/stderr.log
has() { [[ " $PHASES " == *" $1 "* ]]; }
[ -f "$RAW" ] || echo "run,workload,impl,variant,params,metric,value" > "$RAW"
emit() { local r=$1; shift; "$@" 2>>"$ERR" | sed "s/^/$r,/" >> "$RAW"; }   # run one process, tag rows with run index
rot() { local r=$1; shift; local a=("$@") n=$#; for ((i=0;i<n;i++)); do echo "${a[$(((i+r)%n))]}"; done; }
cpus() { [ "$1" -le 1 ] && echo 2 || echo "0-$(($1-1))"; }      # W1/single thread on cpu 2; else cores 0..T-1
say() { echo "[$(date +%H:%M:%S)] $*" | tee -a "$R/progress.log"; }

if has env; then
  { echo "date: $(date -Iseconds)"; uname -a; lscpu | grep -E "Model name|^CPU\(s\)|Thread|L2|L3"
    echo "governor: $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor)  driver: $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_driver)  boost: $(cat /sys/devices/system/cpu/cpufreq/boost 2>/dev/null)"
    echo "cpu0 MHz now: $(grep -m1 MHz /proc/cpuinfo)"; echo "perf_event_paranoid: $(cat /proc/sys/kernel/perf_event_paranoid) (perf binary: $(command -v perf || echo missing))"
    gcc --version | head -1; go version; java -version 2>&1 | head -2; rustc --version; cargo --version; "$D/tools/oha/bin/oha" --version 2>/dev/null
    echo "ulimit -n: $(ulimit -n)"; free -g | head -2; } > "$R/env.txt" 2>&1
fi

if has build; then
  say build
  "$D/c/build.sh" $COPTS O2 2>&1 | grep -E "error" ; (cd "$D/go" && go build -o rcm_go . ) || exit 1
  (cd "$D/java" && mvn -q -B package -DskipTests 2>&1 | grep -E "ERROR" ); (cd "$D/rust" && cargo build --release 2>&1 | tail -1 && cargo bench --bench micro --no-run 2>&1 | tail -1)
  "$D/tools/check_asm.sh" > "$R/asm_check.txt" 2>&1
fi

if has w1; then
  say "W1 C"
  for opt in $COPTS; do for r in $(seq $REPS); do for d in 1 4 16; do
    for v in $(rot $r "${CV[@]}"); do emit $r taskset -c 2 $CB/rcm_${v}_$opt w1 depth=$d iters=100000000; done
  done; done; done
  for r in $(seq $REPS); do for d in 1 4 16; do   # shared flag set, single thread: forces the atomic path of C2/C4 without contention
    for v in C2-flagged C4-elided; do emit $r taskset -c 2 $CB/rcm_${v}_O2 w1 depth=$d iters=100000000 shared=1; done
  done; done
  say "W1 Go";   (cd "$D/go" && taskset -c 2 go test -run XXX -bench 'W1' -count=$REPS -benchtime=1s ./... 2>>"$ERR") | python3 -I "$D/tools/parse_go.py" W1 1 Go-gc >> "$RAW"
  for gc in "g1:$G1" "zgc:$ZGC"; do n=${gc%%:*}; f=${gc#*:}
    say "W1 Java $n"; taskset -c 2 java -jar $JAR jmh 'Micro.w1' -f $REPS -wi 2 -w 1 -i 3 -r 1 -jvmArgsAppend "$f" -rf json -rff "$R/jmh_w1_$n.json" >> "$R/jmh.log" 2>&1
    python3 -I "$D/tools/parse_jmh.py" "$R/jmh_w1_$n.json" Java-$n >> "$RAW"
  done
  say "W1 Rust"; (cd "$D/rust" && rm -rf target/criterion && taskset -c 2 cargo bench --bench micro -- --noplot 'w1/' >>"$R/criterion.log" 2>&1; python3 -I "$D/tools/parse_criterion.py" target/criterion >> "$RAW")
fi

if has w2; then
  say "W2 C"
  for r in $(seq $REPS); do for T in 1 2 4 8 16; do for m in uniform hot_same hot_diff; do
    for v in $(rot $r "${CV[@]}"); do
      [ "$v" = C1-local ] && [ $T -gt 1 ] && continue   # non-atomic counts on shared objects are unsound; not run
      emit $r taskset -c $(cpus $T) $CB/rcm_${v}_O2 w2 threads=$T mode=$m ops=3000000
    done; done; done; done
  say "W2 Go"; for T in 1 2 4 8 16; do (cd "$D/go" && taskset -c $(cpus $T) go test -run XXX -bench 'W2' -cpu $T -count=$REPS -benchtime=1s ./... 2>>"$ERR") | python3 -I "$D/tools/parse_go.py" W2 $T Go-gc >> "$RAW"; done
  for gc in "g1:$G1:1 2 4 8 16" "zgc:$ZGC:1 8 16"; do IFS=: read n f ts <<< "$gc"; for T in $ts; do
    say "W2 Java $n t=$T"; taskset -c $(cpus $T) java -jar $JAR jmh 'Micro.w2' -t $T -f $REPS -wi 1 -w 1 -i 2 -r 1 -jvmArgsAppend "$f" -rf json -rff "$R/jmh_w2_${n}_$T.json" >> "$R/jmh.log" 2>&1
    python3 -I "$D/tools/parse_jmh.py" "$R/jmh_w2_${n}_$T.json" Java-$n >> "$RAW"
  done; done
  say "W2 Rust"; for T in 1 2 4 8 16; do (cd "$D/rust" && W2_THREADS=$T taskset -c $(cpus $T) cargo bench --bench micro -- --noplot 'w2/' >>"$R/criterion.log" 2>&1); done
  (cd "$D/rust" && python3 -I "$D/tools/parse_criterion.py" target/criterion | grep ",W2," >> "$RAW")
fi

if has w3; then
  w3_all() { # r T work reqs  : one repetition of every implementation
    local r=$1 T=$2 work=$3 reqs=$4 a="workers=$2 work=$3 requests=$4"
    for opt in ${W3OPTS:-$COPTS}; do for v in $(rot $r "${CV[@]}"); do
      for tb in shared private; do
        [ "$v" = C1-local ] && [ $T -gt 1 ] && [ $tb = shared ] && continue
        [ $work -ne 1 ] && [ $tb = private ] && continue
        [ $opt != O2 ] && [ $tb = private ] && continue
        emit $r taskset -c $(cpus $T) $CB/rcm_${v}_$opt w3 $a table=$tb
      done; done; done
    emit $r taskset -c $(cpus $T) $GO w3 $a
    emit $r env GOGC=400 taskset -c $(cpus $T) $GO w3 $a variant=Go-gogc400
    emit $r env GOMAXPROCS=$T taskset -c $(cpus $T) $GO w3 $a
    for gc in "g1:$G1" "zgc:$ZGC"; do n=${gc%%:*}; f=${gc#*:}; emit $r taskset -c $(cpus $T) java $f -Drcm.variant=Java-$n -jar $JAR w3 $a; done
    emit $r taskset -c $(cpus $T) $RS w3 $a variant=rust-idiomatic
    emit $r taskset -c $(cpus $T) $RS w3 $a variant=rust-arc
  }
  for r in $(seq $REPS); do for T in 1 4 8; do say "W3 rep $r T=$T"; w3_all $r $T 1 5000000; done; done
  for r in $(seq $SREPS); do say "W3 work=8 rep $r"; W3OPTS=O2 w3_all $r 8 8 2000000; done
  for v in "${CV[@]}"; do emit 1 taskset -c 0-3 $CB/rcmcount_${v}_O2 w3 workers=1 requests=500000 warmup=1000 ; done
fi

if has w4; then
  w4_all() {
    local r=$1 T=$2 wk=$3 a="threads=$2 weak=$3" mask=0-3; [ $T -gt 4 ] && mask=0-$((T-1))
    for v in $(rot $r "${CV[@]}"); do emit $r taskset -c $mask $CB/rcm_${v}_O2 w4 $a; done
    emit $r taskset -c $mask $GO w4 $a
    for gc in "g1:$G1" "zgc:$ZGC"; do n=${gc%%:*}; f=${gc#*:}; emit $r taskset -c $mask java $f -Drcm.variant=Java-$n -jar $JAR w4 $a; done
    for v in rust-box rust-rc rust-arc; do emit $r taskset -c $mask $RS w4 $a variant=$v; done
  }
  for r in $(seq $REPS); do for T in 1 8; do for wk in 0 1; do say "W4 rep $r T=$T weak=$wk"; w4_all $r $T $wk; done; done; done
fi

if has w5; then "$D/run_w5.sh"; fi
if has analyze; then python3 -I "$D/tools/analyze.py" "$R"; fi
say "done phases: $PHASES"
