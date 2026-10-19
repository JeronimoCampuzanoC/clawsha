#!/usr/bin/env bash
# Security tests (report 7.4, table "Pruebas de seguridad"): S1-S9.
# JWTs are forged with the gateway's own secret (JWT_SECRET in
# compose.bench.yml), so each case changes only the condition under test; S6
# is the control (same forgery, still valid) and must pass.
#   results/security.txt
set -euo pipefail
source "$(dirname "$0")/lib.sh"
SECRET=bench-secret

T=$(token)
SUB=$(python3 -c 'import sys,base64,json; p=sys.argv[1].split(".")[1]; print(json.loads(base64.urlsafe_b64decode(p+"="*(-len(p)%4)))["sub"])' "$T")

# jwt SUB SECRET SECONDS_UNTIL_EXP -> HS256 token
jwt() { python3 - "$@" <<'EOF'
import sys, json, hmac, hashlib, base64, time
sub, secret, off = sys.argv[1], sys.argv[2], int(sys.argv[3])
b = lambda d: base64.urlsafe_b64encode(d).rstrip(b"=").decode()
now = int(time.time())
h = b(json.dumps({"alg": "HS256", "typ": "JWT"}).encode())
p = b(json.dumps({"sub": sub, "iat": now - 7200, "exp": now + off}).encode())
print(h + "." + p + "." + b(hmac.new(secret.encode(), (h + "." + p).encode(), hashlib.sha256).digest()))
EOF
}
none_jwt() { python3 -c 'import sys,base64,json; b=lambda d: base64.urlsafe_b64encode(json.dumps(d).encode()).rstrip(b"=").decode(); print(b({"alg":"none","typ":"JWT"})+"."+b({"sub":sys.argv[1],"iat":0,"exp":9999999999})+".")' "$1"; }
code_with() { curl -s -o /dev/null -w '%{http_code}' -H "authorization: Bearer $1" "$GW/api/v1/fs?path=/"; }

out="$RESULTS/security.txt"; : > "$out"
case_() { printf '%-3s| %-55s| expected %-28s| got %s\n' "$1" "$2" "$3" "$4" | tee -a "$out"; }

case_ S1 "request without a token" 401 "$(curl -s -o /dev/null -w '%{http_code}' "$GW/api/v1/fs?path=/")"
case_ S2 "token with a tampered signature" 401 "$(code_with "${T%?}x")"
case_ S3 "token expired 1 s ago, correct secret" 401 "$(code_with "$(jwt "$SUB" "$SECRET" -1)")"
case_ S4 "valid token signed with another secret" 401 "$(code_with "$(jwt "$SUB" wrong-secret 3600)")"
case_ S5 "token with alg=none and no signature" 401 "$(code_with "$(none_jwt "$SUB")")"
case_ S6 "control: forged with the right secret, valid" 200 "$(code_with "$(jwt "$SUB" "$SECRET" 3600)")"
case_ S7 "unknown gateway route" 404 "$(curl -s -o /dev/null -w '%{http_code}' "$GW/nonexistent")"
trav=$(upload "$T" "$DATA/f1" '/../../etc/x')
outside=$(for w in worker-a worker-b worker-c; do dc exec -T "$w" sh -c 'find /data -type f | grep -cv "^/data/\(blocks\|tmp\|quarantine\)/"' || true; done | awk '{s+=$1} END {print s+0}')
case_ S8 "upload to /../../etc/x" "rejected; nothing outside /data" "${trav% *}; files outside the worker dirs: $outside"
printf '<script>alert(1)</script>' > "$TMP/x.html"; upload "$T" "$TMP/x.html" /x.html >/dev/null
hdrs=$(curl -s -D - -o /dev/null -H "authorization: Bearer $T" "$GW/api/v1/files?path=/x.html&disposition=inline" \
  | tr -d '\r' | grep -iE '^(content-disposition|x-content-type-options):' | paste -sd';' -)
case_ S9 "HTML requested with disposition=inline" "attachment + nosniff" "$hdrs"
