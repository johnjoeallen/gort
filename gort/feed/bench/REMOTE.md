# Running the feed bench across two machines

The server runs on one machine (the 5600GT, 5800X or 3700X) and the load generator on another (the 9950X3D), joined by a wired LAN. This is closer to the leaderboard's setup than loopback: the generator can't take CPU from the server, and the network round trip is real (about 0.1–0.3 ms, against the leaderboard's 0.25 ms).

`search.sh` and `loadmp.mjs` were written for one machine. The gaps are listed in [What the scripts don't do yet](#what-the-scripts-dont-do-yet). Until they are closed, the steps below are run by hand.

## What you need

| Machine | Role | Notes |
|---|---|---|
| Ryzen 5 5600GT or Ryzen 7 5800X (a 3700X also works) | server | Linux, wired to the same switch, gcc and cargo installed |
| Ryzen 9950X3D | load generator | Linux, wired to the same switch, Node installed |

- Use wired gigabit or faster. Wi-Fi adds jitter and loss that look like server errors.
- Put both machines on the same switch or router, without a firewall or NAT between them. A tracking firewall can run out of connection slots at 65,000 connections and give errors that aren't the server's.
- Check the round trip first: `ping -c 20 <server>` from the generator. Expect under 0.3 ms.

## 1. Server machine (5600GT, 5800X or 3700X)

### Build

Build on the server machine itself, so the binary matches its glibc and libsqlite3.

```bash
cd gort/feed
./build.sh                 # builds gortc with cargo, then bin/feed
```

You also need the challenge's seed database (`seed/feed.db` in the challenge repo) and its `bench/load.js` user model, if you use k6.

### Limits that match the leaderboard box

The leaderboard box is 1 vCPU, 2 GB, no swap and 65,535 descriptors, with no kernel tuning. Reproduce the first three on the 5600GT:

| Limit | How |
|---|---|
| One core | `taskset -c <core>`. Pick a core whose SMT sibling is idle: check `lscpu -e` or `cat /sys/devices/system/cpu/cpu<core>/topology/thread_siblings_list`, and keep the sibling free of other work. |
| 2 GB, no swap | `systemd-run --user --scope -p MemoryMax=2G -p MemorySwapMax=0` |
| 65,535 descriptors | `ulimit -n 65535` in the shell that starts the server |

Leave the kernel alone otherwise.

### Match the Xeon's speed: frequency cap and calibration

The leaderboard's Xeon 8260 vCPU runs at about 2.4–3.1 GHz. The server machines here are faster per core, so uncapped they report lower CPU % and µs/req than the leaderboard box would. Estimates from specs (not measured):

| Machine | Uncapped vs the Xeon | Capped at 2.4 GHz |
|---|---|---|
| Ryzen 7 3700X (Zen 2) | about 1.3–1.6× faster | about equal |
| Ryzen 7 5800X (Zen 3) | about 1.7–2× faster | about 1.15–1.25× faster |
| Ryzen 5 5600GT (Zen 3) | about 1.7–2× faster | about 1.15–1.25× faster |
| Ryzen 9 9950X3D | about 2× or more faster | not used as a server |

A Zen 3 core does more work per clock than the Xeon, so the cap has to go below 2.4 GHz to match. Pick the cap by calibration, not from the table.

**Step 1: set a cap and turn boost off**

```bash
sudo cpupower frequency-set -u 2.0GHz            # starting guess for Zen 3; try 2.4GHz for a 3700X
echo 0 | sudo tee /sys/devices/system/cpu/cpufreq/boost   # with amd-pstate the file may be elsewhere: see cpupower frequency-info
grep MHz /proc/cpuinfo                           # check the real clock of the server core while the load runs
```

**Step 2: calibrate against a published entry**

The leaderboard lists µs/req for every submission. Pick one that is simple to build:

| Entry | Leaderboard µs/req (65,000 users) |
|---|---:|
| C io_uring #98 (`cpp-iouring-edwin-2024`) | 44 |
| C io_uring #239 (`lean-c-iouring-loafecho`) | 41 |

1. Build the entry on the server machine from `~/git/challenge/submissions/<name>/` with its own `build.sh` / `install.sh`.
2. Run it at 65,000 users with the same setup as gort (one core, 2 GB, 65,535 descriptors, fresh seed database, same generator and 120 s hold).
3. Read its µs/req, as in [Reading the results](#3-reading-the-results).
4. Compare it with the leaderboard's number. If yours is lower, the machine is too fast, so lower the cap. If it is higher, raise the cap. Repeat until the two are within about 5%.

Then run gort at that cap. The calibration uses this workload, so it covers differences in IPC, cache and the kernel's network path that the table can't. The leaderboard entry itself is the control, because it ran on the real Xeon.

**Limits of this method**
- A virtual machine on a shared host pays for the hypervisor and virtual network, and it shares a physical core with another thread. A capped bare-metal machine doesn't, so it stays somewhat optimistic.
- Lowering the core clock doesn't lower memory or network speed by the same ratio. A cap that matches one workload may be off by a few percent for another.
- Record the cap and the calibration result next to every gort number you publish. Restore the machine afterwards: `sudo cpupower frequency-set -u <original max>` and `echo 1 | sudo tee /sys/devices/system/cpu/cpufreq/boost`.

### Start the server

On a fresh copy of the seed database, each run:

```bash
rm -f run/feed.db*; cp ~/git/challenge/seed/feed.db run/feed.db
export SQLITE_PATH=$PWD/run/feed.db JWT_SECRET=twelve-dollar-challenge HOST=0.0.0.0 PORT=3000
systemd-run --user --scope -q -p MemoryMax=2G -p MemorySwapMax=0 \
  taskset -c <core> bash -c 'ulimit -n 65535; exec bash gort/feed/start.sh'
```

Open the port if the machine has a firewall: `sudo ufw allow 3000/tcp`. The leaderboard box has no firewall, so conntrack is off there. If your machine has one, conntrack can fill up at 65k connections, so check `sysctl net.netfilter.nf_conntrack_max` and `dmesg | grep conntrack`.

Check from the generator: `curl http://<server>:3000/health`.

## 2. Generator machine (9950X3D)

### Source ports

One source address can hold about 28,000 connections to one destination address and port with the default port range. 65,000 users need more, so do both of these:

1. Widen the range: `sudo sysctl -w net.ipv4.ip_local_port_range="1024 65535"`. That gives about 64,000 per source address, which is still short of 65,000.
2. Give the generator machine more than one address on its LAN interface, and spread users across them:

   ```bash
   sudo ip addr add 192.168.1.101/24 dev <iface>
   sudo ip addr add 192.168.1.102/24 dev <iface>
   sudo ip addr add 192.168.1.103/24 dev <iface>
   ```

`loadmp.mjs` only picks source addresses for `127.0.0.1`. For any other target it leaves the source address unset, so all users come from one address. See the gaps below.

### Run a probe

```bash
cd gort/feed/bench
taskset -c 4-15,20-31 node loadmp.mjs <users> <rampSec> <holdSec> http://<server>:3000 <serverPid> 4-15,20-31
# e.g. 65,000 users, 60 s ramp, 120 s hold
```

`loadmp.mjs` prints one JSON line with `rps`, `p50`, `p95`, `p99`, `errPct` and `pass`. A probe passes with p95 < 500 ms, p99 < 1 s and errors < 1%.

Raise the descriptor limit in the generator's shell too: `ulimit -n 1048576`, or at least 70,000.

### Search procedure

As in `search.sh`:
1. Warm up with 1,000 users for about 2 minutes (not scored).
2. Probe 2,500 users, double each time (60 s ramp, 120 s hold), until a probe fails.
3. Bisect between the last pass and the first failure, to about 5%.
4. Hold the best result for 5 minutes. If it fails, step down 5% until one passes.

Restart the server on a fresh database before every probe. Wait for TIME_WAIT sockets to drain between probes: `ss -tan state time-wait | wc -l` should be under about 1,000 on both machines.

## 3. Reading the results

- **Server CPU and µs/req** come from the server's own `/proc/<pid>/stat`. `loadmp.mjs` reads that file from the local machine, so across two machines it gives NaN or the wrong process. Sample the server over SSH instead: read `utime + stime` (fields 14 and 15, in ticks of 1/100 s) at the start and end of the hold. `serverCpuPct` = ticks / 100 / holdSeconds × 100. µs/req = ticks × 10,000 / requests in the hold.
- **Peak memory:** read `VmHWM` from `/proc/<pid>/status` on the server after the run.
- **Latency** now includes the real network hop. Compare p95 and p99 with the leaderboard's.
- **CPU %** and **µs/req** are still from a faster core, even with the frequency cap. The leaderboard's Xeon is slower, so expect its numbers to be higher than yours.

## What the scripts don't do yet

- `loadmp.mjs` only binds multiple source addresses for `127.0.0.1`. For a remote target it needs a list of local addresses (for example a `LOCAL_IPS` environment variable) and should pick one per user, the way `--local-ips` works for k6.
- `loadmp.mjs` reads the server's CPU and memory from the local `/proc`. For a remote server these need to be gathered over SSH, or left out and measured by hand as above.
- `search.sh` starts and stops the server on the same machine. A remote version would start it over SSH, copy the fresh seed database, and wait for `/health`.

## Measured: iris (5600GT) server, tornado (9950X3D) generator

Setup: server on iris pinned to CPU 2 (sibling 8 idle), 2 GB cap, no swap, `ulimit -n 65535`, 0.1 ms LAN round trip. Frequency cap 2.0 GHz with boost off (sysfs `scaling_max_freq`; iris has no `cpupower`). Generator: four source addresses (`LOCAL_IPS`), `SERVER_SSH=iris`, 65,000 users, 60 s ramp, 120 s hold, fresh seed database.

| | Leaderboard #239 (Xeon) | `lean-c-iouring-loafecho` on iris at 2.0 GHz | gort feed on iris at 2.0 GHz |
|---|---:|---:|---:|
| µs/req | 41 | 39.2 | **81.5** |
| Server CPU | 28% | 27% | **56%** |
| req/s | 6,399 | 6,880 | 6,877 |
| p95 / p99 (ms) | 32 / 56 | 2.9 / 5.5 | 2.8 / 6.5 |
| Errors | 0% | 0% | 0% |
| Peak memory (MB) | 605 | 138 | 625 |

The cap was calibrated with #239 (39.2 against 41 µs/req, within 5%), one run. On this calibrated core gort uses about twice the CPU per request of the C io_uring entry. The earlier local figure of 24 µs/req was on a 9950X3D core, so it was optimistic.

## Checklist

- [ ] Both machines wired to the same switch, round trip under 0.3 ms
- [ ] Server pinned to one core with its SMT sibling idle, `MemoryMax=2G`, no swap, `ulimit -n 65535`
- [ ] Frequency cap set and boost off; calibrated against a published entry (µs/req within about 5%); the cap and result recorded with the numbers
- [ ] Generator port range widened and more than one source address added
- [ ] No firewall or conntrack between the machines, or its limits raised
- [ ] Fresh seed database and TIME_WAIT drained before every probe
- [ ] Server CPU, µs/req and memory sampled on the server
