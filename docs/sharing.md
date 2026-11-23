# Sharing and access checking

Two reference kinds, `ref T` and `shared T` (spelled `Ref<T>` in the current compiler). Whether an object is shared is a compile-time property of the object, fixed when it is constructed. The programmer states it in the handle's type (`shared T` or `ref T`), and for an untyped `let` the compiler infers it from where the object escapes. It works out where a `sync` block is required and reports a missing or too-weak one as a compile error. An unshared object has no lock, no atomic count and no run-time flag, so it costs nothing extra. All examples are taken or adapted from `gort/redis/db.gort`.

Status: **Direction**. Replaces the open questions about "how shareability is proven or declared" (spec 10.2, 10.3, 7.3). Partly implemented in `gortc`: explicit `sync` blocks with the lock, R1 to R4 (where a block is required, and its mode) and the no-parking rule, with `spawn` as the publishing point (R5, by capture). Not implemented: `fresh` results, the two layouts (R5, R6) and write-once inference across modules; the C beside the `.gort` is hand-written.

## The rules

**R1. When is a block required?** Reading or writing through a `Ref<T>` needs a block naming that reference when **both** hold:

1. the object is not *provably local*, and
2. the accessed field, or anything stored by value inside it, is *mutable*.

**R2. Provably local.** The object was allocated in this function with `new`, and no reference to it has escaped so far on any path: it has not been passed to a function whose summary says it retains or publishes the argument, stored into a non-local object, captured by a closure, or given to `spawn` or a message. Everything else is *incoming*: a parameter, a global, a field of a non-local object, or a call result not marked fresh (below).

**R3. Mutable.** A field is mutable if anything outside object construction writes it, directly or through a method whose access summary says it writes its receiver (`entries.insert(...)` mutates `entries`). A field never written after construction is write-once, and reading it needs no block. A type whose fields are all write-once is immutable, and it needs no block anywhere.

**R4. Modes.** A block lists each object under `reads` (read-only, shared with other readers) or `writes` (exclusive): `sync (reads a, b; writes c) { }`. All of them are locked together in a fixed order, so the order they are written in does not matter. Writing needs `writes`. A call made inside the block to a function whose summary writes the argument, when the argument is listed under `reads`, is an error: `needs writes store, but it is listed under reads`. This is checked even for an object that turns out never to be shared, so behaviour does not depend on sharing.

**What a block covers.** A block on `a` covers `a`'s own fields and the values stored inline in it. It does not cover another object that `a` points to: `a.b.n` needs `sync (writes a.b)`, because `b` may also be held by some other object `x`, whose holder locks `x`, not `b`. A second handle on a local (`let d = c`) is not a new local: once `c` is published, `d` is shared too. An `Array<T>` is an object like any other: `xs[i] = v` on an array that is reachable from elsewhere needs `sync (writes xs)` (or `g.xs` for a field), and `sync (writes g)` does not cover `g.xs`.

**Owned fields.** A field declared `owns` holds an object or array only its struct holds, so the struct's block covers it (and what it owns in turn). The compiler checks that it is only ever given new objects and never copied out. See the [manual](manual.md#55-owned-fields-owns).

**R5. Sharedness is fixed at construction.** Every `new T` is either *shared* or *unshared*, decided at that site and never changed. A handle's type says which: `shared T` or `ref T` (a bare struct name is a value, stored inline; `Array` and `String` are always handles, and a bare `Array<T>` is unshared). A `new` takes its sharedness from its expected type (a local's declared type, a field, a parameter or a return type), so no `new shared` is written: `shared Db db = new Db { .. }`. A `let` with no declared type is inferred: the compiler marks the site shared when a reference to the object crosses a vthread boundary on any path (passed to `spawn`, sent in a message, stored in a global or in a non-local object, or captured by a closure that does one of these), and unshared otherwise. In the first compiler only the declared form exists, and an untyped `let x = new T` is unshared. A wrong inference is repaired by declaring the type, never by converting an object, so there is no promotion and no representation change. A reference to a shared object is treated as non-local on every path, so R1 applies to it.

**Type rule.** Sharedness is part of the handle's type, and there is no conversion between `ref T` and `shared T`. Storing a `ref` handle into a shared object, or passing one across a vthread boundary, is a compile error that names the allocation site and the fix (declare it `shared`). A shared object holds only shared handles, values, scalars and `String`, so nothing is walked at a handoff and no graph is published. `String` is immutable and always counted atomically, whichever object holds it, so it needs no `shared` form.

`shared` qualifies one handle, so a container and its elements are separate choices:

| Type | The container | The elements |
|---|---|---|
| `shared Array<Thing>` | shared (atomic count, lock) | `Thing` is a value, inline, covered by the array's lock |
| `Array<shared Thing>` | unshared (plain count, no lock) | shared handles, each with its own lock |
| `shared Array<shared Thing>` | shared | shared handles; a block on the array covers its membership, not an element's body |
| `shared Array<ref Thing>` | error | an unshared handle inside a shared object |

`Array<ref Thing>` and `Array<shared Thing>` are different instances of the generic, with no conversion between them.

**R6. Two layouts, no run-time flag.** The compiler emits two C representations of one Gort type. An unshared object is a plain (non-atomic) count and the data: no lock, no atomic operation, no flag. A shared object is an atomic count, a vthread-aware reader-writer lock and the data. The compiler picks the allocation, count, destroy and `sync` operations from the type, so no code asks at run time whether an object is shared. A `sync` block on an unshared object compiles to nothing. A function that accepts either kind may be specialised into two C versions; this is an option, not a requirement. The compiler may drop a lock acquisition when it proves the current vthread already holds a sufficient lock.

**R7. No hoisting.** Every access goes through the `Ref`, field or container, so a call that removes an object from its container cannot leave a stale pointer behind. The compiler may keep a pointer in a register where it proves nothing can drop, grow or park, but the program cannot observe that. A pointer the program gets by calling `deref()` itself is unsafe, allowed only inside an `unsafe` block, and never on a shared object outside `sync` (D18 item 8). A `Ref<T>` parameter needs no block in the callee: the caller either owns the object locally or holds the block (R4 checks its mode against the callee's summary). The summary names the object by its path from the parameter (`a`, `a.b`), so the caller must hold a block on that object, not only on the parameter. A lambda has no checked caller and holds its own blocks.

**R8. Runtime types.** A type declared `extern` (Signal, Flag, Limiter, Mailbox, Wakeup) synchronises itself. Its methods need no block, whoever calls them and whether or not the object is shared; the `extern` declaration is a promise, as it is for parking. Only Gort-defined types follow R1 to R7.

**Globals and `spawn`.** A module-level `let` is one process-wide binding set before any vthread starts, so the object it holds is published at that point (R5); a module-level `var` is local to each vthread. `gort/http/store.gort` shows a `let users: Ref<UserTable>`. A `Ref<T>` to a local passed to `spawn` inside a `scope` is allowed because the scope waits for the child; the object it refers to is reachable from another vthread from then on, so R5 applies to it.

**Fresh results.** A function whose result is a new allocation that nothing else refers to is marked `fresh` in its summary, so `let db = Db.make(lim)` is local in the caller until it escapes. Without `fresh` the caller must treat the result as incoming.

Mutability, escape (retains/publishes), access mode per parameter, and `fresh` are Tier A contract data ([modules](architecture/modules.md#access-and-escape-summaries)). Changing a field from write-once to mutable is a breaking change.

## Example 1: local use, no blocks

A test or a command-line tool builds a store and reads it back in one vthread. Nothing escapes, so there is nothing to lock.

```cpp
// tools/load.gort
package tools

fn loadAndCount(path: String): usize {
    let store = new Store {}              // local
    for line in readLines(path) {
        store.entries.insert(Bytes.copyOf(line.key), Entry { value: new Value { bytes: line.value.copy() }, expiresAt: null })
    }
    return store.entries.count            // local: no block
}
```

Under the earlier `SharedRef` idea this would have needed a different type or a separate shared variant. Here it is the same `Store` and the same code as the server uses.

## Example 2: which accesses need a block

`Db` is shared by every connection, so its fields are *incoming* (R2). For each access the question is the one in R1: is the field mutable? A field that is set once in `make()` and never written again is write-once (R3), and reading it needs no block, however many workers share the `Db`.

```cpp
// redis/db.gort
package redis

struct Db {
    // Write-once: each of these is set in make() and never assigned again.
    store: Ref<Store>                 // the reference never changes, but the Store it points to does (see below)
    purgeWake: Signal                 // extern runtime type: synchronises itself (R8)
    closed: Flag                      // extern runtime type (R8)
    reaper: Ref<Mailbox<Reap>>        // the reference is write-once; Mailbox is an extern runtime type (R8)
    lim: Limits                       // plain data, never written after make()
    // ...

    fn get(key: Array<u8>): Ref<Value>? {
        // Reading `store` (the field of Db) needs no block: it is write-once.
        // Reading `store.entries` does: Store.entries is mutable (set and purgeExpired write it),
        // and the Store is incoming, so R1 requires a block naming the reference. sync is enough for a read (R4).
        sync (reads store) {
            let e = store.entries.get(key)
            return e == null ? null : e.value
        }
    }

    fn[nopark] release(channel: Array<u8>, mb: Ref<Mailbox<Message>>) {
        // No block. `reaper` is write-once, and Mailbox is an extern runtime type that
        // synchronises itself (R8). tryPush never parks, so [nopark] is true and cleanup() may call release().
        reaper.tryPush(Reap { ... })
    }
}
```

`gort/redis/db.gort` is written this way, with these notes in its header.

| Access | Block? | Why |
|---|---|---|
| `db.store`, `db.lim` (reading the field) | none | write-once (R3): nothing ever changes them after `make` |
| `store.entries` read | `sync (reads store)` | `entries` is mutable and the `Store` is incoming (R1); a read needs only `reads` (R4) |
| `store.entries` written (`set`, `purgeExpired`) | `sync (writes store)` | writing needs the exclusive mode (R4) |
| `db.purgeWake`, `db.closed`, `db.reaper.tryPush(...)` | none | `extern` runtime types synchronise themselves (R8) |
| `v.bytes` on the `Ref<Value>` that `get` returned | none | `Value` is immutable once built (R3), so the caller reads it after the lock is released |

This is why the code writes `sync (reads store)` and not `sync (reads db)`: the lock belongs to the object with the mutable field, not to the `Db` that merely holds a reference to it.

## Example 3: errors the compiler reports

```cpp
// redis/stats.gort
package redis

fn size(db: Ref<Db>): usize {
    return db.store.entries.count         // error: store.entries is mutable and db is incoming
}                                         //   fix: sync (reads db.store) { return db.store.entries.count }
```

```cpp
// redis/trim.gort
package redis

fn trim(store: Ref<Store>) {
    sync (reads store) {
        store.entries.remove(oldest(store))   // error: entries.remove writes its receiver
    }                                         //   needs writes store, but it is listed under reads
}
```

```cpp
// redis/peek.gort
package redis

fn peekKeys(store: Ref<Store>): Ref<Map<Bytes, Entry>> {
    sync (reads store) {
        return store.entries              // allowed: entries is a Ref<Map>, so this copies the Ref out and takes a count
    }
}

fn countKeys(store: Ref<Store>): usize {
    let m = peekKeys(store)
    return m.count                        // error: m is incoming and Map is mutable: needs `sync (reads m)`
}                                         //   fix: sync (reads m) { return m.count }  (or do the count inside `sync (reads store)`, before the Ref leaves)
```

The messages name the chain and the fix. A missing block on a reference that turns out never to be shared is still reported: that is the over-approximation in R2, and the cost is one extra block that does nothing at run time.

## Example 4: publishing, and what changes after it

```cpp
// redis/main.gort
package redis

fn main(args: Ref<Args>) {
    let db = Db.make(lim)                 // fresh: local so far
    db.purgeWake.notify()                 // Signal is a runtime handle, not db's mutable state
    spawn db.purgeLoop()                  // R5: db is published here
    let n = db.store.entries.count        // error: db is shared from the spawn on; entries is mutable
    serve(listener, db, lim, shutdown, live)
}
```

Before the `spawn`, `db` is local and could be inspected directly. After it, every access to mutable state follows R1. Moving the line above the `spawn` makes it legal, and correct, since no other vthread yet exists.

## Example 5: modes across a call

```cpp
// redis/expire.gort
package redis

fn expireAndCount(store: Ref<Store>): usize {
    sync (writes store) {
        purgeBatch(store)                 // summary: purgeBatch writes param 0 -> writes required: ok
        return store.entries.count
    }
}

fn statsOnly(store: Ref<Store>): usize {
    sync (reads store) {
        purgeBatch(store)                 // error: purgeBatch writes its argument; store is listed under reads
        return store.entries.count
    }
}
```

The check uses `purgeBatch`'s summary, so it works when `purgeBatch` is in another module and its source is not available.

## Example 6: methods on values, and which lock

A method's `self` is a place (the body itself), so one method serves a `Ref<Point>`, an `Array<Point>` element and an `Array<Ref<Point>>` element, with the same call syntax (D18 item 1). The compiler dereferences and passes the body; the developer writes the block, naming the **owner**: the `Ref`, or the array. The compiler only checks that it is there and strong enough (R1, R4).

```cpp
struct Point {
    x: i32
    y: i32
    fn move(dx: i32, dy: i32) { self.x += dx; self.y += dy }       // summary: writes self
    fn len2(): i32 { return self.x * self.x + self.y * self.y }    // summary: reads self
}

struct World { player: Ref<Point> }       // the handle is write-once, the Point is mutable
struct Field { pts: Array<Point> }        // elements stored inline
struct Fleet { ships: Array<Ref<Point>> } // handles to separate objects

fn local() {
    Ref<Point> r = new Point { x: 0, y: 0 }
    r.move(1, 2)                          // local: no block
}

fn step(w: Ref<World>) {
    sync (writes w.player) { w.player.move(1, 0) }     // the Ref is the owner
    sync (reads w.player)   { log(w.player.len2()) }
}

fn drift(f: Ref<Field>, i: usize) {
    sync (writes f.pts) { f.pts[i].move(1, 1) }        // an element has no identity: the array is the owner
}

fn nudge(f: Ref<Fleet>, i: usize) {
    Ref<Point> p
    sync (reads f.ships) { p = f.ships[i] }             // copy the handle out (takes a count), release the array lock
    sync (writes p)     { p.move(1, 0) }               // then lock the Point itself
}
```

Errors the compiler reports:

```cpp
sync (reads w.player) { w.player.move(1, 0) }   // error: Point.move writes its receiver; needs writes w.player, but it is listed under reads
w.player.move(1, 0)                      // error: Point is mutable and w.player is incoming; needs `sync (writes w.player)`
sync (reads f.ships) { f.ships[i].move(1, 0) }  // error: the array lock does not cover the Point behind the Ref
```

The last one assumes each object has its own lock (D18 item 11, Locking: open). Copy or in place is the developer's choice, and the compiler cannot tell which was meant:

```cpp
sync (reads f.pts)   { let q = f.pts[i]; q.move(1, 1) }   // q is a copy: f.pts[i] is untouched
sync (writes f.pts) { f.pts[i].move(1, 1) }              // in place
```

On a growable `Array<T[]>`, an element used as `self` is pinned for the call, so a callee that grows the same array is an error (a compile error when its summary shows it, otherwise a run-time "array pinned" exception):

```cpp
struct Spawner { fn spawnMore(all: Array<Spawner[]>) { all.push(Spawner {}) } }
sync (writes all) { all[0].spawnMore(all) }    // error: all[0] is pinned for the call and spawnMore grows `all`
```

| | Developer writes | Compiler does |
|---|---|---|
| Lock | a `sync` block naming the owner | reports a missing or too-weak block |
| Mode | which one | takes it from the callee summary |
| Local object | nothing | proves it local, skips the block |
| Deref and receiver place | nothing | derefs, passes the body |
| Pin on a growable element | nothing | adds it, drops it when provably unneeded |
| Copy or in place | `let q = a[i]` or `a[i].move()` | cannot choose |
| `Array<Ref<T>>` | copy the handle out, then lock the object | lock order is open |

## Example 7: the same field, inline or `Ref`

A field whose type is a struct is stored **inline** when written plainly, and is a **separate counted object** when written `Ref<...>` (D18 item 1). The choice is the programmer's, and it decides identity, sharing, copying and locking. `gort/` uses both.

```cpp
struct Limits { maxConns: usize; maxSubs: usize; maxFrame: usize }   // plain data
struct Scanner { state: u8; need: usize }

struct Db {
    lim: Limits                 // INLINE: the Db owns its copy; no second object, no pointer chase, nothing else holds it
    store: Ref<Store>           // REF: a separate object that other holders also reach
}

struct Reader {
    lim: Ref<Limits>            // REF: one Limits shared with the server and every connection; a copy per Reader would drift
    scanner: Scanner            // INLINE: one reader's private state
}

fn make(shared: Ref<Limits>): Ref<Db> {
    return new Db { lim: shared, store: new Store {} }   // `lim: shared` copies the body out of the Ref into the Db
}
```

| | Inline (`lim: Limits`) | `Ref` (`lim: Ref<Limits>`) |
|---|---|---|
| Storage | inside the container's body | its own counted object, with a control block |
| Identity | none: `==` compares state | yes: `==` compares identity |
| Assigning it | copies the body | copies the handle and takes a count |
| Sharing | owned by the container alone | any number of holders |
| Locking | the owner's block covers it | its own block (D18 item 11, Locking) |
| Naming it | no `Ref` can name it | a `Ref` is the name |
| Cost | larger container body | an allocation, a count and a pointer chase |

Rule of thumb used in `gort/`: a part the container exclusively owns is inline (`Db.lim`, `Reader.scanner`, `Route.path`, `InFlight.ring`); anything shared, pooled, optional or returned from a handler is a `Ref` (`Reader.lim`, `Server.cfg`, `Server.table`, `Resp`, `Ref<OutBuf>?`). Library containers, `String` and `Array` are always handles.

## Example 8: a plain `new`, and when it is counted

The compiler decides at each `new` whether the value is ever converted to a `Ref` on any path (assignment, a `Ref` parameter, a return, a store, a capture, `spawn`). If so, the object is counted from allocation and the scope holds it in an injected `Ref`. If not, it is scope-owned: a bare body with no control block, freed at exit (D18 item 5).

```cpp
fn demo(flag: bool): Ref<Point> {
    let p = new Point { x: 1, y: 2 }     // COUNTED: p is converted to a Ref below (show, return), so p is the injected Ref
    p.move(1, 1)                         // a method call: self is a place, no conversion
    let q = p                            // a copy of the body (a value), no conversion
    if (flag) {
        show(p)                          // show takes Ref<Point>: a conversion on one path is enough
    }
    return p                             // moves the injected Ref out: the Point outlives demo
}
```

```cpp
fn temp() {
    let b = new Builder {}               // never converted to a Ref on any path: scope-owned
    b.add(1)
}                                        // freed here; may live in a frame, with no control block and no counts
```

The decision is conservative: a conversion on a path that is rarely taken still makes the site counted for every execution. The compiler reports it ("counted because of line N") so the cost is visible, and writing `Ref<Point> p = new Point {...}` makes the choice explicit.

## What the module file carries

```
Db.get      self.store: read via sync, result: not fresh, throws: none, park: May
Db.make     result: fresh, retains lim
Store       entries: mutable   expirations: mutable   topics: mutable
Value       bytes: write-once  (type is immutable)
Db          store, purgeWake, closed, reaper, lim: write-once
purgeBatch  param 0: write
```

A consumer of `db.smod` can run every check above with no source. An implementation change that makes `Db.lim` mutable changes the contract hash and is reported as a breaking change, because every reader of `lim` now needs a block.

## Generated C for a block

```c
// sync (reads store) { ... } where store is a shared type
st_t s = rwlock_read_enter(t, &store->lock);     // may park; PARKED unwinds as usual
/* body: no parking inside, no slice checks */
rwlock_read_exit(&store->lock);                  // on every exit path, including throws
```

For an unshared type the same block emits only the body. Counts follow the type too: `rc_inc_plain` and `rc_dec_plain` on an unshared object, `rc_inc_atomic` and `rc_dec_atomic` on a shared one. There is no flag test per copy or release.

## Syntax

*Status: proposed grammar for the first compiler pass. Implemented so far: `ref T` and `shared T` for Gort structs, with both layouts, the type rules and the spawn, global and extern-generic checks.*

```
type   := qual? name ('<' type (',' type)* '>')? suffix*
qual   := 'ref' | 'shared'
suffix := '?' | '[' N ']'
```

- **`ref` and `shared` are soft keywords** (a qualifier only where a type name follows, so a variable may still be called `shared`) and qualify the one handle written after them. `ref Db` and `shared Db` are handles to an unshared and a shared `Db`. A bare struct name is a value, stored inline. A bare `Array<T>` or `String` is a handle: `Array<T>` is unshared, and `String` is always atomic (R5).
- **A suffix applies to the whole qualified handle.** `shared Db?` is an optional shared handle, and `Array<shared Thing>?` an optional unshared array of shared handles. `T[N]` is only ever a fixed run of values (`Thing[4]`, `u8[N]`), so a qualifier on it is an error. An array of handles is written `Array<shared Thing>`, and a shared array `shared Array<..>`; there is no short form.
- **Qualifiers nest through type arguments only where the argument is a handle:** `Map<String, shared Entry>`, `Ring<ref Task>`. A qualifier on a scalar or a value type is an error.
- **`new` has no qualifier.** Its sharedness is the expected type's: `shared Db db = new Db { .. }`, a `shared Db` field or parameter, a `shared Db` return type. With no expected type (`let x = new Db { .. }`) the first pass makes it `ref Db`.
- **No conversion between `ref T` and `shared T`**, in an assignment, an argument or a return. A call passing a `shared` object to a `ref` parameter is an error that suggests the fix. A function that works on either kind needs the specialisation described under R6, which the first pass does not have.
- **Lending a value to a parameter** (the current borrow rule) works only for `ref T` parameters: a value in a frame is not a shared object.
- **`WeakRef<T>` names its target's kind:** `WeakRef<shared T>`. The weak handle itself follows the array rule.
- **`extern` types are opaque C, so Gort cannot know whether they are thread-safe.** The default `extern struct Conn` is not: it is a handle that takes `ref` or `shared` like any other (`shared Conn` is the `Arc<RwLock<Conn>>` case, the lock being the handle's own). An `extern sync struct Mailbox` promises that its C code synchronises itself (R8); it is always counted atomically, has no lock and takes no qualifier. Each method of a non-`sync` extern type states its mode, `reads` or `writes`, and an unmarked method is `writes`, so the same R1 to R4 checks apply as for Gort functions. A parking method cannot be called inside a `sync` block (R4), so a `shared Conn` cannot call `recv` under its lock; a socket is normally a `ref Conn` owned by one vthread and reached from others through a `sync` type.
- **Control block placement favours C.** Anything that can be passed to C is a plain pointer to its data, so no header precedes an array's run or an `extern` object's body. An array's control block is apart from its run (D18 item 3); an `extern` handle's control block is external too, and holds the count, the lock (in the `shared` layout), the C pointer and the destructor, so a C object that C allocated can be wrapped without copying. Passing an array to C pins it for the call, and a `shared` array's lock is held across that call, so the call cannot park. Slice 0's 16-byte header before the body stays for Gort structs, which C never sees; for arrays and `extern` types the placement is fixed by this rule, and B18 decides only how Gort structs are laid out.

**Migration.** The compiler today gives every counted object atomic counts and a lock, which is the `shared` layout. While the samples are converted, `Ref<T>` is accepted as an alias for `shared T`, so nothing changes behaviour until a sample is rewritten to `ref T` on purpose. Done in `gortc`: `shared T` is `Ref<T>`; `ref T` is the plain layout for Gort structs (a non-atomic count, no lock, no `sync` needed). Also done: `extern sync struct`, `reads fn` and `writes fn` on extern methods (unmarked: writes), and `ref` on a plain extern type. Arrays have both layouts too: a bare `Array<T>` (or `ref Array<T>`) is unshared, `shared Array<T>` has the atomic count and the lock, a new array takes the layout it is stored in, a shared object may hold only `shared` arrays (or `owns` ones), and a runtime (`extern`) function takes either layout. A function over either layout writes `any` before a handle type (`fn sum(a: any Array<i64>)`): it is compiled once for each layout, one `any` per signature means one layout, a call picks by its `any` arguments and rejects a mix, and a call with no handle argument to say (a new array) uses the unshared one. `any` is for free functions only. Not yet: `any` on methods or function values, `copy()` of an array of handles, a C-allocated object wrapped with an external control block, escape inference for an untyped `let` (it takes the layout of its first use), and the lock-free `Plain` header (it keeps the 16-byte header, so only the atomics and the lock are gone). Because the keywords are soft, the four tests that name a variable `shared` needed no rename.

## Growable containers

A container that can relocate its storage (arrays, builders, map tables) is reached through a non-moving header, and references into it are `(header, offset)`. Growing a shared container needs `writes`, reading it needs `reads`, so a raw address is valid exactly inside the block that holds it and a grow cannot happen under a reader (R1, R4, R7). An unshared container has no lock; its raw address is valid until the next call that may grow it or the next park. See spec 4.5 and [D15](architecture/open-decisions.md#d15-growable-arrays-relocation-and-stable-references).

## Open

- **Inference accuracy.** A site is shared if any path publishes it. Measure how often this over-approximates in `gort/` (an object shared only on a rare path pays atomics on all paths).
- **Specialising functions.** Whether a function over `Ref<T>` is compiled once per sharedness (code size) or only where a call site needs it.
- **Generic containers.** A library container over `ref T` and over `shared T` needs two instantiations (table in R5).
- **Reference syntax.** `ref T` and `shared T` replace `Ref<T>`; the parser, `sema.rs`, `codegen.rs`, the samples and the docs still say `Ref<T>` until the rename.
- **Where the lock word lives.** In the shared layout only, so unshared objects pay no bytes. Inline in the control block or out of line (waiters in a table keyed by address) is still open.
- **Write-once across modules.** A field is write-once if no code in its defining module writes it. Public fields that other modules may write need either a mutability declaration or a rule that they are mutable by default.
- **Immutable types.** Inferred from R3 as above; whether to also let the programmer assert it (for a compile error if a later change breaks it) is undecided.
- **Runtime types that are not thread-safe.** R8 treats every `extern` type as self-synchronising. Whether an `extern` declaration may say otherwise (a handle that one vthread must own) is undecided.
- **Interfaces and function values.** A call through one needs a declared access mode per parameter, as it needs an effect bound. Default is "writes", the safe direction for the caller.
- **Lint.** A value read in one `sync (reads x)` block and used to decide a write in a separate `sync (writes x)` block on the same object (the get-then-put race of spec 7.2). A warning, not an error.
