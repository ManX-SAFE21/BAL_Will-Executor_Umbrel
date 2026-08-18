#!/bin/bash
# =============================================================================
# deploy-update.sh — Safe deploy of the Will Executor on Umbrel.
#
# This version performs a FULL-SOURCE sync (used for the Actix 0.3.2 upgrade and
# every deploy after): it replaces rust-src/, ui/, scripts/ and docker/ in the
# app tree from a staged tarball, rebuilds BOTH images, recreates the 3
# containers, and smoke-tests. A complete backup is taken first and a matching
# rollback-update.sh restores it.
#
# Staged input (scp'd from your PC before running this):
#   /home/umbrel/staged-src.tar.gz   containing rust-src/ ui/ scripts/ docker/
#
# RUN ON THE UMBREL BOX:  sudo bash deploy-update.sh
# Roll back with:         sudo bash rollback-update.sh <timestamp>
# =============================================================================
set -euo pipefail

BASE=/home/umbrel/umbrel/app-data/bal-umbrel
APP=$BASE/bal-umbrel
DATA=$BASE/data
STAGE_TAR=/home/umbrel/staged-src.tar.gz
TS=$(date +%Y%m%d-%H%M%S)
BK=$BASE/deploy-backups/$TS

echo "=== [1/5] Backup ($TS) ==="
mkdir -p "$BK"
cp -a "$DATA" "$BK/data"
# Snapshot the source dirs we are about to overwrite, for a clean rollback.
tar -C "$APP" -czf "$BK/src-before.tar.gz" rust-src ui scripts docker 2>/dev/null || true
cp -a "$APP/docker-compose.yml" "$BK/" 2>/dev/null || true
cp -a "$APP/.env"               "$BK/" 2>/dev/null || true
echo "Backup saved to: $BK"

echo "=== [2/5] Sync staged source ==="
if [ ! -f "$STAGE_TAR" ]; then
  echo "ERROR: staged tarball not found at $STAGE_TAR" >&2
  exit 1
fi
tar -xzf "$STAGE_TAR" -C "$APP"
echo "Applied $STAGE_TAR"
echo -n "Version: "; grep -m1 '^version' "$APP/rust-src/Cargo.toml" || true

echo "=== [3/5] Rebuild (Rust + UI) ==="
cd "$APP"
docker compose --env-file .env -f docker-compose.yml build bal-server bal-ui

echo "=== [4/5] Recreate containers ==="
docker compose --env-file .env -f docker-compose.yml up -d --force-recreate bal-server bal-pusher bal-ui

echo "=== [5/5] Smoke test ==="
sleep 8
echo -n "version   : "; curl -s --max-time 8 http://localhost:9140/api/version || echo "(no answer)"; echo
echo -n "zmq-status: "; curl -s --max-time 8 http://localhost:9140/zmq-status.json || echo "(no answer)"; echo
echo -n "txlist    : "; curl -s --max-time 8 "http://localhost:9140/api/txlist?show_failed=1&limit=1" | head -c 160 || echo "(no answer)"; echo
echo -n "txstats   : "; curl -s --max-time 8 "http://localhost:9140/api/txstats/bitcoin" | head -c 160 || echo "(no answer)"; echo
echo
docker compose -f docker-compose.yml ps

echo
echo "============================================================"
echo "DONE. Backup timestamp: $TS"
echo "If the app misbehaves, roll back with:"
echo "   sudo bash rollback-update.sh $TS"
echo "============================================================"
