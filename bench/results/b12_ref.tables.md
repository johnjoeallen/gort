## Steady state (no flush, no mover)

### cache target 100%, database wait 10 us, 256 in flight per worker, 4 worker(s)

Actual hit rate 80%. All 6 configurations serve identical responses (one checksum): yes.

| Configuration | Work per request (ns) | vs combined, 1 lock | Lock retries per request | Requests/s | p50 (us) | p99 (us) | p99.9 (us) | Resident at end (MB) | Spread |
|---|---|---|---|---|---|---|---|---|---|
| combined, 1 lock, copy under lock (previous run) | 423 | (baseline) | 1.69 | 9,372,828 | 0.4 | 165.6 | 177.9 | 86 | 14% |
| combined, 16 locks, copy under lock | 382 | -41 ns (-9.7%) | 0.07 | 10,402,541 | 0.3 | 149.3 | 164.8 | 86 | 2% |
| combined, 16 locks, clone Ref on hit | 383 | -40 ns (-9.4%) | 0.05 | 10,331,881 | 0.3 | 149.8 | 161.8 | 86 | 1% |
| split, 1 lock, copy under lock (previous run) | 531 | +108 ns (+25.4%) | 6.32 | 7,504,844 | 0.5 | 207.3 | 223.9 | 100 | 4% |
| split, 16 locks, copy under lock | 415 | -8 ns (-1.9%) | 0.15 | 9,578,086 | 0.3 | 161.4 | 177.2 | 100 | 2% |
| split, 16 locks, clone Ref on hit | 409 | -14 ns (-3.3%) | 0.04 | 9,723,405 | 0.3 | 159.0 | 168.6 | 100 | 3% |

### cache target 50%, database wait 200 us, 256 in flight per worker, 4 worker(s)

Actual hit rate 49%. All 6 configurations serve identical responses (one checksum): yes.

| Configuration | Work per request (ns) | vs combined, 1 lock | Lock retries per request | Requests/s | p50 (us) | p99 (us) | p99.9 (us) | Resident at end (MB) | Spread |
|---|---|---|---|---|---|---|---|---|---|
| combined, 1 lock, copy under lock (previous run) | 463 | (baseline) | 0.87 | 8,543,591 | 172.8 | 258.4 | 270.9 | 54 | 1% |
| combined, 16 locks, copy under lock | 435 | -28 ns (-6.0%) | 0.05 | 9,059,792 | 167.5 | 251.6 | 261.9 | 54 | 1% |
| combined, 16 locks, clone Ref on hit | 437 | -26 ns (-5.6%) | 0.05 | 9,030,562 | 167.8 | 251.8 | 260.9 | 54 | 1% |
| split, 1 lock, copy under lock (previous run) | 543 | +80 ns (+17.3%) | 3.24 | 7,288,035 | 189.0 | 280.3 | 297.2 | 63 | 2% |
| split, 16 locks, copy under lock | 470 | +7 ns (+1.5%) | 0.11 | 8,430,322 | 174.0 | 260.2 | 273.6 | 63 | 0% |
| split, 16 locks, clone Ref on hit | 467 | +4 ns (+0.9%) | 0.09 | 8,484,917 | 173.5 | 259.4 | 272.0 | 63 | 0% |

### cache target 20%, database wait 500 us, 128 in flight per worker, 4 worker(s)

Actual hit rate 20%. All 6 configurations serve identical responses (one checksum): yes.

| Configuration | Work per request (ns) | vs combined, 1 lock | Lock retries per request | Requests/s | p50 (us) | p99 (us) | p99.9 (us) | Resident at end (MB) | Spread |
|---|---|---|---|---|---|---|---|---|---|
| combined, 1 lock, copy under lock (previous run) | 508 | (baseline) | 0.12 | 1,275,104 | 475.5 | 600.5 | 601.1 | 26 | 1% |
| combined, 16 locks, copy under lock | 504 | -3 ns (-0.7%) | 0.01 | 1,275,382 | 475.5 | 600.5 | 601.1 | 26 | 0% |
| combined, 16 locks, clone Ref on hit | 504 | -4 ns (-0.8%) | 0.01 | 1,275,064 | 475.5 | 600.5 | 601.1 | 26 | 0% |
| split, 1 lock, copy under lock (previous run) | 534 | +26 ns (+5.1%) | 0.21 | 1,274,910 | 475.5 | 600.5 | 601.1 | 30 | 0% |
| split, 16 locks, copy under lock | 528 | +21 ns (+4.1%) | 0.01 | 1,275,575 | 475.5 | 600.5 | 601.1 | 30 | 1% |
| split, 16 locks, clone Ref on hit | 526 | +19 ns (+3.7%) | 0.01 | 1,275,276 | 475.5 | 600.5 | 601.1 | 30 | 1% |

### cache target 100%, database wait 10 us, 256 in flight per worker, 1 worker(s)

Actual hit rate 80%. All 6 configurations serve identical responses (one checksum): yes.

| Configuration | Work per request (ns) | vs combined, 1 lock | Lock retries per request | Requests/s | p50 (us) | p99 (us) | p99.9 (us) | Resident at end (MB) | Spread |
|---|---|---|---|---|---|---|---|---|---|
| combined, 1 lock, copy under lock (previous run) | 360 | (baseline) | 0.00 | 2,768,888 | 0.3 | 142.5 | 156.8 | 82 | 2% |
| combined, 16 locks, copy under lock | 361 | +0 ns (+0.1%) | 0.00 | 2,768,785 | 0.3 | 142.3 | 156.3 | 82 | 2% |
| combined, 16 locks, clone Ref on hit | 364 | +3 ns (+0.9%) | 0.00 | 2,744,859 | 0.3 | 143.7 | 158.5 | 82 | 2% |
| split, 1 lock, copy under lock (previous run) | 395 | +35 ns (+9.7%) | 0.00 | 2,527,398 | 0.4 | 155.1 | 166.1 | 96 | 3% |
| split, 16 locks, copy under lock | 396 | +36 ns (+9.9%) | 0.00 | 2,520,845 | 0.4 | 155.3 | 168.1 | 96 | 2% |
| split, 16 locks, clone Ref on hit | 391 | +31 ns (+8.6%) | 0.00 | 2,551,925 | 0.3 | 153.4 | 163.9 | 96 | 9% |

### cache target 100%, database wait 10 us, 256 in flight per worker, 4 worker(s), 50% of requests on the 1000 hottest users

Actual hit rate 90%. All 6 configurations serve identical responses (one checksum): yes.

| Configuration | Work per request (ns) | vs combined, 1 lock | Lock retries per request | Requests/s | p50 (us) | p99 (us) | p99.9 (us) | Resident at end (MB) | Spread |
|---|---|---|---|---|---|---|---|---|---|
| combined, 1 lock, copy under lock (previous run) | 381 | (baseline) | 0.78 | 10,329,580 | 0.3 | 166.1 | 184.3 | 86 | 2% |
| combined, 16 locks, copy under lock | 358 | -23 ns (-6.0%) | 0.04 | 10,981,915 | 0.3 | 156.9 | 173.3 | 86 | 16% |
| combined, 16 locks, clone Ref on hit | 363 | -18 ns (-4.9%) | 0.04 | 10,968,734 | 0.3 | 158.8 | 174.2 | 86 | 2% |
| split, 1 lock, copy under lock (previous run) | 432 | +51 ns (+13.4%) | 2.52 | 9,210,424 | 0.4 | 186.0 | 203.7 | 100 | 5% |
| split, 16 locks, copy under lock | 379 | -2 ns (-0.6%) | 0.09 | 10,486,122 | 0.3 | 164.5 | 180.2 | 100 | 1% |
| split, 16 locks, clone Ref on hit | 379 | -3 ns (-0.7%) | 0.03 | 10,489,466 | 0.3 | 164.1 | 181.0 | 100 | 1% |

## After a flush (90% evicted at random, then a hot 1/20 of the users), 16 locks

### cache target 50%, database wait 200 us, 256 in flight per worker, 4 worker(s)

| Configuration | Resident before flush (MB) | after flush | at end | Mapped arenas at end (MB) | Work per request (ns) | Requests/s | p99 (us) | p99.9 (us) | Objects moved | Chunks freed | Chunks skipped (pinned) | Mover CPU (ms) | Longest hold, all locks (us) |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| combined, clone on hit | 51 | 52 | 54 | 39 | 388 | 10,204,733 | 268.4 | 341.1 | - | - | - | - | - |
| split, copy under lock, no mover | 60 | 61 | 63 | 48 | 410 | 9,675,907 | 285.3 | 347.6 | - | - | - | - | - |
| split, copy under lock, mover | 60 | 59 | 49 | 34 | 421 | 9,446,020 | 291.0 | 351.4 | 40,914 | 294 | 0 | 3 | 62 |
| split, clone on hit, no mover | 60 | 61 | 63 | 48 | 407 | 9,757,608 | 283.2 | 343.1 | - | - | - | - | - |
| split, clone on hit, mover | 60 | 59 | 49 | 34 | 420 | 9,460,142 | 291.0 | 350.5 | 44,701 | 329 | 1 | 4 | 50 |

