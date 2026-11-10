#!/usr/bin/env python3 -I
"""JMH json -> CSV rows. args: json variant. One row per fork (mean of that fork's measurement iterations)."""
import json, sys
data = json.load(open(sys.argv[1])); variant = sys.argv[2]
for b in data:
    name = b["benchmark"].split(".")[-1]; p = b["params"]; raw = b["primaryMetric"]["rawData"]
    for i, fork in enumerate(raw, 1):
        v = sum(fork) / len(fork)
        if name == "w1":
            d = int(p["depth"]); print(f"{i},W1,java,{variant},depth={d};shared=0,ns_per_pass,{v/d:.6g}")
        else:
            print(f"{i},W2,java,{variant},threads={b['threads']};mode={p['mode']},mops_total,{v:.6g}")
