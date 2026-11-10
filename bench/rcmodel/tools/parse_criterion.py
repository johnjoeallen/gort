#!/usr/bin/env python3 -I
"""criterion target dir -> CSV rows. arg: criterion dir. One row per sample (10 samples/bench)."""
import json, os, sys
for root, dirs, files in os.walk(sys.argv[1]):
    if os.path.basename(root) != "new" or "sample.json" not in files: continue
    bm = json.load(open(os.path.join(root, "benchmark.json"))); sm = json.load(open(os.path.join(root, "sample.json")))
    parts = bm["full_id"].split("/")          # w1/<variant>/depth=N   or  w2/<variant>/<mode>/t=N
    variant = "rust-idiomatic" if parts[1] == "idiomatic" else "rust-arc"
    for i, (it, t) in enumerate(zip(sm["iters"], sm["times"]), 1):
        ns = t / it
        if parts[0] == "w1":
            d = int(parts[2].split("=")[1]); print(f"{i},W1,rust,{variant},depth={d};shared=0,ns_per_pass,{ns/d:.6g}")
        else:
            T = int(parts[3].split("=")[1]); print(f"{i},W2,rust,{variant},threads={T};mode={parts[2]},mops_total,{T*1e3/ns:.6g}")
