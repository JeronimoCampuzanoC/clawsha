#!/usr/bin/env bash
# Runs DynamoDB Local on port 8000 without Docker (needs Java 17+).
# Downloads the official tarball from AWS the first time into .dynamodb-local/.
# Data lives in memory: restarting it gives you empty tables.
#
#   scripts/dev/dynamodb-local.sh          # foreground, Ctrl-C to stop
#   PORT=8001 scripts/dev/dynamodb-local.sh
set -euo pipefail

PORT="${PORT:-8000}"
DIR="$(cd "$(dirname "$0")/../.." && pwd)/.dynamodb-local"
URL="https://d1ni2b6xgvw0s0.cloudfront.net/v2.x/dynamodb_local_latest.tar.gz"

JAVA="${JAVA:-java}"
if ! command -v "$JAVA" >/dev/null 2>&1; then
  if [ -x /opt/homebrew/opt/openjdk/bin/java ]; then
    JAVA=/opt/homebrew/opt/openjdk/bin/java
  else
    echo "java not found. Install it (brew install openjdk) or use docker: " >&2
    echo "  docker run --rm -p 8000:8000 amazon/dynamodb-local -jar DynamoDBLocal.jar -inMemory -sharedDb" >&2
    exit 1
  fi
fi

if [ ! -f "$DIR/DynamoDBLocal.jar" ]; then
  echo "downloading DynamoDB Local into $DIR ..."
  mkdir -p "$DIR"
  curl -sSL "$URL" | tar -xz -C "$DIR"
fi

cd "$DIR"
exec "$JAVA" -Djava.library.path=./DynamoDBLocal_lib -jar DynamoDBLocal.jar -inMemory -sharedDb -port "$PORT"
