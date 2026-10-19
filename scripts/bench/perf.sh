#!/usr/bin/env bash
# Throughput (report 7.3). Every experiment starts from a fresh cluster with
# 8 GiB per worker and repeats REPS times (default 5); summarize.py turns the
# CSVs into medians [min-max], the report tables and the charts.
#   perf.sh size    1 client: 16 MiB, 100 MiB, 1 GiB          -> results/perf-size.csv
#   perf.sh conc    1, 5 and 10 concurrent clients x 100 MiB   -> results/perf-conc.csv
#   perf.sh bsize   256 MiB with 4, 16 and 64 MiB blocks       -> results/perf-bsize.csv
#   perf.sh repl    R=1 vs R=2 with 100 MiB and 1 GiB          -> results/perf-repl.csv
#   perf.sh all
# CSV columns: label,rep,bytes,up_s,down_s,up_mibs,down_mibs,check
set -euo pipefail
source "$(dirname "$0")/lib.sh"
REPS="${REPS:-5}"
QUOTA_PERF=$((8 * 1024 * MiB))
HEADER="label,rep,bytes,up_s,down_s,up_mibs,down_mibs,check"

mibs() { python3 -c "print(round($1 / $MiB / $2, 1))"; }
up() { "$BENCH_DIR/stack.sh" up "${1:-16777216}" "${2:-2}" "$QUOTA_PERF"; }

# one LABEL REP FILE -> one CSV row: upload, download back, compare hashes
one() { local T; T=$(token); local bytes; bytes=$(fsize "$3")
  local u d; u=$(upload "$T" "$3" "/$1-$2"); d=$(download "$T" "/$1-$2" "$TMP/perf.dl")
  if [ "${u% *}" != 201 ] || [ "${d% *}" != 200 ]; then echo "$1,$2,$bytes,FAIL,FAIL,,,up=${u% *} down=${d% *}"; return; fi
  echo "$1,$2,$bytes,${u#* },${d#* },$(mibs "$bytes" "${u#* }"),$(mibs "$bytes" "${d#* }"),$(same "$3" "$TMP/perf.dl")"
  rm -f "$TMP/perf.dl"; }

size() { up; for f in f16m f100m f1g; do for r in $(seq 1 "$REPS"); do one "$f" "$r" "$DATA/$f"; done; done; }

bsize() { for bs in 4 16 64; do up $((bs * MiB))
  for r in $(seq 1 "$REPS"); do one "bs${bs}" "$r" "$DATA/f256m"; done; done; }

repl() { for R in 1 2; do up 16777216 "$R"
  for f in f100m f1g; do for r in $(seq 1 "$REPS"); do one "R${R}-$f" "$r" "$DATA/$f"; done; done; done; }

# N clients at once, each with its own 100 MiB file: all uploads, then all
# downloads. Throughput is total bytes over wall time; check counts the
# clients whose upload, download and hash were all fine.
conc() { for r in $(seq 1 "$REPS"); do up; local T; T=$(token)
  for n in 1 5 10; do
    local t0 t1 t2 ok=0
    t0=$(now); for i in $(seq 1 "$n"); do upload "$T" "$DATA/f100m" "/c$n-$i" > "$TMP/cu$i" & done; wait
    t1=$(now); for i in $(seq 1 "$n"); do download "$T" "/c$n-$i" "$TMP/c$i.dl" > "$TMP/cd$i" & done; wait
    t2=$(now)
    for i in $(seq 1 "$n"); do grep -q '^201' "$TMP/cu$i" && grep -q '^200' "$TMP/cd$i" \
      && [ "$(same "$DATA/f100m" "$TMP/c$i.dl")" = igual ] && ok=$((ok + 1)); rm -f "$TMP/c$i.dl"; done
    local bytes=$((n * 100 * MiB)) us ds
    us=$(python3 -c "print(round($t1 - $t0, 3))"); ds=$(python3 -c "print(round($t2 - $t1, 3))")
    echo "c$n,$r,$bytes,$us,$ds,$(mibs $bytes "$us"),$(mibs $bytes "$ds"),$ok/$n"
  done; done; }

run() { log "perf $1 (REPS=$REPS)"; { echo "$HEADER"; "$1"; } | tee "$RESULTS/perf-$1.csv"; }
case "${1:-}" in
  size|conc|bsize|repl) run "$1" ;;
  all) for e in size conc bsize repl; do run "$e"; done ;;
  *) sed -n '2,12p' "$0"; exit 1 ;;
esac
