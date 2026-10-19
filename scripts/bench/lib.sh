# Shared helpers for the bench scripts. Source it, don't run it.
# Everything talks to the bench gateway (localhost:3101) over its REST API,
# like a browser would, and to the containers through `docker compose`.

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DATA="$BENCH_DIR/data"           # generated test files (gitignored)
TMP="$BENCH_DIR/tmp"             # downloads and scratch (gitignored)
RESULTS="${RESULTS:-$BENCH_DIR/results/latest}"
GW="${GW:-http://localhost:3101}"
MiB=1048576
mkdir -p "$DATA" "$TMP" "$RESULTS"

dc() { docker compose -f "$BENCH_DIR/compose.bench.yml" "$@"; }

now() { python3 -c 'import time; print(f"{time.time():.3f}")'; }
since() { python3 -c "print(round($(now) - $1, 1))"; }
fsize() { wc -c < "$1" | tr -d ' '; }
sha() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }
same() { [ "$(sha "$1")" = "$(sha "$2")" ] && echo igual || echo DISTINTO; }
urlenc() { jq -rn --arg p "$1" '$p|@uri'; }
log() { echo "[$(date +%H:%M:%S)] $*" >&2; }

# --- REST API ----------------------------------------------------------------
register() { curl -s -o /dev/null -w '%{http_code}' -X POST "$GW/api/v1/auth/register" \
  -H 'content-type: application/json' -d "{\"username\":\"$1\",\"password\":\"$2\"}"; }
login() { curl -s -X POST "$GW/api/v1/auth/login" -H 'content-type: application/json' \
  -d "{\"username\":\"$1\",\"password\":\"$2\"}" | jq -r .token; }
# upload TOKEN FILE PATH -> "code seconds"; the JSON answer lands in $TMP/last-upload.json.
# -T streams the file; --data-binary would load it whole into memory.
upload() { curl -s -o "$TMP/last-upload.json" -w '%{http_code} %{time_total}' -T "$2" \
  -H "authorization: Bearer $1" "$GW/api/v1/files?path=$(urlenc "$3")"; }
# download TOKEN PATH OUT -> "code seconds"
download() { curl -s -o "$3" -w '%{http_code} %{time_total}' \
  -H "authorization: Bearer $1" "$GW/api/v1/files?path=$(urlenc "$2")"; }
mkdir_() { curl -s -o /dev/null -w '%{http_code}' -X POST -H "authorization: Bearer $1" \
  "$GW/api/v1/dirs?path=$(urlenc "$2")"; }
ls_() { curl -s -H "authorization: Bearer $1" "$GW/api/v1/fs?path=$(urlenc "$2")"; }
rm_() { curl -s -o /dev/null -w '%{http_code}' -X DELETE -H "authorization: Bearer $1" \
  "$GW/api/v1/files?path=$(urlenc "$2")${3:+&recursive=true}"; }
mv_() { curl -s -o /dev/null -w '%{http_code}' -X POST -H "authorization: Bearer $1" \
  -H 'content-type: application/json' "$GW/api/v1/move" -d "{\"from\":\"$2\",\"to\":\"$3\"}"; }
status() { curl -s -H "authorization: Bearer $1" "$GW/api/v1/cluster/status"; }
under_rep() { status "$1" | jq -r '.under_replicated_blocks // "?"'; }
worker_state() { status "$1" | jq -r --arg w "$2" '.workers[]|select(.id==$w)|.state'; }
block_counts() { status "$1" | jq -r '[.workers[]|"\(.id)=\(.block_count)"]|join(" ")'; }

# wait_for CMD... : retries every 2 s for up to 2 min.
wait_for() { for _ in $(seq 1 60); do "$@" && return 0; sleep 2; done; return 1; }
replicas_ok() { [ "$(under_rep "$1")" = 0 ]; }

# --- containers --------------------------------------------------------------
leader() { curl -s localhost:8181/health/leader | grep -q '^leader' && echo master-a && return
           curl -s localhost:8182/health/leader | grep -q '^leader' && echo master-b; }
# Block files (not .meta) on one worker, one path per line.
# A fresh worker has no blocks/ directory yet, hence the 2>/dev/null.
worker_blocks() { dc exec -T "$1" sh -c 'find /data/blocks -type f ! -name "*.meta" 2>/dev/null; true' | sort; }
# Overwrite the first COUNT bytes of a block file with zeros (silent corruption).
corrupt() { dc exec -T -u 0 "$1" dd if=/dev/zero of="$2" bs="${3:-1048576}" count=1 conv=notrunc 2>/dev/null; }
logs_since() { dc logs --since "$2" "$1" 2>&1 | sed 's/\x1b\[[0-9;]*m//g'; }

# Fresh login for the bench user created by stack.sh.
token() { cat "$TMP/bench.tok"; }
