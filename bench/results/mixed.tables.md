### cache target 100%, database wait 100 us, 256 requests in flight per worker, 1 worker(s)

Actual cache hit rate 80%.

| Variant | Work per request (ns) | Loss vs raw | Requests/s | p50 latency (us) | p99 latency (us) |
|---|---|---|---|---|---|
| raw pointer | 327 | (baseline) | 3,032,895 | 0.3 | 206.2 |
| raw pointer, second copy (noise) | 328 | +0.7 ns (+0.2%) | 3,028,515 | 0.3 | 206.5 |
| handle | 326 | -1.3 ns (-0.4%) | 3,042,659 | 0.3 | 205.9 |
| handle, reload every access | 323 | -3.8 ns (-1.2%) | 3,069,248 | 0.3 | 204.8 |

Run-to-run spread of the raw pointer's work per request: 1.3% (min to max over 8 runs).

### cache target 50%, database wait 200 us, 256 requests in flight per worker, 1 worker(s)

Actual cache hit rate 49%.

| Variant | Work per request (ns) | Loss vs raw | Requests/s | p50 latency (us) | p99 latency (us) |
|---|---|---|---|---|---|
| raw pointer | 365 | (baseline) | 2,411,100 | 162.4 | 242.6 |
| raw pointer, second copy (noise) | 364 | -1.3 ns (-0.4%) | 2,422,916 | 162.4 | 242.1 |
| handle | 364 | -0.8 ns (-0.2%) | 2,420,362 | 162.5 | 242.4 |
| handle, reload every access | 360 | -4.7 ns (-1.3%) | 2,430,588 | 162.4 | 241.9 |

Run-to-run spread of the raw pointer's work per request: 3.1% (min to max over 8 runs).

### cache target 20%, database wait 500 us, 128 requests in flight per worker, 1 worker(s)

Actual cache hit rate 20%.

| Variant | Work per request (ns) | Loss vs raw | Requests/s | p50 latency (us) | p99 latency (us) |
|---|---|---|---|---|---|
| raw pointer | 401 | (baseline) | 318,797 | 475.4 | 600.4 |
| raw pointer, second copy (noise) | 403 | +2.2 ns (+0.5%) | 318,636 | 475.4 | 600.4 |
| handle | 401 | +0.8 ns (+0.2%) | 318,457 | 475.4 | 600.4 |
| handle, reload every access | 397 | -3.7 ns (-0.9%) | 318,446 | 475.4 | 600.4 |

Run-to-run spread of the raw pointer's work per request: 0.5% (min to max over 8 runs).

### cache target 100%, database wait 100 us, 256 requests in flight per worker, 4 worker(s)

Actual cache hit rate 80%.

| Variant | Work per request (ns) | Loss vs raw | Requests/s | p50 latency (us) | p99 latency (us) |
|---|---|---|---|---|---|
| raw pointer | 382 | (baseline) | 10,228,771 | 0.3 | 230.8 |
| raw pointer, second copy (noise) | 381 | -0.9 ns (-0.2%) | 10,299,976 | 0.3 | 230.2 |
| handle | 379 | -2.6 ns (-0.7%) | 10,355,412 | 0.3 | 229.6 |
| handle, reload every access | 390 | +7.9 ns (+2.1%) | 10,092,374 | 0.3 | 233.6 |

Run-to-run spread of the raw pointer's work per request: 2.1% (min to max over 8 runs).

### cache target 20%, database wait 500 us, 128 requests in flight per worker, 4 worker(s)

Actual cache hit rate 20%.

| Variant | Work per request (ns) | Loss vs raw | Requests/s | p50 latency (us) | p99 latency (us) |
|---|---|---|---|---|---|
| raw pointer | 443 | (baseline) | 1,272,767 | 475.4 | 600.4 |
| raw pointer, second copy (noise) | 446 | +2.6 ns (+0.6%) | 1,272,982 | 475.4 | 600.4 |
| handle | 444 | +0.8 ns (+0.2%) | 1,273,148 | 475.4 | 600.4 |
| handle, reload every access | 443 | -0.1 ns (-0.0%) | 1,273,811 | 475.4 | 600.4 |

Run-to-run spread of the raw pointer's work per request: 1.0% (min to max over 8 runs).

