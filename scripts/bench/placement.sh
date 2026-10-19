#!/usr/bin/env bash
# Placement with an empty worker (report 7.3, "Escalabilidad"): load the
# cluster with 15 x 100 MiB, add worker-d (AZ b, empty, same quota), upload
# 20 x 100 MiB more and count where the new copies went. With AvailableSpace
# the empty worker should get more than half of its AZ's new copies (~58 %
# expected, see report 5.2).
#   results/placement.txt
set -euo pipefail
source "$(dirname "$0")/lib.sh"
QUOTA_PERF=$((8 * 1024 * MiB))

"$BENCH_DIR/stack.sh" up 16777216 2 "$QUOTA_PERF"
T=$(token)
usage() { status "$T" | jq -r '.workers[]|"  \(.id) az=\(.az) used=\((.used_bytes*1000/.quota_bytes|floor)/10)% blocks=\(.block_count)"'; }
{
  for i in $(seq 1 15); do upload "$T" "$DATA/f100m" "/load-$i" >/dev/null; done
  echo "after loading 15 x 100 MiB (105 blocks):"; usage
  # Same quota as the others, or its usage percentage would climb faster.
  QUOTA=$QUOTA_PERF dc --profile extra up -d worker-d >/dev/null 2>&1
  four_alive() { [ "$(status "$T" | jq '[.workers[]|select(.state=="alive")]|length')" = 4 ]; }
  wait_for four_alive
  before=$(status "$T" | jq -c '[.workers[]|{(.id):.block_count}]|add')
  echo "worker-d joined (AZ b, empty): $(block_counts "$T")"
  for i in $(seq 1 20); do upload "$T" "$DATA/f100m" "/new-$i" >/dev/null; done
  after=$(status "$T" | jq -c '[.workers[]|{(.id):.block_count}]|add')
  echo "after 20 x 100 MiB more (140 blocks, 280 copies):"; usage
  echo "new copies per worker, and share within its AZ:"
  status "$T" | jq -r --argjson b "$before" --argjson a "$after" '
    [.workers[]|{id, az, new: ($a[.id] - ($b[.id] // 0))}] as $w
    | $w[] | . as $x | ([$w[]|select(.az==$x.az)|.new]|add) as $tot
    | "  \(.id) az=\(.az) new=\(.new) share=\((.new*1000/$tot|floor)/10)%"'
} | tee "$RESULTS/placement.txt"
