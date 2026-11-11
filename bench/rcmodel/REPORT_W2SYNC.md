# W2 follow-up: reader-friendly shared-table synchronisation

Follow-up to `REPORT.md` section 8, recommendation 3: "Do not ship the benchmark's shared-table lock ... evaluate a reader-friendly
scheme (RCU, hazard pointers, or reader-writer stripes) and repeat W2 hot_same against it."

Code: `c/w2sync.c`. Driver: `run_w2sync.sh`. Data: `results_w2sync/raw.csv`, tables in `results_w2sync/tables.md`, environment in
`results_w2sync/env.txt`. 5 repetitions per cell (stall: 3), fixed 1 s duration, median (min-max) reported. 0 leaked objects over all runs.

## 1. Summary

- **The count was the problem, not only the lock.** Removing the lock and keeping counted reads (L2) lifts hot_same at 16 threads
  from 3.0 to 12.0 Mops/s. Removing the count too (L3, borrowed reads) lifts it to 200.6. Lock-free plus counting is 17x slower than lock-free plus borrowing.
- **A borrowed read beats Java on a single hot key.** L3 reaches 201 Mops/s and hazard pointers with an asymmetric fence (L4') reach 219,
  against Java G1's 120 from the main run (different runs; see section 5).
- **Hazard pointers, not QSBR.** This reverses the recommendation made before the run. Asymmetric-fence hazard pointers match or beat
  QSBR on throughput everywhere once the scan threshold is set (2,222 vs 1,261 Mops/s on hot_diff). They also keep garbage strictly
  bounded (16k objects at 16 threads). QSBR's garbage is unbounded even with no deliberate stall: one run with no stall peaked at
  2.0M objects (563 MB RSS), and a 100 ms stall gives 1.3M.
- **Auto-promotion is affordable.** Promoting 10% of borrows costs 28% on the single hot key (the worst case) and 15% on uniform keys.
  With 1 µs of handler work after the read, promoting *every* borrow costs 7.6%. At 10 µs every scheme, including the spinlock, is
  at the ideal 1.5 Mops/s. This supports the decision to auto-clone escaping borrows with a warning.

## 2. Schemes

All variants are the flagged Gort model (`MODE=2`); every table entry is shared. A read looks up a key, walks the entry 3 frames deep
(as in `rcm.c` W2), and drops it. A write (5% by default) allocates a new entry and swaps it in.

| | scheme | read path | the table's reference to a replaced entry is released |
|---|---|---|---|
| L0 | `spin` | exclusive stripe spinlock, clone under the lock, counted walk (= `rcm.c` W2) | at once |
| L1 | `rw` | reader-writer stripe lock, clone under the read lock, counted walk | at once |
| L2 | `qsbr_clone` | lock-free load, clone, counted walk | after a QSBR grace period |
| L3 | `qsbr_borrow` | lock-free load, **borrow**: no clone, uncounted walk | after a QSBR grace period |
| L4 | `hp` | hazard-pointer publish + `mfence` + revalidate, borrow | when no hazard pointer covers it |
| L4' | `hp_asym` | hazard-pointer publish + compiler barrier only, borrow; reclaimer calls `membarrier(PRIVATE_EXPEDITED)` | same as L4 |

QSBR follows liburcu-qsbr. A thread announces a quiescent state every `q` reads (default 16; this models a fiber yield or request
boundary) by bumping its own counter between two full fences. Retired entries are batched by 32 with a snapshot of every counter.
Hazard pointers use one slot per thread and scan once a thread has `hpt` retired entries (default `2T+64` = 96 at 16 threads).

Promotion (`ppct`) models the compiler-inserted clone when a borrow escapes. The clone is taken while the borrow is valid and kept
until the thread's next promotion drops it (one increment and one decrement on the shared count). `work` is thread-local compute after the
read (about 1 ns per unit; 1000 is about 1 µs). `split=1` moves the data the reader loads off the cache line that holds the count.

Before the measured runs, the counted spin baseline was checked against the original benchmark: `rcm.c` C2 gives 3.9-4.1 Mops/s and
`w2sync spin` 3.8 (fixed op count). All schemes ran clean under AddressSanitizer at 16 threads, 20% writes and 30% promotion.

## 3. Results

### 3.1 hot_same (all threads on one key), 5% writes, Mops/s

| | T=1 | T=2 | T=4 | T=8 | T=16 |
|---|---|---|---|---|---|
| L0 spin | 30.0 | 14.3 | 10.5 | 6.9 | **3.0** |
| L1 rw | 27.1 | 12.1 | 11.6 | 10.0 | 5.9 |
| L2 qsbr+clone | 31.8 | 15.5 | 14.7 | 13.9 | 12.0 |
| L3 qsbr+borrow | 157.1 | 146.7 | 220.7 | 305.8 | **200.6** |
| L4 hp | 126.7 | 127.0 | 190.1 | 259.2 | 185.7 |
| L4' hp+membarrier | 158.0 | 148.0 | 217.3 | 297.4 | **218.8** |

Ranges are within +/-4% of the median except where noted in `tables.md`. Reader-writer stripes (L1) only double the spinlock. A
reader-writer lock's reader count is itself one contended line. The counted schemes all collapse the way C3 did in the main report: 8 atomic
read-modify-writes per read on one line. The borrowing schemes scale until the read crosses to the second CCD at T=16 (section 3.6).

### 3.2 uniform keys and hot_diff, T=16, Mops/s

| | uniform | hot_diff |
|---|---|---|
| L0 spin | 291.2 | 474.9 |
| L1 rw | 280.4 | 430.7 |
| L2 qsbr+clone | 198.7 | 457.5 |
| L3 qsbr+borrow | 260.7 (238.9-301.9) | 1,260.7 |
| L4 hp | 261.0 | 1,696.8 |
| L4' hp+membarrier, hpt=96 | 249.2 | 603.6 |
| L4' hp+membarrier, hpt=1024 | - | **2,222.4** |

On uniform keys the borrowing schemes win at T=2 to T=8 (T=8: L3 390 vs L0 218, +79%). At T=16 they lose by 10% (section 4).
At the default threshold, hp_asym collapses on hot_diff: every 96 retires it sends a membarrier IPI to every CPU. Raising
the threshold to 1024 fixes this and makes it the fastest scheme (3.2).

### 3.3 Write ratio (hot_same, T=16): where the remaining cost is

| | wpct=0 | 1 | 5 | 20 |
|---|---|---|---|---|
| L0 spin | 3.1 | 3.1 | 3.0 | 3.0 |
| L2 qsbr+clone | 10.3 | 10.6 | 12.0 | 23.1 |
| L3 qsbr+borrow | **3,758.6** | 587.0 | 200.6 | 70.3 |
| L4' hp+membarrier | **3,958.6** | 616.5 | 218.8 | 78.6 |

With no writes, a borrowed read on a hot key costs nothing shared: 235 Mops/s per thread. All of the remaining hot_same cost of a
borrowing scheme is the writes: the slot line is invalidated under 15 readers, plus malloc and deferred free. L2 *rises* with the write
ratio because every write is one fewer read on the hot count line.

Moving the data the reader loads off the count's cache line (`split=1`) changes nothing measurable, within 3% for every scheme and write
ratio. The counted collapse is true sharing of the count itself, not false sharing with the data.

### 3.4 Promotion and handler work (T=16)

| promotion rate | 0% | 10% | 50% | 100% |
|---|---|---|---|---|
| L3, hot_same | 200.6 | 144.6 | 57.5 | 29.6 |
| L4', hot_same | 218.8 | 150.7 | 57.8 | 29.9 |
| L3, uniform | 260.7 | 222.7 | 206.1 | 216.5 |
| L4', uniform | 249.2 | 225.5 | 203.1 | 213.3 |

Even at 100% promotion, a borrowing scheme is 2.5x L2 and 10x L0 on the hot key: one increment and decrement instead of eight.

| hot_same, T=16 | 1 µs work | 10 µs work |
|---|---|---|
| ideal (16 threads / time per op) | about 15.3 | about 1.54 |
| L0 spin | 3.9 | 1.5 |
| L1 rw | 8.6 | 1.5 |
| L2 qsbr+clone | 11.5 | 1.5 |
| L3, 0% / 100% promotion | 14.5 / 13.4 | 1.5 / 1.5 |
| L4', 0% / 100% promotion | 14.3 / 13.4 | 1.5 / 1.5 |

At 1 µs of handler time, the spinlock still serialises (25% of ideal) while borrowing reaches 95%. Promoting every borrow costs 7.6%.
At 10 µs, everything is at ideal. A handler that makes a network or database call (100 µs or more) cannot see the difference between any of these schemes.
The scheme matters for hot shared data read by tiny handlers or in tight loops.

### 3.5 Garbage and stalled readers (hot_same, T=16)

| | stall 0 ms | 10 ms | 100 ms |
|---|---|---|---|
| L3 qsbr, peak retired objects | 234k (98k-2.0M) | 180k | 1.32M |
| L3 qsbr, RSS MB | 48 (23-563) | 42 | 360 |
| L4 / L4' hp, peak retired objects | 1,536 | 1,536 | 1,536 |
| L4 / L4' hp, RSS MB | 4 | 4 | 4 |

Thread 0 parks for `stall_ms` while holding a borrow, with no announcement. QSBR cannot free anything retired after that point
until it returns. More importantly, it is unbounded **without** a deliberate stall: one of five no-stall runs peaked at 2.0M objects.
Any reader that is descheduled or slow to reach a quiescent point stalls reclamation for every thread. Hazard-pointer garbage is
exactly `T x hpt` (16 x 96 here; 16k at hpt=1024, about 3 MB of 192-byte entries) whatever the readers do. A stalled hazard pointer
pins one object.

Throughput was unaffected by stalls in every scheme.

### 3.6 Other observations

- **QSBR quiescent interval:** announcing at every read (q=1, two `mfence`s) costs 25% against q=16. q=256 gains another 7.5%.
  Hazard pointers pay for their read fence per read instead. The symmetric `mfence` makes L4 about 15% slower than L4' on hot_same.
- **The T=8 to T=16 drop for borrowing schemes** (hot_same L3 306 to 201, uniform 390 to 261) coincides with the run spilling onto
  the second CCD (`lscpu`: cores 8-15 share L3 1). Write-induced line transfers become cross-die. The counted schemes are already
  line-bound, so they don't show it.
- **Fixed duration vs fixed op count:** with a fixed op count, spin measured 3.8 instead of 3.0 (2.4 in a 1 s smoke test). Threads
  that win the unfair lock finish early and the tail runs with less contention. All results here are fixed-duration.

## 4. Pre-registered criteria

Stated before the run (discussion of `REPORT.md` recommendation 3):

| criterion | result | verdict |
|---|---|---|
| L3 >= 0.5x Java G1 hot_same at 16 threads | 200.6 vs 120 (1.67x); L4' 218.8 (1.82x) | **Pass** (cross-run comparison, section 5) |
| L3 within 10% of L0 on uniform keys | T=16: 260.7 vs 291.2 (-10.5%); T=2-8: +3% to +79% | **Fail at T=16, narrowly**; L3's range (239-302) overlaps L0's median |
| Garbage bounded without a stalled reader | QSBR: no (peak 2.0M objects with no stall); HP: yes (exactly T x hpt) | **Fail for QSBR, pass for HP** |
| L2 close to L3 means the count is not the problem | L3 / L2 = 17x on hot_same | **The count is the problem**: shared reads must be borrows |

The uniform T=16 deficit is not explained. Both borrowing schemes show it (L3 260.7, L4 261.0) and L2 is lower still (198.7). That
points at deferred reclamation (entries freed cold, later, from a different thread) rather than the read path, but without `perf`
this is a hypothesis.

## 5. Threats to validity

- **Java comparison is cross-run.** Java G1's 120 Mops/s comes from the main run (performance governor; JMH, which is time-based).
  This run used the `powersave` governor (amd-pstate-epp, boost on). The main report found the two governors agree within noise, but
  Java was not rerun here.
- **Rust and Go comparators were not rerun with lock-free reads** (`arc-swap`, `atomic.Pointer`). The Go and Rust W2 numbers in `REPORT.md`
  still use locks, so they are not comparable to L3/L4.
- **One reader, one borrow.** Each read holds a single hazard pointer. Real handler code holding several borrows at once needs one
  slot per borrow (section 6).
- Single machine, SMT on but unused (cores 0-15 are distinct physical cores), no `perf` counters, hand-written C emulation.

## 6. Recommendation

1. **Ship hazard pointers with an asymmetric fence (L4') for shared-table reads, with a scan threshold of about 1024 retired objects per thread.**
   Read cost: one store, a compiler barrier and a reload, the same throughput as QSBR. Bounded garbage: `threads x 1024` objects.
   On platforms without `membarrier` (or an equivalent such as Windows `FlushProcessWriteBuffers`), fall back to the symmetric fence,
   which is about 15% slower on hot_same.
2. **Reject QSBR as the primary scheme.** Its read path is no faster here, and its garbage depends on the slowest thread. For a
   server under memory limits, that is a failure mode, not a tuning issue.
3. **Hazard slots per thread are enough, because of the auto-promotion decision.** A borrow held across a yield is auto-promoted
   to a counted clone, so a fiber never suspends while holding a hazard pointer. The number of slots a thread needs is the maximum
   number of simultaneous borrows in a non-yielding region, which the compiler knows statically. Where a region needs more borrows
   than there are slots, promote (and warn), exactly as for an escape.
4. **Keep auto-promotion with a warning.** Its cost is bounded by the counted path it falls back to (at worst 30 vs 12 Mops/s on
   a hot key), and it disappears under any realistic handler time (7.6% at 1 µs, nothing at 10 µs).
5. **Next:** investigate the uniform T=16 deficit once `perf` is available. (The Java rerun and the Rust `arc-swap` / Go `atomic.Pointer`
   comparators are done under one protocol in `REPORT_W2X.md`.)

## 7. Reproduce

```
./run_w2sync.sh                       # everything, about 20 min; REPS=5 DUR=1000 by default
PHASES="build hpt analyze" ./run_w2sync.sh   # one phase: env build main wsweep promote work stall q hpt analyze
c/build/w2sync scheme=hp_asym mode=hot_same threads=16 dur_ms=1000 hpt=1024
```
