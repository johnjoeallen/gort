#!/usr/bin/env bash
# bench/search.sh - the challenge's scoring procedure, locally: the server pinned to one core and capped at 2 GB, the load on other cores.
#   bench/search.sh probe <users> [rampSec=60] [holdSec=120]   one run on a fresh database
#   bench/search.sh full [startUsers=2500]                      warm-up, double until a probe fails, bisect, then a 5-minute hold (step down 5% until one passes)
# Env: GEN=node|k6 (node: loadmp.mjs, any size; k6: the challenge's own load.js, about 240 KB per user), SERVER_CPU=2, CLIENT_CPUS=4-15,20-31
# (never the server's SMT sibling 18), K6=/home/jallen/feed-bench/k6, WORK=/home/jallen/feed-bench, MAX_USERS=400000.
# Pass: p95 < 500 ms, p99 < 1 s, errors < 1%. Every run restarts the server on a fresh copy of seed/feed.db (and removes the old -wal and -shm).
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"; FEED="$(cd "$HERE/.." && pwd)"
CH="${CH:-$HOME/git/challenge}"; WORK="${WORK:-$HOME/feed-bench}"; K6="${K6:-$WORK/k6}"
SERVER_CPU="${SERVER_CPU:-2}"; CLIENT_CPUS="${CLIENT_CPUS:-4-15,20-31}"; GEN="${GEN:-node}"; PORT="${PORT:-3000}"; MAX_USERS="${MAX_USERS:-400000}"
LOG="$WORK/search-$(date +%Y%m%d-%H%M%S).log"; mkdir -p "$WORK/run"
say() { echo "$@" | tee -a "$LOG"; }

stop_server() { pkill -x feed 2>/dev/null; for _ in $(seq 20); do pgrep -x feed >/dev/null || return 0; sleep 0.5; done; pkill -9 -x feed; }
# The load generator and the server share this machine, so the sockets one probe closed sit in TIME_WAIT (60 s) on the same loopback addresses and ports
# the next probe needs: it would fail to connect. A separate load machine does not have this problem, so wait until they are gone.
drain() { for _ in $(seq 120); do [ "$(ss -tan state time-wait | wc -l)" -le 1000 ] && return 0; sleep 1; done; }
start_server() {
  stop_server; drain; rm -f "$WORK/run"/feed.db*; cp "$CH/seed/feed.db" "$WORK/run/feed.db"
  export SQLITE_PATH="$WORK/run/feed.db" JWT_SECRET=twelve-dollar-challenge HOST=0.0.0.0 PORT
  # one core, 2 GB, no swap, 65535 descriptors: the leaderboard box's limits
  systemd-run --user --scope -q -p MemoryMax=2G -p MemorySwapMax=0 taskset -c "$SERVER_CPU" bash -c 'ulimit -n 65535; exec bash '"$FEED"'/start.sh' > "$WORK/run/server.log" 2>&1 &
  for _ in $(seq 60); do curl -sf -m 1 "http://127.0.0.1:$PORT/health" >/dev/null && break; sleep 1; done
  SERVER_PID="$(pgrep -x feed | head -1)"; [ -n "$SERVER_PID" ] && curl -sf -m 1 "http://127.0.0.1:$PORT/health" >/dev/null
}
# probe <users> <ramp> <hold>: 0 pass, 1 fail. Prints one line.
probe() {
  local v=$1 ramp=$2 hold=$3 out rc=1
  start_server || { say "  $v users: server did not start"; return 1; }
  if [ "$GEN" = k6 ]; then
    out="$(cd "$CH/bench" && taskset -c "$CLIENT_CPUS" "$K6" run -q --local-ips=127.0.1.1-127.0.12.1 -e VUS="$v" -e RAMP_UP="${ramp}s" -e DURATION="${hold}s" -e BASE_URL="http://127.0.0.1:$PORT" \
          --summary-export "$WORK/run/k6.json" load.js 2>&1 | tail -3)"; rc=$?
    local j="$WORK/run/k6.json"
    out="{\"gen\":\"k6\",\"vus\":$v,\"rps\":$(jq '.metrics.http_reqs.rate|floor' "$j"),\"p95\":$(jq '.metrics.http_req_duration["p(95)"]' "$j"),\"p99\":$(jq '.metrics.http_req_duration["p(99)"]' "$j"),\"errPct\":$(jq '.metrics.http_req_failed.value*100' "$j"),\"serverPeakMB\":$(awk '/VmHWM/{print int($2/1024)}' /proc/$SERVER_PID/status 2>/dev/null || echo 0)}"
    jq -e '.p95<500 and .p99<1000 and .errPct<1' <<<"$out" >/dev/null; rc=$?
  else
    out="$(node "$HERE/loadmp.mjs" "$v" "$ramp" "$hold" "http://127.0.0.1:$PORT" "$SERVER_PID" "$CLIENT_CPUS" | tail -1)"
    jq -e '.pass' <<<"$out" >/dev/null; rc=$?
  fi
  local oom; oom="$(grep -c "" "$WORK/run/server.log" 2>/dev/null)"
  say "  $( [ $rc = 0 ] && echo PASS || echo FAIL ) $out"
  return $rc
}

case "${1:-}" in
probe) probe "$2" "${3:-60}" "${4:-120}"; rc=$?; stop_server; exit $rc ;;
full)
  start=${2:-2500}; say "== search ($GEN) $(date) server cpu $SERVER_CPU, client cpus $CLIENT_CPUS"
  say "-- warm-up: 1000 users, 2 min (not scored)"; probe 1000 20 100 >/dev/null
  say "-- doubling from $start"
  lo=0; hi=0; v=$start
  while [ "$v" -le "$MAX_USERS" ]; do
    if probe "$v" 60 120; then lo=$v; v=$((v * 2)); else hi=$v; break; fi
  done
  [ "$hi" = 0 ] && { say "no failure up to $MAX_USERS users"; hi=$((lo * 2)); }
  [ "$lo" = 0 ] && { say "failed at the first probe"; stop_server; exit 1; }
  say "-- bisecting between $lo (pass) and $hi (fail)"
  while [ $((hi - lo)) -gt $(( lo / 20 > 250 ? lo / 20 : 250 )) ]; do
    mid=$(( (lo + hi) / 2 / 1000 * 1000 )); [ "$mid" -le "$lo" ] && break
    if probe "$mid" 60 120; then lo=$mid; else hi=$mid; fi
  done
  say "-- 5-minute hold, from $lo"
  v=$lo
  while [ "$v" -ge 1000 ]; do
    if probe "$v" 60 300; then say "== SCORE: $v users held 5 minutes ($GEN, $(date))"; stop_server; exit 0; fi
    v=$(( v * 95 / 100 / 1000 * 1000 ))
  done
  say "== no hold passed"; stop_server; exit 1 ;;
*) sed -n 2,10p "$0"; exit 2 ;;
esac
