# 6. Locks that park

## The rule

Entering a `sync` block may park the virtual thread. A holder never parks inside the block (a compile error), so two vthreads on one worker never contend and hold time is bounded by the code in the block.

Several objects in one head, as in `sync (writes a, b)`, were first designed as all-or-nothing: if one is busy, release everything, park on that lock's FIFO queue and retry, keeping the arrival ticket. The compiler does it more simply: it locks the objects one at a time in address order, so two threads naming the same objects in different orders cannot deadlock. Only the first object's wait can park; a wait for a later one, made while earlier ones are held, blocks the thread (a vthread must not yield while holding a lock).

Other things changed once the compiler existed: the lock is re-entrant within a call tree (the thread cannot park while it holds the lock, so a nested block on an object it already holds takes nothing and releases nothing), a `writes` on an object held only for `reads` upgrades in place, and nesting a block on a different object is a warning that suggests naming both in one head, not an error. See `docs/architecture/compiler.md`.

## Wake and retry, and a bound on overtaking

The first parking lock handed the lock directly to the head waiter. That made a lock owner out of a vthread that might sit in a ready queue behind a busy worker, and every other worker stalled on it (one run was 30 times slower). The lock now does wake-and-retry: release wakes the head, which takes the lock itself when it runs, and a newcomer may take a free lock first. The cost is that a queued waiter can be passed. To bound that, a waiter that has waited over 1 ms sets a priority bit; newcomers then queue behind it, the holders drain and it takes the lock. A test with three overlapping readers and a writer shows it: without the bit the writer traps after 5 s, with it the run completes.

## The ordering key

The order is by the address of each object's lock word, or `(arena id, slot index)` for arena objects whose storage can move. A per-allocation sequence number was rejected for its cost. Arenas never compact, and a chunk is freed only when empty, so a live object's address never changes. Only types that appear in a `sync` head carry a lock word.

## The measurement that changed the contract's implementation

With FIFO fairness, one waiting writer makes every later reader queue behind it, and each handoff is a cross-thread wake. Throughput at 4 workers, one lock, 64 connections:

| GET share | park only | spin then park |
|---|---|---|
| 100% | 1.29M | 1.29M |
| 90% | 570k | 1.28M |
| 50% | 330k | 1.27M |
| 0% | 318k | 1.27M |

A 10% write mix lost 55% of throughput. Spinning 2000 rounds before parking removed the loss. The contract is unchanged (entering still "may park"); only the cost differs. A 100-round spin was noisy, from 1.0M to 326k at 50% reads.

