<p align="center">
  <img alt="Gort" src="assets/logo-light.svg#only-light" height="110">
  <img alt="Gort" src="assets/logo-dark.svg#only-dark" height="110">
</p>

# Gort

*Gort is also a small town in County Galway, in the west of Ireland. The name comes from the Irish word for a field.*

Gort is a design for a Go-like language with a sprinkle of Java, aimed at network servers and services. From Go: a small language, plain sequential code, cheap concurrent tasks with blocking-style I/O, little ceremony. From Java: classes, interfaces, exceptions, structured concurrency, packages that follow folders. Beyond both: no tracing garbage collector (compiler-managed lifetimes, deterministic cleanup) and compile-time-checked shared state, which neither Go nor Java has. It is not a systems language: it has a runtime, and it does not compete with Rust or C/C++. There are no lifetime annotations.

!!! note "Status"
    Exploratory. There is no compiler yet. The `.gort` files in the repository are Gort as it would be written, and the C beside them is what a compiler would emit, written by hand and kept honest by tests and benchmarks.

## What this site is

A record of how the design got where it is: what was measured, what was decided and why, and what turned out to be wrong. Each step started as a question and ended as a number or a rule.

[Start the journey](journey/index.md){ .md-button .md-button--primary }
[Decisions so far](decisions.md){ .md-button }

## The design in six lines

1. Ordinary values carry no reference-count header; `new` makes a strong `Ref<T>`, and `WeakRef<T>` breaks cycles.
2. Lifetimes are inferred, not written.
3. Shared state is accessed through `sync` (read) and `mutate` (write) blocks; entering one may park the virtual thread.
4. Nothing is hidden except virtual threads and parking: no implicit buffering, batching or retry.
5. Every unbounded input has a declared limit, enforced before the data is held.
6. Errors are exceptions declared with `throws`; cleanup never parks and never throws.

The full text is [`gort-design.md`](https://github.com/johnjoeallen/gort/blob/main/gort-design.md).
