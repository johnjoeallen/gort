# Open decisions

Each item gives the alternatives, the trade-off and a recommended direction. A recommendation is not a decision: it is what to build first and test.

## D1. Where continuation lowering happens

- **At module compile.** Every function is lowered to the Level-0 convention; the module's C and object are final. *Pros:* fast incremental builds, no IR needed to link, predictable. *Cons:* no cross-module frame fusion.
- **At application link.** Modules ship IR only; lowering runs on the whole program. *Pros:* best optimisation. *Cons:* slow links, requires IR from every dependency, weak caching.
- **Both (recommended).** Lower at module compile. In optimised mode, re-lower only chains whose inlining or fusion changes them, using retained Tier C IR.

Recommendation: both, with module compile authoritative. Release a first compiler that does only module-level lowering.

## D2. Locals whose address a callee holds, and the lazy shape

The `lazy` strategy copies locals into a frame only on a park, which moves them. A pointer to a moved local dangles. With no borrow type (D18) the case is a raw pointer to a stack local; a `Ref<T>` is not affected, because it owns a count on a slot that never moves. See [suspension](suspension.md#locals-whose-address-a-callee-holds).

- **Frame-resident locals; the function uses eager frames** (recommended). Correct; loses the lazy saving for functions with address-held-across-park locals.
- **Pin the local's storage in the task arena.** The local is in the arena from the start, not the stack. Cheaper than a full frame; one more arena operation.
- **Forbid passing a local's address across a park.** Simple; too restrictive (`conn.recv(buf)`).

Recommendation: the first, then measure how many hot functions it affects. The toy benchmarks did not include this.

## D3. The minimum IR for cross-module optimisation

Alternatives: (a) full typed IR for all functions; (b) IR for functions under a size threshold and generic ones only; (c) author-marked inlinable functions only.

Recommendation: ship (b) by default as Tier B plus a size-limited Tier C, with the full IR as an option for libraries that want fusion. Measure size against build gain before deciding the default. Minimum requirements on any IR: explicit ownership operations, call-site effects, park points, cleanup scopes, source-independent.

## D4. Representing effects in public interfaces

Alternatives: (a) infer everything and export it (current plan); (b) require the author to declare the effect of public functions; (c) infer, and let the author optionally assert it.

(b) reintroduces colouring. (a) alone lets an implementation change ripple silently through dependants. Recommendation: (c) - an optional `[nopark]` assertion (see [suspension 1.1](suspension.md#11-attributes-yield-noyield-nopark)), plus tooling that prints the contract diff between versions, and a version policy that treats widening as breaking. Open: whether the attribute form should also replace the existing `parks` modifier (`extern fn[parks]`) and `extern blocking` (`extern fn[blocking]`) for consistency, and whether to allow `parks` on non-extern functions as a "may park" promise for stability.

## D5. Recursion and dynamic dispatch

Recursion: SCC fixed point; per-activation frames; depth-counter fallback to frames on the fast path (see suspension). Dispatch: declared bound, default `May`. Open: the depth limit, and recursion through interfaces where the bound is the only information. Recommendation as written on those pages; test with a recursive-descent parser handler and a visitor over a tree.

## D6. Generics: monomorphisation or shared code

Monomorphised: exact frames and effects, requires Tier B IR, code growth. Shared with dictionaries: no IR dependence, slower, hard to give generic frames a layout. Recommendation: monomorphise; revisit if code size hurts.

## D7. Can frame fusion work across separately compiled modules

It can only when the callee's IR is present (Tier C) at link time. It cannot at module compile. If the callee's module ships A+B only, fusion simply does not happen across it. Alternative: export a *fixed* frame layout for chosen functions so callers can embed them without IR. This freezes the layout (breaking ABI to change). Recommendation: fusion at link time from IR only; no fixed layouts. Measure the benefit before building it: the toy numbers suggest about a nanosecond per call.

## D8. Debugging, stack traces and profiling

A parked task's native stack is gone; the logical stack is the frame chain. Fusion, lazy frames and inlining make logical frames not correspond to physical ones.

- **Park-point tables in every module** (recommended): `(frame type, state) -> chain of logical frames with source locations`, as DWARF-style inline info. Tooling walks the frame chain for parked tasks, and the native stack plus the chain for running ones.
- Profilers sample the native stack and append the chain. Needs a runtime hook.
- Alternative: no maps; stack traces show only physical frames. Cheap, poor for diagnosing the "may park" chain which principle 10 demands tooling show.

## D9. Dynamic module loading and whole-program optimisation

Aggressive devirtualisation and fusion assume a closed world; plugins break it. Alternatives: no dynamic loading; open interfaces declared in build configuration; a runtime flag that turns off closed-world assumptions entirely. Recommendation: no dynamic loading in the first compiler; keep bound-based dispatch so it can be added with open interfaces declared in configuration.

## D10. Blocking FFI

Offload pool, dedicated thread, or run on worker. See the table in [dispatch](dispatch-and-ffi.md#blocking-foreign-calls). Recommendation: nonblocking where possible; `extern blocking` offload by default; watchdog diagnostics.

## D11. Versioning module compatibility and native caches

Format, ABI, contract hashes, cache keys as in [modules](modules.md#4-versioning-and-compatibility). Open: whether the compatibility policy is enforced by the compiler (refuse a minor-version consumer of a major-breaking producer) or only reported; how long caches live; reproducible builds and cache portability across machines.

## D12. Cancellation as a non-exception

Recommended: not in `throws`, not catchable, runs cleanups. Alternative: a catchable exception that must be rethrown. Interacts with the exception model (spec 10.7), and with child-vthread scopes.

## D13. Runaway non-parking code

**Decided by the author:** the compiler inserts slice checks only in functions marked `[yield]`, or in a type (`struct Parser[yield]`) or, once packages exist, a package whose default is `[yield]`. The innermost level wins: function, then type, then package. `[noyield]` overrides an enclosing default; `[nopark]` asserts that nothing parks. With no attribute nothing is inserted. Mechanism and rules: [suspension 1.1](suspension.md#11-attributes-yield-noyield-nopark). Loops add no effect by default, so the library-tagging burden an earlier default-on design carried is gone.

The price is that unmarked CPU-bound code can stall a worker and cannot be cancelled. Mitigations, all Direction: loop warnings with an advisory `loops` class in the module summary ([suspension 1.3](suspension.md#13-loop-warnings)), a watchdog diagnostic that reports a worker making no progress past a threshold and names the function it is in (it reports and does not interrupt), and the type-level (later package-level) default for CPU-heavy code. Open: the package notion itself, and whether the attribute spelling should be unified between functions and types.

Still open:

- **Overhead of a check** in a `[yield]` loop. Unmeasured; strip-mining and small-bound elision are expected to keep it low. Benchmark B13.
- **Slice unit.** An iteration counter (cheap, not time) against a timestamp or a flag set from another thread. Recommendation: counter, sized in iterations per check block; the watchdog flag later.
- **Lambdas** inheriting the enclosing function's setting.
- **Default revisited.** Whether checks should become the default later, once B13 and a real compiler show the cost and the ripple. The earlier design made them the default and was reversed before any implementation.
- **Interaction with the lazy shape.** A check is a park point inside a loop, so the loop's live values must be spillable; a loop with locals whose address a callee holds falls under [D2](#d2-locals-whose-address-a-callee-holds-and-the-lazy-shape) too.

## D14. Packages and imports

*Direction, following the Java folder convention; the details are open.* Every sample `.stra` file starts with `package name`.

- **Rule used in the samples.** A package is a folder. Its name is the folder path under the source root (`stra/`) with dots for slashes, so `stra/redis/` is `redis` and `stra/http/styles/cpp/` is `http.styles.cpp`. Every file in the folder starts with the same `package` line, and the compiler rejects a file whose package line does not match its folder. A package compiles to one `.smod` named after it. Declarations are visible across the files of a package with no import. `import other` names another package, so `import redis.types` inside `redis` is gone and `import runtime.leaves` became `import runtime`.
- **Package default.** `[yield]`, `[noyield]` or `[nopark]` is written on the package line, `package[yield] imaging`, and repeated on that line in every file of the folder. All files must state the identical line; a file that differs or omits it is an error. There is no special file. Alternatives: the attribute in one file only (hidden action at a distance, and one file becomes special), a `package-info`-style file (rejected), or in a build manifest (the setting is no longer visible in source). Recommendation: repeat it, since the repetition is a few tokens and keeps the setting visible wherever the code is read. Open: the compiler could instead infer the line from the folder and only require the attribute to be repeated.
- **Style variants.** `http/styles/{cpp,java,mixed}/server.stra` are now separate packages (`http.styles.cpp`, ...), each of one file, which `import http` and `import runtime`. They are alternatives and not compiled together.
- **Import granularity.** Whole package (used here), or named symbols (`import runtime.Conn`). Whole package is simpler; symbol imports give the contract hashes in `.smod` a natural unit, and they are already per symbol there. Recommendation: whole package at the source level.
- **Visibility.** What a package exports to others, and how encapsulated `sync` code (spec 7.2) is protected from being bypassed. Not specified yet.
- **Nesting.** Dots mean folder nesting. Whether a nested package can see its parent's declarations (Java: it cannot) is undecided; recommendation: it cannot.
- **Versions.** The `.smod` carries the package version ([modules](modules.md)); how a source tree states which version of an imported package it expects.

## D15. Growable arrays, relocation and stable references

*Direction, proposed by the author and not yet prototyped beyond the HTTP buffers.* A growable array can move its backing store when it grows. The rule that keeps every reference safe is that **no reference into a growable container is a raw address**.

- **Handle.** A growable container (`T[]`, string builder, map storage) is a small header that never moves: `{ptr, len, cap}` plus, when the container is shared, its lock word. Only the backing store behind `ptr` moves; growing updates `ptr` in place. This is the shape `Ref<T>` already has (an individually allocated object that stays put).
- **Interior references are `(handle, offset)`.** `&v[3]`, a slice, an iterator and a stored cursor name the container and a position. They cannot dangle, so `let r = &v[0]; v.push(x); use(r)` is legal and correct. This is the offsets rule of the HTTP prototype (a `Request` is a pair of offsets), applied by the compiler instead of the programmer.
- **Deref window.** The raw address exists only inside a window in which the compiler has loaded `ptr` and nothing can move the store.
    - *Shared container:* the window is the `sync` or `mutate` block (spec 7.3, R1 and R4). Growing needs `mutate`; a reader inside `sync` therefore excludes a grow.
    - *Unshared container:* no lock (R6). The window ends at any call whose mutation summary says it may grow the container, and at every park point. After that the compiler reloads `ptr`. Calls through interfaces, function values or foreign code are treated as may-grow.
- **Only growable containers get a handle (superseded in part by [D17](#d17-moving-objects-between-chunks-split-control-blocks-and-weak-promotion)).** Plain structs and objects were to be referenced directly, since indirection would cost locality and an allocation per object for no safety gain. D17 gives every `new T` object a never-moving control block anyway (it carries the counts), and proposes that the body behind it may move for memory reclamation, so the cost argument changes to one extra load.
- **Effect on D2.** No raw array address is frame-resident across a park (a locked window cannot span one, and an unlocked window ends at the park), so pointers into growable arrays do not force an eager frame. Borrows of other locals are still [D2](#d2-locals-whose-address-a-callee-holds-and-the-lazy-shape).
- **Contract.** The mutation summary in the `.smod` (modules.md, "Access and escape summaries") gains a `may-grow` bit per container parameter, so a caller can end its deref window exactly. Widening it is a breaking change.

Costs and open points:

- **One dependent load** of `ptr` per access outside a hoisted window. *Measured, micro-benchmark only* ([`handle.md`](https://github.com/johnjoeallen/strata/blob/main/bench/results/handle.md), B14): reloading on every access is not visible at `-O2` and equals a raw pointer when hoisted. At `-O3` a window that ends every iteration costs vectorisation (about 0.14 to 0.23 ns/element in L1), and a may-grow call in the loop costs the call (0.7 to 0.9 ns), not the reload. A second run with realistic bodies and a lock around every variant ([`handle_real.md`](https://github.com/johnjoeallen/strata/blob/main/bench/results/handle_real.md)) gives a typical loss of 1 to 3% (0.01 to 0.3 ns per element) over a raw pointer for bodies that do work, individual configurations up to about 10%, and up to +28% only for an almost-empty body with no lock (0.25 ns); the noise floor is up to 19% in a few cells because gcc compiled two identical loops differently. One machine, single thread, uncontended locks, synthetic windows, A third run, a mixed request on the C runtime with a lock-protected cache, a simulated database wait and growable-buffer rendering ([`mixed.md`](https://github.com/johnjoeallen/strata/blob/main/bench/results/mixed.md), B12), found the handle build within noise of a raw pointer in work per request, throughput and latency (0.5 to 3% run-to-run spread; worst case +2.1% for the pessimistic reload-at-every-access build). So what remains open is how often real windows end, which needs a compiler.
- **FFI.** C needs a stable address. An `extern` call takes a *pinned* pointer: the container is excluded from growing for the call. A library that retains the pointer needs a non-moving allocation (a fixed-capacity array or a dedicated virtual region), declared in the binding contract (spec 5.2).
- **Large arrays.** A dedicated virtual-address reservation grows in place, so its `ptr` never changes and its handle test is free. Whether to use one is the older question in [open questions](../open-questions.md); it becomes an optimisation under this rule, not a correctness requirement.
- **Precision of the may-grow summary.** A coarse summary ends windows too often. Start per container parameter; refine only if B-series results show it matters.
- **Stable identity beyond arrays.** For objects that other objects point to (graph nodes), use `Ref<T>`: it never moves.

## D16. Composition and interfaces instead of inheritance

*Direction, proposed by the author; no prototype.* A struct is built from other structs and interfaces. There is no class inheritance.

```
struct Dog: Animal with Body {
    fn name(): String { return "dog" }
}
```

- **`: Animal`** lists interfaces (the colon reads as in `x: T` and a generic bound `T: Animal`: Dog satisfies Animal). The struct must provide every function the interface requires. An interface may give default functions (behaviour, no data), so it can share code without sharing fields. A struct converts to any interface it implements. This is the only subtyping.
- **`with Body`** lists structs. Each is stored inline in the new struct, as a field, and its fields and functions are promoted, so `dog.feed(1.0)` forwards to the `Body`. Promotion is shorthand for writing the forwarding functions: no new type relation and no layout prefix rule, so no slicing. A plain field `body: Body` is not embedded and promotes nothing. Both parts are optional (`struct Dog: Animal`, `struct Dog with Body`, `struct Dog: Animal, Named with Body, Tail`). No keyword is reserved for it (`is` is left free for a possible type test).
- **No open recursion.** A function of `Body` that calls `feed` calls `Body.feed`, never a `Dog` override. Dynamic dispatch exists only at an interface call. A `Dog` function with the same name as a promoted one hides it for callers of `Dog` and does not change what `Body` does internally.
- **Why not class inheritance.** Open recursion turns every self-call into a dynamic call, and calls through interfaces compile against a declared effect bound (default may-park). That would degrade the may-park inference, the `[nopark]` and lock-body checks, and the access summaries the sharing rules use, program-wide. It is also where fragile-base-class bugs come from.
- **Fallback if examples need it:** a `sealed` single-inheritance base with `override`, where the compiler sees the whole hierarchy and can keep the effect precise.

Open points: the name of the inline field (`dog.body`?); clashes when two incorporated structs promote the same name (compile error until the author disambiguates); whether `with` may name an interface-typed field; `cleanup()` order (fields in reverse order of declaration, incorporated structs in the same order); how interface access modes (sharing, `sync`/`mutate`) are stated; and whether `: Animal` can be declared outside the struct (as Go and Rust allow) or only in its header.

## D17. Moving objects between chunks: split control blocks, and weak promotion

*Status: the split control block and the moving of bodies between chunks are **rejected for counted objects** by [D18](#d18-storage-classes-locals-scopes-and-dispatch); the measurements below are the evidence. `weak.promote()` returning `Ref<T>?` stays.* *Direction, proposed by the author; unmeasured.* The finding that motivates it: long-lived objects scattered through shared chunks pin memory (Measured, [`group2.md`](https://github.com/johnjoeallen/strata/blob/main/bench/results/group2.md): 6 to 11x retained after a burst). Per-type arenas separate types, not lifetimes, so a type that mixes short and long lifetimes still pins chunks. Chunked arenas already let a type arena grow without moving anything; this decision is about shrinking.

- **Split.** The control block (strong and weak counts, metadata, a pointer to the body as `{chunk, offset}`) lives in one fixed-size control-block arena shared by all types, and never moves. `Ref<T>` and `WeakRef<T>` point at it. The body (the fields) stays in the type's chunked arena and may be moved to another chunk by copying it and updating the one pointer in the control block. No `Ref` or `WeakRef` changes, and counting never touches the body.
- **Lifetimes.** When the strong count reaches zero the body is destroyed (`cleanup()` runs) and its slot freed. The control block stays until the weak count is also zero. Today's layout (spec 3.3) keeps the whole slot until then, so a dead object with a lingering `WeakRef` pins its full size; after the split it pins only the control block (about 32 bytes). A `WeakRef` that is never dropped, for example in a long-lived cache, pins that block until its holder drops the entry; holders should drop an entry when promotion fails.
- **Weak promotion returns `Ref<T>?`** (null if the object is gone). Expiry is an expected result the caller must handle, as in Java's `WeakReference.get()`, Rust's `Weak::upgrade` and C++'s `weak_ptr::lock`. Both ways of keeping the data fail identically, because the object is already destroyed at strong zero, so holding the bytes would not avoid the failure. There is no implicit promotion and no `ExpiredReference` (spec 3.1).
- **When a move is safe.** An unshared object: between turns of its owning worker (scheduling is cooperative, so no raw address is live there). A shared mutable object: under `mutate`, which excludes the `sync` readers. A shared immutable object: after a grace period, once every worker has passed a park or yield. An object passed to FFI is pinned (as in D15). A pointer held in a frame across a park (D2) holds a raw body address: either it blocks the move or it is stored as `(control block, offset)`.
- **A visible service, not magic (principle 10).** Compaction is a runtime service with a stated policy (evacuate chunks below an occupancy threshold), a bounded step per pass, and statistics. Whether it is on is a per-type choice.

Costs and open points:

- **One extra load** (`control block` then `body`) per dereference outside a hoisted window; B14 measured about 0 to 3% for such a load in realistic bodies, but not on pointer-heavy object graphs.
- **`new` becomes two allocations** (control block and body), which could roughly double its 5 to 10 ns. Types that do not opt in could keep the combined slot of 3.3; two layouts cost compiler complexity.
- **Allocation epoch in the control block (Direction).** The slot header has a 32-bit allocation epoch, packed into `meta` beside the worker index so the header does not grow and the layout is the same in every build. It is filled in only when age tracking is on (`--track-age`, implied by `--debug`), from a global epoch counter that a ticker advances (no clock read in `new`); otherwise it is 0, meaning unknown. Because release builds leave it empty, nothing in the runtime may depend on it: it is for age histograms and finding leaks. With D18 nothing moves, so there is no mover to use it.
- **Tiny objects** (about 16 bytes or less) gain little: the control block is a large fraction of the body. They could live inline in the control block.
- **Generations may become unnecessary** for object arenas: a `Ref` points at the control block, so validity comes from the counts, and the chunk-epoch fix for decommit (`group2.md`, section 7) would apply only to the control-block arena. Check before relying on it.
- **Measured, micro-benchmark only** ([`split.md`](https://github.com/johnjoeallen/strata/blob/main/bench/results/split.md), B15; single thread, hand-written C):
    - *Time (loss).* `new` is 9 to 45% slower, a free 58 to 135% slower, a free-plus-allocate pair about 2x. A dereference through the control block is 13 to 97% slower for independent reads and **75 to 174% slower on a pointer chase** (two cache lines in two arenas). This is much more than the D15 header (0 to 3%), which sits next to its array.
    - *Memory (gain).* Dead objects held by a `WeakRef`: 157 MB falls to 50 MB (64 B bodies), 263 MB to 25 MB (256 B). After a burst leaving 1% scattered survivors: 44 MB with moving, 70 MB with page release only, 157 MB with nothing. At 10% survivors only moving helps (64 MB against 157 MB).
    - *Memory (cost).* A live object is 96 B against 80 B for a 64 B body (+22% on a full heap), and the control blocks of scattered survivors stay pinned (about 24 B each), which is the floor of the 44 MB.
    - *When it pays.* Not when objects die roughly in allocation order (nothing is retained and the split is pure cost); for types with mixed lifetimes and for long-lived `WeakRef`s.
- **Measured, a whole service** ([`service_split.md`](https://github.com/johnjoeallen/strata/blob/main/bench/results/service_split.md), the mixed-request service with a cache of `Ref<User>` objects):
    - *Time (loss).* Work per request +4 to +12% with one worker and +6 to +27% with four; throughput -2 to -10% (one worker) and -16 to -21% (four) where requests are served from the cache; no difference where the service waits on its database. With four workers the longer critical section (a second dependent miss inside `sync`) raises failed lock attempts per request from 3.9 to 7.0.
    - *Memory.* Steady state is 14 to 17% larger. After 90% of the cache is evicted at random and traffic narrows, only moving returns memory: 44 MB resident at the end against 54 MB for the combined layout and 63 MB for the split without the mover.
    - *Moving while the service runs.* Readers exist only inside `sync`, so a move takes `mutate` on the cache lock for one chunk at a time (longest hold 48 us); no grace period or epoch tracking is needed. 54,000 bodies cost 48 ms of mover CPU and about 7% of the workers' throughput while it ran.
- **Measured, sharded locks and clone-on-hit** ([`b12_ref.md`](https://github.com/johnjoeallen/strata/blob/main/bench/results/b12_ref.md), the same service with 16 cache locks and a hit that clones the `Ref` and reads the body after leaving the lock):
    - *Time (loss).* Most of the four-worker loss above came from the single lock. With 16 locks the split costs +4 to +10% work per request over the combined layout (one worker: +7 to +10%) and 0 to -8% throughput; failed lock attempts per request fall from 6.3 to 0.15. The remaining cost is the second dependent cache miss per hit (about 35 ns).
    - *Clone-on-hit* is not measurably slower for the combined layout and 0 to 1.5% cheaper for the split (a shorter critical section). With half the requests on 1,000 hot users the atomic count updates cost about 5 ns per request.
    - *Moving with clones.* A move takes every shard lock and skips a chunk if any of its objects has `strong > 1` or is held by a request alone. The flush scenario still ends at 49 MB resident against 63 MB (split without the mover) and 54 MB (combined); 1 of 330 chunks was skipped, because a request holds its clone only across a non-parking render. A Ref held across a park (D2) would pin chunks far more often: not measured.
- **Measured, a Ref held across a park and Refs held for good** ([`hold.md`](https://github.com/johnjoeallen/strata/blob/main/bench/results/hold.md)):
    - *Held across a park* (100 us to 10 ms after every hit): no cost in steady state, and the mover recovers the same memory (35 MB resident at the end against 63 MB without it) while wasting up to about 78% of its attempts on pinned chunks.
    - *Held for good* (a Ref stored somewhere long-lived): with 0.1% of hits keeping their Ref the mover recovers 9 MB instead of 14 MB (54 MB at the end, against 49 MB without stashing and 63 MB with no mover), and at 1% nothing, because a scattered pin blocks its whole chunk. The mover is all-or-nothing per chunk; moving only the unpinned bodies was not tried.
    - *For D2.* A Ref held across a park needs no special rule for moving; a stored Ref is the case that defeats it.
- **Consequence for the direction.** Page-level release of free pages costs nothing at dereference and gets most of the gain at 1% survivors, so it comes first. The split is opt-in per type (the fixed combined slot stays the default), aimed at mixed-lifetime types and `WeakRef` targets. Not measured: moving only the unpinned bodies of a pinned chunk, concentrated (rather than random) long-lived Refs, mutable shared objects, a single very hot object, and a service with real sockets.


## D18. Storage classes, locals, scopes and dispatch

*Direction, decided by the author in a design review of the memory-model concept (`Strata Memory Model: Concept`). Items marked **Open** are not decided. Costs are from the B15 and B12 runs unless stated; nothing here is implemented.*

**1. Two storage models.** Plain objects and arrays need different arena and chunk models.

- **Plain objects (`new T`, `Ref<T>`).** A slot in the type's arena with the control block (counts, metadata) inline, as spec 3.3 shows. The arena never moves and **no object moves between chunks**, so a `Ref` is a direct address and dereferencing costs no extra load. This rejects D17's split control block and its mover. What D17 measured is the reason: the split cost +4 to +10% work per request with sharded locks (+6 to +27% with one lock), made a steady-state service 14 to 17% larger, and a pointer chase through two cache lines 75 to 174% slower; it recovered memory only after a burst. Returning memory to the OS is left to page-level release of free pages. The cost accepted: a dead object that a `WeakRef` still names keeps its whole slot until the weak count reaches zero (INV-8).
- **Arrays (`Array<T>`, fixed length; `Array<T[]>`).** A control block outside the object arenas, in a slab of control blocks that never moves, holding the counts, the absolute pointer to the elements, and the length (and capacity for the growable form). The elements are a contiguous block in a chunk of the element type's array arena; a new chunk is allocated if no block fits. Elements of native values and of objects are stored inline, with no per-element control block. An array of `Ref<T>` holds counted handles and releases them when the array is dropped or an element is removed. A fixed `Array<T>` never moves its block. A `Array<T[]>` is allocated at a block size and, when it outgrows it, is reallocated to a larger block (another chunk if needed), copied, and its control block's pointer updated; no `Ref` to the array changes. This is the only movement of payload between chunks, and the program caused it.
- **Arrays larger than a chunk (decided).** An array whose run does not fit in a chunk gets a **dedicated oversize chunk**. A bounded or fixed-length array gets exactly its run (`cap` elements), with no extra space, since it never grows. An unbounded (`Array<T[]>`) array gets its run **plus extra space**, headroom so it can grow in place before it is reallocated; the headroom size is undecided (part of the block-size policy). An oversize chunk holds one array and is returned whole when the array is dropped. Arrays are never split across chunks.
- *Considered and not taken:* a control block before every element (16 bytes per element: 5x for `i32`, 17x for `u8`, no SIMD, against the flat-array goal), and a separate header-plus-general-allocator store (not rejected; the chunk model keeps one type per chunk).
- **Open:** whether an array handle is counted and shared by reference or an owned value; whether `Array<T>` and `Array<T[]>` convert without copying (a growable array "frozen" into a fixed one); the block-size policy (size classes against first-fit runs); whether array blocks are ever compacted.

**2. Locals.** Simple locals live on the data stack and are reclaimed at scope exit (spec 4.1); they are not counted objects. A local that holds `Ref<T>`, `Array<T>`, a `WeakRef` or anything else that owns counts **releases them when it is cleaned up**: every exit path (return, error, exception, cancellation) runs the type's drop in reverse order of declaration. A `new` object is placed on a stack or in task storage only if the compiler proves it cannot outlive that storage (spec 4.4); where it is unsure, it allocates in the shared arena. The concept's frame-exit promotion (copy a payload that something else still holds into a shared chunk) is **rejected**: it needs a payload move, so a wrong inference cannot be repaired afterwards and the analysis has to be sound.

**3. Escape direction (new rules).**

- Passing a local down to a callee, including as an out parameter, does not make it escape: the callee is shorter-lived. If the argument is a `Ref<T>`, the pass still takes and releases a count (item 7).
- Storing a value into storage the caller supplied does escape: that storage is longer-lived.
- A callee that stores a new object into a container it was passed allocates the object shared, without knowing whether the container is local. Shared is always safe.
- A value that flows to a return, or into a parameter's contents, is allocated shared up front.
- A shared or longer-lived object never holds a reference to a local (the concept's INV-7): a store of a local into one is rejected, or the local is published first (sharing.md R5).

**4. Counts.** Counts on an unshared object may be non-atomic. They become atomic when the object is shared (the shared flag of sharing.md R5, set before the handoff). Spec Phase 1 starts with atomic counts; this is the later optimisation. The saving is unmeasured.

**5. Scopes and hoisting.**

- **Shared objects:** a dereference is allowed only inside a `sync` or `mutate` block, which cannot contain a parking call (sharing.md R1, R4). No exceptions.
- **Unshared objects:** a read or mutate scope retains the resolved pointer for every access in the scope (one dereference per scope, not per access).
- A fixed-size object never moves, so its pointer stays valid across a park and across any call. The block of a fixed `Array<T>` also never moves. An `Array<T[]>` pointer is invalid after a call that may grow it (the D15 may-grow summary), and after leaving the scope or reassigning the variable; the compiler re-resolves there. An unshared container cannot change during a park, because no other virtual thread can reach it, so a park does not invalidate the pointer.
- **A hoisted pointer is taken only from a `Ref` the scope itself holds.** The held `Ref` (a parameter, a local, or a count the scope takes) keeps the slot alive, so the pointer cannot dangle and the slot cannot be reused underneath it. Where the code dereferences through a container or a field (`a.b.c`, `list[i]`), the compiler **injects a local `Ref<T>`**: the hoist copies the `Ref` out of the container or field into a compiler-made local, which takes a count (a non-atomic increment when unshared) and is released at scope exit, and the hoisted pointer comes from that local. The programmer writes nothing. A call that removes the object from its container and drops the container's count is therefore safe. The pointer is **invalidated** by: leaving the scope, reassigning the holding variable, and a may-grow call on a `Array<T[]>`. Without a held `Ref` there is no hoisting; the access reloads through the container each time.
- **`deref()` is unsafe (decided).** A raw pointer the compiler hoists is safe, because the compiler holds the `Ref` and ends the pointer's life at the invalidation points above. A raw pointer a program obtains by calling `deref()` itself is **unsafe**: the checker does not track it, so it can dangle after a drop, a reassignment, a may-grow call or a park, and the program is responsible for those. `deref()` may be called only inside an **`unsafe` block** (decided). `deref()` is **not allowed on a shared object outside `sync`/`mutate`**, even inside an `unsafe` block (decided); this matches the rule for compiler-hoisted pointers above. **Open:** whether an `unsafe` function (a whole body as unsafe) exists besides the block.
- **Exception: a pointer into a stack local across a park.** Under the lazy frame shape the park moves the local into a frame, so the pointer dangles. That is [D2](#d2-locals-whose-address-a-callee-holds-and-the-lazy-shape): the function uses an eager frame, or the local is pinned in the task arena.
- **Open:** whether scopes are required on unshared data (an object access outside any scope is an error) or optional (the compiler also hoists unscoped code). The author's lean is optional, with a written scope as an assertion that it can hoist; not confirmed.

**6. Dispatch.** Strata has no virtual dispatch on struct functions, no inheritance and no open recursion (D16); a call is dynamic only at an interface call or a function value. A direct call has an exact escape and effect summary. An interface or function-value call takes the join of its possible targets' summaries, or a conservative summary (arguments escape, bound may-park) when the target set is not known (separate compilation, dynamic loading, D5, D9). **Open:** whether the compiler may join over every implementation it can see in one application link (closed world), or always uses the conservative summary.

**7. References are `Ref<T>` only (no `T&`), and every `Ref<T>` owns a count.** The language has no separate borrow type. A parameter, local or field that refers to a counted object is a `Ref<T>`, and **every** `Ref<T>` manages the count: a copy or a pass increments it, a drop (scope exit, reassignment, container removal) decrements it. There is no count-less reference. Consequences:

- A `Ref<T>` always keeps its slot alive, so it cannot dangle, and a slot never moves, so it needs no frame-residency rule across a park. This removes the open question of whether a struct may hold a `T&`, and the borrow-origin inference of spec 5.
- The cost is a count increment and decrement at each pass or copy (non-atomic when unshared, item 4). An optimiser may later remove a pair it proves redundant, but nothing in the semantics or the cost model relies on that; **unmeasured**.
- **Hoisting through a container or field injects a `Ref<T>`.** When code dereferences through a container item or a field, the compiler copies the `Ref` into a compiler-made local, which owns a count until scope exit (item 5). The programmer writes nothing.
- A `Ref<T>` can only name a counted object in a slot. A stack local is passed down by value, by move, or as a raw pointer the compiler creates and keeps inside the function and its callees' frames (item 5, D2). Whether an object that is stack-placed under item 2 can be named by a `Ref` (its slot then needs a control block that must reach zero at scope exit) is **Open**.
- Text and `stra/` examples that wrote `T&` were migrated mechanically (`T&` to `Ref<T>`, including `u8[]&` and `string&`). `String` is a language type that behaves like an **immutable `Ref<u8[]>`**: it is reference counted, owns a count on its bytes, and the bytes never change. Copying or passing a `String` is a count operation, reading one needs no `sync` or `mutate` (nothing can mutate it), and it can be shared across workers freely. It is written `String`, not `Ref<...>`. A byte array is not a string. There is no bare native array (item 8): the bytes behind a string are an `Array<u8>`. (An earlier mechanical migration wrapped `string` and `u8[]` in `Ref<>` and that was reverted.) A string literal `"..."` is stored in a **permanent block**, and a `String` is created that refers to it. That `String` is counted like any other, but its count reaching zero does **not** free the permanent block. A **substring is a copy** out of the parent: it allocates new bytes and holds no count on the parent, so a small substring never keeps a large parent alive. **Open:** how a `u8[]` is passed without a copy (pointer and length); the permanent-block layout (one static region, how the `String`'s control block marks it as not freeable).

**8. There are no bare native arrays; every array is an `Array`.** An array's elements live in a chunk of the element type's array arena (item 1), so every array has a control block holding its counts, pointer and length. Nothing is a raw `T*` plus a length with no owner. An array is declared in one of two forms that are **not** equivalent:

- `Array<type[size]> x = new Array<type>` creates a **bounded** array: capacity `size`, length 0. You can add up to `size` elements; adding more is an error (it does not grow). The block for `size` elements is allocated up front and never moves.
- `Array<type> x = new type[size]` creates an array of **length** `size` (elements initialised to the type's zero value or default). Its length is fixed.

`Array<> x = new type[size]` infers the element type. The array type is written `Array<u8>` (decided): `u8[]` in earlier examples was migrated to it, as were the other array types in a type position (`Array<Bytes>`, `Array<Row>`, `Array<String>`, `Array<T?>`). Array literals (`[]`) and constructors (`Row[]`, `T?[](n)`) are not migrated. Decided: `size` is **not** a compile-time constant; it is any runtime value. **Bounds checks apply to every array**, in all three forms: an index outside `0..len` is an error, and adding to a bounded array past its capacity is an error. There is **one array control block** for all of them (counts, pointer, `len`, `cap`), so a bounded array, a fixed-length array and a growable one differ only in how `cap` is used, not in type layout. **Decided:** `Array<type[]> x = new Array<type>` is the growable form (no size in the type). It replaces the separate `GrowableArray` type of earlier drafts, so there is one `Array` type with three declarations: `Array<type[size]>` (bounded), `Array<type>` with `new type[size]` (fixed length) and `Array<type[]>` (growable). **Decided:** a bound or index violation is a **runtime exception**. A fixed-length array and a bounded array are **the same type to a callee**; the callee sees one `Array<T>` with `len` and `cap`. An `Array<T[]>` may move: growing it reallocates its elements, and the control block, and so every `Ref` to the array, stays where it is. The storage of an unbounded array is allocated **in blocks, and the block size is undecided** (see the block-size policy in item 1). **Decided:** the elements of every array, growable included, are **one contiguous run**. A growable array that outgrows its run is reallocated to a larger run (a whole number of blocks) and copied, never chained. A raw pointer hoisted over its elements is therefore a base pointer plus index, and is invalid after a call that may grow it (item 5).

- Passing an array is a count operation like any `Ref<T>` (item 7). A slice such as `buf[..n]` needs a decision: it is either a new view with its own control block that holds a count on the parent, or a copy as for a `String` substring. **Open.**
- A stack buffer (`var buf = [u8; 4096]` in the proxy example) is also an `Array`. Its control block and block are in the frame or task storage (item 2) if the compiler proves it cannot outlive them, else in the shared array arena. **Open:** the cost against a raw stack buffer (B17).
- An array of a growable length is `Array<T[]>` (item 1), not a separate case of `Array`.
- `String` (item 7) is built on a byte array it owns and never exposes for writing.

**9. Passing data to C (Direction, the author's preference).** Calling a plain C function has the same problem as Java's JNI: C wants a raw pointer, for example to the bytes of a `String`, and that is an implicit `deref()`, which is unsafe (item 5). Two ways:

- *Implicit `deref()`.* Pass a pointer to the object's own bytes, pinned for the call (D15). Cheap, but C holds a pointer into Strata-owned storage: if C writes through it, it corrupts an immutable `String` or a permanent block, and it may assume a terminator that is not there.
- *Allocate storage and pass that (preferred).* The compiler allocates storage of the size the call needs, copies the argument into it (a NUL terminator added for a `String` passed as a C string), and passes a pointer to the copy. A write by C cannot damage the original, and the copy is freed when the call returns. For an out buffer the compiler allocates the declared size.

The copy does **not** make the call safe. C can still write past the end of the storage it was given and cause a memory error; the `extern` declaration is a promise, as it is for parking (sharing.md R8), and the call is in an unsafe context. **Open:** where the temporary storage lives (the frame or task storage, or an array arena); the cost of the copy per call against pinning (unmeasured); how an `extern` declaration states the size of a buffer it takes, so the compiler can allocate it and a bounds check can be placed on the Strata side; and how a pointer that C retains after the call is handled (it needs a non-moving allocation, D15).

**10. To measure.** Non-atomic against atomic counts on a shared-nothing path; readers dereferencing while another thread assigns `Ref`s (false sharing), with the slot header's counts on a separate cache line from the body as a switchable layout (the concept's hot and cold split; unmeasured); stack against arena placement for short-lived `new` objects; fixed `Array<T>` block allocation against a general allocator; the cost of two allocations per array. See B16 and B17 in [benchmarks](benchmarks.md).
