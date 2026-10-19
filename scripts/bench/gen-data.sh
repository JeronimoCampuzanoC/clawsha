#!/usr/bin/env bash
# Random test files, sized around the block boundaries (B = 16 MiB):
# empty, 1 byte, exactly one block, one block + 1 byte, 1 MiB, 100 MiB,
# 256 MiB and 1 GiB. About 1.4 GiB in total; existing files are kept.
set -euo pipefail
source "$(dirname "$0")/lib.sh"

make() { [ -f "$DATA/$1" ] && [ "$(fsize "$DATA/$1")" = "$2" ] && return
         log "generating $1 ($2 bytes)"
         # BSD head refuses -c 0
         if [ "$2" = 0 ]; then : > "$DATA/$1"; else head -c "$2" /dev/urandom > "$DATA/$1"; fi; }
make f0    0
make f1    1
make f1m   $((1 * MiB))
make f16m  $((16 * MiB))
make f16m1 $((16 * MiB + 1))
make f100m $((100 * MiB))
make f256m $((256 * MiB))
make f1g   $((1024 * MiB))
