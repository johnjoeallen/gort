# gortc, slice 0

The first piece of the Gort compiler: a Rust front end (lexer, parser, name resolution and type checking, lowering to a
typed tree) and a C emitter. It compiles a small subset of Gort to one C file and runs it through `cc`.

Slice 0 exists to test the **memory model** (D18) on real generated code before anything harder is built on it. It has no
parking, no frames and no scheduler; those are slice 1 and later. The roadmap is in
[`docs/architecture/compiler.md`](../docs/architecture/compiler.md).

```
cargo build --release
gortc run   [--notes] [--sanitize] FILE.gort...     # build and run
gortc build -o OUT [--sanitize] [--keep-c] FILE...  # compile to an executable
gortc emit  FILE.gort...                            # print the C
cargo test                                          # every program under tests/, under ASan and UBSan
```

`--notes` prints, for each `let a = new T {..}`, whether `a` is **scope-owned** (a bare body in the C frame) or
**counted** (a `Ref<T>` from allocation) and which line made it counted (D18 item 5).

## What it compiles

Packages (one per run; the files must agree on the `package` line), `const` scalars, `fn`, `struct` and `enum` (scalar payloads) with fields,
defaults, inline functions, `static fn`, `cleanup()`, and out-of-line `fn T::name` for a function declared in the struct.
Types `bool`, the integer types, `f64`, `String`, structs (values), `Ref<T>`, `Array<scalar>`. Statements `let`/`var` (also
`Type name = e`), assignment and `+=`, `++`, `if`/`else`, `while`, `loop`, `break`, `continue`, `return`, `throw`, `try`/`catch`, optionals (`T?`, `null`, `== null`), `c ? a : b`, top-level `let`, `switch` (enum variants with bindings, `default`, integer and character literals). Expressions
`a[lo..hi]` slices (views), `Enum.Variant(..)`, `new T {..}`, `T {..}`, `new u8[n]`, `as`, string `+` and `==`, indexing with bounds checks, `.length`. Builtins
`print`, `println`, `assert`, `str`.

## What it enforces

- **Values and handles.** A struct without `Ref` is a value, copied field by field; handle fields take a count and the
  copy drops it. `Ref<T> = Point` needs an explicit `new`.
- **Counted or scope-owned, per `new` site.** Decided from the function's own text (any conversion to a `Ref` on any path
  makes the site counted). Nothing moves after allocation.
- **Deterministic drops.** Locals drop in reverse order on every exit path (`return`, `break`, `continue`, scope end).
  Temporaries die at the end of their statement, or of the operand of `&&`/`||` they belong to. Arguments are copies the
  callee owns.
- **No copy of a value with `cleanup()`** (it would run twice), directly or inline.
- `let` bindings and parameters are immutable, including fields of a struct value.
- Bounds, division by zero and assertions trap with exit code 101. A run that ends with a counted object live fails with
  exit code 70: the generated `main` checks it.

## Layout

| File | Role |
|---|---|
| `src/lexer.rs`, `src/parser.rs`, `src/ast.rs` | source to syntax tree. Statements end at `;`, `}` or a line break |
| `src/sema.rs`, `src/hir.rs` | resolution, types, conversions as explicit nodes, the counted/scope-owned decision |
| `src/codegen.rs` | ownership-aware C emission |
| `src/gort_rt0.h` | the runtime the C uses; embedded in every output |
| `tests/ok`, `tests/err`, `tests/trap` | programs with expected output, expected rejections, expected traps |

`gort_rt0.h` reaches a counted object's control block only through `GORT_CTL`, `gort_new`, `gort_retain`,
`gort_release_last` and `gort_free`. Slice 0 keeps a 16-byte header before the body; the layout D18 prefers (a control
section in the chunk, benchmark B18) can replace it in that one file.

## Not in slice 0

Flow narrowing of optionals (a checked unwrap stands in), enum payloads that are not scalars, rethrow of a caught value, generics, interfaces and embedding (D16), closures, `WeakRef`, `sync` and
`mutate`, `spawn`, `extern`, imports other than a name, growable arrays, slices, packages split across
folders. Each is rejected with a message that says so. Running the existing samples in `gort/` shows the first
unsupported feature each one meets: optionals, enums, exceptions, top-level `let`, `extern`.
