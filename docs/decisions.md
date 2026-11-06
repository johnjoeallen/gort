# Decisions

Rules the design has settled on, with the reason each was chosen.

| Decision | Why |
|---|---|
| Nothing hidden except virtual threads and parking | Programs stay predictable; batching and buffering must be asked for. |
| Every unbounded input has a declared limit | Memory is bounded before data is held; big bodies are streamed or spooled. |
| No biased reference counting in v1 | Equal or worse once objects cross threads; confined types get plain counts. |
| Per-type arenas, no compaction | Segregates lifetimes; addresses stay stable, which the lock order relies on. |
| Chunk epochs in resident chunk headers | `MADV_DONTNEED` zero-fills decommitted generation words. |
| `MADV_NOHUGEPAGE` on stack pools | With transparent huge pages a 64 KB stack costs 65 KB resident. |
| Errors are exceptions with `throws` | Throwing is visible in signatures; no `throws` clause means nothing escapes. |
| Cleanup never parks and never throws | It runs on every exit path, including unwinding. |
| `sync` acquisition parks, FIFO queue, wake-and-retry, 1 ms bound on overtaking | Fairness without handing the lock to a descheduled vthread; workers keep running other vthreads. |
| Spin briefly before parking | Pure parking lost up to 75% of throughput under mixed load. |
| Multi-object heads acquire all-or-nothing | Mis-ordered heads cannot deadlock. |
| Default slow-subscriber policy: `Disconnect` | Never silently loses messages; `Notice` is available. Not yet confirmed by the user. |
| Gort identifiers are camelCase | User preference. Constants stay UPPER_CASE; generated C may use underscores. |
| Syntax: `T?`, methods in types, optional `self`, `Pattern -> {}` arms | Avoids Rust-isms. |
| No third-party crates in converted programs | Gort would have no equivalents. |
| Positioning: a small compiled language for network servers and services, with C++-style value types, Go-style embedding and built-in reference counting; not a systems language and not a Rust or C/C++ competitor | Its runtime and its trade of control for simplicity rule out systems work. Types are structs that carry their functions, written inside the struct or implemented out of line as `fn T::name` (Java and C++ styles, both supported; a deliberate break with Go); no inheritance, `struct Dog: Animal with Body` (Direction, D16). Comparison set is Go and Java; Rust is a performance reference. Beyond them (intent, unbuilt): no GC, compile-time-checked sharing. See spec 1.1. |
| Performance goal: same class as Go and Java vthreads (Rust async as a reference ceiling), with a simpler model | Superiority is not claimed; ceremony is measured alongside speed. |
| Programming model (vthreads) is separate from implementation (resumable frames) | No feature may depend on the implementation; stackful runtime kept as reference. |
| Compiler inserts slice checks only in functions marked `[yield]` (or in a type or package marked so, innermost wins); `[noyield]` overrides; `[nopark]` asserts no parking at all | Principle 10: nothing is inserted unless the code says so. Loops add no effect by default, so looping helpers are usable under locks without tagging. The cost is that unmarked CPU-bound code can stall a worker, which warnings aim to find. |
| Loop warnings are a lint with an advisory `loops` class in the module summary, off by default except unbounded loops in lock bodies | Termination is undecidable; the class is kept out of the contract hash so loop changes never force rebuilds. |
| One `Ref<T>`; sharing is inferred, not declared. a `sync` block required where the object is not provably local and the field is mutable; no-ops on unshared objects | Programmer never decides what is shared; a missing or too-weak block is a compile error; layout and behaviour do not depend on other modules. Cost of per-instance flag still to measure. |
| Java folder convention: a package is a folder, every file declares `package path.to.folder`, a package compiles to one `.smod`, imports name packages, and the package default attribute is repeated on the package line of every file (no special file) | A unit for module compilation, visibility and the `[yield]`/`[noyield]`/`[nopark]` default. The details are open (D14). |
| Growable containers sit behind a non-moving header; references into them are `(header, offset)`, a raw address exists only in a deref window (`sync`, or until the next may-grow call or park) | Relocation cannot invalidate a reference, and no error is needed. Direction (D15); micro-benchmark B14 shows no visible per-access cost, real window frequency unmeasured. |
| Direction (D17): `WeakRef` promotion returns `Ref<T>?`. The proposed split control block that let bodies move between chunks is rejected by D18 | The split cost +4 to +10% work per request and 14 to 17% more memory in a whole service (B15), and recovered memory only after a burst. |
| Direction (D18): every handle (`Ref`, `WeakRef`, `String`, `Array`) owns a count; a field is a scalar, a handle or an inline struct value (`==` on values compares state); a `new` is scope-owned (a bare body, freed at exit, uncounted) unless the compiler sees it converted to a `Ref` on any path, in which case it is counted from allocation; it never moves; a chunk is one type with its management external; arrays have an external control block and a contiguous run in the element type's chunk, and only growing moves it. Locals release their counts at scope exit; no promotion; no language-level hoisting; `deref()` is unsafe, in an `unsafe` block only. Data passed to C is a `malloc`'d copy of a C-compatible type | Keeps a `Ref` a direct address and flat arrays flat, with one rule for ownership. Open: whether a lock on an owner covers the objects it holds, counted or owned arrays, closed-world dispatch summaries. |
| C stays the backend | Under the module architecture too. |
| Gort source and C live in separate folders | `gort/` is Gort only; `c/` is runtime and compiler-output stand-ins. |
