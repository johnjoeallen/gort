# Feed bench at 65,000 users

Source: `~/feed-bench/search-*.log` (local runs, Node generator `loadmp.mjs`, server pinned to 1 core / 2 GB / 65,535 descriptors) and `~/git/challenge/LEADERBOARD.md`. Nothing was re-run for this report.

## Result: 5-minute hold, 65,000 users

| Metric | Value |
|---|---|
| Requests per second | 6,882 |
| Total requests | 2,305,270 |
| p95 latency | 0.29 ms |
| p99 latency | 2.7 ms |
| Errors | 0% |
| Server CPU | about 16.5% of its one core |
| Server cost per request | about 24 µs |
| Server peak memory | 632 MB |

The 3-minute probes at 65,000 users gave 6,875–6,882 req/s, so the figure is stable.

## Other user counts in the same logs

| Users | req/s | Result |
|---|---:|---|
| 40,000 | 4,231 | pass |
| 65,625 | 6,954 | pass, 0.22% connection resets |
| 67,000 | 8,450 | fail: 44.6% ECONNREFUSED (60 s probe) |
| 70,000 | about 7,060 | fail: 1.4% timeouts, server at 100% CPU |
| 80,000 | 9,835 | fail: 24.6% ECONNRESET |

The request rate scales with users (about 0.106 req/s per simulated user). At 65,000 users the server is mostly idle, so the limit between 65,000 and 70,000 is connections, not request throughput.

## Comparison with the 65k tier of the leaderboard

Rank 166 is a tie of 65,000-user entries.

| Submission | req/s | p95 ms | p99 ms | CPU % | µs/req | RAM MB |
|---|---:|---:|---:|---:|---:|---:|
| **gort feed (local)** | **6,882** | **0.29** | **2.7** | **16.5** | **24** | **632** |
| C io_uring #239 (connection-capped) | 6,399 | 32 | 56 | 28 | 41 | 605 |
| C++ io_uring #98 | 6,399 | 33 | 55 | 30 | 44 | 200 |
| go-gnet #38 | 6,396 | 39 | 54 | 42 | 61 | 350 |
| C++ Drogon #29 | 6,398 | 36 | 60 | 52 | 76 | 814 |
| go-fasthttp #56 | 6,399 | 4.8 | 19 | 77 | 112 | 547 |
| java-netty #63 | 6,401 | 1.9 | 28 | 62 | 90 | 1,494 |
| node-net #78 | 6,383 | 133 | 312 | 75 | 109 | 483 |

- **req/s**: every 65k entry sits at about 6,400. That is the load model's rate, so it reflects how many users held, not server speed. Our 6,882 is higher because `loadmp.mjs` sends slightly more per user than the challenge's k6 script.
- **Latency and CPU are not comparable across hardware.** Our numbers are from a Ryzen 9950X3D core, and the leaderboard's are from a 1 vCPU Xeon Platinum 8260 at 2.4 GHz. A 9950X3D core is likely about 2× faster for single-thread work, so our 24 µs/req would probably be 45–50 µs/req on the leaderboard box. That is about the same as the best 65k entries (41–44 µs) and well behind the 300k entries (25–26 µs).
- **Top of the board**: the leaders hold 300k+ users at about 82% CPU on the slower Xeon. Our 16.5% CPU at 65k is on the faster CPU, so it overstates our headroom. Only a run on the Xeon vserver shows where CPU becomes the limit.

## What limits us

The leaderboard footnote ᶜ describes C io_uring #239 as "connection-capped at ~65k, not CPU-bound." Our failures at 67k and 70k look like the same wall. The 65,535 descriptor limit caps open connections at about 65k. Passing it needs the descriptor limit lifted or io_uring fixed-file accepts (as in wat-sqlite #466), so connections don't each use a descriptor.

## Caveats

- **Hardware:** our runs are on a Ryzen 9950X3D. The leaderboard box is a Synteq 1 vCPU / 2 GB vserver with a Xeon Platinum 8260 @ 2.4 GHz. The latency, CPU % and µs/req columns are not like for like. Re-run on the vserver for a true comparison.
- **Load generator:** a Node generator on the same machine, versus the leaderboard's separate 16-core load box at 0.25 ms RTT.
- Leaderboard rows are as recorded in `LEADERBOARD.md`.
