#!/bin/bash
# =============================================================================
# rollback-update.sh — Revert a deploy-update.sh run by restoring the backed-up
# source tree and rebuilding both images.
#
# USAGE (on the Umbrel box):
#   sudo bash rollback-update.sh <timestamp> [--with-data]
#
# <timestamp> is printed at the end of deploy-update.sh.
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

echo "=== Restore source tree ==="
if [ -f "$BK/src-before.tar.gz" ]; then
  # Remove the dirs we manage, then restore the pre-deploy snapshot.
  rm -rf "$APP/rust-src" "$APP/ui" "$APP/scripts" "$APP/docker"
  tar -xzf "$BK/src-before.tar.gz" -C "$APP"
  echo "  restored rust-src/ ui/ scripts/ docker/"
else
  echo "  WARNING: no src-before.tar.gz in backup — source not restored"
fi

echo "=== Rebuild (Rust + UI) + recreate ==="
cd "$APP"
docker compose --env-file .env -f docker-compose.yml build bal-server bal-ui
docker compose --env-file .env -f docker-compose.yml up -d --force-recreate bal-server bal-pusher bal-ui

if [ "$WITH_DATA" = "--with-data" ]; then
  echo "=== Restore DB/keys snapshot (--with-data) ==="
  cp -a "$DATA" "$DATA.pre-rollback-$(date +%Y%m%d-%H%M%S)"
  rm -rf "$DATA"
  cp -a "$BK/data" "$DATA"
fi

sleep 8
echo -n "version after rollback: "; curl -s --max-time 8 http://localhost:9140/api/version || echo "(no answer)"; echo
docker compose -f docker-compose.yml ps
echo "Rollback complete (timestamp $TS)."
