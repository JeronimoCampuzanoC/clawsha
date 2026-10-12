#!/usr/bin/env bash
# Runs the master test suite against a local DynamoDB on port 8000.
# Start one first with scripts/dev/dynamodb-local.sh (or the compose file).
set -euo pipefail
cd "$(dirname "$0")/../.."
export DFSHA_TEST_DYNAMO_ENDPOINT="${DFSHA_TEST_DYNAMO_ENDPOINT:-http://localhost:8000}"
exec cargo test -p master "$@"
