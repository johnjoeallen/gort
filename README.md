<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/logo-dark.svg">
    <img alt="Gort" src="docs/assets/logo-light.svg" height="200">
  </picture>
</p>

# Gort

**A small compiled language for network servers: C++-style value types, Go-style embedding, reference counting built into the language, and no garbage collector.**

Gort is a design for a small, statically typed language for network servers and services. Types are plain values, as in C++: a struct is stored inline and copied by value. Reuse is by composition, as in Go: a struct embeds other structs with `with` (their fields and functions are promoted) and satisfies interfaces with `:`, with no class inheritance. Sharing is explicit and counted: `Ref<T>`, `Array<T>` and `String` are language elements with built-in reference counts, so there is no tracing garbage collector and cleanup is deterministic. Concurrency is plain blocking-style code on virtual threads, and the compiler checks that shared mutable state is only touched under `sync` or `mutate`. It compiles to C. Nothing is implemented yet.

*Gort is also a small town in County Galway, in the west of Ireland. The name comes from the Irish word for a field.*

> [!NOTE]
> **Status.** Exploratory. The compiler has only started: `gortc/` compiles a small subset (the memory model, no parking yet). The `.gort` files in the repository are Gort as it would be written, and the C beside them is what a compiler would emit, written by hand and kept honest by tests and benchmarks.

## What this is

A record of how the design got where it is: what was measured, what was decided and why, and what turned out to be wrong. Each step started as a question and ended as a number or a rule.

[Start the journey](docs/journey/index.md) · [Decisions so far](docs/decisions.md) · [The full design](gort-design.md) · [The docs site](https://johnjoeallen.github.io/gort/)

## The design in six lines

1. Plain values carry no reference-count header; a `Ref<T>` is a counted handle to an object made by `new`, and `WeakRef<T>` breaks cycles.
2. Lifetimes are inferred, not written.
3. Shared state is accessed through `sync` (read) and `mutate` (write) blocks; entering one may park the virtual thread.
4. Nothing is hidden except virtual threads and parking: no implicit buffering, batching or retry.
5. Every unbounded input has a declared limit, enforced before the data is held.
6. Errors are exceptions declared with `throws`; cleanup never parks and never throws.

## The repository

```
gort-design.md     the design specification
gort/                Gort source only
  runtime/             the runtime's interface and the parts writable in Gort
  http/                HTTP library and server.gort; styles/{cpp,java,mixed}/server.gort are the same server in three syntax styles
  redis/               mini-redis library and server.gort
  experiments/         handler classification study (older syntax)
c/                   C only
  runtime/             hand-written runtime: scheduler, buffers, I/O, sockets, locks (rt*.h)
  generated/           stand-ins for compiler output: http/, redis/ (each has server.c)
bench/               everything that measures or tests
  http/  redis/        build scripts, tests, load generators for each server
  c/ rust/ go/ java/   micro-benchmarks and comparators
  results/             recorded measurements and write-ups
tools/               gort2html.py (render a .gort file as HTML), camel.py (rename snake_case identifiers)
```

Naming: Gort identifiers (functions, methods, fields, parameters, locals) are camelCase; constants stay UPPER_CASE. The C may use underscores.
To run a server's tests: `cd bench/http && ./build.sh && ./run_tests.sh` (same for `bench/redis`).
