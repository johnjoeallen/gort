# http-mini

The smallest slice of `gort/http` that is a working server: one connection at a time per worker, one route (`GET /` answers "hello"), everything else 404,
a request head that is too large answers 431. It is the first compile target (`gortc check runtime/*.gort http-mini/*.gort`). Everything it leaves out is
still in `gort/http`: routes with handlers and bodies, the chunked decoder, uploads, JSON, the pipelined loop.

`helpers.gort` declares the few runtime functions the code assumes (`leak`, `log`, `exit`, `spawnWorker`) and the one constant (`HELLO`).
