# The compiler: where it stands

*Status: slice 0 implemented in `gortc/` (Rust). Everything else on this page is plan.*

## Choices made

- **Language:** Rust. The compiler is not self-hosted.
- **Backend:** C, as in [index](index.md). `gortc` writes one C file and calls `cc`.
- **Order:** the memory model first, then parking. Slice 0 has no may-park effect and no frames.

## Slice 0 (done)

A subset front end and a C emitter, tested under ASan and UBSan. Subset and rules: `gortc/README.md`. It exercises these
D18 decisions on generated code:

| Decision | What the compiler does |
|---|---|
| Struct values, copied by field, handle fields take counts (item 1) | generated `copy` and `drop` per struct |
| Counted or scope-owned `new`, decided per site (item 5) | `--notes` reports the choice and the line that forced it |
| No copy of a value with `cleanup()` | rejected at compile time |
| Arrays: scalar elements, control block apart from the run (item 3) | `Array<scalar>`, fixed length, bounds-checked |
| Strings: immutable, counted, literals permanent (item 4) | `gort_str` |
| Control block placement (item 11, B18) | hidden behind five macros and functions in `gort_rt0.h` |

Conventions slice 0 settled where the notes were silent:

- An untyped integer literal is `i64` (or takes the type of the operand beside it), a float literal `f64`.
- A call copies each argument; the temporary that held it lives to the end of the statement. Moving out of a temporary
  happens only when the value is stored (`let`, assignment, field, `return`). Moving the last use of a local is an
  optimisation not yet made, so cleanup timing does not depend on it.
- `let x = new T {..}` gives `x` the type "object of T" until a use decides: a conversion to `Ref<T>` makes it counted; a
  field access, a method call or a copy out as a value does not.
- A `var`-bound `new` cannot be reassigned. Write `Ref<T> x = new T {..}` when the variable must change.

## Slice 1, step 1: enums and `switch` (done)

An enum is lowered to a struct whose field 0 is an `i32` tag, followed by every variant's payload fields. Copy, drop,
methods, `static fn` and out-of-line `fn T::name` then work with no change in codegen. `Enum.Variant(args)` builds the
value; `switch` becomes a copy of the subject and an if-chain on the tag (or on the integer). Arms take one or more
variant names, `default`, or integer and character literals. Bindings (`Rect(w, h)`) are immutable copies of the payload.
A `switch` over an enum must cover every variant or have a `default`; over an integer it needs a `default`.

Not yet: handle-typed payloads (the other variants' fields would have to be zero-initialised handles), in-place bindings on
a mutable subject (the note in `gort/http/body.gort`), a union layout that overlaps the payloads, and `switch` as an expression.

## Target: `gort/http/` (first)

`http` is the first sample the compiler must build whole. Needs, from reading the ten files (done = in `gortc/` now):

| Need | Where | State |
|---|---|---|
| enums, `switch`, positional and struct payloads | types, body, route | done (scalar and scalar-struct payloads) |
| `exception`, `throws`, `throw`, `try`/`catch` | types, reader, body, route | done (status returns; `Exception` catch-all; no rethrow of a caught value yet) |
| optional types `T?`, `null` | batching, pipeline, reader | done, with checked unwrap in place of flow narrowing |
| `import runtime` across folders, `extern fn`, `extern struct`, `extern impl`, `extern let` | all | parsed and checked; `extern impl Array<u8>`, varargs and generic externs to do |
| unit literals (`16.kb`, `2.s`, `50us`), `Duration`, `Instant` | config, batching | done (built-in time types, signed nanoseconds) |
| top-level `let` | store, server | done (initialised before `main` in declaration order, dropped at exit; immutable) |
| generics: `Ring<T>`, `Task<T>`, `Mailbox<T>` (explicit arguments) | pipeline, runtime | done (structs and enums, by monomorphisation); inference from arguments, fields or the expected type done; `Map<K,V>` is not defined anywhere yet |
| arrays of optionals and plain values, `T?[](n)`, `u8[N]` | ring, runtime | done |
| varargs `T...` | leaves | done (the extra arguments become an array) |
| `T?&` (borrowed), `Range` | leaves | `&` parsed as the plain type. `Range { lo, hi }` is a prelude struct; `a..b` and `Range(a, b)` build one. `Array` is a language element: `length` is the elements in use and `capacity()` the elements allocated; indexing and slicing are checked against the capacity |
| `Array<T>()`, `grow()`, `moveWithin`, `[a, b]` | reader, server | done: `Array<T>()` starts empty (length 0, capacity 0); `grow()` goes to 256 and then doubles (trap if shared or viewed); `append(x)` and `append(array)` add numbers after the used length; `moveWithin(range, to)` is a memmove; `[..]` literals take any value element, including structs holding Strings; `new T[n]` still needs a zero |
| `json.parse<T>(bytes[, limits])` | server | done as a built-in generic, like serde: the compiler writes a decoder per struct T (ints with range checks, bool, f64, String, optionals, nested structs; unknown keys skipped; missing, duplicate, over-deep or over-long input throws the prelude's `JsonError`). `JsonLimits { maxDepth, maxStringBytes }` is in the prelude. `json.stream<T>`, arrays and enums to do; `User` and `Row` are not defined in `gort/` |
| `var x: T?`, `fn[yield]`, `while (c) stmt` | server, reader | done (`var x: T?` starts as null; the `[yield]` attribute is parsed and ignored until slice 2) |
| `scope { }`, `mutate(x) { }`, `sync(x) { }`, `spawn call` | server, store | stubs, to be replaced by slices 2 and 3: the body runs in place, `spawn f(x)` just calls `f(x)`. No parking, no children, no locks yet |
| enum payloads of any type | body, route | done: String, Array, Ref, structs, optionals. The other variants' payloads are zero; `drop` and `copy` switch on the tag so only the live variant is released or retained |
| lending a value to a `Ref<T>` parameter | reader, route, server | done as a borrow. A plain struct or enum value (a `var` local, a field, a temporary) given to a `Ref<T>` parameter is lent by address, no count moves, and the callee's changes show in the caller. It is allowed only when that parameter *borrows*: the function only reads through it, compares it, or passes it to another borrowing parameter. A function that stores, returns or throws the handle, or is used as a function value, cannot take a lent value: the error names the parameter. A borrowing parameter is not released by the callee. Through a function value the value is copied into a new counted object instead. A value is lent only from a *stable* place: inline in a local, or inside an object that a local or temporary keeps alive. A place reached through a handle read from a field, element or global (`h.child.pos`) is lent too, but the caller first takes its own count on that handle (a `Ref<Child>` temporary held to the end of the statement), so a callee that replaces `h.child` cannot free what the lent address points into. A `Ref` read from such storage and given to a borrowing parameter is retained the same way. Growing an array while one of its elements is lent traps (the callee's copy makes the array shared) |
| `extern struct` as a type | batching, body | an extern struct is a runtime object and always lives behind a counted handle: the name `Conn` means `Ref<Conn>`; `Ref<Ref<T>>` is `Ref<T>`; inside a generic, `Ref<T>` with a non-struct T is T itself (a borrow of a value is the value) |
| `==` on structs and enums; implicit integer widening; `String + number`; `find`, `contains`, `startsWith`, `copyFrom`, `copy`, `min`, `max`; `while (true)` that returns | body, reader, response | done. `==` is field by field (an enum: tag, then the live payload). A narrower integer converts to a wider one of the same signedness (usize and u64 interchange); signed with unsigned stays an error. The search functions take a String or an Array<u8> |
| `Map<K, V>` | store, redis | done, written in Gort in `gortc/src/prelude.gort` (open addressing, tombstones, 3/4 load, `hash(x)` built-in for ints, bools and Strings): `get` (V?), `put`, `insert`, `remove`, `has`, and the `count` field. `Ref<Map<K,V>>` fields left out of `new S { }` start as an empty map. No iteration yet |
| anonymous `fn(..) { }` with captures | server, redis | done in the restricted form: the body is lifted to a function whose leading parameters are the captured values (copies). With captures it can only be passed to an `extern` function, which receives `(code, env)`: the code takes the environment first and frees it. Without captures it is an ordinary function value. `gortc --link FILE.c` supplies the C side for tests |
| labeled loops `l: while`, `break l`, `continue l` | pipeline | done (a `goto` after the scope drops) |
| `for (x in it)` | body, response, server | done for a `Range` and an `Array` (desugared to a `while`; `continue` is safe); the `lines().skip(n)` iterators to do |
| function types `fn(..): T throws E`, `fn[nopark]` | route, batching | done: a plain pointer (no closures). A function name is a value; a non-throwing function given to a throwing type goes through a generated thunk. `fn` types may be enum payloads and fields |
| slices `a[i..j]` | batching, pipeline, reader, io | done (views) |
| a string literal where `Array<u8>` is wanted | reader | done: the literal's bytes as a fresh array (a literal only, for now) |
| fields left out of `new T {..}` / `T {..}` | state types | done: scalars are zero, `bool` false, `String` empty, `Array` empty, optionals null, including `Ref<T>?`; a plain `Ref<T>` field cannot be left out (except `Ref<Map>`, a new empty map). A `Ref` may still be null (a runtime value, or a null from the runtime): `r == null` is allowed, and using a null `Ref` is a `NullValue` exception where the function declares it or a `try` catches it, otherwise a trap, a plain struct has its own defaults |
| ternary `c ? a : b` | pipeline, reader, db | done (`null` in a branch makes the result optional) |
| `mutate`, `sync`, `scope`, `spawn` | store, server | slice 2 and 3 |

Order: exceptions and optionals first (they touch the C representation of every call and every handle), then the
`runtime` package and `extern`, then generics.

## Exceptions (done)

A `throws` function returns `int` (0 ok, 1 thrown) and writes its result through a trailing `T* ret_out`. A thrown exception
is a counted object held in one global slot (`gort_exc`: object, id, release function); `catch (e: T)` moves it into
`e` as `Ref<T>`, `catch (e: Exception)` moves it type-erased. Every call that can throw is followed by a test of the
status, and the failure path is the `return` path: it drops the pending temporaries of the statement and the locals down to the
nearest `try` (a `goto`) or the function (a `return 1`). The callee owns argument copies even when it throws, so the caller
unregisters them before the test. Sema requires every throw and every call that throws to be caught or declared.
`tests/ok/exceptions.gort` throws in the middle of an argument list and through a nested `try`; the leak check passes.

## Optionals (done)

`T?` is a nullable pointer for `Ref`, `String` and `Array` (the drop and copy are null-safe) and a `{ has, v }` pair for
scalars and values. `null` takes the type of its context; `x == null` and `x != null` test presence. **Narrowing is not
implemented**: wherever the value of an optional is needed (a field, a call, an index, an operand, an argument, `print`),
the compiler inserts a checked unwrap. A null there is a built-in `NullValue` exception when the code handles it, and a trap (exit 101) when it does not: the unwrap throws if an enclosing `try` catches `NullValue` or `Exception`, or if the function declares `throws NullValue` or `Exception`. That keeps ordinary code free of `throws` clauses for every optional it touches; the cost is that a function that does not mention it traps instead of passing the exception to its caller. Every program the narrowing rule would accept therefore
compiles and behaves the same; programs it would reject also compile and trap at run time. The flow analysis can come
later as a diagnostic and an optimisation (it removes the check) without changing the generated code for accepted programs.

## Packages, `extern` and slices (done)

Files of several packages can be given in one run (`gortc emit gort/runtime/*.gort gort/http/*.gort`). All packages share
one namespace in slice 1; an `import` must name a package among the files given. `extern fn`, `extern struct` (methods
without bodies, `parks` recorded), `extern impl T { .. }` and `extern let` are checked and emitted as C prototypes
(`gort_x_name`, `gort_x_Type_name`, `gort_g_name`); the C runtime in `c/runtime/` is expected to provide them, and the
linking of the two is not done yet.

A slice `a[lo..hi]` (either bound optional) is a **view**: a new array header sharing the run, with a count on the array it
was cut from (the root, so a view of a view still points at the run's owner). Writes through a view are seen by the parent,
and the run lives until the last view goes. Bounds are checked.

## Time and libraries (done)

`Duration` and `Instant` are built-in types held as signed 64-bit nanoseconds, so they add, subtract and compare as
integers. `Instant - Instant` is a `Duration`; `Instant ± Duration` is an `Instant`; `Duration * n` and `/ n` scale it.
`Duration.seconds(n)`, `.millis(n)`, `.micros(n)`, `.nanos(n)` and `Instant.now()` build them; `2.s`, `500.ms`, `50us`
are literals, and `16.kb`, `1.mb`, `4.gb` are untyped integers. `gortc check` type-checks files with no `main`
(a library); `gortc emit` also accepts a library and emits no `main`.

## Generics (done for structs and enums)

`struct Ring<T> { .. }` is a template. `Ring<Task<Ref<Resp>>>` in a type, in `Ring<..>.make(..)` or in a literal
`Ring<..> { .. }` creates one ordinary struct (named from the arguments) with its fields and methods substituted, once; the
methods are queued for lowering like any others, so an instance created while lowering a body is lowered too. Everything
downstream of sema sees only ordinary structs. The arguments can be left out where they follow from the context:
`Task.completed(resp)` from its argument, `Ring { slots: .. }` and `Ring.withCapacity(n)` from the expected type (a
field, a `let` with a type, a `return`, a parameter), or from the field values. **Not yet:** generic functions.

**The instance list is the pre-link phase.** The whole program is compiled together, so each instance exists once.
`gortc emit --instances` lists them, and the generated C starts with `GORT_EXTERN_INSTANCES(X)`, an X-macro with one
`X(template, struct)` entry per instance of an `extern` generic (`Task<Ref<Resp>>`, `Mailbox<Message>`). The C runtime
defines `X` and expands the list to provide exactly those functions, named per instance (`gort_x_Task_Ref_Resp__isDone`).
For separate compilation (slice 4) a module will carry its generic templates as IR, and instances will be made at link
time, cached by (template hash, arguments).

Arrays now hold optionals (null-filled) and plain values as well as scalars; an array of optional handles releases each
element when its last view goes (an element-drop function recorded in the array header).

## Next

1. **Slice 1: what the samples need.** Done: enums and `switch`. To do: optional types (`T?`, with narrowing), exceptions with
   `throws` and `try`, top-level `let`, `extern` and the `runtime` leaves, generics by monomorphisation (D6), interfaces
   and `with` embedding (D16). Goal: `gort/redis/` and `gort/http/` compile.
2. **Slice 2: parking.** Effect inference, Level-0 frames, the lazy and eager shapes (D1, D2), against the C runtime in
   `c/runtime/`. Goal: the tasks in `c/` come out of the compiler instead of by hand.
3. **Slice 3: sharing.** `sync` and `mutate`, the locking rule (D18 item 11, the largest open item for the programmer),
   atomic counts on shared objects.
4. **Slice 4: modules.** `.smod` with Tier A contracts, incremental builds (D1, D3, D11).

B18 (control block placement) can run at any point: only `gort_rt0.h` depends on its answer.
