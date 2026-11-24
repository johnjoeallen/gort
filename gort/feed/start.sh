#!/usr/bin/env bash
# The challenge's environment (SPEC.md) mapped to the server's flags. Runs in the foreground.
exec "$(dirname "$0")/bin/feed" --port="${PORT:-3000}" --db="${SQLITE_PATH:?}" --secret="${JWT_SECRET:?}" "$@"
