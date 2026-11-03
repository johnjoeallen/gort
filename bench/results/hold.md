# A Ref held across a park, and Refs held for good (D17 with D2, B12 fourth run)

> D17 benchmarks: [1. Allocation, dereference, retained memory](split.md) · [2. A whole service](service_split.md) · [3. Sharded locks and clone-on-hit](b12_ref.md) · **4. Refs held across a park**  
> [← Sharded locks and clone-on-hit](b12_ref.md) | (last) →

**Question.** In the [last run](b12_ref.md) a request held its cloned `Ref<User>` only across a render that does not park, so the mover almost never found a pinned chunk. In real code a `Ref` can be held across a database call, a slow client write or a `sleep`, or be stored somewhere long-lived. A body that someone holds cannot move (it is read outside any lock). How much does that cost, and when does it stop the mover?

**Answer for this test: a Ref held across a park, for as long as 10 ms, costs nothing in steady state and does not change what the mover recovers, only how much it wastes trying. Refs that are never dropped are different: when 0.1% of hits keep their Ref for good the mover recovers 9 MB instead of 14 MB, and at 1% it recovers nothing.**

Source `bench/c/service_split_bench.c` (new options `hold=US` and `stash=PERMILLE`), drivers `bench/run_hold.sh` and `bench/run_stash.sh`, summary `bench/parse_hold.py`; raw output `hold.txt`, `stash.txt`, tables `hold.tables.md`, `stash.tables.md`. Same service, machine and settings as the [last run](b12_ref.md): split layout, 16 cache locks, 50% hits, 200 us database, four workers, 256 requests in flight per worker, flush scenario (90% evicted at random, then a hot 1/20 of the users). Hand-written C. Steady rows: median of 5. Flush rows: median of 3.

## What was changed

- **`hold=US`.** After a cache hit the request parks (a timer, the worker runs other requests) for US microseconds before it renders. With `clone=1` it holds the cloned Ref across that park, so the object is pinned for that long. With `clone=0` it holds a copy: the same queueing and latency with nothing pinned, the control.
- **`stash=PERMILLE`.** That many per thousand hits never drop their clone: a Ref stored in a long-lived structure. Those objects are pinned for the rest of the run. Stash was not combined with a hold, and not run on the combined layout (where nothing moves).
- The mover is unchanged from the last run: all shard locks, then the arena lock, skip the whole chunk if any object in it has `strong > 1` or is held by a request alone. It retries other chunks on its next pass.

## Held across a park: steady state

500 us hold after every hit, no flush, no mover.

| Configuration | Work per request | Requests/s | p50 | p99 | Resident |
|---|---|---|---|---|---|
| combined, copy held | 501 ns | 2.93 M | 240 us | 501 us | 54 MB |
| combined, Ref held | 502 ns | 2.93 M | 240 us | 501 us | 54 MB |
| split, copy held | 527 ns | 2.93 M | 240 us | 501 us | 63 MB |
| split, Ref held | 523 ns | 2.93 M | 240 us | 501 us | 63 MB |

Holding the Ref instead of a copy makes no measurable difference: the count is touched twice per request, and throughput is set by the hold. (Work per request is higher than in the last run; the copy-held control pays the same, and I did not investigate why.)

## Held across a park: after the flush, with the mover

| Hold | Resident at end, copy held / Ref held | Mapped | Mover attempts | Chunks freed | Skipped (pinned) | Work per request, copy / Ref held |
|---|---|---|---|---|---|---|
| 100 us | 46 / 47 MB | 30 / 32 MB | 309 / 326 | 309 / 294 | 0 / 32 (10%) | 472 / 469 ns |
| 500 us | 35 / 35 MB | 20 / 20 MB | 1,105 / 1,426 | 1,104 / 749 | 1 / 683 (48%) | 484 / 479 ns |
| 2 ms | 35 / 35 MB | 20 / 20 MB | 2,417 / 3,156 | 2,414 / 693 | 2 / 2,437 (77%) | 578 / 582 ns |
| 10 ms | 35 / 35 MB | 20 / 20 MB | 3,645 / 6,608 | 3,644 / 1,465 | 1 / 5,143 (78%) | 692 / 655 ns |

(Without the mover, the split ends at 63 MB and the combined layout at 54 MB for every hold.)

- **The mover reaches the same memory.** Held Refs pin an object only while the request waits; the mover sees a skipped chunk as free on a later pass. With up to about 1,000 requests in flight and half of them hits, about 500 objects are pinned at an instant. At holds of 2 ms and more roughly three of every four attempts hit a pinned chunk and are wasted, and the mover takes about twice as many passes, but the final resident and mapped memory are identical to the control.
- **No cost shows up in the workers.** Work per request with a mover and a held Ref is within noise of the same with a held copy (differences of -5% to +1%, inside the run-to-run spread). The longest all-locks hold stayed at 31 to 83 us in all rows; a skipped chunk is cheap to detect.
- **The numbers of bodies moved are not comparable across rows.** The mover runs for the whole run, and with long holds the run lasts several seconds, so it keeps relocating the same bodies from chunk to chunk (608,000 moves in the 2 ms control row, with a hot set of about 50,000 users). That is wasted work in my mover; I did not investigate or tune it.

## Held for good

Flush scenario, no hold, `stash` of 0, 0.1%, 1% and 10% of hits, split layout, mover on (median of 3). A stashed object stays pinned until the run ends, even if the cache evicts it.

| Hits that keep their Ref | Resident at end, mover off / on | Mapped, off / on | Mover attempts | Chunks freed | Skipped (pinned) | Work per request, off / on |
|---|---|---|---|---|---|---|
| 0 | 63 / 49 MB | 48 / 34 MB | 302 | 302 | 0 | 410 / 424 ns |
| 0.1% (about 600 objects) | 63 / 54 MB | 48 / 39 MB | 319 | 239 | 78 (24%) | 410 / 420 ns |
| 1% (about 6,000) | 63 / 63 MB | 48 / 47 MB | 332 | 54 | 221 (67%) | 410 / 414 ns |
| 10% | 63 / 63 MB | 48 / 48 MB | 310 | 51 | 259 (84%) | 410 / 413 ns |

- **Pins that never go away scatter, and a scattered pin blocks its whole chunk.** A chunk holds a few hundred bodies; about 600 pinned objects spread over about 330 chunks blocks about a quarter of the mover's attempts (24%), and 6,000 block two thirds (67%). At 1% the mover recovers only the 1 MB of chunks that were already empty.
- **The mover's all-or-nothing rule is the weak point.** It does not move the unpinned bodies of a chunk it cannot free. Moving those would at least pack the survivors into fewer chunks, leaving only the pinned objects' chunks behind; I did not build that, so the 1% and 10% rows are a worst case for this mover, not for the design.
- **It is a bound on how often a long-lived Ref can be created, not a failure.** The runs stayed correct (identical checksums with and without stashing, no crash, no lost object), and the service lost no throughput. What is lost is only the memory recovery.

## What this means for D17 and D2

- **A Ref held across a park is fine.** The cost is a few wasted mover passes, not memory and not latency. D2 (borrowed values across a park) does not need a special rule for moving as long as pins are short compared to the mover's period.
- **A Ref stored long-lived is the case to design for.** A type whose objects are routinely stored in per-connection state, queues or other caches will block moving of its chunks. Options, none tried: move the unpinned bodies and keep the pinned ones in place; a "pin" count separate from `strong` so the compiler can tell a stored Ref from a held one; or tell the mover which types are stored long (the opt-in per type already in the direction).
- **Moving stays opt-in.** This strengthens the earlier direction: the combined slot by default, the split plus mover for types whose Refs are short-lived or confined to a lock.

## Caveats

- One flush shape, one hot set, 64 B bodies, 16 shards, four workers. The pin density depends on the ratio of pinned objects to chunks, so a bigger heap would tolerate proportionally more pins.
- The hold is a fixed timer on every hit; a real mix would have a spread of holds.
- Stashed Refs are chosen at random across objects. Real long-lived Refs are likely to be concentrated (a few hot objects, or the objects of long connections), which pins fewer chunks than random.
- No real sockets, no Go or Java comparison.

## Reproduce

```
bench/run_hold.sh 300000
bench/run_stash.sh 300000
```
