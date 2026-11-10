#!/bin/bash
# Re-runs ONLY the Java benchmarks (W1-W5) with a capped heap, into results_java1g/ (separate from results/).
# usage: [PHASES="w1 w2 w3 w4 w5"] [JAVA_OPTS=-Xmx1g] [OUT=results_java1g] [REPS=10] ./run_java.sh      Needs passwordless sudo for the governor.
# Same governor handling as run_performance.sh: performance during the run, original settings restored on exit.
set -u
D=$(cd "$(dirname "$0")" && pwd); cd "$D"
JAVA_OPTS=${JAVA_OPTS:--Xmx1g}; R=$D/${OUT:-results_java1g}; REPS=${REPS:-10}; SREPS=${SREPS:-5}
mkdir -p "$R"; JAR=$D/java/target/rcm.jar; RAW=$R/raw.csv; ERR=$R/stderr.log
G1="-XX:+UseG1GC $JAVA_OPTS"; ZGC="-XX:+UseZGC $JAVA_OPTS"
CP=/sys/devices/system/cpu
ORIG_GOV=$(cat $CP/cpu0/cpufreq/scaling_governor); ORIG_EPP=$(cat $CP/cpu0/cpufreq/energy_performance_preference)
restore() { for c in $CP/cpu[0-9]*/cpufreq; do echo "$ORIG_GOV" | sudo -n tee $c/scaling_governor >/dev/null; echo "$ORIG_EPP" | sudo -n tee $c/energy_performance_preference >/dev/null; done
  say "restored governor=$ORIG_GOV epp=$ORIG_EPP"; }
say() { echo "[$(date +%H:%M:%S)] $*" | tee -a "$R/progress.log"; }
trap restore EXIT
for c in $CP/cpu[0-9]*/cpufreq; do echo performance | sudo -n tee $c/scaling_governor >/dev/null; echo performance | sudo -n tee $c/energy_performance_preference >/dev/null; done
[ -f "$RAW" ] || echo "run,workload,impl,variant,params,metric,value" > "$RAW"
{ echo "date: $(date -Iseconds)"; echo "java opts: $JAVA_OPTS"; echo "governor: $(cat $CP/cpu0/cpufreq/scaling_governor) epp: $(cat $CP/cpu0/cpufreq/energy_performance_preference)"; java -version 2>&1 | head -2; free -g | head -2; } > "$R/env.txt"
emit() { local r=$1; shift; "$@" 2>>"$ERR" | sed "s/^/$r,/" >> "$RAW"; }
cpus() { [ "$1" -le 1 ] && echo 2 || echo "0-$(($1-1))"; }
say "java opts: $JAVA_OPTS"
PHASES=${PHASES:-"w1 w2 w3 w4 w5"}; has() { [[ " $PHASES " == *" $1 "* ]]; }
if has w1; then
for gc in "g1:$G1" "zgc:$ZGC"; do n=${gc%%:*}; f=${gc#*:}
  say "W1 Java $n"; taskset -c 2 java -jar $JAR jmh 'Micro.w1' -f $REPS -wi 2 -w 1 -i 3 -r 1 -jvmArgsAppend "$f" -rf json -rff "$R/jmh_w1_$n.json" >> "$R/jmh.log" 2>&1
  python3 -I "$D/tools/parse_jmh.py" "$R/jmh_w1_$n.json" Java-$n >> "$RAW"
done
fi
if has w2; then
for gc in "g1|$G1|1 2 4 8 16" "zgc|$ZGC|1 8 16"; do IFS="|" read n f ts <<< "$gc"; for T in $ts; do
  say "W2 Java $n t=$T"; taskset -c $(cpus $T) java -jar $JAR jmh 'Micro.w2' -t $T -f $REPS -wi 1 -w 1 -i 2 -r 1 -jvmArgsAppend "$f" -rf json -rff "$R/jmh_w2_${n}_$T.json" >> "$R/jmh.log" 2>&1
  python3 -I "$D/tools/parse_jmh.py" "$R/jmh_w2_${n}_$T.json" Java-$n >> "$RAW"
done; done
fi
w3j() { local r=$1 T=$2 work=$3 reqs=$4 a="workers=$2 work=$3 requests=$4"
  for gc in "g1:$G1" "zgc:$ZGC"; do n=${gc%%:*}; f=${gc#*:}; emit $r taskset -c $(cpus $T) java $f -Drcm.variant=Java-$n -jar $JAR w3 $a; done; }
if has w3; then
for r in $(seq $REPS); do for T in 1 4 8; do say "W3 rep $r T=$T"; w3j $r $T 1 5000000; done; done
for r in $(seq $SREPS); do say "W3 work=8 rep $r"; w3j $r 8 8 2000000; done
fi
if has w4; then
for r in $(seq $REPS); do for T in 1 8; do for wk in 0 1; do say "W4 rep $r T=$T weak=$wk"
  mask=0-3; [ $T -gt 4 ] && mask=0-$((T-1)); a="threads=$T weak=$wk"
  for gc in "g1:$G1" "zgc:$ZGC"; do n=${gc%%:*}; f=${gc#*:}; emit $r taskset -c $mask java $f -Drcm.variant=Java-$n -jar $JAR w4 $a; done
done; done; done
fi
if has w5; then
say "W5 Java"; W5_ONLY=java W5_JAVA_OPTS="$JAVA_OPTS" python3 -I "$D/tools/w5.py" "$R" ${W5REPS:-5}
fi
python3 -I "$D/tools/analyze.py" "$R" >/dev/null 2>&1
say "run_java finished"
