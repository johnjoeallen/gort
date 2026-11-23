# http-mid

`http-mini` grown into a small key-value server, written to compile with `gortc` as it is today. It adds, on the same connection loop:

- request parsing (`request.gort`): method, path, query, `Content-Length`, `Connection`, HTTP/1.0 against 1.1; a bad head answers 400;
- request bodies (`reader.gort`): `Content-Length` bytes are read through the same buffer, a body above `maxBody` answers 413, a head above `maxHead` 431;
- a route table (`router.gort`): a list of `Route { method, path, exact, handler, next }` built once, from the last route to the first, and never changed, so
  every worker reads it without a lock. A handler is a function value. 404 for no path, 405 for a path with the wrong method;
- shared state (`store.gort`): a `shared Store` with a `shared Map<String, String>` and two counters, reached with `sync (reads|writes ...)`;
- keep-alive, with per-connection `ref` responses.

| route | |
|---|---|
| `GET /`, `GET /health` | `hello`, `ok` |
| `GET /kv/:key` | 200 and the value, or 404 |
| `PUT /kv/:key` | the body becomes the value: 201 if new, 200 if replaced |
| `DELETE /kv/:key` | 204, or 404 |
| `POST /echo` | the body, back |
| `GET /stats` | key count, requests, writes |

What is `shared` and what is `ref`: the `Store`, its `Map`, the `Route` list and the `ServerConfig` are `shared` (every worker and connection reaches them); each
`Resp` is a `ref` (built and sent by one connection). A `Resp` built per request is one allocation with a plain count and no lock.

## Running it

```
cd gortc && cargo build --release
./target/release/gortc build --link rt/posix.c -o /tmp/httpmid ../gort/runtime/*.gort ../gort/http-mid/*.gort
/tmp/httpmid --port=8080 --workers=4 &
curl -X PUT -d value localhost:8080/kv/a; curl localhost:8080/kv/a; curl localhost:8080/stats
```

`gortc build --atomic-refs ...` builds the same program with every `ref` made atomic, to measure what the plain layout buys.
