# Mixed request: parse, authenticate, cache, database wait, render (D15, B12)

> D15 handle benchmarks: [1. Handle indirection: simple loop](handle.md) · [2. Realistic loop bodies](handle_real.md) · **3. Mixed request**  
> [← Realistic loop bodies](handle_real.md) | (last) →

**Question.** The [realistic run](handle_real.md) timed loops with no I/O. A real request parses, checks a lock-protected cache, often waits on a database,
then renders. Does reaching container data through a non-moving header (D15) show up in the **whole request**?

**Answer for this test: no. Across five scenarios the handle's difference from a raw pointer is between -0.7% and +0.2% (the noise is 0.5 to 3%), and even
the pessimistic build that reloads the pointer at every access is within +2.1%. Requests per second and latency (p50, p99) are the same.**

Source `bench/c/mixed_bench.c`, driver `bench/run_mixed.sh`, summary script `bench/parse_mixed.py`; raw output `mixed.txt`, per-scenario tables `mixed.tables.md`.
Runs on the Gort C runtime (`c/runtime/rt.h`: workers, run queue, timer heap, park and wake). gcc 14.2 `-O2`, workers pinned to cores 2 to 5.
Hand-written C, no Gort compiler.

## What a request does

1. **Read the request**: a 230-byte HTTP request is written into a buffer (it stands in for the socket read), then **parsed** into offsets: request line, 8 headers, case-insensitive header-name compares, the id in the path.
2. **Authenticate**: hash the bearer token and probe a write-once key table (no lock, rule R1).
3. **Cache lookup** under a lock (`sync`): a 4-way set-associative table of 64-byte entries.
4. **On a miss, park** on a simulated database wait (a timer, 100 to 500 microseconds, jittered +-20%) while the worker runs other requests; on wake, decode the row and `mutate` the cache.
5. **Render** a JSON body and an HTTP header into two **growable buffers** that start small and really grow (about three reallocations a request).
6. **Send**: read every output byte once (a checksum stands in for the copy to the socket).

256 (or 128) requests per worker are always in flight, as vthreads. One worker and four workers sharing the cache.

## The three builds

They differ only in how the read loops (parse, header compare, hash, key probe, cache probe, send checksum) reach container data:

| Build | Reads |
|---|---|
| raw pointer | base pointer copied to a local once per window, after the lock |
| handle | every read is `h->ptr[i]`; the C compiler decides when to reload (what a Gort-to-C emitter would write) |
| handle, reload every access | compiler barrier before every read: the pointer is reloaded each time (the pessimistic bound) |

The appends into the growable output buffers are identical in all three: a buffer that can move must be reached through its header in any design.
A fourth run is a second copy of the raw build, to show the noise. Checksums of all responses are equal in every build (each serves the same requests).
The three builds compile to different code (the request function is 890, 898 and 867 instructions).

## What the handle loses

"Work per request" is the time a worker spends running tasks and timers per request (waiting for the database is not counted). Median of 8 runs, interleaved, order rotated.

| Scenario (cache hit rate, database wait, workers) | Raw pointer | Handle | Handle, reload every access | Noise (raw again) |
|---|---|---|---|---|
| 80%, 100 us, 1 worker | 327 ns | 326 ns (-0.4%) | 323 ns (-1.2%) | +0.2% |
| 49%, 200 us, 1 worker | 365 ns | 364 ns (-0.2%) | 360 ns (-1.3%) | -0.4% |
| 20%, 500 us, 1 worker | 401 ns | 401 ns (+0.2%) | 397 ns (-0.9%) | +0.5% |
| 80%, 100 us, 4 workers | 382 ns | 379 ns (-0.7%) | 390 ns (**+2.1%**) | -0.2% |
| 20%, 500 us, 4 workers | 443 ns | 444 ns (+0.2%) | 443 ns (0.0%) | +0.6% |

Run-to-run spread of the raw build was 0.5 to 3.1% (minimum to maximum over 8 runs). No difference is larger than the spread of its own scenario; the
largest, +2.1% (8 ns), equals its scenario's spread. A negative number is a faster handle build, which is noise or code layout (the barrier build has fewer instructions
overall); it is not a gain from the handle.

Throughput and latency do not move: requests per second differ by under 1.5% in every scenario (the one-worker, 80% scenario runs about 3.0 million requests per second
saturating the worker; the database-heavy ones are bounded by the wait), and p50 and p99 latency are the same to within 1 microsecond except 4 workers at 80% (p99 230.8 vs 233.6 us).

## What the time is made of

A request costs 330 to 440 ns of work here. A database wait is 100 to 500 **microseconds**, 300 to 1500 times longer. The container-reading loops are part of those 330 to
440 ns, and the handle changes only how they reach the data. I did not profile how large a share of the work that is, so I cannot say what fraction of the 330 ns a
loss could apply to; the result says the total did not move.

## Caveats

- **In-process requests.** No sockets and no syscalls: a real request adds a few microseconds of kernel time (the HTTP benchmark is about 3 us a request), which makes the handle's share even smaller. The database wait is a timer, not a real connection.
- **The cache does not grow.** It is a fixed set-associative table, so a growing hash table's rehash is not exercised. The growth that is exercised is the output buffers (identical in all builds) and the cache target of "100%" reaches only 80% hits because of set conflicts; the table reports the actual rate.
- **No contention beyond four workers**, and the lock is an uncontended spin lock for most requests. Contended locks, many more cores and larger tables are not tested.
- **The reload frequencies are not a compiler's.** "Handle" is what gcc generates from C that reads through the header, which is plausible for generated code but not the output of a Gort compiler, and the may-grow analysis that decides window length does not exist yet.
- One machine, one compiler, one request shape.

## Conclusion

Measured: with a realistic request (parse, authenticate, lock-protected lookup, database wait, growable-buffer rendering), reaching container data through a non-moving header is indistinguishable from a raw pointer
in work per request, throughput and latency (within 3% noise, worst case +2.1% for the pessimistic reload-every-access build). Direction, not measured: this supports the D15 recommendation for I/O-shaped servers; the open
risk remains how often a compiler would end deref windows in real Gort code.

---
> D15 handle benchmarks: [1. Handle indirection: simple loop](handle.md) · [2. Realistic loop bodies](handle_real.md) · **3. Mixed request**  
> [← Realistic loop bodies](handle_real.md) | (last) →
