#!/usr/bin/env python3 -I
"""go test -bench output (stdin) -> CSV rows run,workload,impl,variant,params,metric,value.
args: workload(W1|W2) threads variant"""
import re, sys
w, T, variant = sys.argv[1], int(sys.argv[2]), sys.argv[3]
seen = {}
for line in sys.stdin:
    m = re.match(r'Benchmark(W[12])/(\S+?)(?:-\d+)?\s+\d+\s+([\d.]+) ns/op', line)
    if not m or m.group(1) != w: continue
    sub, ns = m.group(2), float(m.group(3))
    run = seen[sub] = seen.get(sub, 0) + 1
    if w == "W1":
        d = int(sub.split("=")[1])
        print(f"{run},W1,go,{variant},depth={d};shared=0,ns_per_pass,{ns/d:.6g}")
    else:
        mode = sub.split("=")[1]
        print(f"{run},W2,go,{variant},threads={T};mode={mode},mops_total,{1e3/ns:.6g}")
