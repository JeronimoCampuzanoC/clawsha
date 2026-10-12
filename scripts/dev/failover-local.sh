#!/usr/bin/env bash
# Two masters against one local DynamoDB; kill the leader and watch the other
# one take over. This is test 8 of the plan ("líder caído") in local form.
#
# Needs: scripts/dev/dynamodb-local.sh running on port 8000, and the master
# built (cargo build -p master).
set -euo pipefail
cd "$(dirname "$0")/../.."

BIN=target/debug/master
[ -x "$BIN" ] || cargo build -p master

common=(--dynamodb-endpoint http://localhost:8000 --table-prefix failover_ --create-tables)
$BIN --id master-a --az a --port 50061 --health-port 8081 --advertise-addr 127.0.0.1:50061 "${common[@]}" > /tmp/master-a.log 2>&1 &
A=$!
$BIN --id master-b --az b --port 50062 --health-port 8082 --advertise-addr 127.0.0.1:50062 "${common[@]}" > /tmp/master-b.log 2>&1 &
B=$!
trap 'kill $A $B 2>/dev/null || true' EXIT

leader() {
  # Prints which health port answers 200 on /health/leader.
  for p in 8081 8082; do
    if [ "$(curl -s -o /dev/null -w '%{http_code}' "http://localhost:$p/health/leader")" = "200" ]; then
      echo "$p"; return
    fi
  done
  echo none
}

sleep 4
first=$(leader)
echo "leader at health port: $first"
[ "$first" != none ] || { echo "nobody took the lease"; exit 1; }

if [ "$first" = 8081 ]; then victim=$A; else victim=$B; fi
echo "killing the leader (pid $victim) at $(date +%T)"
kill -9 "$victim"

start=$(date +%s)
while true; do
  now=$(leader)
  if [ "$now" != none ] && [ "$now" != "$first" ]; then
    echo "new leader at health port $now after $(( $(date +%s) - start )) s"
    break
  fi
  if [ $(( $(date +%s) - start )) -gt 30 ]; then
    echo "no failover within 30 s"; exit 1
  fi
  sleep 1
done
