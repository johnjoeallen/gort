<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/logo-dark.svg">
    <img alt="Gort" src="docs/assets/logo-light.svg" height="110">
  </picture>
</p>

# Gort

A Go-like language with a sprinkle of Java, for network servers and services: plain sequential code, virtual threads, classes and exceptions, no tracing garbage collector (compiler-managed lifetimes and deterministic cleanup), compile-time-checked shared state, native output through C. It is not a systems language and does not compete with Rust or C/C++. The design is in `gort-design.md`.
Nothing here compiles Gort yet; the `.gort` files are the source as it would be written, and `c/generated/` holds the C a compiler would emit for them (written by hand and kept in step by the tests).

*Gort is also a small town in County Galway, in the west of Ireland. The name comes from the Irish word for a field.*

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
