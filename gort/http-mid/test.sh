#!/usr/bin/env bash
# http-mid/test.sh - builds http-mid with gortc, starts it, and checks request framing and the key-value routes over a real socket.
# Usage: gort/http-mid/test.sh [port]      (needs nc and curl; exits non-zero if any check fails)
set -u
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
port=${1:-8099}
bin=$(mktemp -d)/httpmid
(cd "$root/gortc" && cargo build --release -q && ./target/release/gortc build --link rt/posix.c -o "$bin" ../gort/runtime/*.gort ../gort/http-mid/*.gort) || { echo "build failed"; exit 2; }
"$bin" --port="$port" >"$bin.log" 2>&1 &
pid=$!
trap 'kill $pid 2>/dev/null' EXIT
for _ in $(seq 50); do nc -z 127.0.0.1 "$port" 2>/dev/null && break; sleep 0.1; done

pass=0; fail=0
# raw <request, with \r \n escapes> -> the whole response
raw() { printf '%b' "$1" | timeout 3 nc -q1 127.0.0.1 "$port"; }
# check <name> <response> <expected substring>...
check() {
    local name=$1 resp=$2; shift 2
    for want in "$@"; do
        if [[ $resp != *"$want"* ]]; then echo "FAIL $name: missing '$want'"; echo "$resp" | head -8 | sed 's/^/    /'; fail=$((fail+1)); return; fi
    done
    echo "ok   $name"; pass=$((pass+1))
}
# count <name> <response> <substring> <n>
count() {
    local n; n=$(grep -o -- "$3" <<<"$2" | wc -l)
    if [ "$n" -eq "$4" ]; then echo "ok   $1"; pass=$((pass+1)); else echo "FAIL $1: '$3' x$n, wanted $4"; fail=$((fail+1)); fi
}

H='Host: x\r\n'
check "GET /health"            "$(raw "GET /health HTTP/1.1\r\n${H}Connection: close\r\n\r\n")" "200 OK" "ok"
check "404"                    "$(raw "GET /nope HTTP/1.1\r\n${H}Connection: close\r\n\r\n")" "404"
check "405"                    "$(raw "DELETE /health HTTP/1.1\r\n${H}Connection: close\r\n\r\n")" "405"
check "400 bad request line"   "$(raw "garbage\r\n\r\n")" "400"

check "PUT content-length"     "$(raw "PUT /kv/cl HTTP/1.1\r\n${H}Content-Length: 3\r\nConnection: close\r\n\r\nabc")" "201"
check "GET it back"            "$(curl -s "localhost:$port/kv/cl")" "abc"
check "PUT replaces -> 200"    "$(raw "PUT /kv/cl HTTP/1.1\r\n${H}Content-Length: 1\r\nConnection: close\r\n\r\nz")" "200 OK"
check "DELETE"                 "$(raw "DELETE /kv/cl HTTP/1.1\r\n${H}Connection: close\r\n\r\n")" "204"

check "chunked PUT, extension and trailer" \
    "$(raw "PUT /kv/ch HTTP/1.1\r\n${H}Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhello\r\n6;ext=1\r\n world\r\n0\r\nX-T: 1\r\n\r\n")" "201"
check "chunked body stored"    "$(curl -s "localhost:$port/kv/ch")" "hello world"
check "chunked, upper-case hex size" \
    "$(raw "POST /echo HTTP/1.1\r\n${H}Transfer-Encoding: CHUNKED\r\nConnection: close\r\n\r\nA\r\n0123456789\r\n0\r\n\r\n")" "0123456789"

# a chunk split across two packets, then a pipelined request behind the body
resp=$( (printf "POST /echo HTTP/1.1\r\n${H}Transfer-Encoding: chunked\r\n\r\n3\r\nab"; sleep 0.3; printf "c\r\n0\r\n\r\nGET /health HTTP/1.1\r\nConnection: close\r\n\r\n") | timeout 3 nc -q1 127.0.0.1 "$port")
check "split chunk, echoed"    "$resp" "abc"
count "pipelined request answered" "$resp" "HTTP/1.1 200 OK" 2

check "chunked + content-length -> 400" "$(raw "POST /echo HTTP/1.1\r\nTransfer-Encoding: chunked\r\nContent-Length: 3\r\n\r\n0\r\n\r\n")" "400"
check "unknown transfer coding -> 400"  "$(raw "POST /echo HTTP/1.1\r\nTransfer-Encoding: gzip\r\n\r\n")" "400"
check "repeated content-length -> 400"  "$(raw "POST /echo HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\nx")" "400"
check "bad hex size -> 400"             "$(raw "POST /echo HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n")" "400"
check "missing CRLF after data -> 400"  "$(raw "POST /echo HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nabXX0\r\n\r\n")" "400"
check "chunk over maxBody -> 413"       "$(raw "POST /echo HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n200000\r\n")" "413"
check "content-length over maxBody -> 413" "$(raw "POST /echo HTTP/1.1\r\nContent-Length: 2000000\r\n\r\n")" "413"

# path parameters, percent-decoding, query parameters
check "param"                  "$(curl -s "localhost:$port/hello/world")" "hello, world"
check "param, percent-decoded" "$(curl -s "localhost:$port/hello/a%20b")" "hello, a b"
check "param, utf-8 escapes"   "$(curl -s "localhost:$port/hello/caf%C3%A9")" "hello, café"
check "query flag"             "$(curl -s "localhost:$port/hello/x?shout=1")" "hello, x!"
check "query flag, off"        "$(curl -s "localhost:$port/hello/x?shout=0")" "hello, x"
check "query, other names"     "$(curl -s "localhost:$port/hello/x?a=1&shout=yes&b=2")" "hello, x!"
check "query name, encoded"    "$(curl -s "localhost:$port/hello/x?sh%6Fut=1")" "hello, x!"
check "empty param -> 404"     "$(raw "GET /hello/ HTTP/1.1\r\n${H}Connection: close\r\n\r\n")" "404"
check "extra segment -> 404"   "$(raw "GET /hello/a/b HTTP/1.1\r\n${H}Connection: close\r\n\r\n")" "404"
check "405 on a pattern"       "$(raw "DELETE /hello/x HTTP/1.1\r\n${H}Connection: close\r\n\r\n")" "405"
check "bad escape -> 400"      "$(raw "GET /hello/%zz HTTP/1.1\r\n${H}Connection: close\r\n\r\n")" "400"
check "truncated escape -> 400" "$(raw "GET /hello/%2 HTTP/1.1\r\n${H}Connection: close\r\n\r\n")" "400"
check "kv key, encoded slash"  "$(raw "PUT /kv/a%2Fb HTTP/1.1\r\n${H}Content-Length: 1\r\nConnection: close\r\n\r\nv")" "201"
check "kv key read back"       "$(curl -s "localhost:$port/kv/a%2Fb")" "v"
check "kv key with a slash -> 404" "$(raw "GET /kv/a/b HTTP/1.1\r\n${H}Connection: close\r\n\r\n")" "404"
check "kv empty key -> 404"    "$(raw "GET /kv/ HTTP/1.1\r\n${H}Connection: close\r\n\r\n")" "404"

# a handler that throws: 500, and the connection stays usable
resp=$(raw "GET /boom HTTP/1.1\r\n${H}\r\nGET /boom?ok=1 HTTP/1.1\r\n${H}Connection: close\r\n\r\n")
check "handler throws -> 500, then the next request on the same connection" "$resp" "500 Internal Server Error" "internal error" "200 OK" "fine"
count "500 keeps the connection open" "$resp" "Connection: close" 1
check "500 is logged" "$(cat "$bin.log")" "GET /boom: the handler threw"

# Expect: 100-continue with the body not yet sent must be answered before the body arrives
resp=$( (printf "POST /echo HTTP/1.1\r\nExpect: 100-continue\r\nContent-Length: 2\r\nConnection: close\r\n\r\n"; sleep 0.3; printf "hi") | timeout 3 nc -q1 127.0.0.1 "$port")
check "100-continue then 200"  "$resp" "100 Continue" "200 OK" "hi"

n=$(head -c 300000 /dev/zero | tr '\0' x | curl -s -H 'Transfer-Encoding: chunked' -X POST --data-binary @- "localhost:$port/echo" | wc -c)
if [ "$n" -eq 300000 ]; then echo "ok   300 KB chunked through curl"; pass=$((pass+1)); else echo "FAIL 300 KB chunked: got $n bytes"; fail=$((fail+1)); fi

echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
