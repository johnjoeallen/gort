#!/bin/bash
# W5: builds the three C server variants, the Go server and the idle-connection holder, then runs tools/w5.py.
D=$(cd "$(dirname "$0")" && pwd); R=$D/results; W5REPS=${W5REPS:-5}
for m in 0 2 3; do n=(C0-raw x C2-flagged C3-atomic); gcc -O2 -march=native -fno-optimize-sibling-calls -DMODE=$m -DVARIANT="\"${n[$m]}\"" -DIMPL='"c-O2"' -o $D/c/build/httpd_${n[$m]}_O2 $D/c/httpd.c -lpthread || exit 1; done
gcc -O2 -o $D/tools/idle $D/tools/idle.c && (cd $D/go && go build -o server_go ./server) || exit 1
(cd $D/java && mvn -q -B package -DskipTests 2>&1 | grep ERROR)
python3 -I $D/tools/w5.py $R $W5REPS
