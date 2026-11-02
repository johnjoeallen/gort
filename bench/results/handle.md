# Handle indirection (D15)

Source: `bench/c/handle_bench.c`, run by `bench/run_handle.sh`; raw output in `handle.txt` (`-O2`) and `handle_O3.txt` (`-O3`).
Machine: 32 logical CPUs, gcc 14.2, pinned to one core, best of 15 repetitions. Hand-written C only: no Strata compiler is involved.

**Question.** If a growable array is a non-moving header `{ptr, len, cap}` and every element access goes through `h->ptr`, what does it cost
against a raw pointer held in a register?

**Method.** Sum over 512, 32768 and 8M `u64` elements (4 KB, 256 KB, 64 MB), sequentially and by random gather. Variants:
`raw` (pointer in a register), `raw+bar` (same, with a compiler barrier each iteration), `hoisted` (header loaded once per window),
`reload` (header loaded every access, forced by the barrier), `cursor` ((handle, offset) iterator), and `every K` (a noinline call that
may grow the array every K accesses, with the base reloaded after it) against `call only K` (same calls, no reload: the call's own cost).
Loops are `noinline` so each is compiled in the same context; an earlier run with them inlined gave a 2x difference between two
identical loops, which is why.

**Measured (ns per element, `-O2`, `-O3` in brackets where different).**

| | 4 KB | 256 KB | 64 MB |
|---|---|---|---|
| raw | 0.29 (0.14) | 0.23 (0.16) | 0.18 |
| hoisted | 0.29 (0.16) | 0.23 (0.16) | 0.18 |
| reload every access | 0.29 (0.23) | 0.23 (0.18) | 0.18 |
| every 16, call + reload | 0.31 (0.14) | 0.28 (0.11) | 0.18 (0.12) |
| every 1, call + reload | 0.94 (0.74) | 1.13 (0.72) | 0.91 (0.73) |
| random gather, raw / reload | 0.29 / 0.29 (0.23 / 0.23) | 0.25 / 0.26 (0.19 / 0.24) | 0.44 / 0.45 (0.60 / 0.62) |

**What it shows.**

- Reloading the base pointer on every access costs nothing measurable at `-O2`, in cache or in memory: the extra load hits L1 and is off the critical path.
- A hoisted window is the same speed as a raw pointer, which is what the compiler would generate when no may-grow call is in the loop.
- At `-O3` the only cost is lost vectorisation (0.14 to 0.23 ns/element in L1, 0.16 to 0.18 in L2; in memory both are bandwidth-bound and equal). It comes from the barrier, not the handle: `raw+bar` costs the same as `reload`. In Strata this corresponds to a loop that contains a may-grow call, which is exactly where D15 ends the window.
- The call is the cost when there is one: every iteration, about 0.7 to 0.9 ns, and `call only` is the same as `every`, so the reload adds nothing to the call. Every 16 or 256 iterations it is lost in the noise at `-O2` (run-to-run variation here is a few percent, and the 256 KB `every 1` figure moved between runs from 0.91 to 1.13).
- Correctness check in the same program: 17 grows moved the store 6 times; the (handle, offset) reference read the right value afterwards and a raw pointer taken before the first grow no longer pointed at the element.

**What it does not show.**

- The loop body is a one-cycle dependent add, which is the best case for hiding a load. A body with more independent work per element, or one that is already load-port-bound, could show the extra load. Not tested.
- Random gather uses a precomputed index permutation, so loads are independent; a dependent pointer-chasing pattern is not covered (linked nodes would use `Ref<T>`, not this scheme).
- No realistic container operations (insert, push with growth, map probing), no multi-core contention, and no C-compiler-generated code from Strata. The may-grow summary's precision (how often a window ends) cannot be measured until there is a compiler.
- One machine, one compiler.

**Conclusion (Measured for this micro-benchmark, Direction for the design).** The per-access cost of the handle is not visible here; the realistic risk is windows
ending too often (lost vectorisation, repeated call overhead), which depends on the compiler's may-grow precision, not on the handle itself.
