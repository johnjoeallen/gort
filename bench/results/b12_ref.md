# The mixed-request service on the D17 model: sharded locks and clone-on-hit (B12, third run)

> D17 benchmarks: [1. Allocation, dereference, retained memory](split.md) · [2. A whole service](service_split.md) · **3. Sharded locks and clone-on-hit** · [4. Refs held across a park](hold.md)  
> [← A whole service](service_split.md) | [Refs held across a park →](hold.md)

**Question.** The [whole-service run](service_split.md) used one global cache lock and copied the body inside the lock. Both choices favoured the combined layout. Does the split's cost survive when the cache lock is sharded, and when a hit clones the `Ref<User>` out of the cache and reads the body after leaving the lock (the way ordinary Gort code would hold it)?

**Answer for this test: most of the four-worker loss in the last run came from the single lock. With 16 locks the split costs 4 to 10% more work per request than the combined layout (it was 6 to 27%), and 0 to 8% of throughput (it was 16 to 21%). Cloning the Ref on a hit costs nothing measurable, and it makes the split slightly cheaper. Moving bodies still works and still saves memory, but it now has to take every shard lock and skip chunks that a request is holding.**

Source `bench/c/service_split_bench.c` (extended; it is `mixed_bench.c` plus `Ref<User>` cache objects), driver `bench/run_b12_ref.sh`, summary `bench/parse_b12_ref.py`; raw output `b12_ref.txt`, tables `b12_ref.tables.md`.
Same machine, compiler and harness as the other two D17 runs (gcc 14.2 `-O2`, Ryzen 9 9950X3D, workers on cores 2 to 5, mover on core 6, 1,000,000 users, 64 B bodies, per-worker arenas). Hand-written C, no Gort compiler. Steady rows: median of 5, the six configurations rotating in order, 300,000 requests per worker. Flush rows: median of 3. Responses are byte-identical (one checksum) in every configuration of every scenario.

## What changed from the last run

- **`shards=16`.** The cache lock becomes 16 cache-line-padded spin locks; a set belongs to shard `set % 16`. `shards=1` reproduces the previous run.
- **`clone=1`.** A hit does an atomic `strong++` inside `sync(shard)`, leaves the lock, renders from the body, then drops the Ref. Dropping the last strong frees the object, so a User evicted while a request holds it dies in that request. `clone=0` copies the body inside the lock as before.
- **Moving with clones.** A body is now read outside any lock, so the mover may not move a chunk if any object in it has `strong > 1` (a request holds a clone) or is not in the cache (held by a request alone). It takes every shard lock in ascending order, then the arena lock; workers take one shard lock then an arena lock, so there is no cycle. A skipped chunk is retried later.
- **`skew=50`.** Half the requests draw their key from the 1,000 most popular users, so the atomic count updates of clone-on-hit land on the same cache lines.
- **A race fixed in the harness.** In the previous run `obj_new` wrote the body's back pointer after releasing the arena lock, so the mover could read an uninitialised pointer for a freshly allocated slot. The pointer is now written inside the lock. The previous run's checksums and counts looked sane, so the window was probably never hit, but its mover rows were not re-run; the flush rows here are the current ones.

## Loss first: steady state

Work per request in ns (requests per second), median of 5. "Previous" is the single-lock, copy-under-lock configuration of the last run, rebuilt here.

| scenario | combined, previous | split, previous | combined, 16 locks | split, 16 locks | split, 16 locks, clone |
|---|---|---|---|---|---|
| 80% hits, 10 us database, 4 workers | 423 (9.37 M) | 531 (7.50 M) | 382 (10.40 M) | 415 (9.58 M) | 409 (9.72 M) |
| 49% hits, 200 us, 4 workers | 463 (8.54 M) | 543 (7.29 M) | 435 (9.06 M) | 470 (8.43 M) | 467 (8.48 M) |
| 20% hits, 500 us, 4 workers | 508 (1.275 M) | 534 (1.275 M) | 504 (1.275 M) | 528 (1.275 M) | 526 (1.275 M) |
| 80% hits, 10 us, 1 worker | 360 (2.77 M) | 395 (2.53 M) | 361 (2.77 M) | 396 (2.52 M) | 391 (2.55 M) |
| 90% hits, half on 1,000 hot users, 4 workers | 381 (10.33 M) | 432 (9.21 M) | 358 (10.98 M) | 379 (10.49 M) | 379 (10.49 M) |

Split against combined at the same lock count:

| scenario | one lock | 16 locks | 16 locks, clone on hit |
|---|---|---|---|
| 80% hits, 4 workers | +26% work, -20% throughput | +8.6% work, -7.9% throughput | +6.8% work, -5.9% throughput |
| 49% hits, 4 workers | +17%, -15% | +8.0%, -6.9% | +6.9%, -6.1% |
| 20% hits, 4 workers | +5%, 0% | +4.8%, 0% | +4.4%, 0% |
| 80% hits, 1 worker | +10%, -9% | +9.7%, -9% | +7.4%, -7% |
| hot keys, 4 workers | +13%, -11% | +5.9%, -4.5% | +4.4%, -4.5% |

What the numbers say:

- **Sharding removes most of the contention penalty, for both layouts.** Failed lock attempts per request with four workers fall from 1.7 (combined) and 6.3 (split) to 0.07 and 0.15. The combined layout also gets faster (0 to 10% less work per request), so the split's gap narrows because the combined layout improves less than the split does, not because the split got cheaper in isolation.
- **What remains is the cost of a second dependent cache miss per hit**, which is about 35 ns per request and does not depend on the lock. The one-worker row, which has no contention at all, shows it at +10%.
- **Clone-on-hit is free here, and helps the split.** For the combined layout the difference is within noise (-1 to +1%). For the split, taking the count in the control block and reading the body after releasing the lock shortens the critical section: 0 to 1.5% fewer ns per request, and lock retries per request halve (0.15 to 0.04 in the first row).
- **Hot keys did not hurt.** With half the requests on 1,000 users, the atomic updates cost about 5 ns per request in the combined layout and nothing in the split, and retries stayed at 0.03 to 0.04. A single very hot object, or many more workers, would show more; neither was tested.
- **Database-bound is unchanged.** At 20% hits and 500 us waits, throughput and latency are identical across all six configurations; the split adds 4 to 5% to work per request.
- **Memory.** Steady state is 14 to 17% larger for the split, as before (63 against 54 MB, 100 against 86 MB, 30 against 26 MB, 96 against 82 MB at one worker).

## Gain: the flush scenario, with moving

90% of the cache evicted at random after 40% of the run, then traffic on a hot 1/20 of the users. 50% hits, 200 us wait, four workers, 16 locks, median of 3.

| configuration | resident at end | mapped arenas | work per request | requests/s | bodies moved | chunks freed | chunks skipped (pinned) | mover CPU | longest hold (all 16 locks) |
|---|---|---|---|---|---|---|---|---|---|
| combined, clone on hit | 54 MB | 39 MB | 388 ns | 10.20 M | | | | | |
| split, copy under lock, no mover | 63 MB | 48 MB | 410 ns | 9.68 M | | | | | |
| split, copy under lock, mover | 49 MB | 34 MB | 421 ns | 9.45 M | 40,914 | 294 | 0 | 3 ms | 62 us |
| split, clone on hit, no mover | 63 MB | 48 MB | 407 ns | 9.76 M | | | | | |
| split, clone on hit, mover | 49 MB | 34 MB | 420 ns | 9.46 M | 44,701 | 329 | 1 | 4 ms | 50 us |

- **Moving still returns the memory**, with and without clones: 49 MB at the end against 63 MB for the split without the mover and 54 MB for the combined layout. That is 9% below combined; the last run, with a single lock, ended at 44 MB (18% below). The mover here moved fewer bodies (41,000 to 45,000 against 54,000); I did not investigate why, and with three runs I would not read much into the difference.
- **Pinned chunks were nearly absent (0 and 1 of 330).** That is because a request holds its clone only across the render, which does not park. A Ref held across a database wait would pin a chunk for hundreds of microseconds, and then skips would be common. That case, which is the D2 interaction, is not measured.
- **The mover's price while it ran** was +2.7 to +3.2% work per request over the split without the mover (the last run: +6.8%) and an all-locks hold of 50 to 62 us. Taking every shard lock makes each move a stop-the-world pause for the cache, shorter than it sounds because a chunk holds only a few hundred bodies. A mover that locked only the shards of the bodies it moves would pause less; I did not build it.
- **Mover CPU fell from 48 ms to 3 to 4 ms.** I believe the earlier figure included the mover spinning on the single contended lock, but did not check.

## Caveats

- One process, a simulated database, 64 B bodies, one allocation per miss, 16 shards chosen once (no sweep of 4, 64, ...).
- A request holds its clone only across a non-parking render. Holding a Ref across a park and storing one for good are measured in the [fourth run](hold.md).
- Reference counts are atomic everywhere in this run (the cache entry is the only strong reference at rest). A thread-local or deferred counting scheme was not tried.
- Skew was one setting (50% on 1,000 users); no single-object hot spot.
- The first row's combined-previous configuration has a 14% run-to-run spread; the deltas that matter are 5% or more, so read the single-digit ones as ranges, not points.
- Still no real sockets and no Go or Java version of the service.

## Reproduce

```
bench/run_b12_ref.sh 5 300000
```
