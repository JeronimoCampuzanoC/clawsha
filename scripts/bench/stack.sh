#!/usr/bin/env bash
# Local cluster for the tests.
#   stack.sh build                          build the images from this checkout
#   stack.sh up [BLOCK] [R] [QUOTA]         fresh cluster (wipes data), waits until ready
#   stack.sh down                           stop and wipe
# up waits for: gateway answering, 3 workers ALIVE and the leader out of safe mode.
set -euo pipefail
source "$(dirname "$0")/lib.sh"

case "${1:-}" in
build)
  dc --profile extra build ;;
up)
  export BLOCK_SIZE="${2:-16777216}" REPLICATION="${3:-2}" QUOTA="${4:-2147483648}"
  dc --profile extra down -v >/dev/null 2>&1 || true
  dc up -d >/dev/null 2>&1
  # The gateway restarts until the first leader is elected, so retry.
  registered() { [ "$(register bench bench-password-1)" = 201 ]; }
  three_alive() { [ "$(status "$(token)" | jq '[.workers[]|select(.state=="alive")]|length')" = 3 ]; }
  out_of_safe_mode() { [ "$(status "$(token)" | jq .safe_mode)" = false ]; }
  wait_for registered || { echo "gateway did not come up" >&2; exit 1; }
  login bench bench-password-1 > "$TMP/bench.tok"
  wait_for three_alive || { echo "workers did not register" >&2; exit 1; }
  wait_for out_of_safe_mode
  log "cluster ready: block=$BLOCK_SIZE R=$REPLICATION quota=$QUOTA leader=$(leader)" ;;
down)
  dc --profile extra down -v ;;
*)
  sed -n '2,6p' "$0"; exit 1 ;;
esac
