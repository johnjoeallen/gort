# 4. Measured Against Go, Java and Rust

Gort is positioned as a small compiled language for servers (see [Why](01-why.md)), so Go and Java are the comparison that sets the bar and Rust (Tokio) is a reference ceiling. This chapter has what was measured: the same design written in Rust and Go (a pipelined server, and plain `net/http`), driven by the same tests and load generator, and a Java virtual-thread microbenchmark. The Gort column is hand-written C that follows the design, not output of a Gort compiler, which does not exist yet.

| Pipelined, 1 worker | requests/s |
|---|---|
| Gort-style C | 4.05M |
| Rust (Tokio) | 2.1M |
| Go | 2.12M |

| Idle memory per connection | bytes |
|---|---|
| Gort-style C | 89 to 192 |
| Rust | about 1,400 |
| Go | 4,000 to 5,000 |
| Go `net/http` | 33,000 |

## Java virtual threads

There is no Java HTTP server in this comparison yet. What exists is a microbenchmark of idle units and yields on one scheduler thread (OpenJDK 25, [group 1](https://github.com/johnjoeallen/gort/blob/main/bench/results/group1.md)):

| Measure | Java virtual thread | Tokio and C fibers, same benchmark (no Go figure) |
|---|---|---|
| Idle unit, nothing held | 809 bytes of heap | Tokio task 395; pooled C fiber 4,113 |
| Idle unit, 4 KB held | 4,848 bytes of heap (10,401 resident, with touched garbage) | Tokio 4,491; C fiber 8,209 |
| Yield and resume | 530 to 670 ns (an upper bound) | Tokio about 33 ns; C fiber 13 to 39 ns |

Java is the closest model to Gort's (blocking-style code on cheap threads), so it is the comparison that matters most, and it is the one with the least evidence. The microbenchmark is not a request benchmark: the Java yield figure includes a carrier hand-off and the numbers say nothing about throughput or latency under load. A Java and a Go server written for the [evaluation plan](../architecture/benchmarks.md), with tuned settings, are still to do.

## How to read it

!!! warning "Caveats"
    Real handlers do real work, which shrinks the throughput gap a lot. Go's runtime is the model Gort is closest to in ceremony, and it is the comparison to watch. What survives is memory per connection, in-connection concurrency, the timer floor and Go's garbage-collection tail. All runs are single runs on loopback.

Browsers do not pipeline HTTP/1.1 (they multiplex over HTTP/2), so the pipelined number says more about the runtime's cost per request than about web traffic.
