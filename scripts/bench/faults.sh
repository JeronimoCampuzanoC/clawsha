#!/usr/bin/env bash
# Fault tests (report 7.4): each one breaks something for real and checks that
# committed files still read with the same SHA-256 and that every block gets
# back to 2 copies on its own. Every scenario starts from a fresh cluster.
#   faults.sh d1|d2|d3|d4|d5|d6|all     ->  results/faults-<id>.txt
#
#   d1  one replica corrupted on disk: scrubber quarantines it, leader repairs it
#   d2  the AZ-b copy of every block of a file corrupted: reads fall back
#   d3  worker-c stopped, then back: re-replication, then GC trims extras
#   d4  all of AZ b killed (leader + its only worker)
#   d5  leader killed in the middle of a 1 GiB upload
#   d6  leader killed in the middle of a 1 GiB download
set -euo pipefail
source "$(dirname "$0")/lib.sh"

fresh() { "$BENCH_DIR/stack.sh" up; T=$(token); }
# Prints a line only when it differs from the previous one, prefixed with t=+Ns.
watch_changes() { local t0=$1 prev="" line; shift
  for _ in $(seq 1 "${MAX_POLLS:-120}"); do
    line=$("$@"); [ "$line" != "$prev" ] && echo "t=+$(since "$t0")s $line"; prev=$line
    case "$line" in *DONE*) return 0;; esac; sleep 1; done; return 1; }
new_blocks() { comm -13 "$1" "$2"; }

d1() {
  fresh
  for w in worker-a worker-b worker-c; do worker_blocks $w | sed "s|^|$w |"; done | sort > "$TMP/before"
  upload "$T" "$DATA/f16m" /d1.bin >/dev/null
  for w in worker-a worker-b worker-c; do worker_blocks $w | sed "s|^|$w |"; done | sort > "$TMP/after"
  echo "replicas of the new block:"; new_blocks "$TMP/before" "$TMP/after" | sed 's/^/  /'
  path=$(new_blocks "$TMP/before" "$TMP/after" | awk '$1=="worker-b"{print $2}')
  healthy=$(new_blocks "$TMP/before" "$TMP/after" | awk '$1!="worker-b"{print $1; exit}')
  id=$(basename "$path")
  t0=$(now); corrupt worker-b "$path"; echo "t=0 first MiB of the worker-b copy overwritten with zeros"
  for i in 1 2 3; do r=$(download "$T" /d1.bin "$TMP/d1.dl"); echo "download $i: ${r% *} sha256 $(same "$DATA/f16m" "$TMP/d1.dl")"; done
  state() { q=$(dc exec -T worker-b sh -c "find /data/quarantine -name '$id' 2>/dev/null | wc -l" | tr -d ' ')
            c=$(dc exec -T worker-b sh -c "test -f $path && sha256sum $path | cut -c1-12" || true)
            u=$(under_rep "$T"); s="quarantined=$q under_replicated=$u worker-b_copy=${c:-none}"
            [ "$q" -ge 1 ] && [ "$u" = 0 ] && [ -n "$c" ] && s="$s DONE"; echo "$s"; }
  watch_changes "$t0" state
  echo "repaired copy: $(dc exec -T worker-b sha256sum "$path" | cut -c1-16)  healthy copy on $healthy: $(dc exec -T "$healthy" sha256sum "$path" | cut -c1-16)"
  logs_since master-b 3m | grep -E 'reported corrupt|re-replicated' | cut -c1-200 || true
  logs_since master-a 3m | grep -E 'reported corrupt|re-replicated' | cut -c1-200 || true
}

d2() {
  fresh
  worker_blocks worker-b > "$TMP/before"
  upload "$T" "$DATA/f100m" /d2.bin >/dev/null
  worker_blocks worker-b > "$TMP/after"
  paths=(); while IFS= read -r p; do paths+=("$p"); done < <(new_blocks "$TMP/before" "$TMP/after")
  echo "blocks of the file with a copy on worker-b: ${#paths[@]}"
  start=$(date -u +%Y-%m-%dT%H:%M:%S); t0=$(now)
  for p in "${paths[@]}"; do corrupt worker-b "$p" 65536; done
  r=$(download "$T" /d2.bin "$TMP/d2.dl")
  echo "download right after corrupting: ${r% *} in ${r#* }s, sha256 $(same "$DATA/f100m" "$TMP/d2.dl")"
  sleep 1
  echo "worker-b answered DATA_LOSS for: $(logs_since worker-b 1m | grep -c 'code=DataLoss' || true) blocks (each one was then read from the other replica)"
  ids=$(printf '%s\n' "${paths[@]}" | xargs -n1 basename)
  state() { n=0; for id in $ids; do logs_since worker-b 5m | grep -q "moved to quarantine block_id=$id" && n=$((n+1)); done
            u=$(under_rep "$T"); s="quarantined=$n/${#paths[@]} under_replicated=$u"
            [ "$n" = "${#paths[@]}" ] && [ "$u" = 0 ] && s="$s DONE"; echo "$s"; }
  MAX_POLLS=240 watch_changes "$t0" state
  echo "scrub passes on worker-b since the corruption:"; logs_since worker-b 5m | grep 'scrub pass finished' | awk -v s="$start" '$3>s' | grep -o 'checked=.*' || true
  r=$(download "$T" /d2.bin "$TMP/d2.dl"); echo "download after the repair: ${r% *} sha256 $(same "$DATA/f100m" "$TMP/d2.dl")"
}

d3() {
  fresh
  upload "$T" "$DATA/f1g" /big.bin >/dev/null; upload "$T" "$DATA/f100m" /mid.bin >/dev/null
  echo "before: $(block_counts "$T")"
  t0=$(now); dc stop worker-c >/dev/null 2>&1; echo "t=0 worker-c stopped"
  ( r=$(download "$T" /big.bin "$TMP/d3.dl"); echo "1 GiB download started right after the stop: ${r% *} in ${r#* }s, sha256 $(same "$DATA/f1g" "$TMP/d3.dl")" > "$TMP/d3-dl" ) &
  state() { s="worker-c=$(worker_state "$T" worker-c) under_replicated=$(under_rep "$T")"
            [[ "$s" == "worker-c=dead under_replicated=0" ]] && s="$s DONE"; echo "$s"; }
  watch_changes "$t0" state
  wait; cat "$TMP/d3-dl"
  echo "after repair: $(block_counts "$T")"
  t0=$(now); dc start worker-c >/dev/null 2>&1; echo "t=0 worker-c back"
  total=$(status "$T" | jq .total_blocks)
  copies() { c=$(status "$T" | jq '[.workers[]|.block_count]|add'); s="copies=$c (2 x $total blocks = $((2*total))) $(block_counts "$T")"
             [ "$c" = $((2*total)) ] && [ "$(worker_state "$T" worker-c)" = alive ] && s="$s DONE"; echo "$s"; }
  MAX_POLLS=180 watch_changes "$t0" copies
  logs_since master-b 4m | grep -o 'extra_copies_deleted: [1-9][0-9]*' || true
  logs_since master-a 4m | grep -o 'extra_copies_deleted: [1-9][0-9]*' || true
}

d4() {
  fresh
  for f in f1 f16m f16m1 f100m f1g; do upload "$T" "$DATA/$f" "/$f" >/dev/null; done
  # The scenario is "the leader's AZ goes down": make master-b the leader first.
  if [ "$(leader)" = master-a ]; then
    dc stop master-a >/dev/null 2>&1; wait_for sh -c 'curl -s localhost:8182/health/leader | grep -q ^leader'
    dc start master-a >/dev/null 2>&1; wait_for sh -c 'curl -s localhost:8181/health/leader | grep -q standby'
    # The new leader needs a moment to rebuild its block map.
    counts_known() { [ -n "$(block_counts "$T")" ] && replicas_ok "$T"; }
    wait_for counts_known
  fi
  echo "leader before: $(leader); blocks: $(block_counts "$T")"
  t0=$(now); dc kill master-b worker-b >/dev/null 2>&1; echo "t=0 AZ b killed (master-b, worker-b)"
  until [ "$(curl -s -o /dev/null -w '%{http_code}' -H "authorization: Bearer $T" "$GW/api/v1/fs?path=/")" = 200 ]; do sleep 0.5; done
  echo "t=+$(since "$t0")s metadata answers again; master-a: $(curl -s localhost:8181/health/leader)"
  until [ "$(download "$T" /f16m /dev/null | cut -d' ' -f1)" = 200 ]; do sleep 0.5; done
  echo "t=+$(since "$t0")s downloads work again (block map rebuilt from block reports)"
  for f in f1 f16m f16m1 f100m f1g; do r=$(download "$T" "/$f" "$TMP/d4.dl"); echo "  /$f: ${r% *} sha256 $(same "$DATA/$f" "$TMP/d4.dl")"; done
  state() { j=$(status "$T"); s="safe_mode=$(echo "$j" | jq .safe_mode) under_replicated=$(echo "$j" | jq .under_replicated_blocks)"
            [[ "$s" == *"safe_mode=false under_replicated=0" ]] && s="$s DONE"; echo "$s"; }
  watch_changes "$t0" state
  echo "upload during the outage (both copies in AZ a): $(upload "$T" "$DATA/f16m1" /during-outage.bin)"
  dc start master-b worker-b >/dev/null 2>&1; wait_for sh -c 'curl -s localhost:8182/health/leader | grep -q standby'
  echo "AZ b back: master-b is $(curl -s localhost:8182/health/leader)"
}

d5() {
  fresh
  upload "$T" "$DATA/f100m" /committed.bin >/dev/null
  L=$(leader); t0=$(now)
  ( r=$(upload "$T" "$DATA/f1g" /during-failover.bin); echo "upload result: ${r% *} after ${r#* }s" > "$TMP/d5-up" ) &
  sleep 3; dc kill "$L" >/dev/null 2>&1; echo "t=+$(since "$t0")s killed the leader ($L) during a 1 GiB upload"
  wait; cat "$TMP/d5-up"
  until [ "$(curl -s -o /dev/null -w '%{http_code}' -H "authorization: Bearer $T" "$GW/api/v1/fs?path=/")" = 200 ]; do sleep 0.5; done
  echo "t=+$(since "$t0")s the other master answers ($(leader))"
  echo "partial file visible: $(ls_ "$T" / | jq -c '[.entries[].name|select(startswith("during"))]')"
  sleep 15; r=$(download "$T" /committed.bin "$TMP/d5.dl"); echo "file committed before the failure: ${r% *} sha256 $(same "$DATA/f100m" "$TMP/d5.dl")"
  dc start "$L" >/dev/null 2>&1
}

d6() {
  fresh
  upload "$T" "$DATA/f1g" /big.bin >/dev/null
  L=$(leader); t0=$(now)
  ( r=$(download "$T" /big.bin "$TMP/d6.dl"); echo "download result: ${r% *} in ${r#* }s" > "$TMP/d6-dl" ) &
  sleep 1.5; dc kill "$L" >/dev/null 2>&1; echo "t=+$(since "$t0")s killed the leader ($L) during a 1 GiB download"
  wait; cat "$TMP/d6-dl"; echo "sha256 $(same "$DATA/f1g" "$TMP/d6.dl")"
  dc start "$L" >/dev/null 2>&1
}

run() { log "fault test $1"; "$1" 2>&1 | tee "$RESULTS/faults-$1.txt"; rm -f "$TMP"/*.dl; }
case "${1:-}" in
  d1|d2|d3|d4|d5|d6) run "$1" ;;
  all) for d in d1 d2 d3 d4 d5 d6; do run $d; done ;;
  *) sed -n '2,13p' "$0"; exit 1 ;;
esac
