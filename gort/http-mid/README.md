# http-mid

`http-mini` grown into a small key-value server, written to compile with `gortc` as it is today. It adds, on the same connection loop:

- request parsing (`request.gort`): method (a `Method` enum; an unknown one answers 501), path, query, headers (`req.header("content-type") ?? ""`, any case, first wins), `Content-Length`, `Connection`, HTTP/1.0 against 1.1; a bad head answers 400;
- request bodies (`reader.gort`, `body.gort`): `Content-Length` and `Transfer-Encoding: chunked` are decoded as a stream through the same buffer (chunk
  extensions skipped, trailers dropped, a pipelined request behind the body left alone). A body above `maxBody` answers 413 (a chunk that would pass the limit
  is refused as its header arrives), bad framing or `chunked` together with `Content-Length` answers 400, a head above `maxHead` 431. `Expect: 100-continue`
  gets its interim answer when the body has not started to arrive;
- a route table (`router.gort`): a list of `Route { method, pattern, handler, next }` built once, from the last route to the first, and never changed, so
  every worker reads it without a lock. A handler is a function value that returns a `Reply` (`reply(200, "ok\n")`, `Reply.bytes(status, type, body)`, `.with("Location", ...)`) and `throws Exception`: a throw is logged and answers 500 on a connection that stays open. 404 for no path, 405 (with `Allow`) for a path with the wrong method, `OPTIONS` on a known path answers 204 with `Allow`. `HEAD` is answered by the `GET` handler with the body cut off;
- route patterns (`pattern.gort`): `/kv/:key` matches one non-empty segment, a final `*rest` matches everything left (slashes included). Matching walks the pattern
  and the path side by side and allocates nothing; a handler reads its parameters from `Params` (`p.get("key") ?? ""`), which cuts the
  segment out and percent-decodes it only when asked. `queryGet(req.query, "name")` does the same for the query string (`+` is a space). A `%` not followed
  by two hex digits, in the path or the query, answers 400 when the head is parsed, so decoding cannot fail later;
- two readers (`reader.gort`, `body.gort`): a route is `Handler.Memory(fn)` (the body is read into memory, at most `maxBody`) or `Handler.Upload(limit, fn)` (the body is
  spooled to an unnamed temporary file under the route's own limit, so memory does not grow with it; the handler gets an `Upload` to link into place). The server finds
  the route first and picks the reader from it. A 60 MB upload peaks at under 4 MB of memory;
- shared state (`store.gort`): a `shared Store` with a `shared Map<String, String>` and two counters, reached with `sync (reads|writes ...)`;
- keep-alive, with per-connection `ref` responses;
- concurrent pipelining (`pipeline.gort`, the default; `--sequential` selects the one-at-a-time loop of `server.gort`, which batches its answers into one `send`): requests that
  can overlap run as child vthreads of the connection and are answered in request order, as one write each time the connection runs (nothing is held for a timer).
  `depth` (16) requests are in flight at most. A request with a body, one to an unsafe method (anything but GET, HEAD, OPTIONS) and a route declared `Cost.Unbounded` are
  barriers: each starts only when nothing is in flight, and nothing starts beside it. A request with nothing in flight and nothing buffered behind it runs inline, with no
  spawn. The connection waits in `waitAny` for input or a finishing child, and leaves `scope { }` (which waits for every child, by an exception too) before it ends. A slot
  the child fills in is `shared`: a spawn is a vthread boundary, so the language requires it. Eight pipelined 200 ms sleeps take 200 ms instead of 1600;

| route | |
|---|---|
| `GET /`, `GET /health` | `hello`, `ok` |
| `GET /kv/:key` | 200 and the value, or 404 |
| `PUT /kv/:key` | the body becomes the value: 201 if new, 200 if replaced |
| `DELETE /kv/:key` | 204, or 404 |
| `POST /echo` | the body, back |
| `GET /hello/:name` | `hello, name`; with `?shout=1`, `hello, name!` |
| `PUT /files/:name` | an **upload**: the body (up to 64 MB, chunked or not) is spooled to an unnamed temporary file as it arrives, then linked into the upload directory under `name`: 201 if new, 200 if replaced |
| `GET /files/:name`, `DELETE /files/:name` | the file (read into memory, up to 16 MB), or 404; 204 or 404 |
| `POST /users` | `application/json` parsed with `json.parse<User>` (name, optional email and age): 201 with the user as JSON and a `Location`; 400 for bad JSON or a missing name, 415 for another content type |
| `GET /users/:id` | the user as JSON, or 404 |
| `GET /sleep/:ms` | parks for `ms` (at most 5000): what overlapping looks like |
| `GET /spin/:ms` | busy for `ms` without parking; declared `Cost.Unbounded`, so it runs alone |
| `GET /stats` | key count, requests, writes |

What is `shared` and what is `ref`: the `Store`, its `Map`, the `Route` list and the `ServerConfig` are `shared` (every worker and connection reaches them); each
`Resp` is a `ref` (built and sent by one connection). A `Resp` built per request is one allocation with a plain count and no lock.

## Running it

```
cd gortc && cargo build --release
./target/release/gortc build --link rt/posix.c -o /tmp/httpmid ../gort/runtime/*.gort ../gort/http-mid/*.gort
/tmp/httpmid --port=8080 --workers=4 --uploads=/var/tmp &
curl -X PUT -d value localhost:8080/kv/a; curl localhost:8080/kv/a; curl localhost:8080/stats
```

`gortc build --atomic-refs ...` builds the same program with every `ref` made atomic, to measure what the plain layout buys.

## Testing

`gort/http-mid/test.sh [port]` builds the server, starts it and checks routes, `Content-Length` and chunked bodies (extensions, trailers, split and
pipelined requests, every 400 and 413 case, the number of writes a pipelined burst takes, counted with `sendcount.c` when `gcc` is there; `pipeline_test.py` checks overlap, order, barriers, depth, errors and clients that leave with requests in flight). `HTTPMID_ARGS=--sequential gort/http-mid/test.sh` runs the same suite against the baseline loop and `Expect: 100-continue` over a real socket. It needs `nc` and `curl`.
