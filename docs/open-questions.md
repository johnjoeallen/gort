# Open questions

## Design

- Per-instance sharing: does the flag test on each reference-count operation cost more than always-atomic for non-local references? See [sharing](sharing.md).
- Sharing (direction chosen, see [sharing](sharing.md)): the cost of the per-instance shared flag, publishing a graph, where the lock word lives, write-once fields across modules, access modes on interfaces.
- Relocation (direction chosen, see [D15](architecture/open-decisions.md#d15-growable-arrays-relocation-and-stable-references)): a non-moving header and `(handle, offset)` references. Open: the per-access cost, when a dedicated virtual region is worth it, FFI pinning, and the precision of the may-grow summary.
- Exact string representation, slicing and UTF-8 indexing.
- How does a throw interact with a scope that still has child vthreads?
- Stack model: fixed reserve with guard pages, or segmented stacks.
- How are exported lifetime and effect metadata versioned? Direction in [architecture](architecture/modules.md); policy open.
- Architecture decisions D1 to D18 (D18: memory model, values, handles, chunks, arrays, scopes and C interop, D14: packages and imports, D15: relocation and stable references, D16: composition and interfaces instead of inheritance, D17: moving objects between chunks and weak promotion) (where lowering happens, borrowed locals with lazy frames, minimum IR, effects in public interfaces, fusion across modules, debugging maps, dynamic loading, blocking FFI, slice-check overhead and loop warnings): [open decisions](architecture/open-decisions.md).

## Not yet built or measured

- Any compiler, and therefore any module file, effect inference or generated continuation code. Everything here is hand-written C beside `.gort` source.
- Contention: run Gort's spin-then-park `sync` lock in the W2 harness with and without work, and measure tail latency near the lock ceiling. See [contention budget](architecture/contention.md#6-open).
- The multi-object lock retry, and a sharded store to exercise it.
- Incremental table resize and key-expiry accuracy in mini-redis.
- A Rust or Go mini-redis for a baseline.
- Streaming and multipart bodies, offloaded file I/O and exception unwinding in the C.

## Waiting on a decision

- Is `Disconnect` the right default for a slow subscriber?
- Should constants become camelCase too?
