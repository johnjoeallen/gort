# Split control block: moving objects between chunks (D17, B15)

> D17 benchmarks: **1. Allocation, dereference, retained memory** · [2. A whole service](service_split.md) · [3. Sharded locks and clone-on-hit](b12_ref.md)  
> (first) | [A whole service →](service_split.md)

**Question.** [D17](../../docs/architecture/open-decisions.md#d17-moving-objects-between-chunks-split-control-blocks-and-weak-promotion) proposes a never-moving control block (counts and a body pointer) in one arena, with the object body in a type arena where it can be moved. What does that cost in time, what does it save in memory, and when?

**Answer for this test: the split costs real time and buys real memory. It is not free the way the D15 header was.**

- **Time (loss).** Allocation is 9 to 45% slower, freeing 58 to 135% slower, a free-plus-allocate pair about 2x. A dereference through the control block is 13 to 97% slower for independent reads and **75 to 174% slower on a pointer chase**, because the control block and the body are two cache lines in two arenas. The D15 header, which sits next to its array, measured 0 to 3%. This is a different mechanism.
- **Memory (gain).** Objects still held only by a `WeakRef` give their body memory back: 157 MB falls to 50 MB for 64 B bodies, and 263 MB to 25 MB for 256 B bodies, where the combined layout keeps all of it. After a burst that leaves 1% of objects scattered, moving bodies out of sparse chunks leaves 44 MB against 70 MB for the cheaper alternative (releasing free pages) and 157 MB for doing nothing. At 10% scattered survivors only moving helps (64 MB against 157 MB).
- **Memory (cost).** A live object costs more: 96 B against 80 B for a 64 B body, so a full heap is 22% larger (192 MB against 157 MB for 2M objects). The control blocks of scattered survivors stay pinned: 44 MB is almost all control blocks (2M x 24 B).

Source `bench/c/split_bench.c`, driver `bench/run_split.sh`, summary script `bench/parse_split.py`; raw output `split.txt`, tables `split.tables.md`.
gcc 14.2 `-O2`, one thread pinned to core 2, Ryzen 9 9950X3D (large L3), Linux 6.12, THP=always (chunks are marked `MADV_NOHUGEPAGE`). Hand-written C, no Strata compiler.
The two layouts are the same source built with `-DSPLIT=0` and `-DSPLIT=1`, with the same arena code (64 KB chunks, free-index stack and used bitmap in the chunk header, empty chunk unmapped). Timing rows are the median and range of 5 runs with the layouts alternating in order; memory rows are the median of 3.

## Loss first: time

Allocation, ns per object (combined layout against split):

| body | operation | combined | split | split vs combined |
|---|---|---|---|---|
| 16 B | allocate | 6.4 | 9.3 | +45% |
| 16 B | free, random order | 10.4 | 24.5 | +135% |
| 16 B | free + allocate | 13.5 | 40.2 | +197% |
| 64 B | allocate | 13.0 | 16.3 | +26% |
| 64 B | free, random order | 16.8 | 29.7 | +77% |
| 64 B | free + allocate | 26.3 | 55.3 | +111% |
| 256 B | allocate | 41.2 | 45.1 | +9% |
| 256 B | free, random order | 27.8 | 43.9 | +58% |
| 256 B | free + allocate | 30.8 | 63.3 | +106% |

Dereference of a 64 B body, ns per access (combined against split):

| working set | sequential | random, independent | pointer chase |
|---|---|---|---|
| about 160 KB | 0.33 / 0.44 (+35%) | 0.30 / 0.53 (+74%) | 2.6 / 4.5 (+75%) |
| about 8 MB | 0.51 / 0.64 (+25%) | 0.74 / 1.27 (+72%) | 10.6 / 21.8 (+107%) |
| about 120 MB | 1.14 / 1.52 (+33%) | 2.69 / 4.98 (+85%) | 35.7 / 97.8 (+174%) |
| about 480 MB | 1.62 / 1.83 (+13%) | 3.94 / 7.77 (+97%) | 88.6 / 168.2 (+90%) |

The pointer chase is the worst case: every hop reads a control block, then the body, then the next control block, so with a large working set each hop is two cache misses against one. A tree or a linked structure walked from a cold cache would see this. A loop that reads a few fields of one object, or an object read repeatedly, would see closer to the sequential rows.

## Gain: memory

Resident MB above the baseline, 2M objects of 64 B (1M of 256 B in the last row):

| scenario | layout | all live | after freeing the rest | after releasing free pages | after moving bodies |
|---|---|---|---|---|---|
| 1% kept, scattered | combined | 157 | 157 | 70 | not possible |
| 1% kept, scattered | split | 192 | 192 | 110 | **44** |
| 10% kept, scattered | combined | 157 | 157 | 157 | not possible |
| 10% kept, scattered | split | 192 | 192 | 192 | **64** |
| 1% kept, oldest objects | combined | 157 | 2 | 2 | not needed |
| 1% kept, oldest objects | split | 192 | 2 | 2 | not needed |
| 1% kept, scattered, 256 B | combined | 263 | 237 | 51 | not possible |
| 1% kept, scattered, 256 B | split | 280 | 258 | 72 | **24** |

The move pass took 7 to 15 ms for about 1% to 10% survivors of 2M objects, which includes unmapping 2000 to 3700 emptied chunks (not timed separately). Survivors read back correctly after the move in every run.

Every object dead while a `WeakRef` to it is held (resident MB):

| objects | layout | all live | all dead, weak refs held | weak refs dropped |
|---|---|---|---|---|
| 64 B x 2M | combined | 157 | 157 | 0.1 |
| 64 B x 2M | split | 192 | **50** | 0.2 |
| 256 B x 1M | combined | 263 | 263 | 0.1 |
| 256 B x 1M | split | 280 | **25** | 0.2 |

## How to read it

- **The split pays only where memory is pinned.** If a type's objects die in roughly the order they were made (the "oldest objects" row), neither layout retains anything and the split is pure cost. It earns its keep for types with mixed lifetimes (sessions, cache entries) and where `WeakRef`s outlive their targets.
- **Pages first.** Releasing free pages needs no indirection. At 1% scattered 64 B survivors it cut 157 MB to 70 MB (combined) with no change to dereference cost. Moving bodies is the further step, to 44 MB, and the only one that works at 10%.
- **The remaining floor is the control blocks.** They never move, so scattered survivors pin their arena: 24 B x 2M = 48 MB. Moving bodies cannot go below it. A smaller control block, or surviving objects keeping their blocks in a separate arena, would lower it.
- **For a whole service** see the [second run](service_split.md): 4 to 12% more work per request with one worker, up to 27% with four workers sharing a cache lock, and no difference when the service waits on its database.

## Caveats

- Single thread: no remote frees, no contention, no real `Ref` count traffic. The extra cost under concurrency (the control block is written by every count change, the body only by its users) is untested.
- The working sets of 160 KB and 8 MB sit in cache; 120 MB is partly in this CPU's large L3; 480 MB is not. The pointer chase makes the most of the cache-miss difference and is a worst case, not a typical access.
- The dereference rows time a read of the first word of a body. Real code reads several fields, which amortises the control-block load, and the compiler could hoist it out of a loop. Neither is modelled.
- Move passes ran with the world stopped (nothing else touched the objects). The cost of making a move safe against concurrent readers (a `mutate`, or a grace period for immutable objects) is not measured.
- The freed-memory result depends on the allocator returning empty 64 KB chunks to the OS; a program that keeps spare chunks would show less.
- Allocation rows include first-touch page faults in the fill pass (they dominate the 256 B row), identically in both layouts.
- Synthetic: one body size per run, uniformly random survivors.

## Reproduce

    bench/run_split.sh        # builds split_0 and split_1, runs everything, writes results/split.txt and split.tables.md (about 40 s)
