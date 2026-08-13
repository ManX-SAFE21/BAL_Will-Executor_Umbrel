#!/bin/bash
# =============================================================================
# rollback-update.sh — Revert a deploy-update.sh run.
#
# USAGE (on the Umbrel box):
#   sudo bash rollback-update.sh <timestamp>
#
# <timestamp> is the value printed at the end of deploy-update.sh, e.g.
#   sudo bash rollback-update.sh 20260814-2210
#
# It restores the previous image (fast tag), the previous source files, and
# — only if you pass --with-data — the previous database/keys snapshot.
# By default the live database is KEPT (you almost never want to roll back the
# DB, since new transactions may have arrived since the deploy).
# =============================================================================
set -euo pipefail

TS="${1:-}"
WITH_DATA="${2:-}"
if [ -z "$TS" ]; then echo "Usage: sudo bash rollback-update.sh <timestamp> [--with-data]"; exit 1; fi

BASE=/home/umbrel/umbrel/app-data/bal-umbrel
APP=$BASE/bal-umbrel
DATA=$BASE/data
BK=$BASE/deploy-backups/$TS
if [ ! -d "$BK" ]; then echo "Backup not found: $BK"; exit 1; fi

cd "$APP"
IMG=$(grep -m1 -E 'image:.*rust' docker-compose.yml | sed -E 's/.*image:[[:space:]]*//' | tr -d '\r"')
IMG=${IMG:-bal-umbrel-rust:latest}

echo "=== Restore source files ==="
cp -a "$BK/Cargo.toml.bak"    rust-src/Cargo.toml
cp -a "$BK/bal-pusher.rs.bak" rust-src/src/bin/bal-pusher.rs

echo "=== Restore previous image (fast) ==="
if docker image inspect "${IMG%:*}:rollback-$TS" >/dev/null 2>&1; then
  docker tag "${IMG%:*}:rollback-$TS" "$IMG"
  echo "Re-tagged ${IMG%:*}:rollback-$TS -> $IMG"
else
  echo "No fast-rollback image; rebuilding from restored source..."
  docker compose --env-file .env -f docker-compose.yml build bal-server
fi

if [ "$WITH_DATA" = "--with-data" ]; then
  echo "=== Restore database/keys snapshot (--with-data) ==="
  cp -a "$DATA" "$DATA.pre-rollback-$(date +%Y%m%d-%H%M%S)"   # keep current just in case
  rm -rf "$DATA"
  cp -a "$BK/data" "$DATA"
fi

echo "=== Recreate containers ==="
docker compose --env-file .env -f docker-compose.yml up -d --force-recreate bal-server bal-pusher ui

sleep 6
echo -n "version after rollback: "; curl -s --max-time 8 http://localhost:9140/api/version || echo "(no answer)"; echo
docker compose -f docker-compose.yml ps
echo "Rollback complete (timestamp $TS)."
