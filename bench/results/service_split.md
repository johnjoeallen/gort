# Split control block in a whole service (D17, B15 second run)

> D17 benchmarks: [1. Allocation, dereference, retained memory](split.md) · **2. A whole service** · [3. Sharded locks and clone-on-hit](b12_ref.md)  
> [← Allocation, dereference, retained memory](split.md) | [Sharded locks and clone-on-hit →](b12_ref.md)

**Question.** The [first run](split.md) timed the split control block in isolation and found real costs and real memory savings. What does it do to a **whole request**: parse, authenticate, look up a shared cache, wait for a database, render, send? And what does it cost to move bodies while that service is running?

**Answer for this test: the split shows up in the whole service when the service is CPU-bound, and not when it is waiting on its database.**

- **Time (loss).** Work per request is 4 to 12% higher with one worker and 6 to 27% higher with four. Throughput falls by 2 to 10% with one worker and by 16 to 21% with four, in the scenarios where requests are mostly served from the cache. In the database-bound scenario (20% hits, 500 us wait) the difference in work per request is 4 to 6% and throughput and latency are identical.
- **Why four workers pay more.** Reading the User's body happens inside the cache's `sync` block. The split puts a second dependent cache miss inside that lock, the lock is held longer, and the other workers retry more often: failed lock attempts per request went from 3.9 to 7.0 (50% hits) and from 3.6 to 8.3 (80% hits).
- **Memory (gain, only after the cache is emptied).** After 90% of the cache is evicted at random and traffic moves to a small hot set, the combined layout keeps its arenas (39 MB mapped, 54 MB resident at the end). With the mover running, the split layout ends at 29 MB mapped and 44 MB resident, 19% less resident memory than combined.
- **Memory (cost, always).** In steady state the split service is larger: resident at the end is 58 against 50 MB, 25 against 22 MB, 96 against 82 MB (one worker). 14 to 17% more.
- **Moving while the service runs.** The mover takes the cache lock (`mutate`) for each chunk it evacuates, so no reader can be inside a window while a body moves. The longest hold was 48 us. Moving 54,000 bodies cost the mover thread 48 ms of CPU, and the workers' work per request rose 6.8% (664 to 709 ns) and p99 latency 3.5% while it ran.

Source `bench/c/service_split_bench.c`, driver `bench/run_service_split.sh`, summary `bench/parse_service_split.py`; raw output `service_split.txt`, tables `service_split.tables.md`.
Built on the Strata C runtime (`c/runtime/rt.h`: workers, run queue, timer heap, park and wake) like the [mixed-request run](mixed.md). gcc 14.2 `-O2`, Ryzen 9 9950X3D, workers pinned to cores 2 to 5, the mover on core 6. Hand-written C, no Strata compiler.
One million users; the cache holds `Ref<User>` objects (a 64 B body each) allocated from per-worker arenas (64 KB chunks), a single cache lock, 4-way set-associative with random replacement. A hit reads the body inside `sync(cache)`; a miss waits for the simulated database (parks), builds a new User, and replaces an entry inside `mutate(cache)`, freeing the evicted one. Checksums of the rendered output are identical in the two layouts in every scenario. Steady-state rows: median of 5 runs with the layouts alternating in order (300,000 requests per worker). Flush rows: median of 3.

## Loss first: steady state

Work per request in ns, requests per second, resident MB at the end (combined, then split):

| scenario (cache target, database wait, workers) | actual hits | work per request | requests/s | resident MB |
|---|---|---|---|---|
| 50%, 200 us, 1 | 49% | 388 / 408 (+5.4%) | 2.42 M / 2.38 M (-1.8%) | 50 / 58 |
| 20%, 500 us, 1 | 20% | 427 / 446 (+4.4%) | 319 k / 319 k (0%) | 22 / 25 |
| 100%, 100 us, 1 | 80% | 379 / 420 (+10.8%) | 2.63 M / 2.37 M (-9.7%) | 82 / 96 |
| 100%, 10 us, 1 | 80% | 376 / 420 (+11.6%) | 2.65 M / 2.38 M (-10.4%) | 82 / 96 |
| 50%, 200 us, 4 | 49% | 585 / 696 (+18.9%) | 6.81 M / 5.72 M (-15.9%) | 54 / 63 |
| 20%, 500 us, 4 | 20% | 513 / 546 (+6.4%) | 1.275 M / 1.275 M (0%) | 26 / 30 |
| 100%, 10 us, 4 | 80% | 524 / 664 (+26.6%) | 7.59 M / 6.01 M (-20.8%) | 86 / 100 |

(The "100%" target gives about 80% actual hits because a 4-way random-replacement cache does not hold every key.) Latency follows throughput: p99 is 292 us against 322 us at 50% hits with four workers, 205 against 259 us at 80% hits, and the same where the service waits on the database. Run-to-run spread is 0.3 to 3% for these rows, except 7.4% for the combined layout in the last one.

Under the cache lock the cost of a lookup is the cost of the whole critical section. With the split, a hit reads the control block, then the body (two dependent cache misses, as in the [first run's pointer chase](split.md)), and the other workers wait. Failed lock attempts per request, combined against split: 0 / 0 with one worker, 3.9 / 7.0 and 3.6 / 8.3 with four. A cache with sharded locks, or a hit that copies the body after leaving the lock, would behave differently; neither was tried.

## Gain: memory after the cache is emptied

90% of the cache evicted at random once 40% of the requests were done; afterwards all traffic is on 1/20 of the users, so the cache is not refilled and the arenas stay sparse. 50% cache target, 200 us database wait, 256 requests in flight per worker.

| layout | workers | resident before flush (MB) | after flush | at end | mapped arenas at end (MB) | work per request (ns) | requests/s | p99 (us) |
|---|---|---|---|---|---|---|---|---|
| combined | 4 | 51 | 51 | 54 | 39 | 538 | 7.37 M | 296.5 |
| split, no mover | 4 | 60 | 60 | 63 | 48 | 664 | 5.99 M | 321.5 |
| split, mover running | 4 | 60 | 60 | **44** | **29** | 709 | 5.61 M | 332.8 |
| combined | 1 | 49 | 49 | 50 | 39 | 388 | 2.40 M | 298.6 |
| split, no mover | 1 | 57 | 57 | 58 | 47 | 428 | 2.21 M | 302.6 |
| split, mover running | 1 | 57 | 57 | **54** | **43** | 441 | 2.16 M | 304.1 |

The flush itself frees no memory in either layout (resident is unchanged right after it): the evicted objects are scattered, so every chunk keeps a few survivors. Only moving bodies returns it. The mover moved 53,800 bodies, freed 459 chunks, used 48 ms of CPU (about 0.9 us per body), and held the cache lock for at most 48 us at a time (4 workers; 9 ms and 18 us with one worker). What remains after moving is mostly control blocks, which never move, plus the non-arena memory of the process (the cache table, latency arrays, request pool: about 15 MB).

## How to read it

- **For an I/O-bound service the cost is small.** With a database wait that dominates and few cache hits, work per request is 4 to 6% higher and throughput and latency are unchanged. The more the service lives in the cache, the more the split costs.
- **The saving needs a trigger.** In steady state nothing is retained in either layout (freed slots are reused at once), and the split is larger. It saves memory only after a burst or a shift in traffic leaves sparse chunks, and then about 10 MB of 54 here. It cannot go below the control blocks of the survivors.
- **Moving is cheap in this design.** Because readers exist only inside `sync`, a move needs only `mutate` on the same lock: no grace period, no epoch tracking. The costs are the copy (about 1 us per body including lock waits), the cache-lock holds (under 50 us per chunk here), and a few percent of throughput while the mover runs.
- **Direction.** This supports keeping the combined slot as the default and offering the split per type, aimed at types whose objects die in a different order than they were made, with the cheaper page release tried first (see the [first run](split.md)).

## Caveats

- One process, a simulated database, a single cache lock, 64 B bodies, one allocation per miss. The pessimistic single lock amplifies the four-worker result; the one-worker rows are the ones with no contention.
- The cache holds the only reference to each User and a hit copies the body inside the lock, so there are no reference-count operations on the hit path. A design that clones a `Ref` out of the cache and reads the body afterwards, and a sharded cache lock, are measured in the [third run](b12_ref.md): the four-worker loss shrinks from 6 to 27% to 4 to 10% of work per request once the lock is sharded. That run also fixes a harness race (the back pointer was written after the arena lock dropped) that could affect this run's mover rows.
- The flush scenario is one shape of fragmentation (random eviction, then a narrower working set). Other shapes may leave more or less to recover.
- Page release of free pages (the cheaper option in the [first run](split.md)) was not run for the combined layout here.
- Resident figures include about 15 MB of memory that is not in the arenas.
- Latency percentiles are for a closed loop with 128 to 256 requests in flight per worker, so they include queueing; compare layouts, not absolute values.

## Reproduce

    bench/run_service_split.sh        # builds both layouts, runs everything, writes results/service_split.txt and service_split.tables.md (about 40 s)
