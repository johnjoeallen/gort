#!/usr/bin/env bash
# http-mid/test.sh - builds http-mid with gortc, starts it, and checks request framing and the key-value routes over a real socket.
# Usage: gort/http-mid/test.sh [port]      (needs nc and curl; exits non-zero if any check fails)
set -u
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
port=${1:-8099}
bin=$(mktemp -d)/httpmid
(cd "$root/gortc" && cargo build --release -q && ./target/release/gortc build --link rt/posix.c -o "$bin" ../gort/runtime/*.gort ../gort/http-mid/*.gort) || { echo "build failed"; exit 2; }
uploads="$bin.uploads"; mkdir -p "$uploads"
"$bin" --port="$port" --uploads="$uploads" >"$bin.log" 2>&1 &
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

# methods, HEAD, request headers
check "HEAD has the GET headers, no body" "$(raw "HEAD /hello/zed HTTP/1.1\r\n${H}Connection: close\r\n\r\n")" "200 OK" "Content-Length: 11"
resp=$(raw "HEAD /hello/zed HTTP/1.1\r\n${H}Connection: close\r\n\r\n")
if [[ $resp == *"hello, zed"* ]]; then echo "FAIL HEAD sent a body"; fail=$((fail+1)); else echo "ok   HEAD sends no body"; pass=$((pass+1)); fi
resp=$(raw "HEAD /health HTTP/1.1\r\n${H}\r\nGET /health HTTP/1.1\r\n${H}Connection: close\r\n\r\n")
count "HEAD then GET on one connection" "$resp" "HTTP/1.1 200 OK" 2
check "HEAD of a missing path -> 404" "$(raw "HEAD /nope HTTP/1.1\r\n${H}Connection: close\r\n\r\n")" "404"
check "HEAD of a POST-only path -> 405" "$(raw "HEAD /echo HTTP/1.1\r\n${H}Connection: close\r\n\r\n")" "405"
check "unknown method -> 501" "$(raw "BREW /pot HTTP/1.1\r\n${H}Connection: close\r\n\r\n")" "501"
check "lower-case method -> 501" "$(raw "get / HTTP/1.1\r\n${H}Connection: close\r\n\r\n")" "501"
check "request Content-Type reflected" "$(raw "POST /echo HTTP/1.1\r\n${H}content-TYPE:  application/json \r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")" "Content-Type: application/json"
check "no Content-Type -> default" "$(raw "POST /echo HTTP/1.1\r\n${H}Content-Length: 2\r\nConnection: close\r\n\r\nab")" "Content-Type: application/octet-stream"

# the response builder: extra headers, Allow, OPTIONS, 204
check "201 carries Location"        "$(raw "PUT /kv/loc HTTP/1.1\r\n${H}Content-Length: 1\r\nConnection: close\r\n\r\nv")" "201 Created" "Location: /kv/loc"
check "Location is percent-encoded" "$(raw "PUT /kv/a%20b%2Fc%C3%A9 HTTP/1.1\r\n${H}Content-Length: 1\r\nConnection: close\r\n\r\nv")" "Location: /kv/a%20b%2Fc%C3%A9"
check "405 lists what is allowed"   "$(raw "POST /kv/x HTTP/1.1\r\n${H}Content-Length: 0\r\nConnection: close\r\n\r\n")" "405" "Allow: GET, HEAD, PUT, DELETE, OPTIONS"
check "405 on a GET-only path"      "$(raw "POST /health HTTP/1.1\r\n${H}Content-Length: 0\r\nConnection: close\r\n\r\n")" "Allow: GET, HEAD, OPTIONS"
check "OPTIONS -> 204 and Allow"    "$(raw "OPTIONS /kv/x HTTP/1.1\r\n${H}Connection: close\r\n\r\n")" "204 No Content" "Allow: GET, HEAD, PUT, DELETE, OPTIONS"
resp=$(raw "OPTIONS /kv/x HTTP/1.1\r\n${H}Connection: close\r\n\r\n")
if [[ $resp == *"Content-Length"* || $resp == *"Content-Type"* ]]; then echo "FAIL 204 has body headers"; fail=$((fail+1)); else echo "ok   204 has no Content-Length or Content-Type"; pass=$((pass+1)); fi
check "OPTIONS on an unknown path -> 404" "$(raw "OPTIONS /nope HTTP/1.1\r\n${H}Connection: close\r\n\r\n")" "404"
check "DELETE answers 204 the same way" "$(raw "DELETE /kv/loc HTTP/1.1\r\n${H}Connection: close\r\n\r\n")" "204 No Content"
resp=$(raw "HEAD /kv/missing HTTP/1.1\r\n${H}Connection: close\r\n\r\n")
check "HEAD 404 keeps Content-Length" "$resp" "404" "Content-Length: 12"
if [[ $resp == *"no such key"* ]]; then echo "FAIL HEAD 404 sent a body"; fail=$((fail+1)); else echo "ok   HEAD 404 sends no body"; pass=$((pass+1)); fi

# pipelining: answers come back in request order, and a burst takes far fewer writes than requests
resp=$(raw "PUT /kv/p HTTP/1.1\r\n${H}Content-Length: 1\r\n\r\n1GET /kv/p HTTP/1.1\r\n${H}\r\nPUT /kv/p HTTP/1.1\r\n${H}Content-Length: 1\r\n\r\n2GET /kv/p HTTP/1.1\r\n${H}Connection: close\r\n\r\n")
order=$(grep -o "created\|replaced\|^[12]$" <<<"$resp" | tr '\n' ' ')
if [ "$order" = "created 1 replaced 2 " ]; then echo "ok   pipelined PUT/GET answered in order"; pass=$((pass+1)); else echo "FAIL pipelined order: '$order'"; fail=$((fail+1)); fi
burst="$bin.burst"
{ for _ in $(seq 40); do printf "GET /health HTTP/1.1\r\nHost: x\r\n\r\n"; done; printf "GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"; } >"$burst"
n=$(timeout 5 nc -q1 127.0.0.1 "$port" <"$burst" | grep -c "HTTP/1.1 200")
if [ "$n" -eq 41 ]; then echo "ok   41 pipelined requests, 41 answers"; pass=$((pass+1)); else echo "FAIL pipelined burst: $n answers"; fail=$((fail+1)); fi
if command -v gcc >/dev/null && gcc -shared -fPIC -o "$bin.shim.so" "$here/sendcount.c" -ldl 2>/dev/null; then
    LD_PRELOAD="$bin.shim.so" "$bin" --port=$((port+1)) >"$bin.sends" 2>&1 &
    spid=$!
    for _ in $(seq 50); do nc -z 127.0.0.1 $((port+1)) 2>/dev/null && break; sleep 0.1; done
    timeout 5 nc -q1 127.0.0.1 $((port+1)) <"$burst" >/dev/null
    sleep 0.3; kill $spid 2>/dev/null
    sends=$(grep -c '^SEND' "$bin.sends")
    if [ "$sends" -ge 1 ] && [ "$sends" -le 10 ]; then echo "ok   41 pipelined answers took $sends writes"; pass=$((pass+1)); else echo "FAIL batching: $sends writes for 41 requests"; fail=$((fail+1)); fi
else
    echo "skip write counting (no gcc)"
fi

# a handler that throws: 500, and the connection stays usable
resp=$(raw "GET /boom HTTP/1.1\r\n${H}\r\nGET /boom?ok=1 HTTP/1.1\r\n${H}Connection: close\r\n\r\n")
check "handler throws -> 500, then the next request on the same connection" "$resp" "500 Internal Server Error" "internal error" "200 OK" "fine"
count "500 keeps the connection open" "$resp" "Connection: close" 1
check "500 is logged" "$(cat "$bin.log")" "GET /boom: the handler threw"

# uploads: bodies above maxBody are spooled to a file, under the route's own limit
check "upload, small"          "$(curl -s -i -X PUT --data-binary 'file body' "localhost:$port/files/a.txt")" "201 Created" "stored 9 bytes" "Location: /files/a.txt"
check "download it"            "$(curl -s "localhost:$port/files/a.txt")" "file body"
check "upload replaces"        "$(curl -s -i -X PUT --data-binary 'new' "localhost:$port/files/a.txt")" "200 OK" "replaced 3 bytes"
check "upload, empty body"     "$(curl -s -i -X PUT "localhost:$port/files/empty")" "201 Created" "stored 0 bytes"
head -c 3000000 /dev/urandom > "$bin.big"
check "upload 3 MB (over maxBody), Content-Length" "$(curl -s -X PUT --data-binary @"$bin.big" "localhost:$port/files/big")" "stored 3000000 bytes"
if cmp -s "$bin.big" "$uploads/big"; then echo "ok   the 3 MB file on disk is identical"; pass=$((pass+1)); else echo "FAIL the stored file differs"; fail=$((fail+1)); fi
curl -s "localhost:$port/files/big" -o "$bin.back"
if cmp -s "$bin.big" "$bin.back"; then echo "ok   downloading it returns the same bytes"; pass=$((pass+1)); else echo "FAIL the download differs"; fail=$((fail+1)); fi
check "upload 3 MB chunked"    "$(curl -s -X PUT -H 'Transfer-Encoding: chunked' --data-binary @"$bin.big" "localhost:$port/files/bigc")" "stored 3000000 bytes"
check "upload over the route limit -> 413" "$(raw "PUT /files/huge HTTP/1.1\r\n${H}Content-Length: 70000000\r\n\r\n")" "413"
check "the same size to an ordinary route -> 413" "$(raw "POST /echo HTTP/1.1\r\n${H}Content-Length: 2000000\r\n\r\n")" "413"
check "hidden file name -> 400" "$(raw "PUT /files/.secret HTTP/1.1\r\n${H}Content-Length: 1\r\nConnection: close\r\n\r\nx")" "400"
check "slash in file name -> 400" "$(raw "PUT /files/a%2Fb HTTP/1.1\r\n${H}Content-Length: 1\r\nConnection: close\r\n\r\nx")" "400"
if [ -e "$uploads/.secret" ] || [ -e "$uploads/a/b" ]; then echo "FAIL a bad name made a file"; fail=$((fail+1)); else echo "ok   no file was made for a bad name"; pass=$((pass+1)); fi
check "download of a missing file -> 404" "$(raw "GET /files/nope HTTP/1.1\r\n${H}Connection: close\r\n\r\n")" "404"
check "delete"                 "$(raw "DELETE /files/a.txt HTTP/1.1\r\n${H}Connection: close\r\n\r\n")" "204"
check "delete again -> 404"    "$(raw "DELETE /files/a.txt HTTP/1.1\r\n${H}Connection: close\r\n\r\n")" "404"
leftover=$(ls -A "$uploads" | grep -c "\.tmp$")
if [ "$leftover" -eq 0 ]; then echo "ok   no temporary names left behind"; pass=$((pass+1)); else echo "FAIL $leftover .tmp files left"; fail=$((fail+1)); fi
check "an upload on a keep-alive connection, then a GET" \
    "$(raw "PUT /files/k HTTP/1.1\r\n${H}Content-Length: 4\r\n\r\nabcdGET /files/k HTTP/1.1\r\n${H}Connection: close\r\n\r\n")" "stored 4 bytes" "abcd"

# JSON: parsed into a struct, answered as JSON
J='Content-Type: application/json'
resp=$(curl -s -i -X POST -H "$J" -d '{"name":"Ann","age":30,"extra":[1,{"a":null}]}' "localhost:$port/users")
check "POST /users"            "$resp" "201 Created" "Content-Type: application/json" "Location: /users/" '{"id":' '"name":"Ann"' '"age":30'
id=$(grep -o '"id":[0-9]*' <<<"$resp" | cut -d: -f2)
check "GET /users/:id"         "$(curl -s "localhost:$port/users/$id")" '"name":"Ann"'
check "optional field absent"  "$(curl -s -X POST -H "$J" -d '{"name":"Bo"}' "localhost:$port/users")" '"name":"Bo"}'
check "email and escapes"      "$(curl -s -X POST -H "$J" -d '{"name":"Q\"uote\\n","email":"a@b.c"}' "localhost:$port/users")" '"name":"Q\"uote\\n"' '"email":"a@b.c"'
check "unicode escape decoded" "$(curl -s -X POST -H "$J" -d '{"name":"Al\u00e9"}' "localhost:$port/users")" 'Alé'
check "ids differ"             "$(curl -s -X POST -H "$J" -d '{"name":"C"}' "localhost:$port/users")" '"id":'"$((id+4))"
check "invalid JSON -> 400"    "$(curl -s -i -X POST -H "$J" -d '{"name":' "localhost:$port/users")" "400" "invalid JSON"
check "wrong type -> 400"      "$(curl -s -i -X POST -H "$J" -d '{"name":5}' "localhost:$port/users")" "400"
check "missing name -> 400"    "$(curl -s -i -X POST -H "$J" -d '{"age":5}' "localhost:$port/users")" "400"
check "empty name -> 400"      "$(curl -s -i -X POST -H "$J" -d '{"name":""}' "localhost:$port/users")" "400" "needs a name"
check "age out of range -> 400" "$(curl -s -i -X POST -H "$J" -d '{"name":"x","age":70000}' "localhost:$port/users")" "400"
check "deeply nested -> 400"   "$(curl -s -i -X POST -H "$J" -d '{"name":"x","a":[[[[[[[[[[[1]]]]]]]]]]]}' "localhost:$port/users")" "400"
check "no content type -> 415" "$(curl -s -i -X POST -d '{"name":"x"}' -H 'Content-Type:' "localhost:$port/users")" "415"
check "user chunked"           "$(curl -s -X POST -H "$J" -H 'Transfer-Encoding: chunked' -d '{"name":"Chunky"}' "localhost:$port/users")" '"name":"Chunky"'
check "unknown user -> 404"    "$(curl -s -i "localhost:$port/users/99999")" "404"
check "non-numeric user -> 404" "$(curl -s -i "localhost:$port/users/abc")" "404"

# Expect: 100-continue with the body not yet sent must be answered before the body arrives
resp=$( (printf "POST /echo HTTP/1.1\r\nExpect: 100-continue\r\nContent-Length: 2\r\nConnection: close\r\n\r\n"; sleep 0.3; printf "hi") | timeout 3 nc -q1 127.0.0.1 "$port")
check "100-continue then 200"  "$resp" "100 Continue" "200 OK" "hi"

n=$(head -c 300000 /dev/zero | tr '\0' x | curl -s -H 'Transfer-Encoding: chunked' -X POST --data-binary @- "localhost:$port/echo" | wc -c)
if [ "$n" -eq 300000 ]; then echo "ok   300 KB chunked through curl"; pass=$((pass+1)); else echo "FAIL 300 KB chunked: got $n bytes"; fail=$((fail+1)); fi

echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
