#!/bin/bash
# =============================================================================
# deploy-update.sh — Safe deploy of the Will Executor on Umbrel.
#
# WHAT IT DOES:
#   1. Full backup: data dir (DB + keys + settings), docker-compose.yml, .env,
#      and a copy of every source file it is about to overwrite (for rollback).
#   2. Syncs updated source files staged in /home/umbrel/staged/ into the app
#      tree (copy them there with scp from your PC before running this).
#   3. Ensures the version string, then rebuilds BOTH images (Rust + UI) —
#      the UI image bakes in ui/index.html, so a rebuild is required to pick up
#      dashboard changes.
#   4. Recreates the 3 containers and smoke-tests (incl. the new /api/txlist).
#
# Staged files (optional; only those present are applied):
#   /home/umbrel/staged/bal-server.rs -> rust-src/src/bin/bal-server.rs
#   /home/umbrel/staged/bal-pusher.rs -> rust-src/src/bin/bal-pusher.rs
#   /home/umbrel/staged/index.html    -> ui/index.html
#
# RUN IT ON THE UMBREL BOX (sudo prompts for the password once):
#   sudo bash deploy-update.sh
#
# Roll back with:  sudo bash rollback-update.sh <timestamp>   (printed at the end)
# =============================================================================
set -euo pipefail

BASE=/home/umbrel/umbrel/app-data/bal-umbrel
APP=$BASE/bal-umbrel
DATA=$BASE/data
STAGE=/home/umbrel/staged
TS=$(date +%Y%m%d-%H%M%S)
BK=$BASE/deploy-backups/$TS
TARGET_VERSION="0.2.3-umbrel.13"

# staged filename -> path relative to $APP
declare -A FILES=(
  [bal-server.rs]=rust-src/src/bin/bal-server.rs
  [bal-pusher.rs]=rust-src/src/bin/bal-pusher.rs
  [index.html]=ui/index.html
)

echo "=== [1/5] Backup ($TS) ==="
mkdir -p "$BK/src"
cp -a "$DATA" "$BK/data"
cp -a "$APP/docker-compose.yml" "$BK/" 2>/dev/null || true
cp -a "$APP/.env"               "$BK/" 2>/dev/null || true
cp -a "$APP/rust-src/Cargo.toml" "$BK/Cargo.toml.bak" 2>/dev/null || true
for name in "${!FILES[@]}"; do
  rel=${FILES[$name]}
  [ -f "$APP/$rel" ] && cp -a "$APP/$rel" "$BK/src/$name"
done
echo "Backup saved to: $BK"

echo "=== [2/5] Sync staged source ==="
if [ -d "$STAGE" ]; then
  for name in "${!FILES[@]}"; do
    if [ -f "$STAGE/$name" ]; then
      cp -a "$STAGE/$name" "$APP/${FILES[$name]}"
      echo "  applied $name -> ${FILES[$name]}"
    fi
  done
else
  echo "  no staging dir ($STAGE) — deploying current on-box source"
fi

echo "=== [3/5] Ensure version string ==="
cd "$APP"
sed -i -E "s/^version = \".*\"/version = \"${TARGET_VERSION}\"/" rust-src/Cargo.toml
echo "Cargo.toml: $(grep -m1 '^version' rust-src/Cargo.toml)"

echo "=== [4/5] Rebuild (Rust + UI) + recreate ==="
docker compose --env-file .env -f docker-compose.yml build bal-server bal-ui
docker compose --env-file .env -f docker-compose.yml up -d --force-recreate bal-server bal-pusher bal-ui

echo "=== [5/5] Smoke test ==="
sleep 6
echo -n "version   : "; curl -s --max-time 8 http://localhost:9140/api/version || echo "(no answer)"; echo
echo -n "zmq-status: "; curl -s --max-time 8 http://localhost:9140/zmq-status.json || echo "(no answer)"; echo
echo -n "txlist    : "; curl -s --max-time 8 "http://localhost:9140/api/txlist?limit=1" | head -c 200 || echo "(no answer)"; echo
echo
docker compose -f docker-compose.yml ps

echo
echo "============================================================"
echo "DONE. Backup timestamp: $TS"
echo "If the app misbehaves, roll back with:"
echo "   sudo bash rollback-update.sh $TS"
echo "============================================================"
