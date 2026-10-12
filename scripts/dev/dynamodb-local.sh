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

# Something already listening there is almost always a previous copy of this
# same server; say so instead of dying with a Java stack trace.
if lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; then
  echo "port $PORT is already in use:" >&2
  lsof -nP -iTCP:"$PORT" -sTCP:LISTEN | tail -n +2 >&2
  echo "if that is an old DynamoDB Local, stop it with: kill <PID>" >&2
  exit 1
fi

# Prefer Homebrew's JDK: on macOS a bare `java` can be the system stub, which
# exists but only prints "Unable to locate a Java Runtime".
if [ -z "${JAVA:-}" ]; then
  if [ -x /opt/homebrew/opt/openjdk/bin/java ]; then
    JAVA=/opt/homebrew/opt/openjdk/bin/java
  else
    JAVA=java
  fi
fi
if ! "$JAVA" -version >/dev/null 2>&1; then
  echo "no working java found ($JAVA). Install it (brew install openjdk) or use docker:" >&2
  echo "  docker run --rm -p 8000:8000 amazon/dynamodb-local -jar DynamoDBLocal.jar -inMemory -sharedDb" >&2
  exit 1
fi

if [ ! -f "$DIR/DynamoDBLocal.jar" ]; then
  echo "downloading DynamoDB Local into $DIR ..."
  mkdir -p "$DIR"
  curl -sSL "$URL" | tar -xz -C "$DIR"
fi

cd "$DIR"
exec "$JAVA" -Djava.library.path=./DynamoDBLocal_lib -jar DynamoDBLocal.jar -inMemory -sharedDb -port "$PORT"
