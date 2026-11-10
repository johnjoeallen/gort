#!/usr/bin/env python3 -I
"""W5 driver. Starts each server pinned to cores 0-7, load generators (oha, idle holder) on cores 8-15.
usage: w5.py <results_dir> <reps> ; appends rows to <results_dir>/raw.csv"""
import json, os, signal, subprocess, sys, time, socket
D = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
R, REPS = sys.argv[1], int(sys.argv[2])
OHA = f"{D}/tools/oha/bin/oha"; IDLE = f"{D}/tools/idle"; PORT = 18080
WARM, MEAS = int(os.environ.get("W5_WARM", 3)), int(os.environ.get("W5_MEAS", 8))
SRV_CPUS, LG_CPUS = "0-7", "8-15"
JAR = f"{D}/java/target/rcm.jar"
SERVERS = {   # (impl, variant) -> command
    ("c-O2", "C0-raw"): [f"{D}/c/build/httpd_C0-raw_O2", str(PORT), "8"],
    ("c-O2", "C2-flagged"): [f"{D}/c/build/httpd_C2-flagged_O2", str(PORT), "8"],
    ("c-O2", "C3-atomic"): [f"{D}/c/build/httpd_C3-atomic_O2", str(PORT), "8"],
    ("go", "Go-gc"): ["env", "GOMAXPROCS=8", f"{D}/go/server_go", str(PORT)],
    ("java", "Java-g1"): ["java", "-XX:+UseG1GC", "-Dsun.net.httpserver.maxIdleConnections=100000", "-Dsun.net.httpserver.idleInterval=3600", "-cp", JAR, "rcm.Server", str(PORT)],
    ("java", "Java-zgc"): ["java", "-XX:+UseZGC", "-Dsun.net.httpserver.maxIdleConnections=100000", "-Dsun.net.httpserver.idleInterval=3600", "-cp", JAR, "rcm.Server", str(PORT)],
}
CONFIGS = [(10, 0), (100, 0), (1000, 0), (100, 10000)]   # (active connections, idle connections)
def rss(pid, key="VmRSS"):
    for l in open(f"/proc/{pid}/status"):
        if l.startswith(key + ":"): return int(l.split()[1]) / 1024.0
def wait_port():
    for _ in range(200):
        try: socket.create_connection(("127.0.0.1", PORT), 0.2).close(); return True
        except OSError: time.sleep(0.1)
    return False
def oha(c, secs):
    out = subprocess.run(["taskset", "-c", LG_CPUS, OHA, "-z", f"{secs}s", "-c", str(c), "--no-tui", "--output-format", "json", f"http://127.0.0.1:{PORT}/"], capture_output=True, text=True)
    return json.loads(out.stdout)
def run_one(rep, impl, variant, cmd, active, idle):
    p = subprocess.Popen(["taskset", "-c", SRV_CPUS] + cmd, stdout=subprocess.DEVNULL, stderr=open(f"{R}/w5_server.err", "a"))
    rows = []; params = f"active={active};idle={idle}"
    def row(m, v): rows.append(f"{rep},W5,{impl},{variant},{params},{m},{v:.6g}")
    idle_p = None
    try:
        if not wait_port(): print("server did not start", variant, file=sys.stderr); return
        oha(10, 2)                                  # settle (JIT, heaps)
        time.sleep(0.5); base = rss(p.pid)
        if idle:
            idle_p = subprocess.Popen(["taskset", "-c", LG_CPUS, IDLE, str(PORT), str(idle)], stdout=subprocess.PIPE, text=True)
            line = idle_p.stdout.readline(); got = int(line.split()[1]) if line.startswith("ready") else 0
            time.sleep(1.0); row("idle_conns_established", got)
            row("rss_per_idle_conn_bytes", (rss(p.pid) - base) * 1048576 / max(got, 1))
        oha(active, WARM)                           # warm-up, discarded
        j = oha(active, MEAS)
        s = j["summary"]; lp = j["latencyPercentiles"]
        row("rps", s["requestsPerSec"]); row("p50_us", lp["p50"] * 1e6); row("p99_us", lp["p99"] * 1e6); row("p999_us", lp.get("p99.9", 0) * 1e6)
        row("success_rate", s["successRate"]); row("rss_mb_peak", rss(p.pid, "VmHWM"))
    finally:
        if idle_p: idle_p.send_signal(signal.SIGTERM); idle_p.wait()
        p.send_signal(signal.SIGTERM)
        try: p.wait(5)
        except subprocess.TimeoutExpired: p.kill()
        time.sleep(0.5)
    with open(f"{R}/raw.csv", "a") as f: f.write("\n".join(rows) + "\n")
for rep in range(1, REPS + 1):
    keys = list(SERVERS); keys = keys[rep % len(keys):] + keys[:rep % len(keys)]
    for active, idle in CONFIGS:
        for k in keys:
            print(f"W5 rep {rep} {k[1]} active={active} idle={idle}", flush=True); run_one(rep, k[0], k[1], SERVERS[k], active, idle)
