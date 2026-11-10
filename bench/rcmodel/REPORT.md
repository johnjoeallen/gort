# Is refcounting everywhere viable for Gort? A benchmark report

Date of runs: 2026-11-10. Machine: AMD Ryzen 9 9950X3D (16 cores / 32 threads), Linux 6.12, gcc 14.2, Go 1.24.4,
JDK 25.0.4 (Zulu), Rust 1.93.1. All numbers below are from the **`performance`-governor run** (governor and EPP `performance`, boost on,
frequency not pinned). A first full run under `powersave` agreed with it (section 4.6), so the verdict does not depend on the governor. This report summarises `RESULTS.md` (full tables, scorecard) and `METHODOLOGY.md`
(emulation rules, bias list). Raw data is in `results/raw.csv`; every number below can be traced there.

## 1. Summary

**Verdict: the model is viable by the pre-registered threshold. Continue.**

On the pre-registered workload (W3, a middleware-style request path, 8 workers, shared session table), the Gort model
(non-atomic counts plus a "shared" flag that selects atomic counts, variant **C2**) reaches 31.2 Mreq/s. That is
**1.71x Go** (18.3), 1.45x Java G1 (21.5) and 1.08x idiomatic Rust (28.9). It is 8% below raw C with no counting at all (34.0). The
threshold required C2 to be within 1.5x of Go, that is, no worse than 1.5x slower. C2 is faster than Go, so the elision step (C4) was not needed to pass.

The more interesting result is *where* the cost is. Counting itself is nearly free on this hardware. The cost sits in three other places:

1. **Atomic operations on objects that really are shared**, about 3.7 ns each uncontended (about 10x a local pass).
2. **Contention on hot shared entries**, which collapses throughput for any scheme that must synchronise to clone safely.
3. **Per-object malloc/free with cascading destruction**, where a bump-allocating generational GC wins (Java G1 was 5x faster in the
   graph test).

Two robustness checks (section 4.6): rerunning everything under the `performance` governor changed no verdict (median metric moved 0.0-0.3%),
and capping the Java heap at 1 GB moves the Java-vs-C2 gap in C2's favour (C2 is 1.86x G1 and 2.0x ZGC in W3 at 8 workers).

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
| C0 raw (-O2) | 34.0 (33.8-34.1) | 250 ns | 70 MB |
| **C2 flagged (-O2)** | **31.2 (30.9-31.3)** | **270 ns** | 39.5 MB |
| C2 flagged (-O3) | 31.3 | 270 ns | 39.5 MB |
| C4 elided | 32.4 | 250 ns | 39.4 MB |
| C3 atomic | 18.3 | 460 ns | 39.4 MB |
| Rust idiomatic | 28.9 | 290 ns | 40.7 MB |
| Java G1 | 21.5 | 695 ns | 1.27 GB |
| Java ZGC | 17.3 | 750 ns | 8.6 GB |
| Go (default GC) | 18.3 | 1050 ns | 51 MB |
| Go (GOGC=400) | 20.4 | 970 ns | 117 MB |
| Rust pessimal (`Arc` per pass) | 16.9 | 510 ns | 40.7 MB |

| Comparison | Ratio |
|---|---|
| C2 / Go | 1.71 (R = 0.59 at -O2, 0.58 at -O3) |
| C2 / Java G1, ZGC | 1.45, 1.80 (1.86, 2.0 with a 1 GB Java heap) |
| C2 / Rust idiomatic, pessimal | 1.08, 1.85 |
| C2 / C0 | 0.92 |
| C4 / C0 | 0.96 |
| C3 / C2 | 0.59 |

Robustness of the verdict:

- **Worker count:** R = 0.80 / 0.74 / 0.59 at 1 / 4 / 8 workers. C2 is ahead of Go at every count.
- **Heavier requests (work x8):** C2 10.7 vs Go 8.22 Mreq/s (R = 0.77). The margin shrinks, as expected when the work in each request grows relative to counting.
- **Optimisation level:** -O2 and -O3 agree.
- **Worst case for the model:** even C3, with every count atomic, ties Go (R = 1.00). So the verdict does not depend on the flag
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
| C1 local | 0.770 |
| C2 flagged | 0.776 |
| C4 elided | 0.735 |
| C2 with the shared flag set (atomic path) | 7.42 |
| C3 atomic | 7.418 |
| Rust `Arc` | 7.80 |
| Rust idiomatic | 0.781 |
| Go | 0.910 |
| Java G1 | 0.414 (the JIT collapses the chain, so this is not the same work) |

A non-atomic increment and decrement pair costs 0.00-0.03 ns over raw: it overlaps with call/return latency. An atomic pair
costs 7.42 ns, about 3.7 ns per `lock xadd`, **a 10x step**. At depth 1 and 4 the code-layout noise (about +/-0.15 ns) is larger than the effect.

### 4.3 W2: shared-table read contention

Every entry in W2 is shared, so C2 always takes the atomic path and behaves like C3 and Rust `Arc`. (Java W2 rows were missing from
the first two runs because of a script bug and were rerun afterwards; they are included here. See `RESULTS.md`, erratum.)

- **Single thread:** C2 reaches 30 Mops/s against 122 for raw C (4x below). Java G1 reaches 117 on uniform keys and 233 on a single hot key.
- **Uniform keys:** scales; C2 reaches 216 Mops/s at 8 threads (2.9x below raw C's 616). Java G1 reaches 714 and Go 158.
- **hot_diff** (each thread has its own hot entry): C2 482 Mops/s at 16 threads against Go 545, Rust idiomatic 793 and Java G1 1,602.
- **hot_same** (all threads on one key): collapses to 4-10 Mops/s at 16 threads for every implementation that uses a stripe lock.
  Go's `RWMutex` holds 67 Mops/s. Java G1, which reads a `ConcurrentHashMap` with no lock and no count, holds 120.

The hot_diff-to-hot_same slowdown at 16 threads is 125x for C2, 134x for C3, 53x for raw C, 106x for Rust idiomatic, 314x for C4,
8.1x for Go and 13x for Java G1. The collapse is dominated by the single hot lock and cache line, not by the counts. The C-vs-Go and
C-vs-Java gaps here are artefacts of lock design (my C lock is an exclusive spinlock, and Java needs no count), not evidence about refcounting.

### 4.4 W4: graph allocation and free (1M-node tree)

- **1 thread:** C2 costs 23.2 ns per node per cycle against 22.2 for raw (+4%). Traversal passing refs: 3.9 vs 3.5 ns. C3: 29.9 ns.
  The free cascade is 9.7 ns per node, dominated by `free`.
- **Weak back-references:** add about 6% in C at 1 thread (23.2 to 24.6 ns), below the 10-30% predicted.
- **8 threads:** all C variants are memory-bound; counts cost about 14% (C2 74.6 vs C0 65.3 ns per node per cycle).
- **Versus GC:** Java G1 is 4.6 ns per node per cycle at 1 thread against C's 23.2, about 5x faster (bump allocation, and a dead tree dies in bulk). At 8 threads G1 is 17 ns
  (46 with weak references) against 65-77 for C.
  Go is 18 ns per node wall against C's 23, but burns 38 ns of CPU against C's 26. With a 1 GB Java heap the 8-thread G1 and ZGC numbers get 1.3-4x worse (section 4.6).

### 4.5 W5: loopback HTTP

All three C servers plateau at about 1.54M req/s with 100 or more connections, **regardless of variant**. The load generator (`oha` on 8
cores) saturated first, so **W5 cannot separate C0, C2 and C3**. What it does show:

| At 100 connections, 10,000 idle | kreq/s | p99 | Server peak RSS | RSS per idle connection |
|---|---|---|---|---|
| C2 flagged | 1,536 | 60 µs | 2.2 MB | 47.9 B |
| Go | 1,010 | 339 µs | 343 MB | 14.2 KB |
| Java G1 | 769 | 193 µs | 1.72 GB | 17 B (not meaningful: heap sizing dominates) |
| Java ZGC | 735 | 230 µs | 26.5 GB | 4 B (not meaningful) |

The Gort-model server is at least 1.5x Go (1.54M vs 1.01M req/s here; 1.75x against the 876k Go figure in the plain 100-connection run) and about 2x Java,
with far lower tail latency and memory. These are lower bounds, since the generator was the bottleneck.

### 4.6 Sensitivity checks

**Governor.** The suite was run twice: under `powersave` with EPP `balance_performance`, then under `performance` for both. Across the 1,098
comparable metrics the median change is 0.0-0.3% per workload (W1 0.0%, W2 0.3%, W3 0.0%, W4 0.3%, W5 0.3%), the mean signed change is within
+/-1%, and only 30 metrics moved by more than 8%. C2 / Go was 1.73 and is now 1.71; C2 / C0 was and is 0.92; C3 / C2 is 0.59 in both. The
exceptions are W2 at 16 threads (Rust idiomatic uniform +24%, Rust `Arc` hot_diff +21%, raw C uniform +14%, Go hot_diff +10%), a Java G1 8-thread
W4 configuration (+38%) and tail latencies. The CPU boosts quickly under `amd-pstate-epp`, so the governor setting does not matter for these workloads.
The `powersave` data is in git history (commit 9b7d544).

**Java heap capped at 1 GB** (`-Xmx1g`, results in `results_java1g/`, performance governor, no out-of-memory failures):

| Metric | Uncapped | 1 GB | Change |
|---|---|---|---|
| W3, 8 workers, G1, Mreq/s | 21.5 | 16.8 | -22% |
| W3, 8 workers, ZGC, Mreq/s | 17.3 | 15.5 | -11% |
| C2 / Java G1, C2 / Java ZGC (W3, 8 workers) | 1.45, 1.80 | 1.86, 2.0 | |
| W3 RSS at 8 workers, G1 / ZGC | 1.27 GB / 8.6 GB | 0.70 GB / 1.1 GB | -44% / -87% |
| W3 p99, G1 / ZGC | 695 / 750 ns | 815 / 915 ns | +17% / +22% |
| W4, 8 threads, G1 ns/node/cycle (weak 0 / 1) | 17.0 / 46.2 | 31.9 / 62.1 | +87% / +34% |
| W4, 8 threads, ZGC ns/node/cycle (weak 0 / 1) | 24.2 / 26.1 | 74.6 / 98.8 | +208% / +279% |
| W5, 100 conn + 10k idle: throughput G1 / ZGC | 769 / 735 kreq/s | 778 / 736 | +1% / 0% |
| W5 peak RSS, G1 / ZGC | 1.7 GB / 26.5 GB | 0.75 GB / 1.1 GB | -56% / -96% |

W1 differs by 1.5% or less, W2 G1 is within about +/-7% (a few 2-4 thread runs +10-17%), and single-worker W3/W4 are mostly within noise. The cap hurts where
eight threads each build a 1M-node tree (W4) and where default Java simply took more memory (W3 at 8 workers). W5 throughput is unaffected, which confirms that
the uncapped Java RSS in W5 reflects heap sizing and not live data. One W2 row (ZGC, 16 threads, hot_diff) swings between 504 and 1,435 Mops/s across the two passes and should not be used.

## 5. Hypothesis scorecard

| # | Prediction | Verdict | Result |
|---|---|---|---|
| H1 | Non-atomic inc/dec costs +1-2 ns/pass | **Wrong** | 0.00-0.03 ns; overlaps with call/ret cost |
| H2 | Flag test within 10% of C1 | **Right** | C2/C1 = 1.02 (W1), 0.98 (W3, 1 worker) |
| H3 | Atomic costs 3-4x C1's overhead | **Mis-stated** | about 3.7 ns per atomic, ~10x a pass, because C1's overhead is ~0 |
| H4 | hot_same >= 10x slower than hot_diff at 16 threads | **Number right, reason wrong** | 125x (C2), but raw C is 53x: it is the lock, not the count |
| H5 | C2/C0 = 0.75-0.90 in W3 | **Better than predicted** | 0.92 (8 workers), 0.94 (1) |
| H6 | C2 >= Go, about 1.0-2.0x | **Right** | 1.71x |
| H7 | C2 beats Rust pessimal; Rust idiomatic beats C2 by 10-30% | **Half right** | beats pessimal by 1.85x; Rust idiomatic is 8% *slower* than C2 (its shared table needs `Arc` + `RwLock`) |
| H8 | Gap split: traffic > atomics > flag; flag < 10% | **Not resolvable** | components are inside run-to-run noise (section 6) |
| H9 | Cascade free <= GC cost; WeakRef +10-30% | **Mixed** | G1 5x faster; Go similar wall, more CPU; WeakRef +6% |
| H10 | C4/C0 >= 0.95 | **Right** | 0.96 |

Tally: right on H2, H6, H10 (and the number in H4); partly right on H7 and H9; wrong or mis-stated on H1, H3 and H5 (H5 in the favourable
direction); H8 cannot be called. Of the pre-registered priors, the ones about *magnitude of counting cost* were wrong in the model's favour, and the ones about
*atomic and contention cost* were right or understated.

## 6. Decomposing the C2-vs-C0 gap (W3)

Nanoseconds per request per core:

| Workers | C0 | C2 | Gap | Inc/dec traffic | Flag test | Atomics on shared objects |
|---|---|---|---|---|---|---|
| 1 | 222 | 236 | 14 | 7 | 8 | -1 |
| 8 | 236 | 256 | 21 | 8 | 8 | 4 |

The whole gap is 14-21 ns out of about 230 ns. Each component is a few ns, inside run-to-run noise, and the "flag" step is as large
as the "traffic" step, so no ordering can be claimed. The pre-registered "traffic > atomics > flag" is neither confirmed nor refuted.
What is solid is the *large* atomic effect when objects are really shared and passed often: C3 is 0.59x C2.

## 7. Threats to validity

The three largest biases, as recorded in `RESULTS.md` section 1.4:

1. **Allocator and GC asymmetry, and a thin request.** C and Rust malloc/free each object; Go and Java bump-allocate and collect.
   C-vs-Go/Java in W3 is partly "malloc vs GC", not only "refcount vs trace". A real handler is heavier than 0.2 µs, which pulls all ratios toward
   1 (at work=8, C2/Go falls from 1.71 to 1.30).
2. **Shared-table synchronisation differs per language.** Counted readers need load+increment to be atomic, so C, Rust and Go take a stripe lock,
   while Java reads a `ConcurrentHashMap` without lock or count (W2 now shows the effect directly: Java G1 holds 120 Mops/s on a single hot key where C2 manages 4). My C lock is an exclusive spinlock; Go and Rust use reader-writer locks.
   A refcounted design genuinely needs something like this (hazard pointers, RCU or a lock), so it is a real cost of the model, but my lock is not what Gort would ship.
3. **C2 and C4 are hand-defined.** What counts as a "pass" decides the traffic (33 incs/request), and C4's elisions are an upper bound on what an
   optimiser recovers. Mitigated by the fact that even all-atomic C3 ties Go.

Additional limits:

- Frequency was not pinned. The final run used the `performance` governor with boost on, and a `powersave` run agreed within noise (section 4.6), but neither fixed the clock.
- `perf` is not installed: no cycle, IPC or cache-miss counters.
- Single machine; SMT was on.
- The load generator saturated in W5.
- Latencies are closed-loop service times, not queueing latency under offered load.
- Java RSS reflects heap sizing, not live data (the main runs use the default heap; section 4.6 caps it at 1 GB).
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
6. **Close the measurement gaps before relying on fine ratios:** pin the CPU frequency (boost off), install `perf`, add a load generator that does not
   saturate in W5, and repeat on a second machine.

## 9. Reproducing

```
cd bench/rcmodel
./run_all.sh          # env: REPS, PHASES, COPTS; writes results/raw.csv
./run_w5.sh           # loopback HTTP (needs oha; built into tools/oha by cargo)
./run_performance.sh  # whole suite with governor/EPP set to performance (needs passwordless sudo; restores settings)
./run_java.sh         # Java only, capped heap by default (JAVA_OPTS=-Xmx1g), into results_java1g/; PHASES=w2 for one phase
python3 tools/analyze.py   # results/summary.csv and results/tables.md
```

`results/env.txt` records the machine, toolchain versions and governor. `tools/check_asm.sh` regenerates `results/asm_check.txt`, which
verifies that the count instructions were not optimised away.
