Median requests per second, `ab -k -c 64`, four workers, `-O2`.

| path | ref (plain) | --atomic-refs | plain / atomic |
|---|---|---|---|
| /health | 268374 | 264012 | 1.017 |
| /kv/a | 277614 | 285369 | 0.973 |
