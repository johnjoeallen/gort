#!/bin/bash
# D15 handle indirection cost. Output: results/handle.txt (-O2) and results/handle_O3.txt (-O3, lets the raw loop vectorise)
D=$(dirname "$0")
gcc -O2 -o "$D/c/handle_bench" "$D/c/handle_bench.c" && taskset -c 4 "$D/c/handle_bench" 15 > "$D/results/handle.txt"
gcc -O3 -o "$D/c/handle_bench_O3" "$D/c/handle_bench.c" && taskset -c 4 "$D/c/handle_bench_O3" 15 > "$D/results/handle_O3.txt"
