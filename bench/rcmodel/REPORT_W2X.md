# W2 comparative hot path: Gort schemes vs Go, Java, Rust, with and without real work

Follow-up to `REPORT_W2SYNC.md`. That report compared synchronisation schemes for the Gort model in C only, and borrowed its Java number
from an earlier run with a different protocol. This one runs every language through **one protocol**, on the same machine in the
same session. Each language gets its idiomatic lock-based table and its best idiomatic lock-free table, each with and without
real work per read.

Drivers: `c/w2sync.c` (with `warm_ms`), `go/w2x/main.go`, `java/src/main/java/rcm/W2X.java`, `rust/src/bin/w2x.rs`.
Runner: `run_w2x.sh`. Data: `results_w2x/raw.csv`, tables in `results_w2x/tables.md`, environment in `results_w2x/env.txt`.

## 1. Protocol

Identical in all four drivers:

- **Table and ops:** 10,000-entry shared table, all threads on key 17 (hot_same). 95% reads, 5% writes; a write allocates a new entry and swaps it in.
- **Read:** look up the entry, walk it 3 frames deep (non-inlined calls), add its value to a sink.
- **Real work** (`hash=`): FNV-1a over `hash` bytes of a thread-local 16 KB request buffer, seeded by the value read, after the read
  (a handler parsing or serialising). Single-thread calibration at hash=1500: 0.974-0.980 Mops/s in **every** language, so the work
  costs the same everywhere: about 1.02 µs per read at hash=1500 and about 10 µs at hash=15000.
- **Measurement:** each thread publishes its op count every 64 ops. After 500 ms of warm-up, the op-count delta over a 1,000 ms window gives Mops/s. Same pinning as
  all earlier runs (cpu 2 for one thread, cores 0..T-1 otherwise; distinct physical cores; cores 8-15 are the second CCD).
- **Repetitions:** 5 per cell, implementation order rotated per repetition, median (min-max) reported.

| implementation | read | reclamation |
|---|---|---|
| C spin, counted | stripe spinlock, clone, counted walk: **Gort as benchmarked today** | immediate |
| C lock-free, counted (QSBR) | lock-free load, clone, counted walk | QSBR |
| C borrow, QSBR | lock-free load, borrow, uncounted walk | QSBR |
| C borrow, hazard ptr (L4') | hazard pointer, asymmetric fence, borrow | scan at 1024/thread, `membarrier` |
| C borrow, hazard ptr, 100% promoted | as L4', and every borrow escapes: compiler-inserted clone + later drop | as L4' |
| Go RWMutex stripes | `sync.RWMutex` stripes (as the main run) | GC |
| Go `atomic.Pointer` | `[]atomic.Pointer[Entry]` | GC |
| Java `ConcurrentHashMap` (G1, ZGC) | `CHM<Integer,Entry>.get` (as the main run) | GC |
| Java `AtomicReferenceArray` (G1, ZGC) | `ARA<Entry>.get` | GC |
| Rust `RwLock<Arc>` stripes | read lock, `Arc` clone (as the main run) | refcount |
| Rust `arc-swap` 1.9.5 | `ArcSwap::load()` guard (debt-list borrow) | refcount |
| Rust `crossbeam-epoch` 0.9.21 | `pin()` guard, `Atomic::load` | epoch (EBR) |

## 2. Results: hot_same, Mops/s total

### 2.1 No work

| | T=1 | T=4 | T=8 | T=16 |
|---|---|---|---|---|
| C spin, counted (Gort today) | 30.2 | 10.4 | 6.73 | **3.01** |
| C lock-free, counted | 32.0 | 14.6 | 14.1 | 12.3 |
| C borrow, QSBR | 154 | 220 | 305 | 202 |
| **C borrow, hazard ptr (L4')** | 147 | 216 | 299 | **219** |
| C borrow, hazard ptr, 100% promoted | 88.5 | 48.5 | 40.5 | 30.1 |
| Go RWMutex stripes | 84.1 | 69.3 | 63.2 | 59.2 |
| Go atomic.Pointer | 167 | 309 | 340 | **298** |
| Java G1 ConcurrentHashMap | 235 | 165 | 149 | 107 |
| Java ZGC ConcurrentHashMap | 196 | 188 | 150 | 101 |
| Java G1 AtomicReferenceArray | 305 | 318 | 325 | 193 (169-271) |
| Java ZGC AtomicReferenceArray | 255 | 340 | 370 | **279** |
| Rust RwLock<Arc> stripes | 60.0 | 19.5 | 12.5 | 7.44 |
| Rust arc-swap | 82.4 | 99.2 | 100 | 59.8 |
| Rust crossbeam-epoch | 115 | 177 | 236 | 155 |

### 2.2 About 1 µs of real work per read (hash=1500)

Ideal at T=16 is about 15.6 Mops/s (16 x the single-thread rate). Efficiency is relative to each implementation's own single-thread rate.

| | T=16 Mops/s | efficiency at T=8 | T=16 |
|---|---|---|---|
| C spin, counted (Gort today) | 4.01 | 94% | **26%** |
| C lock-free, counted | 11.6 | 94% | 75% |
| C borrow, QSBR | 14.6 | 98% | 94% |
| **C borrow, hazard ptr (L4')** | **14.5** | 98% | **93%** |
| C borrow, hazard ptr, 100% promoted | 13.6 | 96% | 87% |
| Go RWMutex stripes | 2.54 (1.75-3.13) | 50% | 16% |
| Go atomic.Pointer | 14.2 | 98% | 91% |
| Java G1 / ZGC ConcurrentHashMap | 14.6 / 14.6 | 98% | 93% / 94% |
| Java G1 / ZGC AtomicReferenceArray | 14.6 / 14.7 | 98% | 93% / 94% |
| Rust RwLock<Arc> stripes | 9.50 | 94% | 61% |
| Rust arc-swap | 13.8 | 98% | 88% |
| Rust crossbeam-epoch | 14.6 | 98% | 94% |

### 2.3 About 10 µs of real work per read (hash=15000)

Every implementation is at 1.52-1.54 Mops/s at T=16 (97-100% efficiency), **including the C spinlock (98%)**, with one exception:
Go RWMutex at 0.69 (44%). Two Java CHM runs had one slow repetition each (0.27-0.28); the medians are unaffected.

### 2.4 Reference: hot_diff (each thread its own hot key), T=16, no work

| | Mops/s |
|---|---|
| C borrow, hazard ptr (L4') | **2,063** |
| Java G1 / ZGC AtomicReferenceArray | 1,818 / 1,779 (wide ranges) |
| C borrow, hazard ptr, 100% promoted | 1,298 |
| C borrow, QSBR | 1,285 |
| Java G1 / ZGC ConcurrentHashMap | 1,041 / 564 |
| Rust RwLock<Arc> | 945 |
| Go atomic.Pointer / RWMutex | 785 / 560 |
| Rust crossbeam-epoch | 505 |
| C spin / lock-free counted | 479 / 476 |
| Rust arc-swap | 84.9 |

### 2.5 Peak RSS at T=16, hot_same, no work

C hazard ptr 8 MB, C QSBR 47 MB, Go 9-44 MB, Rust 5-31 MB, Java 950-2,550 MB. Java RSS reflects default heap sizing, not live data (see `REPORT.md` 4.6).

## 3. Findings

1. **With no work, the borrow design is competitive with the best lock-free reads in GC'd languages.** It trails a few of them on one hot key.
   L4' reaches 219 Mops/s at T=16: ahead of idiomatic Java (CHM, 101-107), Java G1 ARA (193) and every Rust option
   (epoch 155, arc-swap 60). It is behind Go `atomic.Pointer` (298) and Java ZGC ARA (279). On a single CCD (T=8) the order is the
   same: C 299-305 vs Go 340 and Java ARA 325-370. The remaining gap is the write path: 5% of ops allocate, and Gort frees through
   `malloc`/`free` and a reclamation scan, while GC'd languages bump-allocate and free in bulk. That is `REPORT.md` recommendation 4 (allocation, not counting),
   not a read-path problem. With independent keys (hot_diff) L4' is the fastest implementation measured, at 2,063.
2. **Gort's current lock is the worst option in the field:** 3.0 Mops/s, against 7.4 for the Rust idiom and 59 for Go RWMutex. This settles
   recommendation 3 of `REPORT.md`: it must not ship.
3. **With about 1 µs of real work, every lock-free implementation reaches parity at 88-94% efficiency, and Gort is in that group** (L4' 14.5,
   QSBR 14.6, Java 14.6-14.7, Rust epoch 14.6, Go atomic 14.2, arc-swap 13.8). Locks still cost a lot at this grain:
   C spin 26%, Rust RwLock 61%, Go RWMutex 16%. Counted lock-free reads (75%) are not enough either. The borrow is what reaches parity.
4. **With about 10 µs of real work, the choice no longer matters:** every implementation, including the spinlock, is at 97-100%.
   Go RWMutex is the exception (44%). The likely cause is that its writer preference parks readers behind a pending writer, so each
   reader pays a park/wake cycle. That is unverified without a profiler.
5. **Auto-promotion is cheap under real work.** Promoting every borrow costs 86% of throughput with no work (219 to 30, the cost of one
   contended count round-trip per read). That drops to 6% at 1 µs (14.5 to 13.6) and 0% at 10 µs. This confirms the auto-clone-with-warning decision:
   the cost only appears where there is nothing else to do.
6. **Rust's popular `arc-swap` is a poor fit for this pattern.** It reaches 60 Mops/s on the hot key and collapses to 85 on hot_diff: writes
   to independent keys contend on its shared debt list. Epoch-based reclamation is the Rust option that tracks Gort's borrow.

## 4. Threats to validity

- **Governor and machine.** `powersave` governor (amd-pstate-epp, boost on). Single machine, SMT siblings idle, no `perf`.
- **Gort is emulated in C.** The C numbers are an upper bound on what a Gort compiler emits for this code, as in the main report.
- **Java is not the default idiom in every row.** `AtomicReferenceArray` is the best-case lock-free Java table, not the idiom (CHM is). Java
  ARA G1 at T=16 has a wide range (169-271).
- **The work is synthetic.** Real work here is CPU work that never yields. Real I/O would also yield, which forces promotion of a held borrow.
  That case is the "100% promoted" row, which is at parity from 1 µs of work up.
- **One hot key and 5% writes are the stress case.** On uniform keys `REPORT_W2SYNC.md` found an unexplained 10% deficit for borrowing
  schemes against the spinlock at T=16. It was not rerun here.

## 5. Conclusion

Under one protocol, the borrow design with hazard pointers puts Gort's hot shared reads in the same class as the best Go, Java
and Rust lock-free reads. It is at parity from about 1 µs of handler work up, within 0.7-1.4x of the fastest with no work at all,
and the fastest on independent hot keys. The benchmark's current lock is 70x behind the best with no work and 3.6x behind at 1 µs.
Promoting escaping borrows costs nothing measurable once a request does real work. The remaining no-work gap belongs to allocation, not to reads or counts.

## 6. Reproduce

```
./run_w2x.sh                                  # about 25 min; REPS=5 WARM=500 DUR=1000
PHASES="build diff analyze" ./run_w2x.sh      # phases: env build hot diff analyze
go/w2x/w2x_go impl=atomic threads=16 mode=hot_same hash=1500
java -XX:+UseZGC -Drcm.variant=zgc -cp java/target/rcm.jar rcm.W2X impl=ara threads=16
rust/target/release/w2x impl=epoch threads=16 hash=15000
c/build/w2sync scheme=hp_asym hpt=1024 threads=16 mode=hot_same hash=1500 warm_ms=500 dur_ms=1000
```
