#!/usr/bin/env bash
# Latency against the RNF10 targets (report 7.3, table "Latencia"):
# 100 x ls / mkdir / mv / rm, and 30 x time to the first body byte of a
# 1 MiB file and of a full 16 MiB block. One sample (ms) per line in
# results/latency-<op>.txt; summarize.py computes p50/p95.
set -euo pipefail
source "$(dirname "$0")/lib.sh"

"$BENCH_DIR/stack.sh" up
T=$(token); H="authorization: Bearer $T"
ms() { python3 -c "import sys; print(f'{float(sys.argv[1]) * 1000:.2f}')" "$1"; }

for op in ls mkdir mv rm; do : > "$RESULTS/latency-$op.txt"; done
for i in $(seq 1 100); do
  ms "$(curl -s -o /dev/null -w '%{time_total}' -X POST -H "$H" "$GW/api/v1/dirs?path=/d$i")" >> "$RESULTS/latency-mkdir.txt"
  ms "$(curl -s -o /dev/null -w '%{time_total}' -H "$H" "$GW/api/v1/fs?path=/")" >> "$RESULTS/latency-ls.txt"
  ms "$(curl -s -o /dev/null -w '%{time_total}' -X POST -H "$H" -H 'content-type: application/json' \
        "$GW/api/v1/move" -d "{\"from\":\"/d$i\",\"to\":\"/e$i\"}")" >> "$RESULTS/latency-mv.txt"
  ms "$(curl -s -o /dev/null -w '%{time_total}' -X DELETE -H "$H" "$GW/api/v1/files?path=/e$i")" >> "$RESULTS/latency-rm.txt"
done

upload "$T" "$DATA/f1m" /lat1 >/dev/null; upload "$T" "$DATA/f16m" /lat16 >/dev/null
python3 -I "$BENCH_DIR/ttfb.py" "$GW" "$T" /lat1 30 > "$RESULTS/latency-ttfb1m.txt"
python3 -I "$BENCH_DIR/ttfb.py" "$GW" "$T" /lat16 30 > "$RESULTS/latency-ttfb16m.txt"
log "latency samples in $RESULTS/latency-*.txt"
