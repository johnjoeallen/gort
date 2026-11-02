## Steady state (no flush, no mover)

### cache target 50%, database wait 200 us, 256 requests in flight per worker, 1 worker(s)

Actual hit rate 49%. Same checksums in both layouts: yes.

| Layout | Work per request (ns) | Difference | Cache-lock retries per request | Requests/s | p50 (us) | p99 (us) | p99.9 (us) | Resident at end (MB) |
|---|---|---|---|---|---|---|---|---|
| combined | 388 | (baseline) | 0.00 | 2,418,234 | 162.9 | 243.6 | 252.7 | 50 |
| split | 408 | +21 ns (+5.4%) | 0.00 | 2,375,694 | 164.1 | 246.2 | 255.5 | 58 |

Run-to-run spread of the combined layout's work per request: 0.8% (min to max over 5 runs); of the split layout's: 0.5%.

### cache target 20%, database wait 500 us, 128 requests in flight per worker, 1 worker(s)

Actual hit rate 20%. Same checksums in both layouts: yes.

| Layout | Work per request (ns) | Difference | Cache-lock retries per request | Requests/s | p50 (us) | p99 (us) | p99.9 (us) | Resident at end (MB) |
|---|---|---|---|---|---|---|---|---|
| combined | 427 | (baseline) | 0.00 | 319,053 | 475.4 | 600.4 | 600.9 | 22 |
| split | 446 | +19 ns (+4.4%) | 0.00 | 319,073 | 475.4 | 600.4 | 601.0 | 25 |

Run-to-run spread of the combined layout's work per request: 0.4% (min to max over 5 runs); of the split layout's: 0.9%.

### cache target 100%, database wait 100 us, 256 requests in flight per worker, 1 worker(s)

Actual hit rate 80%. Same checksums in both layouts: yes.

| Layout | Work per request (ns) | Difference | Cache-lock retries per request | Requests/s | p50 (us) | p99 (us) | p99.9 (us) | Resident at end (MB) |
|---|---|---|---|---|---|---|---|---|
| combined | 379 | (baseline) | 0.00 | 2,628,296 | 0.3 | 229.5 | 243.8 | 82 |
| split | 420 | +41 ns (+10.8%) | 0.00 | 2,372,940 | 0.4 | 245.3 | 260.2 | 96 |

Run-to-run spread of the combined layout's work per request: 1.4% (min to max over 5 runs); of the split layout's: 2.4%.

### cache target 100%, database wait 10 us, 256 requests in flight per worker, 1 worker(s)

Actual hit rate 80%. Same checksums in both layouts: yes.

| Layout | Work per request (ns) | Difference | Cache-lock retries per request | Requests/s | p50 (us) | p99 (us) | p99.9 (us) | Resident at end (MB) |
|---|---|---|---|---|---|---|---|---|
| combined | 376 | (baseline) | 0.00 | 2,653,198 | 0.3 | 148.3 | 162.4 | 82 |
| split | 420 | +44 ns (+11.6%) | 0.00 | 2,377,961 | 0.4 | 164.9 | 177.7 | 96 |

Run-to-run spread of the combined layout's work per request: 3.9% (min to max over 5 runs); of the split layout's: 6.1%.

### cache target 50%, database wait 200 us, 256 requests in flight per worker, 4 worker(s)

Actual hit rate 49%. Same checksums in both layouts: yes.

| Layout | Work per request (ns) | Difference | Cache-lock retries per request | Requests/s | p50 (us) | p99 (us) | p99.9 (us) | Resident at end (MB) |
|---|---|---|---|---|---|---|---|---|
| combined | 585 | (baseline) | 3.86 | 6,807,585 | 197.8 | 292.2 | 317.8 | 54 |
| split | 696 | +111 ns (+18.9%) | 7.02 | 5,724,849 | 221.1 | 322.1 | 340.8 | 63 |

Run-to-run spread of the combined layout's work per request: 1.1% (min to max over 5 runs); of the split layout's: 0.5%.

### cache target 20%, database wait 500 us, 128 requests in flight per worker, 4 worker(s)

Actual hit rate 20%. Same checksums in both layouts: yes.

| Layout | Work per request (ns) | Difference | Cache-lock retries per request | Requests/s | p50 (us) | p99 (us) | p99.9 (us) | Resident at end (MB) |
|---|---|---|---|---|---|---|---|---|
| combined | 513 | (baseline) | 0.17 | 1,275,114 | 475.5 | 600.5 | 601.1 | 26 |
| split | 546 | +33 ns (+6.4%) | 0.22 | 1,274,992 | 475.5 | 600.5 | 601.2 | 30 |

Run-to-run spread of the combined layout's work per request: 0.1% (min to max over 5 runs); of the split layout's: 0.4%.

### cache target 100%, database wait 10 us, 256 requests in flight per worker, 4 worker(s)

Actual hit rate 80%. Same checksums in both layouts: yes.

| Layout | Work per request (ns) | Difference | Cache-lock retries per request | Requests/s | p50 (us) | p99 (us) | p99.9 (us) | Resident at end (MB) |
|---|---|---|---|---|---|---|---|---|
| combined | 524 | (baseline) | 3.57 | 7,589,172 | 0.5 | 205.5 | 220.4 | 86 |
| split | 664 | +140 ns (+26.6%) | 8.29 | 6,010,426 | 0.6 | 259.4 | 276.9 | 100 |

Run-to-run spread of the combined layout's work per request: 7.4% (min to max over 5 runs); of the split layout's: 4.2%.

## After a flush: 90% of the cache evicted at random, then traffic on a hot 1/20 of the users

### cache target 50%, database wait 200 us, 256 requests in flight per worker, 4 worker(s)

| Layout | Resident before flush (MB) | after flush | at end | Mapped arenas at end (MB) | Work per request (ns) | Requests/s | p99 (us) | p99.9 (us) | Objects moved | Chunks freed | Mover CPU (ms) | Longest cache-lock hold by the mover (us) |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| combined | 51 | 51 | 54 | 39 | 538 | 7,373,695 | 296.5 | 401.3 | - | - | - | - |
| split, no mover | 60 | 60 | 63 | 48 | 664 | 5,992,621 | 321.5 | 400.1 | - | - | - | - |
| split, mover running | 60 | 60 | 44 | 29 | 709 | 5,614,373 | 332.8 | 417.6 | 53,812 | 459 | 48 | 48 |

### cache target 50%, database wait 200 us, 256 requests in flight per worker, 1 worker(s)

| Layout | Resident before flush (MB) | after flush | at end | Mapped arenas at end (MB) | Work per request (ns) | Requests/s | p99 (us) | p99.9 (us) | Objects moved | Chunks freed | Mover CPU (ms) | Longest cache-lock hold by the mover (us) |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| combined | 49 | 49 | 50 | 39 | 388 | 2,397,983 | 298.6 | 364.8 | - | - | - | - |
| split, no mover | 57 | 57 | 58 | 47 | 428 | 2,212,872 | 302.6 | 369.5 | - | - | - | - |
| split, mover running | 57 | 57 | 54 | 43 | 441 | 2,162,854 | 304.1 | 372.6 | 7,328 | 71 | 9 | 18 |

