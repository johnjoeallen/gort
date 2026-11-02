# Handle indirection with realistic loop bodies (D15, B14 second run)

Source: `bench/c/handle_real.c`, run by `bench/run_handle_real.sh`; raw output `handle_real.txt` (`-O2`) and `handle_real_O3.txt` (`-O3`),
per-configuration overhead tables in `handle_real.tables.md` and `handle_real_O3.tables.md`. gcc 14.2, one pinned core, best of 9 repetitions,
one pass is 2^20 element bodies. Hand-written C; no Strata compiler is involved. Checksums of all five variants agree in every cell.

**Why a second run.** The first run ([`handle.md`](handle.md)) summed an array: a one-cycle dependent add, the best case for hiding a load.
Here every element does real work through the pointer, and every variant, including the raw one, takes a lock the way real code would.

**Element body.** `x = a[i]`; a data-dependent gather `g = a[(x*K1 >> 7) & mask]`; `r = x ^ g`; then **W rounds** of dependent mixing
(`r ^= r>>17; r *= K2`); then `out[i] = r`. Both reads go through the container pointer. W = 0, 1, 2, 4, 8, 16, 32, 64 is the work axis:
the raw body costs about 0.5 ns at W=0 and about 44 ns at W=64 (4 KB, no lock).

**Lock** (the same for every variant, taken once per batch, base pointer loaded after the lock like real code):
`none` (unshared object, R6), `spin` (atomic exchange, release store, uncontended), `mutex` (pthread, uncontended).
**Batch** B = 1 (a lock per element, like `db.get`) or 256 (one `sync` around a loop). **Array** 4 KB, 256 KB, 64 MB.

**Variants** (what the compiler would generate for a window of that length):

| mode | base pointer |
|---|---|
| `raw` | loaded once per lock window, in a register: the C baseline |
| `raw2` | an identical second copy of `raw`: its difference from `raw` is the noise floor |
| `rl16` | reloaded every 16 elements (a may-grow call every 16th element) |
| `rlelem` | reloaded every element (both reads share that load) |
| `rlacc` | reloaded at every access, twice per element: the worst case |

Overhead is reported against the faster of `raw` and `raw2`, because two identical loops were compiled differently (see the caveat).

## Measured (18 configurations x 8 W values per mode, 144 cells each, `-O2`; `-O3` is within a point or two)

Median overhead over the configurations, by work per element (percent over raw):

| W (mixing rounds) | 0 | 1 | 2 | 4 | 8 | 16 | 32 | 64 |
|---|---|---|---|---|---|---|---|---|
| raw ns/element, 4 KB, no lock | 0.52 | 0.80 | 1.06 | 1.69 | 3.13 | 6.71 | 18.9 | 44.0 |
| rl16 | +3.5 | +7.2 | +3.5 | +2.3 | +3.6 | +2.4 | +1.9 | +1.0 |
| rlelem | +4.0 | +1.8 | +0.9 | +2.0 | +2.3 | +1.9 | +0.8 | +0.6 |
| rlacc | +0.9 | +0.3 | +0.8 | +1.5 | +1.7 | +1.3 | +2.5 | +6.1 |

All cells together: **median** rl16 +2.3%, rlelem +1.3%, rlacc +1.0%; **90th percentile** +7.7%, +4.7%, +9.0%; the maxima are below.

Where the overhead is largest (all absolute values small):

- **Almost-empty body, no lock** (4 KB, `none`, B=1, W=0): `rlelem` +0.25 ns on 0.87 ns (+28%); 256 KB: +0.14 ns on 0.78 ns (+18%). The reload is a visible fraction only when the whole element costs under a nanosecond and there is no lock.
- **With a lock the same bodies are hidden:** spin lock, B=1: 4.2 ns per element raw, and the reload variants are within 0.0 to 0.2 ns (0 to +4%). A mutex per element costs more than that.
- **Typical windows** (B=256, W 1 to 16, any lock, all three reload modes, 135 cells): median +2.3%, 90th percentile +6.4%, maximum +13.5%; 77% of cells are under +4%.
- The `rl16` column is a little worse than `rlelem` at small W in several cells. I did not investigate why; it may be branch or alignment effects of the `k & 15` test, not the load.

## Caveats on these numbers

- **The noise floor is not small everywhere.** `raw` and `raw2` are the same code, yet differ by a median 0.3% but up to **19%** (90th percentile 9%). At W=32 gcc compiled the mixing loop differently in `raw` than in the other variants (a 3 ns, about 18% difference), so the `rlacc` "+18%" cells at W=32 are that codegen artefact, not handle cost, and the same cells show `rl16` and `rlelem` "faster" than raw when raw is the baseline. Treat any single cell as plus or minus a few percent, and the W=32 and 64 cells as unreliable. The medians are more trustworthy than the maxima.
- **One loop shape.** Independent loads and a dependent arithmetic chain. A body with stores into the container, branchy parsing, or a hot inner loop that the compiler could otherwise vectorise or unroll would behave differently; the first run showed a lost vectorisation of 0.1 ns per element at `-O3` for a loop that cannot keep the pointer in a register.
- **Single thread, uncontended locks.** Contention changes the lock cost, not the reload, but this is not measured. The `sync` rule that a block cannot be entered while a writer holds the lock is not exercised.
- **The windows are synthetic.** `rl16`, `rlelem` and `rlacc` are chosen window lengths, not the output of a compiler's may-grow analysis. How often a real program ends a window is still unmeasured.
- Reads only; no growth, no `push`; one machine, one compiler.

## Conclusion

Measured on this micro-benchmark: reloading through a non-moving header costs a median of 1 to 2% over a raw pointer for bodies that do real work, with
individual configurations up to about 10% (excluding the compiler artefact above), and up to +28% only when the whole element costs under a nanosecond
and there is no lock. It shrinks as work per element grows and disappears behind a lock. Direction (not measured): that is small enough to accept for the
safety it buys, provided the compiler does not end windows needlessly; that depends on how precise the may-grow summaries are.
