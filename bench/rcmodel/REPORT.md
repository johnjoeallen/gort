# Is refcounting everywhere viable for Gort? A benchmark report

Date of runs: 2026-11-10. Machine: AMD Ryzen 9 9950X3D (16 cores / 32 threads), Linux 6.12, gcc 14.2, Go 1.24.4,
JDK 25.0.4 (Zulu), Rust 1.93.1. This report summarises `RESULTS.md` (full tables, scorecard) and `METHODOLOGY.md`
(emulation rules, bias list). Raw data is in `results/raw.csv`; every number below can be traced there.

## 1. Summary

**Verdict: the model is viable by the pre-registered threshold. Continue.**

On the pre-registered workload (W3, a middleware-style request path, 8 workers, shared session table), the Gort model
(non-atomic counts plus a "shared" flag that selects atomic counts, variant **C2**) reaches 31.2 Mreq/s. That is
**1.73x Go** (18.1), 1.44x Java G1 (21.7) and 1.08x idiomatic Rust (28.9). It is 8% below raw C with no counting at all (33.9). The
threshold required C2 to be within 1.5x of Go, that is, no worse than 1.5x slower. C2 is faster than Go, so the elision step (C4) was not needed to pass.

The more interesting result is *where* the cost is. Counting itself is nearly free on this hardware. The cost sits in three other places:

1. **Atomic operations on objects that really are shared**, about 3.7 ns each uncontended (about 10x a local pass).
2. **Contention on hot shared entries**, which collapses throughput for any scheme that must synchronise to clone safely.
3. **Per-object malloc/free with cascading destruction**, where a bump-allocating generational GC wins (Java G1 was 5x faster in the
   graph test).

What this does *not* show: that Gort would beat Go. The C side uses glibc malloc and a hand-written spinlock table, the Go and Java
sides pay GC and idiomatic locking, and the request is only about 0.2 µs of work (section 7).

## 2. Question and pre-registration

The hypotheses (H1-H10) and the decision thresholds were written at 10:30Z, before any benchmark ran, and were not edited
afterwards (`RESULTS.md` section 0). The thresholds, comparing Go to C2 on W3 at 8 workers with R = Go rps / C2 rps:

| R | Outcome |
|---|---|
| R <= 1.5 | model viable, continue |
| 1.5 < R <= 2 | viable only if C4 (elided) is within 1.3x and the elisions are ones a compiler could realistically make |
| R > 2 and C4 still > 1.5x | confirmed not viable, stay parked |

"Throughput" is the median over at least 10 runs (5 for W5) of requests per second.

## 3. What was measured

**The model, emulated in C** (`c/rc.h`, `c/rcm.c`). A 16-byte header holds `rc`, `weak`, a `shared` flag and a type. One source file builds five variants:

| Variant | Count update | Purpose |
|---|---|---|
| C0 raw | none; the owner frees | upper bound on speed |
| C1 local | plain `rc++` / `--rc` | cost of traffic alone (single-threaded or private tables only) |
| C2 flagged | `if (shared) atomic else plain` | **the Gort model** |
| C3 atomic | always `lock xadd` | upper bound on atomic cost |
| C4 elided | C2 plus hand-applied elisions (borrow, last-use move) | what an optimiser might recover |

Rules: passing a ref to a callee is an increment and the callee drops it at its end; loading a ref from a field into a local is clone
plus drop; returning moves. A compiler barrier follows each non-atomic increment and precedes each non-atomic decrement. Without it
gcc cancels inc/dec pairs, which would have given C1/C2 a free elision that C3 could not get.

**Comparators:** Go 1.24 (GC), Java 25 (G1 and ZGC), and Rust in two styles: *idiomatic* (borrows, no counting on the hot path) and *pessimal*
(an `Arc` clone on every pass).

**Workloads**, with the same RNG, seeds, object sizes and arithmetic in every language. A single-worker checksum agrees
across all implementations (3806582316), which confirms they do the same work.

| ID | What | What it isolates |
|---|---|---|
| W1 | recursive call chain passing one ref (depth 1/4/16) | raw cost of a count pass |
| W2 | 10k-entry striped shared table, 95/5 read/write; uniform, hot-same-key and hot-different-key modes | contention on shared reads |
| W3 | 10-layer middleware, 6 heap objects per request, session lookup in a shared table; 1/4/8 workers; 5M requests | **the pre-registered test** |
| W4 | 1M-node 4-ary tree: build, traverse, free; with and without weak back-references; 1 and 8 threads | allocation/free cost and cascading destructors |
| W5 | loopback HTTP/1.1 keep-alive server, `oha` load generator, up to 10k idle connections | end-to-end sanity and per-connection memory |

## 4. Results

### 4.1 W3: the pre-registered test (8 workers, shared session table)

| Implementation | Mreq/s (median, min-max) | p99 latency | Peak RSS |
|---|---|---|---|
| C0 raw (-O2) | 33.9 (33.4-34.1) | 250 ns | 70 MB |
| **C2 flagged (-O2)** | **31.2 (30.6-31.4)** | **270 ns** | 39.5 MB |
| C2 flagged (-O3) | 31.3 | 270 ns | 39.6 MB |
| C4 elided | 32.4 | 250 ns | 39.6 MB |
| C3 atomic | 18.3 | 460 ns | 39.5 MB |
| Rust idiomatic | 28.9 | 290 ns | 40.6 MB |
| Java G1 | 21.7 | 645 ns | 1.26 GB |
| Java ZGC | 18.3 | 740 ns | 8.5 GB |
| Go (default GC) | 18.1 | 1070 ns | 54 MB |
| Go (GOGC=400) | 20.5 | 970 ns | 119 MB |
| Rust pessimal (`Arc` per pass) | 16.9 | 510 ns | 40.7 MB |

| Comparison | Ratio |
|---|---|
| C2 / Go | 1.73 (R = 0.58, at both -O2 and -O3) |
| C2 / Java G1, ZGC | 1.44, 1.70 |
| C2 / Rust idiomatic, pessimal | 1.08, 1.85 |
| C2 / C0 | 0.92 |
| C4 / C0 | 0.96 |
| C3 / C2 | 0.59 |

Robustness of the verdict:

- **Worker count:** R = 0.80 / 0.74 / 0.58 at 1 / 4 / 8 workers. C2 is ahead of Go at every count.
- **Heavier requests (work x8):** C2 10.7 vs Go 8.15 Mreq/s (R = 0.76). The margin shrinks, as expected when the work in each request grows relative to counting.
- **Optimisation level:** -O2 and -O3 agree.
- **Worst case for the model:** even C3, with every count atomic, ties Go (R = 0.99). So the verdict does not depend on the flag
  keeping objects off the atomic path.

Executed counting operations per request, measured with a COUNT build at 1 worker:

| Variant | Incs | Atomic | Decs | Mallocs |
|---|---|---|---|---|
| C1 / C2 | 33.0 | 0 / 3.0 | 39.1 | 6.1 |
| C3 | 33.0 | 33.0 | 39.1 | 6.1 |
| C4 | 1.0 | 1.0 | 7.1 | 6.1 |

The 3 atomic incs in C2 are the shared-session clones. Elision removes 97% of traffic and recovers about 4 points of raw
throughput (0.92 to 0.96).

### 4.2 W1: the cost of one pass (ns per pass, depth 16)

| Variant | ns/pass |
|---|---|
| C0 raw | 0.747 |
| C1 local | 0.760 |
| C2 flagged | 0.775 |
| C4 elided | 0.736 |
| C2 with the shared flag set (atomic path) | 7.42 |
| C3 atomic | 7.42 |
| Rust `Arc` | 7.80 |
| Rust idiomatic | 0.781 |
| Go | 0.909 |
| Java G1 | 0.414 (the JIT collapses the chain, so this is not the same work) |

A non-atomic increment and decrement pair costs 0.00-0.03 ns over raw: it overlaps with call/return latency. An atomic pair
costs 7.42 ns, about 3.7 ns per `lock xadd`, **a 10x step**. At depth 1 and 4 the code-layout noise (about +/-0.15 ns) is larger than the effect.

### 4.3 W2: shared-table read contention

Every entry in W2 is shared, so C2 always takes the atomic path and behaves like C3 and Rust `Arc`.

- **Single thread:** C2 reaches 30 Mops/s against 123 for raw C (4x below).
- **Uniform keys:** scales; C2 reaches 216 Mops/s at 8 threads (2.8x below raw).
- **hot_diff** (each thread has its own hot entry): scales almost linearly for everyone (C2 482 vs Go 493 Mops/s at 16 threads).
- **hot_same** (all threads on one key): collapses to 4-10 Mops/s at 16 threads for every implementation that uses a stripe lock.
  Go's `RWMutex` holds 63 Mops/s.

The hot_same slowdown relative to hot_diff at 16 threads is 121x for C2, 131x for C3, 52x for raw C, 111x for Rust idiomatic, 316x for C4 and
7.8x for Go. The collapse is dominated by the single hot lock and cache line, not by the counts. The C-vs-Go gap here is an artefact of
lock design (my C lock is an exclusive spinlock), not evidence about refcounting.

### 4.4 W4: graph allocation and free (1M-node tree)

- **1 thread:** C2 costs 23.0 ns per node per cycle against 22.5 for raw (+2%). Traversal passing refs: 3.9 vs 3.6 ns. C3: 29.9 ns.
  The free cascade is 9.6 ns per node, dominated by `free`.
- **Weak back-references:** add about 7% in C at 1 thread (23.0 to 24.7 ns), below the 10-30% predicted.
- **8 threads:** all C variants are memory-bound; counts cost 12-14% in traverse (C2 74.9 vs C0 65.5 ns wall per node).
- **Versus GC:** Java G1 is 4.6 ns per node per cycle at 1 thread against C's 23, about 5x faster (bump allocation, and a dead tree dies in bulk). At 8 threads G1 is 18 vs 65-75 ns per node.
  Go is 19 ns per node wall against C's 23, but burns 39 ns of CPU against C's 26.

### 4.5 W5: loopback HTTP

All three C servers plateau at about 1.5M req/s with 100 or more connections, **regardless of variant**. The load generator (`oha` on 8
cores) saturated first, so **W5 cannot separate C0, C2 and C3**. What it does show:

| At 100 connections, 10,000 idle | kreq/s | p99 | Server peak RSS | RSS per idle connection |
|---|---|---|---|---|
| C2 flagged | 1,540 | 57 µs | 2.2 MB | 47.5 B |
| Go | 1,010 | 341 µs | 349 MB | 14.3 KB |
| Java G1 | 777 | 191 µs | 1.7 GB | not measurable (heap sizing dominates) |

The Gort-model server is at least 1.5x Go (1.54M vs 1.01M req/s here; 1.75x against the 878k Go figure in the plain 100-connection run in
`RESULTS.md`) and about 2x Java, with far lower tail latency and memory. These are lower bounds, since the generator was the bottleneck.

## 5. Hypothesis scorecard

| # | Prediction | Verdict | Result |
|---|---|---|---|
| H1 | Non-atomic inc/dec costs +1-2 ns/pass | **Wrong** | 0.00-0.03 ns; overlaps with call/ret cost |
| H2 | Flag test within 10% of C1 | **Right** | C2/C1 = 1.02 (W1), 0.98 (W3, 1 worker) |
| H3 | Atomic costs 3-4x C1's overhead | **Mis-stated** | about 3.7 ns per atomic, ~10x a pass, because C1's overhead is ~0 |
| H4 | hot_same >= 10x slower than hot_diff at 16 threads | **Number right, reason wrong** | 121x (C2), but raw C is 52x: it is the lock, not the count |
| H5 | C2/C0 = 0.75-0.90 in W3 | **Better than predicted** | 0.92 (8 workers), 0.94 (1) |
| H6 | C2 >= Go, about 1.0-2.0x | **Right** | 1.73x |
| H7 | C2 beats Rust pessimal; Rust idiomatic beats C2 by 10-30% | **Half right** | beats pessimal by 1.85x; Rust idiomatic is 8% *slower* than C2 (its shared table needs `Arc` + `RwLock`) |
| H8 | Gap split: traffic > atomics > flag; flag < 10% | **Not resolvable** | components are inside run-to-run noise (section 6) |
| H9 | Cascade free <= GC cost; WeakRef +10-30% | **Mixed** | G1 5x faster; Go similar wall, more CPU; WeakRef +7% |
| H10 | C4/C0 >= 0.95 | **Right** | 0.96 |

Tally: right on H2, H6, H10 (and the number in H4); partly right on H7 and H9; wrong or mis-stated on H1, H3 and H5 (H5 in the favourable
direction); H8 cannot be called. Of the pre-registered priors, the ones about *magnitude of counting cost* were wrong in the model's favour, and the ones about
*atomic and contention cost* were right or understated.

## 6. Decomposing the C2-vs-C0 gap (W3)

Nanoseconds per request per core:

| Workers | C0 | C2 | Gap | Inc/dec traffic | Flag test | Atomics on shared objects |
|---|---|---|---|---|---|---|
| 1 | 222 | 236 | 14 | 7 | 8 | -1 |
| 8 | 236 | 256 | 20 | 6 | 9 | 5 |

The whole gap is 14-20 ns out of about 230 ns. Each component is a few ns, inside run-to-run noise, and the "flag" step is as large
as the "traffic" step, so no ordering can be claimed. The pre-registered "traffic > atomics > flag" is neither confirmed nor refuted.
What is solid is the *large* atomic effect when objects are really shared and passed often: C3 is 0.59x C2.

## 7. Threats to validity

The three largest biases, as recorded in `RESULTS.md` section 1.4:

1. **Allocator and GC asymmetry, and a thin request.** C and Rust malloc/free each object; Go and Java bump-allocate and collect.
   C-vs-Go/Java in W3 is partly "malloc vs GC", not only "refcount vs trace". A real handler is heavier than 0.2 µs, which pulls all ratios toward
   1 (at work=8, C2/Go falls from 1.73 to 1.31).
2. **Shared-table synchronisation differs per language.** Counted readers need load+increment to be atomic, so C, Rust and Go take a stripe lock,
   while Java reads a `ConcurrentHashMap` without lock or count. My C lock is an exclusive spinlock; Go and Rust use reader-writer locks.
   A refcounted design genuinely needs something like this (hazard pointers, RCU or a lock), so it is a real cost of the model, but my lock is not what Gort would ship.
3. **C2 and C4 are hand-defined.** What counts as a "pass" decides the traffic (33 incs/request), and C4's elisions are an upper bound on what an
   optimiser recovers. Mitigated by the fact that even all-atomic C3 ties Go.

Additional limits:

- CPU governor was `powersave`: frequency was not fixed.
- `perf` is not installed: no cycle, IPC or cache-miss counters.
- Single machine; SMT was on.
- The load generator saturated in W5.
- Latencies are closed-loop service times, not queueing latency under offered load.
- Java RSS reflects heap sizing, not live data.
- Go/Java escape-analysis behaviour was assumed from design, not verified.
- The emulation is hand-written C, not a Gort compiler.

## 8. Conclusions and recommended next steps

1. **Continue with the refcount-everywhere model.** It passed the pre-registered threshold with margin, and the verdict survives
   the worst-case C3 variant, so it does not hinge on how well the shared flag works.
2. **Design to minimise flagged objects and passes of flagged objects**, not the flag test. A shared object passed freely costs about 7.4 ns per
   pass versus about 0.75 ns for a local one.
3. **Do not ship the benchmark's shared-table lock.** Shared mutable state is the largest open risk. Evaluate a reader-friendly scheme (RCU, hazard
   pointers, or reader-writer stripes) and repeat W2 hot_same against it.
4. **Attack allocation, not counting.** Per-object `free` with a cascading destructor is where Java G1 won by 5x. Candidate work: a bump or arena
   allocator for request-local objects, and batch freeing.
5. **Compiler-level elision is a modest gain** (about 4 points of raw throughput here) and optional. It is not what makes the model viable.
6. **Close the measurement gaps before relying on fine ratios:** fix the CPU frequency, install `perf`, add a load generator that does not
   saturate in W5, and repeat on a second machine.

## 9. Reproducing

```
cd bench/rcmodel
./run_all.sh          # env: REPS, PHASES, COPTS; writes results/raw.csv
./run_w5.sh           # loopback HTTP (needs oha; built into tools/oha by cargo)
python3 tools/analyze.py   # results/summary.csv and results/tables.md
```

`results/env.txt` records the machine, toolchain versions and governor. `tools/check_asm.sh` regenerates `results/asm_check.txt`, which
verifies that the count instructions were not optimised away.
