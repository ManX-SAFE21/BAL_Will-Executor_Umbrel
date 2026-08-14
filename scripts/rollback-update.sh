#!/bin/bash
# =============================================================================
# rollback-update.sh — Revert a deploy-update.sh run by restoring the backed-up
# source files and rebuilding both images.
#
# USAGE (on the Umbrel box):
#   sudo bash rollback-update.sh <timestamp> [--with-data]
#
# <timestamp> is printed at the end of deploy-update.sh, e.g.
#   sudo bash rollback-update.sh 20260814-2310
#
# By default the live database is KEPT (new transactions may have arrived since
# the deploy). Pass --with-data to also restore the DB/keys snapshot.
# =============================================================================
set -euo pipefail

TS="${1:-}"
WITH_DATA="${2:-}"
[ -z "$TS" ] && { echo "Usage: sudo bash rollback-update.sh <timestamp> [--with-data]"; exit 1; }

BASE=/home/umbrel/umbrel/app-data/bal-umbrel
APP=$BASE/bal-umbrel
DATA=$BASE/data
BK=$BASE/deploy-backups/$TS
[ -d "$BK" ] || { echo "Backup not found: $BK"; exit 1; }

declare -A FILES=(
  [bal-server.rs]=rust-src/src/bin/bal-server.rs
  [bal-pusher.rs]=rust-src/src/bin/bal-pusher.rs
  [index.html]=ui/index.html
)

cd "$APP"

echo "=== Restore source files ==="
for name in "${!FILES[@]}"; do
  if [ -f "$BK/src/$name" ]; then
    cp -a "$BK/src/$name" "$APP/${FILES[$name]}"
    echo "  restored ${FILES[$name]}"
  fi
done
[ -f "$BK/Cargo.toml.bak" ] && cp -a "$BK/Cargo.toml.bak" "$APP/rust-src/Cargo.toml" && echo "  restored rust-src/Cargo.toml"

echo "=== Rebuild (Rust + UI) + recreate ==="
docker compose --env-file .env -f docker-compose.yml build bal-server bal-ui
docker compose --env-file .env -f docker-compose.yml up -d --force-recreate bal-server bal-pusher bal-ui

if [ "$WITH_DATA" = "--with-data" ]; then
  echo "=== Restore DB/keys snapshot (--with-data) ==="
  cp -a "$DATA" "$DATA.pre-rollback-$(date +%Y%m%d-%H%M%S)"
  rm -rf "$DATA"
  cp -a "$BK/data" "$DATA"
fi

sleep 6
echo -n "version after rollback: "; curl -s --max-time 8 http://localhost:9140/api/version || echo "(no answer)"; echo
docker compose -f docker-compose.yml ps
echo "Rollback complete (timestamp $TS)."
