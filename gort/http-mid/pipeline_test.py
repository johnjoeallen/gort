#!/usr/bin/env python3
"""pipeline_test.py PORT - checks the concurrent pipelined loop of http-mid over real sockets: overlap, order, barriers, depth, errors.
Prints one line per check ("ok   name" or "FAIL name: why"); exits non-zero if any failed. Called by test.sh."""
import socket, sys, time

port = int(sys.argv[1])
failed = 0

def get(path, close=False, host="x"):
    return f"GET {path} HTTP/1.1\r\nHost: {host}\r\n" + ("Connection: close\r\n" if close else "") + "\r\n"

def run(requests, hold=0.0, want_close=True):
    """Sends all requests at once on one connection; returns (elapsed ms, raw response). Reads until the server closes (or `hold` seconds pass)."""
    s = socket.create_connection(("127.0.0.1", port))
    t = time.time()
    s.sendall("".join(requests).encode())
    s.settimeout(5)
    buf = b""
    try:
        while True:
            d = s.recv(65536)
            if not d:
                break
            buf += d
    except socket.timeout:
        pass
    ms = (time.time() - t) * 1000
    s.close()
    return ms, buf.decode(errors="replace")

def bodies(raw):
    """The answers in order, as (status, body) pairs (a chunked body is decoded)."""
    out, i = [], 0
    while True:
        j = raw.find("HTTP/1.1 ", i)
        if j < 0:
            return out
        k = raw.find("\r\n\r\n", j)
        head = raw[j:k]
        status = int(head.split()[1])
        n, chunked = 0, False
        for line in head.split("\r\n"):
            if line.lower().startswith("content-length:"):
                n = int(line.split(":")[1])
            if line.lower().startswith("transfer-encoding:") and "chunked" in line.lower():
                chunked = True
        pos = k + 4
        if chunked:
            body = ""
            while True:
                e = raw.find("\r\n", pos)
                size = int(raw[pos:e], 16)
                if size == 0:
                    pos = e + 4          # the CR LF after the 0 line and the empty trailer
                    break
                body += raw[e + 2:e + 2 + size]
                pos = e + 2 + size + 2
            out.append((status, body))
            i = pos
        else:
            out.append((status, raw[pos:pos + n]))
            i = pos + n

def check(name, ok, why=""):
    global failed
    if ok:
        print(f"ok   {name}")
    else:
        failed += 1
        print(f"FAIL {name}: {why}")

# ---- overlap: parking handlers run side by side ----
ms, raw = run([get("/sleep/200")] * 8 + [get("/health", close=True)])
a = bodies(raw)
check("8 pipelined 200 ms sleeps overlap", len(a) == 9 and ms < 800, f"{len(a)} answers in {ms:.0f} ms (one after another would be 1600)")

ms, raw = run([get("/sleep/50")] * 40 + [get("/health", close=True)])
a = bodies(raw)
check("depth: 40 requests, 16 at a time", len(a) == 41 and ms < 700, f"{len(a)} answers in {ms:.0f} ms (sequential would be 2000)")

# ---- order: answers come back in request order, whichever finishes first ----
ms, raw = run([get("/sleep/300"), get("/sleep/10"), get("/health", close=True)])
a = bodies(raw)
check("answers keep request order", [b for _, b in a] == ["slept 300\n", "slept 10\n", "ok\n"], str(a))
check("the quick ones waited for the slow one, not the reverse", ms >= 280, f"{ms:.0f} ms")

# ---- barriers ----
run(["DELETE /kv/barrier HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"])
ms, raw = run([get("/sleep/100"), get("/kv/barrier"), "PUT /kv/barrier HTTP/1.1\r\nHost: x\r\nContent-Length: 1\r\n\r\n1", get("/kv/barrier", close=True)])
a = bodies(raw)
check("a write waits for the reads before it, and the reads after it see it",
      [s for s, _ in a] == [200, 404, 201, 200] and a[3][1] == "1\n", str(a))

ms, raw = run([get("/sleep/200"), get("/spin/100"), get("/sleep/200"), get("/health", close=True)])
a = bodies(raw)
check("an Unbounded route runs alone: before it, after it, never beside it",
      [b for _, b in a] == ["slept 200\n", "spun 100\n", "slept 200\n", "ok\n"] and 450 <= ms < 1200, f"{ms:.0f} ms {a}")

# ---- exceptions, closing, errors ----
ms, raw = run([get("/sleep/50"), get("/boom"), get("/sleep/50"), get("/health", close=True)])
a = bodies(raw)
check("a throwing handler answers 500 in its place and the connection goes on", [s for s, _ in a] == [200, 500, 200, 200], str(a))

ms, raw = run([get("/sleep/100"), get("/health", close=True), get("/sleep/10")])
a = bodies(raw)
check("nothing is answered after Connection: close", [b for _, b in a] == ["slept 100\n", "ok\n"], str(a))

ms, raw = run([get("/sleep/50"), "garbage\r\n\r\n", get("/health")])
a = bodies(raw)
check("a bad request in the middle: the earlier ones are answered, then 400, then the end", [s for s, _ in a] == [200, 400], str(a))

ms, raw = run([get("/sleep/50"), "BREW /pot HTTP/1.1\r\nHost: x\r\n\r\n"])
a = bodies(raw)
check("an unknown method in the middle: 200 then 501", [s for s, _ in a] == [200, 501], str(a))

# ---- a client that leaves with requests in flight ----
s = socket.create_connection(("127.0.0.1", port))
s.sendall("".join([get("/sleep/150")] * 6).encode())
s.close()
time.sleep(0.4)
ms, raw = run([get("/health", close=True)])
check("the server is fine after a client left with requests in flight", bodies(raw) == [(200, "ok\n")], raw[:80])

# ---- many connections at once, each pipelining ----
socks = [socket.create_connection(("127.0.0.1", port)) for _ in range(20)]
t = time.time()
for c in socks:
    c.sendall("".join([get("/sleep/100")] * 4 + [get("/health", close=True)]).encode())
good = 0
for c in socks:
    c.settimeout(5)
    buf = b""
    try:
        while True:
            d = c.recv(65536)
            if not d:
                break
            buf += d
    except socket.timeout:
        pass
    if len(bodies(buf.decode())) == 5:
        good += 1
    c.close()
ms = (time.time() - t) * 1000
check("20 connections x 4 sleeping requests overlap", good == 20 and ms < 1000, f"{good} complete, {ms:.0f} ms (one after another: 8000)")

# ---- streamed answers in the pipeline: in order, alone, without holding anything else up ----
put = "PUT /files/pipe.bin HTTP/1.1\r\nHost: x\r\nContent-Length: 10\r\nConnection: close\r\n\r\n0123456789"
run([put])
ms, raw = run([get("/sleep/150"), get("/stream/3"), get("/files/pipe.bin"), get("/health", close=True)])
a = bodies(raw)
check("a stream and a file after a slow request: answered in order",
      [b for _, b in a] == ["slept 150\n", "line 0\nline 1\nline 2\n", "0123456789", "ok\n"] and ms >= 140, f"{ms:.0f} ms {a}")

ms, raw = run([get("/stream/2"), get("/stream/2"), get("/health", close=True)])
a = bodies(raw)
check("two streams back to back", [b for _, b in a] == ["line 0\nline 1\n", "line 0\nline 1\n", "ok\n"], str(a))

# the client leaves in the middle of a stream
s = socket.create_connection(("127.0.0.1", port))
s.sendall(get("/stream/1000?ms=5").encode())
s.settimeout(3)
first = b""
while b"line 0" not in first:
    first += s.recv(4096)
s.close()
time.sleep(0.3)
ms, raw = run([get("/health", close=True)])
check("the server is fine after a client left in the middle of a stream", b"line 0" in first and bodies(raw) == [(200, "ok\n")], raw[:60])

# a slow reader: the download parks while its window is full, and other connections are not held up
size = 8_000_000
run(["PUT /files/slow.bin HTTP/1.1\r\nHost: x\r\nContent-Length: %d\r\nConnection: close\r\n\r\n" % size + "q" * size])
slow = socket.create_connection(("127.0.0.1", port))
slow.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 65536)
slow.sendall(get("/files/slow.bin", close=True).encode())
got = b""
worst = 0.0
t = time.time()
nextcheck = t + 0.05
slow.settimeout(10)
while True:
    d = slow.recv(65536)
    if not d:
        break
    got += d
    time.sleep(0.004)                                   # slower than the server could send: the download parks on a full window
    if time.time() >= nextcheck:
        t0 = time.time()
        run([get("/health", close=True)])
        worst = max(worst, (time.time() - t0) * 1000)
        nextcheck = time.time() + 0.05
slow.close()
body = got.split(b"\r\n\r\n", 1)[1] if b"\r\n\r\n" in got else b""
check("a slow reader gets all of an 8 MB file", len(body) == size, f"{len(body)} of {size} bytes")
check("... and other connections are answered meanwhile", 0 < worst < 200, f"the slowest /health took {worst:.0f} ms")

sys.exit(1 if failed else 0)
