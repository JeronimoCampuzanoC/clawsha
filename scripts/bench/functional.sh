#!/usr/bin/env bash
# Functional tests (report 7.2): integrity by file size and cases F1-F13.
# Needs a running cluster (stack.sh up) and the files from gen-data.sh.
#   results/functional-integrity.csv   file,bytes,blocks,up_code,up_s,down_code,down_s,sha256
#   results/functional-cases.txt       one line per case: ID | expected | got
set -euo pipefail
source "$(dirname "$0")/lib.sh"

register alice pw-alice-123 >/dev/null
A=$(login alice pw-alice-123)

# --- integrity: every size that matters for the partition ---------------------
out="$RESULTS/functional-integrity.csv"
echo "file,bytes,blocks,up_code,up_s,down_code,down_s,sha256" > "$out"
for f in f0 f1 f16m f16m1 f100m f1g; do
  up=$(upload "$A" "$DATA/$f" "/$f"); blocks=$(jq -r .block_count "$TMP/last-upload.json" 2>/dev/null || echo -)
  dn=$(download "$A" "/$f" "$TMP/$f.dl")
  echo "$f,$(fsize "$DATA/$f"),$blocks,${up% *},${up#* },${dn% *},${dn#* },$(same "$DATA/$f" "$TMP/$f.dl")" | tee -a "$out"
  rm -f "$TMP/$f.dl"
done

# --- namespace, isolation, concurrency ------------------------------------------
out="$RESULTS/functional-cases.txt"
: > "$out"
case_() { printf '%-4s| %-58s| expected %-22s| got %s\n' "$1" "$2" "$3" "$4" | tee -a "$out"; }

case_ F1  "mkdir /docs and /docs/sub" "201 201" "$(mkdir_ "$A" /docs) $(mkdir_ "$A" /docs/sub)"
case_ F2  "mkdir with a missing parent (/no/parent)" "404" "$(mkdir_ "$A" /no/parent)"
case_ F3  "mkdir of an existing directory" "409" "$(mkdir_ "$A" /docs)"
upload "$A" "$DATA/f1" /docs/a.bin >/dev/null
case_ F4  "ls /docs after uploading a.bin" "a.bin, sub" "$(ls_ "$A" /docs | jq -c '[.entries[]|"\(.name):\(.kind):\(.size)"]')"
mv=$(mv_ "$A" /docs/a.bin /docs/sub/b.bin)
dn=$(download "$A" /docs/sub/b.bin "$TMP/b.dl"); old=$(download "$A" /docs/a.bin /dev/null)
case_ F5  "mv a.bin -> sub/b.bin; download new and old path" "204; 200 same hash; 404" "$mv; ${dn% *} $(same "$DATA/f1" "$TMP/b.dl"); ${old% *}"
case_ F6  "delete non-empty /docs without recursive" "409" "$(rm_ "$A" /docs)"
case_ F7  "rm a file, then rm -r /docs" "204 204, gone" "$(rm_ "$A" /docs/sub/b.bin) $(rm_ "$A" /docs r) $(ls_ "$A" / | jq -c '[.entries[].name|select(.=="docs")]')"
upload "$A" "$DATA/f1" /dup.bin >/dev/null; upload "$A" "$DATA/f1" /dup.bin >/dev/null
case_ F8  "upload /dup.bin twice" "/dup (1).bin" "$(jq -r .path "$TMP/last-upload.json")"
case_ F9  "register an existing username" "409" "$(register alice other-password-123)"
case_ F10 "register with a password shorter than 8" "400" "$(register zed short)"
case_ F11 "login with a wrong password" "401" "$(curl -s -o /dev/null -w '%{http_code}' -X POST "$GW/api/v1/auth/login" -H 'content-type: application/json' -d '{"username":"alice","password":"nope-nope"}')"
register bob pw-bob-123 >/dev/null; B=$(login bob pw-bob-123)
bdl=$(download "$B" /f1 /dev/null); bls=$(ls_ "$B" / | jq -c '[.entries[].name]'); brm=$(rm_ "$B" /f1); still=$(download "$A" /f1 /dev/null)
case_ F12 "bob downloads, lists and deletes alice's files" "404; []; 404; alice 200" "${bdl% *}; $bls; $brm; alice ${still% *}"

# F13: two writers on the same path, 50 ms apart. The path lease lets one in.
( upload "$A" "$DATA/f100m" /race.bin > "$TMP/race1" ) &
( sleep 0.05; curl -s -o /dev/null -w '%{http_code} %{time_total}' -T "$DATA/f100m" \
    -H "authorization: Bearer $A" "$GW/api/v1/files?path=/race.bin" > "$TMP/race2" ) &
wait
case_ F13 "two 100 MiB uploads to /race.bin, 50 ms apart" "one 201, one 409" "$(cat "$TMP/race1") | $(cat "$TMP/race2") | files: $(ls_ "$A" / | jq -c '[.entries[].name|select(startswith("race"))]')"
