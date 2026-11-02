# Open decisions

Each item gives the alternatives, the trade-off and a recommended direction. A recommendation is not a decision: it is what to build first and test.

## D1. Where continuation lowering happens

- **At module compile.** Every function is lowered to the Level-0 convention; the module's C and object are final. *Pros:* fast incremental builds, no IR needed to link, predictable. *Cons:* no cross-module frame fusion.
- **At application link.** Modules ship IR only; lowering runs on the whole program. *Pros:* best optimisation. *Cons:* slow links, requires IR from every dependency, weak caching.
- **Both (recommended).** Lower at module compile. In optimised mode, re-lower only chains whose inlining or fusion changes them, using retained Tier C IR.

Recommendation: both, with module compile authoritative. Release a first compiler that does only module-level lowering.

## D2. Borrowed locals and the lazy shape

The `lazy` strategy copies locals into a frame only on a park, which moves them. A borrow of a moved local dangles. See [suspension](suspension.md#borrows-and-locals-that-cannot-be-moved).

- **Frame-resident borrowed locals; the function uses eager frames** (recommended). Correct; loses the lazy saving for functions with borrowed-across-park locals.
- **Pin the local's storage in the task arena.** The local is in the arena from the start, not the stack. Cheaper than a full frame; one more arena operation.
- **Forbid borrowing across a park.** Simple; too restrictive (`recv(buf&)`).

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
- **Interaction with the lazy shape.** A check is a park point inside a loop, so the loop's live values must be spillable; a loop with borrowed locals falls under [D2](#d2-borrowed-locals-and-the-lazy-shape) too.

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
- **Only growable containers get a handle.** Fixed-size structs and objects never relocate, so they are referenced directly. Indirection for every object would cost locality and an allocation per object for no safety gain.
- **Effect on D2.** No raw array address is frame-resident across a park (a locked window cannot span one, and an unlocked window ends at the park), so borrows into growable arrays do not force an eager frame. Borrows of other locals are still [D2](#d2-borrowed-locals-and-the-lazy-shape).
- **Contract.** The mutation summary in the `.smod` (modules.md, "Access and escape summaries") gains a `may-grow` bit per container parameter, so a caller can end its deref window exactly. Widening it is a breaking change.

Costs and open points:

- **One dependent load** of `ptr` per access outside a hoisted window. *Measured, micro-benchmark only* ([`handle.md`](https://github.com/johnjoeallen/strata/blob/main/bench/results/handle.md), B14): reloading on every access is not visible at `-O2` and equals a raw pointer when hoisted. At `-O3` a window that ends every iteration costs vectorisation (about 0.14 to 0.23 ns/element in L1), and a may-grow call in the loop costs the call (0.7 to 0.9 ns), not the reload. A second run with realistic bodies and a lock around every variant ([`handle_real.md`](https://github.com/johnjoeallen/strata/blob/main/bench/results/handle_real.md)) gives a typical loss of 1 to 3% (0.01 to 0.3 ns per element) over a raw pointer for bodies that do work, individual configurations up to about 10%, and up to +28% only for an almost-empty body with no lock (0.25 ns); the noise floor is up to 19% in a few cells because gcc compiled two identical loops differently. One machine, single thread, uncontended locks, synthetic windows, A third run, a mixed request on the C runtime with a lock-protected cache, a simulated database wait and growable-buffer rendering ([`mixed.md`](https://github.com/johnjoeallen/strata/blob/main/bench/results/mixed.md), B12), found the handle build within noise of a raw pointer in work per request, throughput and latency (0.5 to 3% run-to-run spread; worst case +2.1% for the pessimistic reload-at-every-access build). So what remains open is how often real windows end, which needs a compiler.
- **FFI.** C needs a stable address. An `extern` call takes a *pinned* borrow: the container is excluded from growing for the call. A library that retains the pointer needs a non-moving allocation (a fixed-capacity array or a dedicated virtual region), declared in the binding contract (spec 5.2).
- **Large arrays.** A dedicated virtual-address reservation grows in place, so its `ptr` never changes and its handle test is free. Whether to use one is the older question in [open questions](../open-questions.md); it becomes an optimisation under this rule, not a correctness requirement.
- **Precision of the may-grow summary.** A coarse summary ends windows too often. Start per container parameter; refine only if B-series results show it matters.
- **Stable identity beyond arrays.** For objects that other objects point to (graph nodes), use `Ref<T>`: it never moves.

## D16. Composition and interfaces instead of inheritance

*Direction, proposed by the author; no prototype.* A struct is built from other structs and interfaces. There is no class inheritance.

```
struct Dog: Animal with Body {
    fn name(): string { return "dog" }
}
```

- **`: Animal`** lists interfaces (the colon reads as in `x: T` and a generic bound `T: Animal`: Dog satisfies Animal). The struct must provide every function the interface requires. An interface may give default functions (behaviour, no data), so it can share code without sharing fields. A struct converts to any interface it implements. This is the only subtyping.
- **`with Body`** lists structs. Each is stored inline in the new struct, as a field, and its fields and functions are promoted, so `dog.feed(1.0)` forwards to the `Body`. Promotion is shorthand for writing the forwarding functions: no new type relation and no layout prefix rule, so no slicing. A plain field `body: Body` is not embedded and promotes nothing. Both parts are optional (`struct Dog: Animal`, `struct Dog with Body`, `struct Dog: Animal, Named with Body, Tail`). No keyword is reserved for it (`is` is left free for a possible type test).
- **No open recursion.** A function of `Body` that calls `feed` calls `Body.feed`, never a `Dog` override. Dynamic dispatch exists only at an interface call. A `Dog` function with the same name as a promoted one hides it for callers of `Dog` and does not change what `Body` does internally.
- **Why not class inheritance.** Open recursion turns every self-call into a dynamic call, and calls through interfaces compile against a declared effect bound (default may-park). That would degrade the may-park inference, the `[nopark]` and lock-body checks, and the access summaries the sharing rules use, program-wide. It is also where fragile-base-class bugs come from.
- **Fallback if examples need it:** a `sealed` single-inheritance base with `override`, where the compiler sees the whole hierarchy and can keep the effect precise.

Open points: the name of the inline field (`dog.body`?); clashes when two incorporated structs promote the same name (compile error until the author disambiguates); whether `with` may name an interface-typed field; `cleanup()` order (fields in reverse order of declaration, incorporated structs in the same order); how interface access modes (sharing, `sync`/`mutate`) are stated; and whether `: Animal` can be declared outside the struct (as Go and Rust allow) or only in its header.
