# Handle indirection: what does it cost? (D15, first run)

> D15 handle benchmarks: **1. Handle indirection: simple loop** · [2. Realistic loop bodies](handle_real.md) · [3. Mixed request](mixed.md)  
> ← (first) | [Realistic loop bodies →](handle_real.md)

**Question.** A growable array is a small fixed header `{ptr, len, cap}` and every element access goes through `h->ptr`, so the array can move
without breaking references. How much slower is that than holding a raw pointer?

**Answer for this test: usually nothing, and at worst a loss of about 0.1 ns per element.** This is the simplest possible loop (summing an array),
so it is the *hardest* case for a handle to hide in. The [realistic run](handle_real.md) does real work per element and shows 1 to 2% typical.

Source `bench/c/handle_bench.c`, run by `bench/run_handle.sh`; raw output `handle.txt` (`-O2`) and `handle_O3.txt` (`-O3`).
gcc 14.2, one pinned core, best of 15 runs. Hand-written C, no Gort compiler. Times are nanoseconds per element.

## Where the handle loses

Each row compares a raw pointer with the same loop going through the handle. Only rows with a loss are listed.

**Build `-O2`**

| Loop | Array size | Raw pointer | Through handle | Loss |
|---|---|---|---|---|
| random reads, pointer reloaded every access | 256 KB | 0.251 | 0.263 | **+0.012 ns (+5%)** |
| random reads, pointer reloaded every access | 64 MB | 0.439 | 0.449 | **+0.010 ns (+2%)** |

**Build `-O3`** (the compiler can vectorise the raw loop, so there is more to lose)

| Loop | Array size | Raw pointer | Through handle | Loss |
|---|---|---|---|---|
| sum, pointer reloaded every access | 4 KB | 0.137 | 0.234 | **+0.097 ns (+71%)** |
| sum, pointer reloaded every access | 256 KB | 0.164 | 0.181 | **+0.017 ns (+10%)** |
| sum, pointer loaded once per window | 4 KB | 0.137 | 0.156 | **+0.019 ns (+14%)** |
| random reads, pointer reloaded every access | 256 KB | 0.186 | 0.244 | **+0.058 ns (+31%)** |
| random reads, pointer reloaded every access | 64 MB | 0.597 | 0.622 | **+0.025 ns (+4%)** |

The `-O3` loss in the first row is the loss of vectorisation: the raw loop adds several elements at once, and forcing a reload every access prevents it.
The same loop with the pointer loaded once is only 14% slower, and I suspect much of that is code layout rather than the handle, because the
`raw` and `hoisted` loops are the same code. That is a guess; I did not check the assembly.

## Where it loses nothing

At `-O2`, summing 4 KB, 256 KB or 64 MB, the handle is exactly as fast as the raw pointer whether the pointer is loaded once or reloaded
on every access (0.293, 0.228 and 0.181 ns for both). At `-O3` the 64 MB sum and the 4 KB random reads are also identical. A loop that is waiting on memory, or that does
other work between loads, hides the extra load.

## The cost of the call, which is not the handle

If code calls a function that *might* grow the array on every iteration, each iteration pays for that call: **0.7 to 1.1 ns per call**
(`-O2` 0.9 to 1.1, `-O3` 0.7). That cost is the same with and without the pointer reload afterwards (for example 0.938 and 0.938 at `-O2`), so the handle does not add to it.
Called every 16th element instead, the call is 0.11 to 0.23 ns per element. Here the reload after the call did add a little at `-O2`
(0.234 to 0.312 ns in 4 KB, 0.227 to 0.284 in 256 KB, so +0.06 to +0.08 ns); at `-O3` and in the 64 MB case it added nothing.

## Limits of this test

- The loop body is one add per element. A body that does more work shows less loss; that is the [realistic run](handle_real.md).
- One machine, one compiler, one thread, reads only, no growing, no locks.
- Two identical loops once differed by 2x here when the compiler inlined them differently, so the loops are `noinline` and small differences in the tables above (a few percent) are within noise.

Each result above is from the final run; correctness check in the same program: 17 grows moved the array 6 times, a reference stored as
(handle, offset) still read the right element afterwards, and a raw pointer taken earlier no longer pointed at it.

---
> D15 handle benchmarks: **1. Handle indirection: simple loop** · [2. Realistic loop bodies](handle_real.md) · [3. Mixed request](mixed.md)  
> ← (first) | [Realistic loop bodies →](handle_real.md)
