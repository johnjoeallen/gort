#!/usr/bin/env bash
# Builds the feed server: gortc (cargo) and then the Gort program with feed.c and libsqlite3 linked in. Output: gort/feed/bin/feed (the generated feed.c lands beside it, so the binary must not share a directory with native.c)
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
mkdir -p "$root/gort/feed/bin"
(cd "$root/gortc" && cargo build --release -q)
"$root/gortc/target/release/gortc" build --link "$root/gortc/rt/posix.c" --link "$root/gort/feed/native.c" \
    --link -l:libsqlite3.so.0 -o "$root/gort/feed/bin/feed" "$root"/gort/runtime/*.gort "$root"/gort/feed/*.gort 2>/dev/null
