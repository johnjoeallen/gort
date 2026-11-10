## W1 pass-down micro: ns per pass (median (min–max) over runs)

| variant | depth 1 | depth 4 | depth 16 |
|---|---|---|---|
| Java-g1 | 0.549 (0.548–0.55) | 0.369 (0.336–0.384) | 0.409 (0.409–0.409) |
| Java-zgc | 0.729 (0.728–0.73) | 0.342 (0.338–0.348) | 0.416 (0.415–0.417) |

```
Java-g1                    ███████████████████████████████████████████ 0.409 ns/pass @depth16
Java-zgc                   ████████████████████████████████████████████ 0.416 ns/pass @depth16
```

## W2 shared-table read contention: total Mops/s (median (min–max)); 95/5 read/write, 10k entries

### mode = uniform

| variant | 1 thr | 2 thr | 4 thr | 8 thr | 16 thr |
|---|---|---|---|---|---|
| Java-g1 | 119 (118–121) | 208 (188–220) | 414 (397–417) | 749 (650–757) | 704 (658–717) |
| Java-zgc | 106 (105–107) | – | – | 399 (285–527) | 534 (496–577) |

```
Java-g1                    ████████████████████████████████████████████ 704 Mops/s @16 thr
Java-zgc                   █████████████████████████████████ 534 Mops/s @16 thr
```

### mode = hot_same

| variant | 1 thr | 2 thr | 4 thr | 8 thr | 16 thr |
|---|---|---|---|---|---|
| Java-g1 | 247 (240–250) | 216 (170–253) | 174 (165–191) | 149 (145–176) | 118 (114–139) |
| Java-zgc | 207 (205–215) | – | – | 155 (146–165) | 115 (112–137) |

```
Java-g1                    ████████████████████████████████████████████ 118 Mops/s @16 thr
Java-zgc                   ██████████████████████████████████████████ 115 Mops/s @16 thr
```

### mode = hot_diff

| variant | 1 thr | 2 thr | 4 thr | 8 thr | 16 thr |
|---|---|---|---|---|---|
| Java-g1 | 248 (245–249) | 429 (368–463) | 837 (810–859) | 1.44e+03 (1.42e+03–1.59e+03) | 1.58e+03 (1.54e+03–1.63e+03) |
| Java-zgc | 210 (206–214) | – | – | 866 (797–977) | 1.43e+03 (974–1.53e+03) |

```
Java-g1                    ████████████████████████████████████████████ 1.58e+03 Mops/s @16 thr
Java-zgc                   ███████████████████████████████████████ 1.43e+03 Mops/s @16 thr
```

Per-op slowdown of hot_same vs hot_diff at 16 threads (H4 asks >= 10x for C2/C3):

| variant | hot_diff / hot_same throughput |
|---|---|
| Java-g1 | 13.4x |
| Java-zgc | 12.5x |

## W3 request path (5M requests, CPU only): throughput, latency, RSS

### 1 worker(s), shared session table

| variant | Mreq/s | Go ÷ this (R) | p50 ns | p99 ns | peak RSS MB |
|---|---|---|---|---|---|
| Java-g1 | 3.73 (3.44–4) | – | 240 (210–260) | 335 (300–340) | 695 (694–705) |
| Java-zgc | 3.69 (3.45–3.9) | – | 245 (230–260) | 445 (430–480) | 1e+03 (995–1.01e+03) |

```
Java-g1                    ████████████████████████████████████████████ 3.73 Mreq/s @1w
Java-zgc                   ███████████████████████████████████████████ 3.69 Mreq/s @1w
```

### 4 worker(s), shared session table

| variant | Mreq/s | Go ÷ this (R) | p50 ns | p99 ns | peak RSS MB |
|---|---|---|---|---|---|
| Java-g1 | 12.2 (10.6–14.9) | – | 265 (230–290) | 480 (410–530) | 704 (698–708) |
| Java-zgc | 12.7 (11.1–13.7) | – | 265 (240–290) | 485 (390–670) | 836 (704–989) |

```
Java-g1                    ██████████████████████████████████████████ 12.2 Mreq/s @4w
Java-zgc                   ████████████████████████████████████████████ 12.7 Mreq/s @4w
```

### 8 worker(s), shared session table

| variant | Mreq/s | Go ÷ this (R) | p50 ns | p99 ns | peak RSS MB |
|---|---|---|---|---|---|
| Java-g1 | 16.8 (15.3–25.6) | – | 300 (260–330) | 815 (550–911) | 705 (701–709) |
| Java-zgc | 15.5 (14.1–23.5) | – | 270 (250–280) | 915 (590–1.02e+03) | 1.11e+03 (1.11e+03–1.12e+03) |

```
Java-g1                    ████████████████████████████████████████████ 16.8 Mreq/s @8w
Java-zgc                   ████████████████████████████████████████ 15.5 Mreq/s @8w
```

### C variants with private session tables (no sharing; used for the gap decomposition)

| variant | 1 w Mreq/s | 4 w | 8 w |
|---|---|---|---|

### Sensitivity: heavier per-request work (work=8: body/response hash loops x8), 8 workers, shared table

| variant | Mreq/s | Go ÷ this |
|---|---|---|
| Java-g1 | 9.06 (8.98–10.3) | – |
| Java-zgc | 9.2 (8.71–9.5) | – |

## Pre-registered verdict (W3, 8 workers, shared table)

| C opt | Go Mreq/s | C2 Mreq/s | R = Go/C2 | Go/C4 | outcome |
|---|---|---|---|---|---|

| comparison at 8 workers (c-O2) | ratio |
|---|---|
| C2 ÷ Go (throughput) | – |
| C2 ÷ Rust pessimal (Arc per pass) | – |
| C2 ÷ Rust idiomatic | – |
| C2 ÷ Java G1 | – |
| C2 ÷ Java ZGC | – |
| C2 ÷ C0 raw | – |
| C4 ÷ C0 raw | – |
| C3 ÷ C2 | – |

## Decomposing the C2 vs C0 gap in W3 (ns per request per core = workers / rps)

| workers | C0 ns/req | C2 ns/req | gap | inc/dec traffic (C1p-C0p) | flag test (C2p-C1p) | atomics on shared objs ((C2s-C2p)-(C0s-C0p)) | table sharing cost in raw (C0s-C0p, not Gort-specific) |
|---|---|---|---|---|---|---|---|

## W4 graph alloc/free: 1M-node 4-ary tree, ns per node (median over runs of the median of cycles 2-6)

### 1 thread(s), weak back-references = 0

| variant | build | traverse (passes Refs) | free (cascade) | cycle wall | cpu ns/node/cycle | RSS MB |
|---|---|---|---|---|---|---|
| Java-g1 | 3.76 (3.46–3.89) | 1.08 (1.04–1.3) | 0 (0–0) | 4.86 (4.51–5.01) | 15 (15–16.7) | 253 (249–254) |
| Java-zgc | 6.86 (6.27–7.45) | 1.82 (1.64–1.9) | 0 (0–0) | 8.6 (7.91–9.31) | 40 (36.7–41.7) | 320 (315–322) |

```
Java-g1                    ████████████████████████ 4.86 ns/node/cycle (wall)
Java-zgc                   ████████████████████████████████████████████ 8.6 ns/node/cycle (wall)
```

### 1 thread(s), weak back-references = 1

| variant | build | traverse (passes Refs) | free (cascade) | cycle wall | cpu ns/node/cycle | RSS MB |
|---|---|---|---|---|---|---|
| Java-g1 | 3.84 (3.47–4.1) | 1.17 (1.12–1.27) | 0 (0–0) | 4.98 (4.61–5.28) | 16.7 (16.7–18.3) | 251 (248–253) |
| Java-zgc | 7.15 (6.73–7.46) | 2.8 (2.71–2.92) | 0 (0–0) | 9.97 (9.5–10.2) | 44.2 (43.3–46.7) | 342 (332–350) |

```
Java-g1                    █████████████████████ 4.98 ns/node/cycle (wall)
Java-zgc                   ████████████████████████████████████████████ 9.97 ns/node/cycle (wall)
```

### 8 thread(s), weak back-references = 0

| variant | build | traverse (passes Refs) | free (cascade) | cycle wall | cpu ns/node/cycle | RSS MB |
|---|---|---|---|---|---|---|
| Java-g1 | 25.5 (19.7–26.2) | 6.73 (6.2–10.8) | 0 (0–0) | 31.9 (30.4–33.4) | 35.9 (31.9–40.8) | 1.08e+03 (1.08e+03–1.09e+03) |
| Java-zgc | 65.8 (33.8–96.5) | 8.98 (7.1–10.1) | 0 (0–0) | 74.6 (43.1–106) | 40.2 (36.7–54.8) | 1.12e+03 (1.11e+03–1.14e+03) |

```
Java-g1                    ██████████████████ 31.9 ns/node/cycle (wall)
Java-zgc                   ████████████████████████████████████████████ 74.6 ns/node/cycle (wall)
```

### 8 thread(s), weak back-references = 1

| variant | build | traverse (passes Refs) | free (cascade) | cycle wall | cpu ns/node/cycle | RSS MB |
|---|---|---|---|---|---|---|
| Java-g1 | 49.5 (45.3–58.6) | 12.1 (8.08–19.4) | 0 (0–0) | 62.1 (53.4–76.9) | 54.5 (50.8–59.6) | 1.09e+03 (1.09e+03–1.09e+03) |
| Java-zgc | 87.8 (47.5–124) | 10.3 (8.85–13.8) | 0 (0–0) | 98.8 (57.9–135) | 53.1 (46–66.5) | 1.13e+03 (1.11e+03–1.16e+03) |

```
Java-g1                    ███████████████████████████ 62.1 ns/node/cycle (wall)
Java-zgc                   ████████████████████████████████████████████ 98.8 ns/node/cycle (wall)
```

## W5 loopback HTTP/1.1 (server on cores 0-7, oha + idle holder on cores 8-15)

### 10 active connections, 0 idle

| server | kreq/s | p50 µs | p99 µs | server peak RSS MB | RSS bytes / idle conn | success % |
|---|---|---|---|---|---|---|
| Java-g1 | 469 (466–470) | 20.5 (20.4–20.6) | 31.8 (31.7–32) | 729 (724–730) | – | 100 (100–100) |
| Java-zgc | 464 (459–469) | 20.5 (20.2–21) | 32.4 (32–32.6) | 1.06e+03 (1.05e+03–1.06e+03) | – | 100 (100–100) |

```
Java-g1                    ████████████████████████████████████████████ 469 kreq/s
Java-zgc                   ███████████████████████████████████████████ 464 kreq/s
```

### 100 active connections, 0 idle

| server | kreq/s | p50 µs | p99 µs | server peak RSS MB | RSS bytes / idle conn | success % |
|---|---|---|---|---|---|---|
| Java-g1 | 799 (791–821) | 123 (119–124) | 189 (188–192) | 733 (730–736) | – | 100 (100–100) |
| Java-zgc | 755 (751–783) | 129 (123–130) | 210 (206–217) | 1.13e+03 (1.13e+03–1.13e+03) | – | 100 (100–100) |

```
Java-g1                    ████████████████████████████████████████████ 799 kreq/s
Java-zgc                   █████████████████████████████████████████ 755 kreq/s
```

### 1000 active connections, 0 idle

| server | kreq/s | p50 µs | p99 µs | server peak RSS MB | RSS bytes / idle conn | success % |
|---|---|---|---|---|---|---|
| Java-g1 | 569 (561–589) | 1.61e+03 (1.56e+03–1.63e+03) | 3.05e+03 (2.9e+03–3.08e+03) | 803 (796–805) | – | 100 (100–100) |
| Java-zgc | 566 (564–570) | 1.64e+03 (1.63e+03–1.64e+03) | 2.94e+03 (2.87e+03–2.96e+03) | 1.14e+03 (1.08e+03–1.15e+03) | – | 100 (100–100) |

```
Java-g1                    ████████████████████████████████████████████ 569 kreq/s
Java-zgc                   ███████████████████████████████████████████ 566 kreq/s
```

### 100 active connections, 10000 idle

| server | kreq/s | p50 µs | p99 µs | server peak RSS MB | RSS bytes / idle conn | success % |
|---|---|---|---|---|---|---|
| Java-g1 | 778 (762–788) | 126 (124–129) | 194 (191–195) | 755 (749–755) | 14.3 (11.9–16.8) | 100 (100–100) |
| Java-zgc | 736 (724–755) | 134 (130–136) | 210 (210–214) | 1.13e+03 (1.13e+03–1.14e+03) | 1.23 (1.23–2.05) | 100 (100–100) |

```
Java-g1                    ████████████████████████████████████████████ 778 kreq/s
Java-zgc                   █████████████████████████████████████████ 736 kreq/s
```
