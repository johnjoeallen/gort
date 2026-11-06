# http-mini

The smallest slice of `gort/http` that is a working server: one connection at a time per worker, one route (`GET /` answers "hello"), everything else 404,
a request head that is too large answers 431. It is the first compile target (`gortc check runtime/*.gort http-mini/*.gort`). Everything it leaves out is
still in `gort/http`: routes with handlers and bodies, the chunked decoder, uploads, JSON, the pipelined loop.

`helpers.gort` declares the few runtime functions the code assumes (`leak`, `log`, `exit`, `spawnWorker`) and the one constant (`HELLO`).

## Running it

```
cd gortc && cargo build
./target/debug/gortc build --link rt/posix.c -o /tmp/httpmini ../gort/runtime/*.gort ../gort/http-mini/*.gort
/tmp/httpmini --port=8080 &
curl -i http://127.0.0.1:8080/        # 200 hello
curl -i http://127.0.0.1:8080/nope    # 404
```

`gortc/rt/posix.c` is the C side of the `extern` declarations: blocking sockets, one thread per worker, and `spawn call` is `spawnTask(fn() { call })`,
one OS thread per connection (vthreads and parking are still to come). Reference counts are atomic, so `--workers=N` is fine.
