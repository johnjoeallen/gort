# gortc: ref against shared in generated code

`refcount.gort` runs four workloads on `ref` (plain count, no lock) and `shared` (atomic count, reader-writer lock) objects, one thread.
`../run_gortc_refcount.sh [reps]` builds it twice with `gortc` (`GORTC_OPT=-O2`), once normally and once with `--atomic-refs`, which gives every
`ref` object the shared layout, and writes `../results/gortc_refcount.{txt,md}`. The table is in picoseconds per iteration.

| workload | what one iteration does |
|---|---|
| store | stores a handle into a struct field: one retain, one release of the old handle |
| sync | a `sync (writes x)` block that writes one field: a lock and an unlock for a shared object, nothing for a ref |
| alloc | `new` of a one-field object, then it goes out of scope |
| list | appends a node to a 1000-node list and walks it. The shared version needs a `sync` per node (the compiler requires it), so it also pays locks |

## Reading the result

- `--atomic-refs` makes the `ref` column equal the `shared` column in every workload that has no extra `sync` (store 7248 against 7241, alloc 13937
  against 13933), so the flag does what it says and any program can be measured both ways. It does not add `sync` blocks to source that has none, so
  the `list` row differs by exactly those locks (37.8 ns atomic counts only, 68.2 ns with the locks).
- The cost is the atomic read-modify-write: about 3.6 ns per retain or release here (7.2 ns for the pair). It is a per-core cost, not a contention
  cost. Nothing here has two threads touching one object, which is where a shared object gets much worse (see `rcmodel` and `docs/architecture/contention.md`).
- `alloc` differs by 3.6 ns because the allocation count and the release use the atomic and the plain forms; both pay the same atomic live-object counter that
  gortc keeps for its leak check, which is why this gap is smaller than the others. Removing that counter from a release build would widen it.
- A function argument that the callee only reads is lent without a count, so passing a handle costs nothing in either layout; `store` is the workload that
  shows the count traffic.

## Caveats

The CPU governor is `powersave` (the earlier rcmodel runs used `performance`), so absolute times are lower-clocked than they could be; the ratios are what to
read. One machine, one thread, `-O2` from gortc's C (no PGO, no LTO). The loops are written so that gcc cannot fold them away, which I checked by seeing
non-zero times; I did not read the assembly.
