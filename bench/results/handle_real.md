# Handle indirection with realistic loop bodies (D15, second run)

> D15 handle benchmarks: [1. Handle indirection: simple loop](handle.md) · **2. Realistic loop bodies** · [3. Mixed request](mixed.md)  
> [← Handle indirection: simple loop](handle.md) | [Mixed request →](mixed.md)

**Question.** Same as the [first run](handle.md): what does it cost to reach array elements through a non-moving header (`h->ptr`) instead of
a raw pointer? The first run summed an array, the easiest loop to be unfair to a handle. Here every element does real work, and every variant takes a lock.

**Answer for this test: a typical loss of 1 to 3%, shrinking as the work per element grows. The worst single case is +28%, and that is 0.25 ns.**

Source `bench/c/handle_real.c`, run by `bench/run_handle_real.sh`. Raw output `handle_real.txt` (`-O2`) and `handle_real_O3.txt` (`-O3`), per-case tables
`handle_real.tables.md` and `handle_real_O3.tables.md`. gcc 14.2, one pinned core, best of 9 runs. Hand-written C, no Strata compiler.

## What was timed

Per element: read `a[i]` through the pointer, read a second element chosen by that value (a gather), do **W rounds** of dependent arithmetic, store the result.
More rounds means more work per element. The lock is taken once per batch (none, a spin lock, or a pthread mutex; a batch of 1 or 256 elements), and the
raw-pointer variant takes the same lock. Three array sizes: 4 KB, 256 KB, 64 MB. Checksums match across all variants.

The handle variants differ in how often the pointer is reloaded, which models how often the compiler must end a deref window (D15):

| Variant | Pointer is reloaded |
|---|---|
| raw pointer | never inside the lock window (the C baseline) |
| every 16 | every 16th element, as if a may-grow call sat there |
| every element | once per element |
| every access | before each of the two reads: the worst case |

## What the handle loses

Loss against the raw pointer, as a percentage. Each cell is the **median over 18 configurations** (3 sizes x 3 locks x 2 batch sizes), `-O2`.

| Work per element | Raw cost per element | Reload every 16 | Reload every element | Reload every access | Worst single case |
|---|---|---|---|---|---|
| none (W=0) | 1.8 ns | **+3.4%** | **+2.5%** | +0.8% | **+28%** (0.25 ns) |
| W=1 | 2.4 ns | **+6.9%** | +1.8% | +0.2% | +14% |
| W=2 | 3.1 ns | **+3.5%** | +0.8% | +0.8% | +6% |
| W=4 | 4.1 ns | +2.2% | +1.9% | +1.4% | +11% |
| W=8 | 5.2 ns | +3.6% | +2.3% | +1.1% | +8% |
| W=16 | 8.1 ns | +2.4% | +1.8% | +1.2% | +5% |
| W=32 | 17.7 ns | +1.8% | +0.8% | +2.4% | +19% (artefact, see below) |
| W=64 | 43.4 ns | +1.0% | +0.6% | +3.6% | +10% (artefact, see below) |

Reading it: the loss is small everywhere, and is largest when the body is nearly empty, which is when one extra load is a visible part of the element.
In nanoseconds the typical loss is 0.01 to 0.3 ns per element.

## Where the large percentages come from

The only large losses are for a nearly empty body with **no lock** (the whole element costs about 0.5 to 0.9 ns):

| Case (4 KB array, W=0, reload every element) | Raw | Loss |
|---|---|---|
| no lock, a batch of 1 | 0.87 ns | **+0.25 ns (+28%)** |
| spin lock, a batch of 1 | 4.16 ns | 0.00 ns (0%) |
| mutex, a batch of 1 | 3.43 ns | no loss (the reload variant measured 0.22 ns faster, which is noise) |

With any real lock around the element, the lock costs 3 to 4 ns and the reload is not visible. In the 64 MB array (a batch of 256, W=0) the loss was
+7 to +10% (0.13 to 0.18 ns) for the every-element and every-access variants.

## How to read this for an I/O-bound service

Strata targets servers, where most of a request is spent in blocking calls: a database query, a remote call, a socket or file read. The handle cost applies
only to the in-memory work *between* those calls, so this benchmark is deliberately the conservative case: CPU-bound loops over container data with no I/O in them.

- **Scale.** A database round trip or socket wait takes tens of microseconds to milliseconds; the handle costs 0.01 to 0.3 ns per element. After each park the base pointer is reloaded once, about a nanosecond, so a request with ten parks adds roughly ten nanoseconds. The simple HTTP handler in this repo already costs about 3 microseconds per request and is syscall-bound.
- **I/O is never inside a locked window.** `sync` and `mutate` bodies cannot park (a database call inside one is a compile error), so a shared container's window is a lock, a tight in-memory loop and an unlock, the shape measured here. For an unshared container the window simply ends at each park.
- **Where the cost can matter** is the CPU-bound parts of a request: sorting, grouping and mapping rows (`handleReport` in `stra/experiments/handlers.stra`), parsing headers or frames, rendering JSON, bulk imports. Those loops are a small share of an I/O-bound request.
- **Not measured here:** a mixed request (parse, query, render, with real parks, benchmark B12); cache effects when many vthreads interleave and a container's header is evicted between a park and its resume; the cost of the park itself (separately measured in [`lazy.md`](lazy.md), hand-written C).

## Caveats

- **The noise is bigger than the table suggests.** Two copies of the same raw loop differed by a median 0.3%, but by up to 19% (90th percentile 9%).
  At W=32 and 64 gcc compiled the arithmetic loop differently in one copy, a 3 to 4 ns difference. The "artefact" cells above are that, not handle cost.
  I compared against the faster of the two copies, but single cells are good to a few percent only; trust the medians more than the worst cases.
- Reload frequencies are chosen, not produced by a compiler. How often a real program ends a window (the may-grow analysis) is unmeasured.
- Single thread, uncontended locks, reads only, no growing; one machine, one compiler. `-O3` gives the same picture within a point or two
  (`handle_real_O3.tables.md`).
- One body shape: independent loads and a dependent arithmetic chain. Branchy parsing or a loop the compiler could vectorise would differ (the first run showed up to +71% on a vectorised sum).

## Conclusion

Measured: for bodies that do real work, reaching the data through a non-moving header costs about 1 to 3% (0.01 to 0.3 ns per element), the largest percentage loss
is on an almost-empty body without a lock, and a lock around the element hides it completely. Direction, not measured: this is small enough to accept for the safety it
buys, provided the compiler does not end windows needlessly.

---
> D15 handle benchmarks: [1. Handle indirection: simple loop](handle.md) · **2. Realistic loop bodies** · [3. Mixed request](mixed.md)  
> [← Handle indirection: simple loop](handle.md) | [Mixed request →](mixed.md)
