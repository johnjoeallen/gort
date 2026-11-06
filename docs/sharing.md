# Sharing and access checking

One reference kind, `Ref<T>`. Whether an object is shared is not written anywhere. The compiler works out where a `sync` block is required, reports a missing or too-weak one as a compile error, and the blocks cost almost nothing on an object that was never shared. All examples are taken or adapted from `gort/redis/db.gort`.

Status: **Direction**. Replaces the open questions about "how shareability is proven or declared" (spec 10.2, 10.3, 7.3). Partly implemented in `gortc`: explicit `sync` blocks with the lock, R1 to R4 (where a block is required, and its mode) and the no-parking rule, with `spawn` as the publishing point (R5, by capture). Not implemented: `fresh` results, the shared flag (R6) and write-once inference across modules; the C beside the `.gort` is hand-written.

## The rules

**R1. When is a block required?** Reading or writing through a `Ref<T>` needs a block naming that reference when **both** hold:

1. the object is not *provably local*, and
2. the accessed field, or anything stored by value inside it, is *mutable*.

**R2. Provably local.** The object was allocated in this function with `new`, and no reference to it has escaped so far on any path: it has not been passed to a function whose summary says it retains or publishes the argument, stored into a non-local object, captured by a closure, or given to `spawn` or a message. Everything else is *incoming*: a parameter, a global, a field of a non-local object, or a call result not marked fresh (below).

**R3. Mutable.** A field is mutable if anything outside object construction writes it, directly or through a method whose access summary says it writes its receiver (`entries.insert(...)` mutates `entries`). A field never written after construction is write-once, and reading it needs no block. A type whose fields are all write-once is immutable, and it needs no block anywhere.

**R4. Modes.** A block lists each object under `reads` (read-only, shared with other readers) or `writes` (exclusive): `sync (reads a, b; writes c) { }`. All of them are locked together in a fixed order, so the order they are written in does not matter. Writing needs `writes`. A call made inside the block to a function whose summary writes the argument, when the argument is listed under `reads`, is an error: `needs writes store, but it is listed under reads`. This is checked even for an object that turns out never to be shared, so behaviour does not depend on sharing.

**R5. Publishing.** A reference to a local object crosses a vthread boundary when it is passed to `spawn`, sent in a message, stored in a global or in a non-local object, or captured by a closure that is. From that point the object is *shared*: the compiler treats the reference as no longer local on every later path, so R1 applies after it. At run time the object's shared flag is set before the handoff, and everything reachable from it is set too (or the compiler refuses the publish; see Open).

**R6. Cost on an unshared object.** A `sync` block tests the flag once at entry. If it is clear, it does nothing. The flag only ever goes from clear to set, so the test at exit agrees with the one at entry.

**R7. No hoisting.** Every access goes through the `Ref`, field or container, so a call that removes an object from its container cannot leave a stale pointer behind. The compiler may keep a pointer in a register where it proves nothing can drop, grow or park, but the program cannot observe that. A pointer the program gets by calling `deref()` itself is unsafe, allowed only inside an `unsafe` block, and never on a shared object outside `sync` (D18 item 8). A `Ref<T>` parameter needs no block in the callee: the caller either owns the object locally or holds the block (R4 checks its mode against the callee's summary).

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
// sync (reads store) { ... } where store is incoming
bool sh = obj_is_shared(store);                  // one load of the control-block flag
if (sh) { st_t s = rwlock_read_enter(t, &store->lock); /* may park; PARKED unwinds as usual */ }
/* body: no parking inside, no slice checks */
if (sh) rwlock_read_exit(&store->lock);          // on every exit path, including throws
```

Reference counts follow the same split. An object the compiler proves local uses plain counts. A reference to a non-local object tests the flag: plain count if clear, atomic if set. That test per copy and release is a real cost, and the alternative (always atomic for non-local) is simpler and slower; `bench/c/refcount_bench.c` is where it gets measured (Open).

## Growable containers

A container that can relocate its storage (arrays, builders, map tables) is reached through a non-moving header, and references into it are `(header, offset)`. Growing a shared container needs `writes`, reading it needs `reads`, so a raw address is valid exactly inside the block that holds it and a grow cannot happen under a reader (R1, R4, R7). An unshared container has no lock; its raw address is valid until the next call that may grow it or the next park. See spec 4.5 and [D15](architecture/open-decisions.md#d15-growable-arrays-relocation-and-stable-references).

## Open

- **Publishing a graph.** Setting the flag on everything reachable costs a walk at the publish. Alternatives: refuse to publish an object whose graph has local-only parts that are not themselves being published; or make `spawn` and message sends take only values whose reachable graph is already shared or immutable.
- **Where the lock word lives.** In every control block (costs bytes on every promoted object), or allocated at the first publish (costs an indirection).
- **Counts per instance.** A flag test in each count operation, or atomic always for non-local references. Needs `refcount_bench` on realistic access patterns.
- **Write-once across modules.** A field is write-once if no code in its defining module writes it. Public fields that other modules may write need either a mutability declaration or a rule that they are mutable by default.
- **Immutable types.** Inferred from R3 as above; whether to also let the programmer assert it (for a compile error if a later change breaks it) is undecided.
- **Runtime types that are not thread-safe.** R8 treats every `extern` type as self-synchronising. Whether an `extern` declaration may say otherwise (a handle that one vthread must own) is undecided.
- **Interfaces and function values.** A call through one needs a declared access mode per parameter, as it needs an effect bound. Default is "writes", the safe direction for the caller.
- **Lint.** A value read in one `sync (reads x)` block and used to decide a write in a separate `sync (writes x)` block on the same object (the get-then-put race of spec 7.2). A warning, not an error.
