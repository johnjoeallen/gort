# Language manual

The reference for Gort as `gortc` accepts it today. It is kept in step with the compiler: when a rule changes, this page changes in the same commit. Rules marked **checked** are enforced by the compiler and have a test under `gortc/tests/` (`ok/` programs run, `err/` programs are rejected with the quoted message). Rules marked **design** are in [`gort-design.md`](https://github.com/johnjoeallen/gort/blob/main/gort-design.md) but not built yet. Why the rules are what they are is in [Sharing and access checking](sharing.md) and the [compiler notes](architecture/compiler.md); this page says what they are.

## 1. Values and handles

Every type is one of two kinds.

| Kind | Types | Stored | Copy | Counted |
|---|---|---|---|---|
| value | integers, `bool`, `f64`, `Duration`, `Instant`, `struct`, `enum` | inline in the variable, field or element that holds it | by value | no |
| handle | `Ref<T>`, `String`, `Array<T>`, `WeakRef<T>` | the variable holds an address | copying the handle shares the object | yes |

A `struct` is a value. `let b = a` copies it, and `b.x = 1` does not change `a`. A struct inside a struct is stored inline, so `Line { a: Point, b: Point }` is one allocation, and a struct cannot contain itself by value (**checked**: `contains itself by value`).

A **handle** names an object that lives elsewhere. Copying a handle adds a count; the object is freed when the last count goes. There is no tracing collector, so cleanup is deterministic: a count drops at scope exit, on `return`, `break`, `continue` and `throw`.

### 1.1 Making a counted object

`new T { ... }` allocates an object of struct type `T` and gives a `Ref<T>`. Writing a struct literal where a `Ref` is wanted, without `new`, is an error (`write new to allocate a counted object`). A `new` that never becomes a `Ref` on any path of the function is *scope-owned*: a bare body, freed at the end of its scope, with no count at all. A `new` that is converted to a `Ref` on any path is counted from the start. Both look the same in source.

A variable bound to `new` cannot be reassigned (**checked**: `cannot be reassigned`), because the compiler may have given it an uncounted body.

### 1.2 Copying and cleanup

A struct with a `cleanup()` function owns something that must be released once, so it **cannot be copied** (**checked**: `cannot copy a value of type Res`; this includes a struct that holds one inline: `Outer`). Hand it around as a `Ref<Res>`, or lend it (section 2). `cleanup()` runs when the last count goes. It must not park or throw (**checked**: `cleanup() of Res may park`).

### 1.3 Optionals and null

`T?` is a value or `null`. For a handle it is a nullable handle; for a scalar it is the value plus a presence flag. A `T` that is not optional is never null (**checked**: `x == null` on a non-optional is `never null`). `var x: T?` starts as `null`. A `switch` over an optional must cover `Null`.

### 1.4 Mutability of names

`let` binds once (`x is not mutable`), `var` can be reassigned. A `let` struct value cannot have its fields assigned (`p is not mutable`). A `let` that holds a `Ref` can still change the *object's* fields, because the object is not the variable.

## 2. Borrowing: lending a value to a `Ref<T>` parameter

A function that takes `Ref<T>` normally gets a counted handle. A plain struct or enum value (a `var` local, a field, a temporary) can be given to such a parameter without making an object: it is **lent by address**. No count moves, and the callee's changes show in the caller (`ok/borrow.gort`).

```
struct Pos { x: i64 }
fn bump(p: Ref<Pos>) { p.x += 1 }
fn main() {
    var p = Pos { x: 1 }
    bump(p)              // lent: p.x is now 2
}
```

### 2.1 When a parameter borrows

A parameter **borrows** when the function only *reads through it, compares it, or passes it to another borrowing parameter*. These are inferred; you do not write them.

A function that **stores** the handle (into a field, array element or global), **returns** it, **throws** it, or is **used as a function value**, does not borrow that parameter. Giving it a lent value is an error that names the parameter (**checked**: `cannot lend a value to parameter 2 of keep`, `err/borrow_escapes.gort`). The fix is to make the object with `new`, or keep the value as a `Ref` from the start.

A borrowing parameter is not released by the callee. Through a function value a lent value is copied into a new counted object instead, because the callee's borrow status is unknown there.

### 2.2 Where a value can be lent from

A value is lent only from a **stable place**: inline in a local, or inside an object that a local or temporary keeps alive.

A place reached *through a handle read from a field, element or global* (`bump(h.child.pos)`) is lent too, but first the caller takes its own count on that handle, held to the end of the statement (a `Ref<Child>` temporary). So a callee that replaces `h.child` cannot free what the lent address points into (`ok/borrow_owner.gort`, `ok/borrow_field.gort`, `ok/borrow_local.gort`). A `Ref` read from such storage and given to a borrowing parameter is retained the same way.

### 2.3 Arrays and lent elements

Growing an array while one of its elements is lent **traps** at run time: the callee's copy of the handle makes the array shared, and a shared array cannot grow (`grow while the array is shared or viewed`). The same trap applies to appending to an array that has a slice view.

### 2.4 Summary

| You write | What happens | Count |
|---|---|---|
| `f(r)` with `r: Ref<T>` | the callee gets a retained handle, or borrows it if it only reads | +1 for the call if retained |
| `f(v)` with `v` a struct value and a borrowing `Ref<T>` parameter | lent by address | none |
| `f(v)` with a non-borrowing parameter | error | |
| `f(h.child.pos)` | caller retains `h.child` for the statement, lends the place | +1 temporary |
| `f(v)` through a function value | copied into a new counted object | new object |

## 3. Arrays, strings and slices

`Array<T>` is a counted handle to a run of `T`. `length` is the elements in use; `capacity()` is the elements allocated. Indexing and slicing are checked against the capacity (a violation traps). `String` is immutable and counted; literals are permanent and never counted.

An element type must be a value, a `Ref<T>`, or an optional of one (**checked**: `an array element must be ...` for `new Ref<P>[3]`). An array of `T?` where `T` is a handle drops each live element when the array dies.

A **slice** `a[lo..hi]` is a view that shares the same storage and keeps the parent alive. Writes through a view show in the parent. An array can **grow** (append, `grow`) only while it has exactly one count and is not a view; otherwise it traps. Growth replaces the run with one twice as long, the first growth being to 256.

Arrays are objects for locking purposes (section 5).

## 4. Enums

Enum payloads can be of any type. Only the live variant's payload is held; `drop` and `copy` switch on the tag. A `switch` binding of a payload names the payload itself, with no copy, when the arm assigns to it or calls a method on it. A `switch` needs a `default` arm unless it covers every variant.

## 5. Sharing and `sync`

Whether an object is shared is not written anywhere. The compiler decides where a `sync` block is needed and reports a missing or too-weak one as an error. The full rules R1 to R8 are in [Sharing and access checking](sharing.md); this section is what they mean for the programmer.

### 5.1 The block

```
sync (reads a, b; writes c) { ... }
```

Each named object is listed under `reads` (shared with other readers; the body may not write through it) or `writes` (exclusive). All of them are locked together in one fixed order (by address), so the order written does not matter. Naming one object under both is an error. The target must be a `Ref<T>` or an `Array<T>`, or an optional one (`Ref<T>?`, `Array<T>?`), which locks what it points to and fails like any use of a null (`null value used`) when it is null (`ok/sync_optional.gort`, `trap/sync_null.gort`). So an element of an array of handles can be locked by copying it out under the array's lock and naming the copy: `sync (reads g.items) { cur = g.items[i] }` then `sync (writes cur) { cur.n = 5 }`; an `extern` type needs no block (it synchronises itself).

* The lock is **re-entrant within a call tree**: a nested block on an object the thread already holds takes nothing and releases nothing. A `writes` entry inside a block that holds the object under `reads` upgrades in place.
* A block is released on `return`, `break`, `continue` and `throw`.
* The body **must not park** (**checked**: `a sync block must not park`). A function that enters a block is itself marked may-park.
* A block nested inside another on a *different* object is a warning; use the grouped form.

### 5.2 When a block is required

Reading or writing through a `Ref<T>` needs a block **naming that reference** when both hold: the object is not provably local, and the field touched is mutable.

* **Provably local**: made with `new` in this function and its handle has not escaped on any path so far (stored, captured, passed to something that keeps it, given to `spawn`, or copied). A fresh function result is local until it escapes. Everything else is incoming: a parameter, a global, a field of a non-local object, a call result that is not fresh.
* **Mutable**: some code anywhere assigns the field, grows it, or calls something that writes it. A field never written after `new` is write-once and reads without a block.
* **Copies are not new objects**: `let d = c` is a second handle. Once `c` is published, `d` is shared.

### 5.3 What a block covers

A block on `a` covers `a`'s own fields and values stored inline in it. It does **not** cover another object `a` points to. `a.b.n` needs `sync (writes a.b)`, because `b` may be held by another object `x`, whose holder locks `x` and not `b` (**checked**, `err/share_alias_inner.gort`).

A `Ref` parameter or receiver needs no block in the callee: the caller either owns the object locally or holds the block, and the compiler checks the caller's block against the callee's summary, including its **mode** (`passing it to bump, which writes through it, needs writes g`).

This holds at any depth. The callee's summary records the *path* to each object it reaches through its receiver or a parameter (`a`, `a.b`, `a.b.c`), and the caller must hold a block naming that object: `bump(a)` that writes `a.b.n` needs `sync (writes a.b)`, and a block on `a` alone is rejected (`err/share_ref_deep_caller.gort`, `ok/sync_ref_deep.gort`). Writing through `a.b` does not make the field `b` itself mutable; only assigning `a.b = ...` does.

**Lambdas hold their own locks.** A lambda is started by `spawn` or called through a function value, so nothing checks its caller. Whatever it needs of the objects it captured, or of its parameters, it must hold itself: `startThread(fn() { bump(g) })` is rejected unless the lambda wraps the call in `sync (writes g)` (`err/share_lambda_captured.gort`, `ok/sync_lambda_captured.gort`).

### 5.4 Arrays under `sync`

An `Array<T>` is an object of its own with its own lock; a slice locks the array it looks into.

| Access | Needs |
|---|---|
| array made in this function, or returned by a *fresh* function (every `return` is a new array, or a view of a parameter whose argument is itself local) | nothing |
| array returned by a function that returns a stored array (`return self.xs`) | as shared as the field it came from: the copy needs its own block |
| `g.xs[i]`, `g.xs.length`, append, where `g` is shared and something changes `xs` in place | `sync (reads g.xs)` / `sync (writes g.xs)` |
| `let xs = g.xs` then `xs[i] = v` | `sync (writes xs)` on the copy |
| global array that something changes in place | a block naming it |
| array field reached through this function's receiver or parameter, at any depth of plain field steps (`self.xs`, `g.xs`, `a.b.xs`) | the caller's block on that array: the callee's summary records the field path it reads or writes, and a caller that does not hold the object locally must hold `sync (reads/writes a.b.xs)` |
| array reached through an element (`self.items[i].xs`) | a block in this function: the caller cannot name "element i" |
| array field of a struct held by value | nothing |

`sync (writes g)` **does not cover `g.xs`**.

An `extern` function receives an array as a raw pointer and a length, `(void* p, size_t p_len)`, and the caller keeps the array alive for the call. It gives an array back as `gort_xarr { void* p; size_t n; }`. The runtime compares the returned pointer with the data of each array argument:

* inside an argument's data (at an element boundary): the result is **that array**, or a view of it when the pointer or length differs (`firstHalf`, `dropFirst` in `ok/extern_array.gort`); writes through the result show in the original;
* anywhere else: C allocated it, so it is **copied into a new Gort array and freed**;
* a null pointer with length 0 is an empty array; a pointer inside an argument that runs past it, or is not on an element boundary, traps.

The element type of an array that crosses into C (a variadic parameter such as `sources: Wakeup...` packs the call's arguments and is not checked) is a number, `bool`, or a plain struct made only of those (no `String`, `Ref`, `Array`, optional, enum, `cleanup()` or extern struct): anything else is a compile error (`err/extern_array_*.gort`). The checker cannot tell the two result cases apart, so an extern result is local only when every array argument is local; otherwise it is as shared as the arguments. Counts do not enter into it; the local/shared distinction matters only for `sync`.

A block on the holder never stands in for a block on the array: `sync (writes g) { bump(g) }` is rejected when `bump` writes `g.xs`. So every access to one array, whether direct, through a callee, or through a copied-out handle, takes the array's own lock, and they exclude each other (`ok/sync_array_one_lock.gort`, `err/share_array_holder_block.gort`).

Known gaps: the shared flag (the lock is always taken); lock order across separate nested blocks (the 5 s wait timeout traps a deadlock).

### 5.5 `spawn` and publishing

A `Ref` passed to `spawn`, sent in a message, stored in a global or a non-local object, or captured by a closure that is, is **published**: from then on it is no longer local on any path. A module-level `let` holds an object published at startup; a module-level `var` is local to each vthread.

## 6. Parking, effects and `fn[nopark]`

A function **may park** if it is an `extern ... parks` function, calls through a plain function value, enters a `sync` block, or calls a function that may park (a fixpoint over the call graph). `--effects` lists them.

* `fn[nopark]` asserts a function never parks; the compiler checks it (`f is marked nopark but may park: it calls wait`).
* The effect is part of a function type: `fn[nopark](A): R` is a value whose call never parks. A `fn[nopark]` value goes where a plain function is wanted. The other way is allowed only for a named function or a lambda, which is then checked; an arbitrary plain value is an error.
* `cleanup()` must not park.
* A `sync` body must not park, though it may call a function that enters a nested block (a nested wait never parks).

## 7. Errors

Errors are exceptions, declared with `throws`. `throw Name(args)` needs a declared exception (the compiler reports a throw that is not declared as `may be thrown here`). A function with a result must return on every path (`may end without returning a value`). This section is thin: the full exception rules are not yet written up here.

## 8. Keeping this page current

When a change touches a rule above:

1. add or change the test (`ok/` to show it works, `err/` with the exact message to show it is rejected);
2. change the paragraph here and, if it is a design change, [sharing.md](sharing.md) and the row in [the compiler notes](architecture/compiler.md);
3. keep the **checked** label only where a test exists.

Rules the compiler does not enforce yet stay in the *Known gaps* lines, not in the prose above them.
