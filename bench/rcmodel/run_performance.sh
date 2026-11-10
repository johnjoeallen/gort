#!/bin/bash
# Re-runs the full suite with the CPU governor / EPP set to "performance", then restores the original settings.
# Previous (powersave) results live in git history (commit 9b7d544). Needs passwordless sudo. Detached-safe: start with setsid nohup.
set -u
D=$(cd "$(dirname "$0")" && pwd); cd "$D"
CP=/sys/devices/system/cpu
ORIG_GOV=$(cat $CP/cpu0/cpufreq/scaling_governor); ORIG_EPP=$(cat $CP/cpu0/cpufreq/energy_performance_preference)
restore() {
  for c in $CP/cpu[0-9]*/cpufreq; do
    echo "$ORIG_GOV" | sudo -n tee $c/scaling_governor >/dev/null
    echo "$ORIG_EPP" | sudo -n tee $c/energy_performance_preference >/dev/null
  done
  echo "[$(date +%H:%M:%S)] restored governor=$ORIG_GOV epp=$ORIG_EPP" >> "$D/performance_run.log"
}
trap restore EXIT
# (previous powersave results are in git history, commit 9b7d544)
for c in $CP/cpu[0-9]*/cpufreq; do
  echo performance | sudo -n tee $c/scaling_governor >/dev/null
  echo performance | sudo -n tee $c/energy_performance_preference >/dev/null
done
echo "[$(date +%H:%M:%S)] governor=$(cat $CP/cpu0/cpufreq/scaling_governor) epp=$(cat $CP/cpu0/cpufreq/energy_performance_preference) (was $ORIG_GOV/$ORIG_EPP)" >> "$D/performance_run.log"
./run_all.sh
echo "[$(date +%H:%M:%S)] run_all finished rc=$?" >> "$D/performance_run.log"
