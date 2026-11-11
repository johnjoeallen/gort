# Contention budget: when shared-state synchronisation matters

**Question.** A web service has 10,000 requests in flight, all of them hitting a shared cache and blocking on a database. Does the
choice of lock for the shared cache matter? And if not, why build anything more than a lock?

**Answer.** For that service, no. A lock costs nothing until the **rate** of acquisitions on one lock nears a fixed ceiling of a few
million per second. A request blocked on the database is neither holding nor taking the lock, so the number in flight is irrelevant.
Connection pools cap how fast requests can reach the database, and that keeps a database-bound service orders of magnitude under
the ceiling. The ceiling is reached by in-memory services at hundreds of thousands to millions of requests per second, and by
caches whose hit path bypasses the pool. Those cases need sharding first, and possibly borrowed reads later. Neither requires a
language change.

Status: the ceilings are **Measured** (hand-written C emulation and Go, Java and Rust, one machine). The worked examples are
arithmetic on those measurements. The recommendations are **Direction**.

Sources: [`REPORT_W2SYNC.md`](https://github.com/johnjoeallen/gort/blob/main/bench/rcmodel/REPORT_W2SYNC.md) (schemes for the Gort model),
[`REPORT_W2X.md`](https://github.com/johnjoeallen/gort/blob/main/bench/rcmodel/REPORT_W2X.md) (14 implementations under one protocol, with real work),
[`b12_ref.md`](https://github.com/johnjoeallen/gort/blob/main/bench/results/b12_ref.md) (mixed service, single vs sharded cache lock),
[mini-redis](../journey/05-mini-redis.md) and [locks that park](../journey/06-locks.md).

## 1. The model

Treat a lock as a server with a fixed capacity.

- **C, the ceiling:** the most acquisitions per second one lock sustains when every core wants it. It depends on the mechanism and the
  core count, not on the work the program does elsewhere.
- **λ, the demand on that lock:** λ = X × k, where X is requests per second and k is acquisitions of **that one lock** per request.
- **U = λ / C, the lock's utilisation.** Below about 30% the queueing delay is a fraction of one hold time (tens to hundreds of
  nanoseconds) and invisible in throughput and latency. Near 100%, the lock caps the service.

Two consequences follow:

- **Time spent waiting elsewhere does not count.** A request parked on a database, a timer or a pool is not acquiring the lock. Only the
  rate at which requests pass through the lock matters, and only CPU time between acquisitions spreads them out.
- **The budget is per lock.** Sharding a structure into N locks divides λ by up to N (less for skewed keys: the hottest key sets the limit).

## 2. Measured ceilings

All threads on one key, 5% writes, 16 cores (Ryzen 9 9950X3D), total operations per second. Acquisitions and operations are one-for-one.

| mechanism | ceiling C, no work | delivered with ~1 µs of CPU work per access (demand 15.6M/s) |
|---|---|---|
| exclusive spinlock, counted reads (the benchmark's lock) | 3.0M | 4.0M (26% of demand) |
| reader-writer stripes, counted reads | 5.9M | - |
| Rust `RwLock<Arc>` | 7.4M | 9.5M (61%) |
| Go `sync.RWMutex` | 59M | 2.5M (16%) |
| lock-free, counted reads | 12M | 11.6M (75%) |
| Java `ConcurrentHashMap` (no lock, no count) | 101-107M | 14.6M (93%) |
| Gort model, borrowed reads + hazard pointers | 219M | 14.5M (93%) |

With about 10 µs of CPU work per access, demand is 1.6M/s, under every ceiling. Every mechanism delivers 97-100%, including the
spinlock, except Go `RWMutex` (44%).

Two cautions:

- **A parking lock can do worse under load than its no-work ceiling suggests.** Go `RWMutex` sustains 59M/s flat out, but only 2.5M/s once
  readers do 1 µs of work, presumably because readers park behind a pending writer and pay a wake each time. Gort's `sync` lock
  also parks. Pure parking lost 55% of throughput at a 10% write mix until spin-then-park was added ([locks that park](../journey/06-locks.md)).
  **Its ceiling in this harness has not been measured** (section 6).
- **C shrinks as cores grow.** The borrowing schemes lost a third of their throughput going from 8 to 16 threads, when the run spread across two
  CCDs. Demand grows with cores while the ceiling falls, so a budget that fits on 16 cores may not fit on 64.

## 3. What bounds X: pools

The request rate X is the smallest of the limits on the request's path:

- **CPU:** X ≤ cores / CPU time per request.
- **Each pool on the path (Little's law):** X ≤ pool size / (time a request holds a pool slot × fraction of requests that use the pool).
  This applies to database connections, outbound HTTP clients, the blocking-FFI offload pool and any `Limiter`.

A database pool of 50 connections with 2 ms queries admits at most 25,000 queries per second, whatever the number of requests in
flight. Requests beyond that park in the pool's queue. A parked request costs memory (a frame), but it does not touch the cache lock.

**The pool is itself shared state.** Acquiring and releasing a connection is about two lock operations per query, so its demand is at most
2 × pool size / query time: 50k/s for the pool above, about 1.6% of the weakest ceiling. Waiters park rather than spin. When a burst
of P queries completes together, P requests reach the cache lock at once, and the lock drains them in P × hold time (50 × about 0.3 µs
is about 15 µs, against a 2 ms query). The exception is a pool in front of a very fast backend: 50 connections at 50 µs is 1M
queries/s, which is 2M pool operations per second on one lock. Such a pool should be per worker or sharded.

**Pools cap the miss path, not the hit path.** A cache exists so that most requests skip the database. With hit rate h, the pool bounds
X at pool / (query time × (1 - h)). At 90% hits, that is ten times the pool's own limit.

## 4. Worked examples

C = 3M/s (the weakest mechanism measured) unless stated.

| service | X (requests/s) | k on the hottest lock | λ | U | verdict |
|---|---|---|---|---|---|
| The question: 10k req/s, each touching a single-lock cache 10 times, then the database | 10,000 | 10 | 100k/s | 3% | irrelevant |
| Database-bound, no cache: pool 50, 2 ms query, every request queries, 10 accesses to one lock | ≤ 25,000 (pool) | 10 | 250k/s | 8% | irrelevant |
| Same, with a cache in front at 90% hits, 20 µs CPU per request on 16 cores | ≤ 250,000 (pool) and ≤ 800,000 (CPU) | 10 | 2.5M/s | 83% | **at the ceiling** |
| Same cache, sharded 16 ways, uniform keys | 250,000 | 10 / 16 | 156k/s per shard | 5% | irrelevant |
| Same, Zipf keys with the hottest key at 2% of accesses | 250,000 | 0.2 | 50k/s on the hottest key's shard | 2% | irrelevant |
| mini-redis: one shared database lock, one acquisition per command, 4 workers | 1.1M-1.94M (measured) | 1 | 1.1M-1.94M/s | 37-65% (19-33% with RW stripes) | **matters** |
| B12 mixed service, in-process requests, simulated database (a timer, no pool), 4 workers | 8.5M-10.4M (measured) | 1 | 8.5M-10.4M/s | over | **matters**: the single lock was most of the 4-worker loss; 16 locks cut failed attempts per request from 1.7 to 0.07 |

For the database-bound rows the result does not depend on the lock: B12 at 20% hits and a 500 µs wait shows identical throughput
and latency in all six configurations. The lock matters only when the hit path runs at hundreds of thousands of requests per second through one lock, or the
service has no slow backend at all.

## 5. What Gort already gets right, and what follows

Already **Decided**, and part of why the budget holds:

- **A `sync` holder never parks** (compile error). Holding the cache lock across the database call, the one way a database-bound
  service *can* make its lock the bottleneck, cannot be written. A cache fill has to be: `sync (reads cache)`, miss, leave, query,
  `sync (writes cache)`, insert.
- **A `sync` on an unshared object costs a flag test.** Counts are non-atomic unless the object is shared (R6, D18). With no work,
  the atomic path costs about 7.4 ns per pass against 0.75 for a local one; with real work, neither is visible.

**Direction** for v1:

1. **Shared containers use the `sync` lock with counted reads.** No borrowed reads and no hazard pointers. Every database-bound row
   above leaves the weakest mechanism at 8% or less.
2. **The standard cache and map types are striped by default** (16 or more locks). This is the cheapest lever, and it is measured: it fixed B12,
   and it turns the 83% row into 5%.
3. **Pools in front of fast backends are per worker or sharded.** A connection pool is a lock taken twice per query. That is invisible at
   millisecond queries and a hot lock at 50 µs.
4. **Single-flight cache fills in the standard library.** Concurrent misses on one key should wait for one query, not send P of them.
   This is the thundering-herd fix, and it is independent of the lock.
5. **Per-lock telemetry from the runtime:** acquisitions per second, failed fast-path attempts and parks per lock, so a service can see
   its U instead of guessing. B12 already counted failed attempts per request. Exposing that is cheap.
6. **Borrowed reads with hazard pointers stay in reserve** for the in-memory class (mini-redis, gateways, rate limiters), where they
   raise the hot-key ceiling from about 3-6M/s to 219M/s. Adopting them later needs no source change: escaping borrows are
   promoted to counted references automatically, with a compiler warning. See `REPORT_W2SYNC.md` for why hazard pointers and not QSBR.
7. **Hot write counters** (metrics, rate-limit tallies) are sharded per worker and summed on read. One atomic counter has the same
   ceiling as a lock, and no read-side scheme helps a write.

## 6. Open

- **Gort's own `sync` lock (spin, then FIFO park) has not been run in the W2 harness**, with or without work. Go's parking `RWMutex` shows that
  parking can degrade far below the no-work ceiling under load. This is the number every row in section 4 rests on.
- **Tail latency.** All the ceilings are throughput. Queueing on a lock shows in p99 first. B12's p99 moved by 3 µs at most, but it was not
  run near the ceiling.
- **More cores.** One 16-core machine. The 8-to-16 drop says the ceilings will be lower on larger parts.
- **Real pools.** B5 (database pool and queries) is still unbuilt. The pool arithmetic in section 3 is Little's law, not a measurement.
