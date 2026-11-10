# Methodology and everything that could bias the numbers

Run `./run_all.sh` (env `REPS`, `PHASES`, `COPTS`). Raw rows land in `results/raw.csv`
(`run,workload,impl,variant,params,metric,value`); `tools/analyze.py` produces `results/summary.csv` and `results/tables.md`.
`results/env.txt` records the machine, toolchain versions and the CPU governor.

The published results come from `./run_performance.sh`, which sets the governor and EPP to `performance` for the whole run and restores the
original settings on exit (needs passwordless sudo). `./run_java.sh` reruns only the Java benchmarks (W1-W5) with extra JVM flags
(`JAVA_OPTS`, default `-Xmx1g`) into a separate directory (`OUT`, default `results_java1g/`); `PHASES` selects workloads. `tools/w5.py` honours
`W5_ONLY` and `W5_JAVA_OPTS` for the same purpose.

## 1. The memory model, as emulated in C (`c/rc.h`, `c/rcm.c`)

Object header (16 B): `rc`, `weak`, `shared` flag, `type`. One source file builds five variants with `-DMODE`:

| variant | `rc_inc` / `rc_dec` | notes |
|---|---|---|
| C0 raw | nothing | ownership is explicit: the owner frees. Non-owning references are plain pointers. |
| C1 local | `rc++` / `--rc` | non-atomic. Unsound when objects are shared across threads, so C1 is run only single-threaded, or with per-thread private tables (`table=private`). It is **not** run in W2, nor in shared-table W3 with more than one worker. |
| C2 flagged | `if (o->shared) atomic else plain` | the real Gort model. The flag is set (plain store, before the object becomes visible) when an object is published into the shared table. |
| C3 atomic | always `lock xadd` (relaxed inc, release dec + acquire fence on zero) | |
| C4 elided | C2 + hand elisions, listed in section 3 | |

Rules of the emulation ("no borrows"): passing a Ref to a callee = `PASS` (inc) and the callee `DROP`s at its end; loading a
reference out of a field into a local = clone + drop; storing a freshly made object into a field = clone + drop of the local
(this is the "move on last use" that C4 removes); returning a reference moves it (no traffic). `rc` reaching 0 runs the type's
destructor, which drops its children (cascade), then `free`.

`CBARRIER()` (an empty `asm volatile` with a memory clobber) is placed after every **non-atomic** increment and before every
non-atomic decrement. Without it, gcc cancels adjacent non-atomic inc/dec pairs (it cannot do that for atomics), which handed
C1/C2 a free elision that C3 did not get. The static count of count-updating instructions in `layer()` went from 11 to 24 (C1)
after adding it, against 25 for C3. `tools/check_asm.sh` -> `results/asm_check.txt` shows the instructions for every variant and
optimisation level. C0 has none; C4 has none in W1 by construction (that is the elision).

`-fno-optimize-sibling-calls` is used for all C builds so the recursion in W1 stays a real call chain in C0 too (otherwise gcc
turns the raw chain into a loop). The same effect is forced in Rust idiomatic with `black_box` on the return value.

## 2. Workloads, identical across languages

All languages use the same xorshift64* RNG, seeds, object sizes, field layout and per-layer arithmetic. **Cross-check:** with one
worker, W3 returns a checksum over all request results; C (all variants), Go, Java and Rust (both) produce the same value
(3806582316 for the default parameters), which shows the work is the same.

* **W1** `walk(entry, depth)`: a recursive non-inlined call chain, `depth` calls, reads `entry->val` at the bottom. A pool of 64
  entries (128 B each) keeps data in L1. 100M iterations in C. Go (`testing.B`), Java (JMH) and Rust (criterion) choose their own
  iteration counts, so only ns/pass is comparable. ns/pass = time per chain / depth.
* **W2** 10 000-entry table, striped by `key & 255`. Read (95%): look up under the stripe lock and clone the reference, then
  `walk(e, 3)`. Write (5%): allocate a new entry and swap it into its slot. Modes: `uniform`, `hot_same` (all threads use key 17),
  `hot_diff` (thread i uses key `i*601+7`). 3M ops per thread in C. Entries are 128 B and 64 B-aligned in C and Rust; Go and Java
  entries are padded to about 112-128 B but alignment is up to the allocator.
* **W3** 10 nested layers (middleware chain by recursion), 6 heap objects per request (Req, header array, 512 B body, Ctx,
  Resp, 256 B response body), a session lookup in a 1024-entry shared table with 5% session replacement, ~0.2 µs of arithmetic
  per request in `work=1`. 5M requests split across the workers, a 300k-request warm-up per worker that is discarded, per-request
  latency from two clock reads. Sensitivity run `work=8` multiplies the hash loops.
* **W4** 1M-node 4-ary tree: build, traverse (passes the child Ref into the recursive call), free. 6 cycles, the first discarded,
  median of the rest. `weak=1` adds a parent back-reference (WeakRef in C, `Weak` in Rust, a plain pointer in Go/Java) and a
  parent upgrade during traversal. 1 and 8 threads (each with its own tree).
* **W5** loopback HTTP/1.1 keep-alive, fixed 38-byte JSON body, `oha` 1.16 as the driver (`wrk` is not installed; `oha` was
  built with cargo into `tools/oha`). Servers on cores 0-7 and load generators on cores 8-15.

## 3. Elision sites in C4 (hand-applied, `-DMODE=4`)

All are macros in `c/rc.h`: `PASS_B`/`DROP_B` (borrow) and `PASS_L`/`DROP_L` (last-use move). `grep -n "PASS_B\|DROP_B\|PASS_L\|DROP_L" c/rcm.c`.

1. **Borrow into a callee when the caller provably outlives it.** (a) every `layer()` call passes `rq` and `cx` (the caller,
   `one_request`, holds the owning references for the whole chain; 2 pairs per layer, 18 pairs per request); (b) every load of
   a field into a local inside a layer (`headers`, `body`, `sess`, `resp`, `resp->body`): the parent is itself borrowed and no
   code can replace the field during the borrow; (c) W1 `walk` and the W4 traversal pass the object/child they already hold.
2. **Move on last use.** The constructor stores of `headers` and `body` into the request, `body` into the response, `resp` into
   the Ctx, and the session reference returned by the table lookup into `ctx->sess`.

Not elided (a compiler could not prove them): the clone made by the shared-table lookup, the release of replaced sessions, and
everything done through the table lock. Whether a real Gort compiler could do (1b) depends on its escape/aliasing analysis; (1a)
and (2) are the standard "caller outlives callee" and "last use" cases.
**C4 is an upper bound on what an optimiser recovers, not a measurement of one.** In W1, C4 is C0 by construction.

## 4. Environment and measurement control

* Machine: see `results/env.txt`. 16 physical cores with SMT (cpu N and N+16 are siblings). Threads are pinned with `taskset`
  to cores `0..T-1` (distinct physical cores, SMT siblings idle), cpu 2 for single-threaded work. For T > 8 the threads span two
  CCDs (cores 0-7 and 8-15), which changes cache-to-cache latency; this affects every implementation alike.
* All implementations are pinned by an **affinity mask**, not one thread to one core, for equal treatment: Go and the JVM cannot
  pin individual threads without extra machinery.
* **CPU frequency was not pinned.** The published run used the `performance` governor with EPP `performance` under `amd-pstate-epp` with boost on, set by
  `run_performance.sh` and restored afterwards. An earlier full run used `powersave` / `balance_performance`; the two agree (median metric change
  0.0-0.3% per workload, 30 of 1,098 metrics beyond 8%, mostly W2 at 16 threads and tail latencies). Boost was on in both, and idle cores ramp from
  about 600 MHz under load, so each run begins with a warm-up and long runs are favoured. Frequency differences between runs remain a source of noise
  that the min/max columns expose. Boost-off or a fixed clock was not tried.
* `perf` is not installed (and `perf_event_paranoid=3`), so **cycles, instructions and cache-misses were not collected**.
  Cache-line bouncing is inferred from throughput scaling only.
* Java: JDK as in `env.txt`, flags `-XX:+UseG1GC` or `-XX:+UseZGC` set explicitly (the JDK would silently pick SerialGC on a 1-CPU
  mask). Heap sizing is the default in the main runs; `run_java.sh` repeats them with `-Xmx1g`. JMH 1.37 for W1/W2 (`-f 10 -wi 2 -i 3` for W1; W2 `-wi 1 -i 2`, 10 forks, G1 at 1/2/4/8/16 threads, ZGC W2 only at 1/8/16
  threads to bound runtime). **Known defect, fixed:** the first two full runs produced no Java W2 rows, because `run_all.sh` split
  its "name:flags:threads" argument list on `:`, which also occurs in `-XX:+UseG1GC`; the delimiter is now `|` and W2 Java was rerun afterwards. W3/W4: JIT pre-warm of 1.5M requests on a throwaway table (so the shared table's state evolution
  still matches the other languages), then the same discarded warm-up as C, then measurement. Worker threads are virtual threads.
* Go: `testing.B`, `-count=10`, `-cpu T` for W2; W3/W4 with `GOMAXPROCS = workers` (default `GOGC=100`); an extra `Go-gogc400`
  row is reported as a sensitivity run, **not** as the Go baseline.
* Rust 1.93, `opt-level=3`, fat LTO, `codegen-units=1`, `panic=abort`; criterion with `sample_size(10)` (10 samples = 10 "runs"),
  W2 through `iter_custom` with T threads.
* C: gcc 14.2 `-O2` and `-O3`, `-march=native`. W1 and W3 are run at both. W2 and W4 at `-O2` only (time).
* Runs: 10 per cell (W5: 5 for time; W3 `work=8`: 5). Median with min–max reported. C/Go/Rust W3/W4 runs interleave the
  implementations (repetition-major) so slow drift in machine state does not favour whoever ran last. W1/W2 harnesses for Go/Java/Rust
  repeat internally, so those are not interleaved.

## 5. Known sources of bias (ranked roughly by how much I expect them to matter)

1. **Allocator asymmetry.** C and Rust use glibc `malloc`/`free` for every object (6 per request); Go and Java use bump allocation
   plus GC. In W3 the allocation path is a large part of the request cost, so this dominates the C-vs-Go/Java comparison in either
   direction, independent of refcounting. glibc's per-thread arenas make C's frees cheap here (objects are freed by the thread that
   allocated them). A real Gort runtime could use something better or worse than glibc. The C0/C1/C2/C3 comparison is not affected
   (same allocator in all).
2. **The request is thin.** At ~0.2-0.3 µs of CPU per request, refcount traffic is a larger fraction than in a service doing JSON
   parsing, database calls and so on. `work=8` shows the sensitivity. A real server handler is probably heavier, which would shrink
   every ratio toward 1.
3. **C4 is hand-optimised** and C2 is hand-pessimised ("naive clone per pass") by my choice of what counts as a pass. A different
   but still defensible definition of "pass" (e.g. not counting field loads) moves the C2 number. The static/dynamic counts per
   request are in `results/tables.md`.
4. **Shared-table synchronisation is not the same in every language.** C and Rust use striped locks (spinlock in C, `RwLock` in
   Rust, `RWMutex` in Go) because a refcounted reader needs the lock to make load+increment atomic. Java uses `ConcurrentHashMap`,
   whose lock-free reads need no count at all (the measured Java G1 hot_same throughput, 120 Mops/s against 4 for C2, shows the size of the effect). W2/W3 numbers therefore include a cost that Java does not pay and that is a consequence
   of the counted design, not of the flag or atomics. C0 raw in W2 keeps the lock for the same reason (it is the same table code);
   in W3 `table=private` vs `table=shared` for C0 measures that cost on its own.
5. **Raw cannot free a replaced object that readers may hold.** C0 parks replaced W2/W3 objects on a per-thread list and frees
   them after the timed region. That gives raw a (small) cost advantage that a real raw design cannot have without epochs or hazard
   pointers.
6. **C1 can only run where it is sound**, so the C1 ladder rungs in W3 use private tables.
7. **Escape analysis.** Go and Java could stack-allocate or scalar-replace request objects if they did not escape. The Go build
   creates them through non-inlined constructors and dispatches layers through a function value; in Java the recursive layer call
   is not fully inlined so the objects escape. I did not verify this with `-gcflags=-m` / `-XX:+PrintEscapeAnalysis`, only by the
   allocation behaviour (RSS growth/GC activity), so it remains an assumption.
8. **Java RSS is dominated by heap sizing** (G1/ZGC grow the heap before collecting), so peak RSS is not a measure of live data.
   The Go figure is also GC-pacing dependent. Peak RSS in W3 includes a 4 B/request latency array (20 MB) in every language.
9. **Latency measurement cost.** Two clock reads per request: `clock_gettime` in C, `time.Since` in Go, `System.nanoTime` in Java,
   `Instant::now` in Rust. They cost 20-40 ns each and differ slightly; p50 on a 200 ns request is sensitive to this. Latency is
   service time of a closed loop, not queueing delay under offered load.
10. **Flat arrays in Java.** Java headers are one `long[24]`, not an array of structs, so object sizes match but the layout is
    not byte-identical. Rust and Go use inline struct arrays.
11. **W1 is dominated by call/ret throughput.** On this core the non-atomic count update overlaps with the call machinery, so the
    "pure inc/dec cost" in W1 is partly hidden, and C0 can even measure slower than C1 at small depths (code layout noise).
    W1 reports the cost of passes in a call chain, not the isolated cost of one instruction.
12. **JMH/criterion/testing.B do not run 100M iterations**; each chooses its own count. Only ns/pass is comparable.
13. **W5** uses a thread-per-core epoll design for C but goroutines (net/http) and the JDK's built-in `HttpServer` (virtual thread per
    request, selector for connections) for the others. They are idiomatic stacks, not equivalent designs. The C server frees its
    4 KB read buffer between requests, which makes its per-idle-connection RSS small; Go and Java cannot easily do that.
    The load generator is not `wrk`. 10k idle connections are held by a small C program, not by `oha`.

## 6. Things not measured

* CPU counters (cycles, IPC, cache misses): `perf` unavailable.
* Fixed frequency / SMT-off runs, a second machine, NUMA effects (single-socket machine).
* A real Gort compiler's output: the C is hand-written; the elisions in C4 were chosen by me, not found by a tool.
* Real network I/O beyond loopback, TLS, or request payloads that vary in size.
* W2 with C1 at more than one thread (unsound by construction), and Java W2 under ZGC at 2 and 4 threads (time; G1 W2 and ZGC at 1/8/16 were measured).
* Anything about long-term fragmentation, memory returned to the OS, or tail latency under GC pressure beyond the W3 p99/p99.9.
