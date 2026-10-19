#!/usr/bin/env bash
# The whole campaign of the report, section 7, in order. About 45 minutes on
# a laptop; keep the machine otherwise idle, the numbers are sensitive to it.
#   scripts/bench/run-all.sh [--no-build]
# Results go to scripts/bench/results/<date-time>/ (RESULTS overrides it).
set -euo pipefail
cd "$(dirname "$0")"
export RESULTS="${RESULTS:-$PWD/results/$(date +%Y-%m-%d-%H%M)}"
source ./lib.sh

for tool in docker jq curl python3; do command -v $tool >/dev/null || { echo "missing: $tool" >&2; exit 1; }; done
[ "${1:-}" = --no-build ] || ./stack.sh build
./gen-data.sh

{ echo "start: $(date -u +%FT%TZ)"; echo "commit: $(git rev-parse --short HEAD)"
  echo "host: $(uname -sm), $(docker info --format '{{.NCPU}} CPU, {{.MemTotal}} bytes for Docker')"; } > "$RESULTS/environment.txt"

./stack.sh up; ./functional.sh
./stack.sh up; ./security.sh
./faults.sh all
./perf.sh all
./latency.sh
./placement.sh
./stack.sh down

echo "end: $(date -u +%FT%TZ)" >> "$RESULTS/environment.txt"
python3 ./summarize.py "$RESULTS"
log "done: $RESULTS"
