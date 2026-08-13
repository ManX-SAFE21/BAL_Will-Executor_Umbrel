#!/bin/bash
# =============================================================================
# deploy-update.sh — Safe in-place version update of the Will Executor on Umbrel
#
# WHAT IT DOES (in order):
#   1. Full backup: data dir (SQLite DB + Ed25519 keys + settings + logo),
#      current docker-compose.yml + .env, and a fast-rollback tag of the
#      currently-running image.
#   2. Applies the version alignment fix (0.2.3 -> 0.2.13-umbrel) to the
#      on-box source. Idempotent: safe to re-run.
#   3. Rebuilds the Rust image and recreates the 3 containers.
#   4. Smoke test.
#
# RUN IT ON THE UMBREL BOX (sudo will prompt for the password once):
#   sudo bash deploy-update.sh
#
# If anything looks wrong afterwards, run:  sudo bash rollback-update.sh <timestamp>
# (the timestamp is printed at the end of this script).
# =============================================================================
set -euo pipefail

BASE=/home/umbrel/umbrel/app-data/bal-umbrel
APP=$BASE/bal-umbrel
DATA=$BASE/data
TS=$(date +%Y%m%d-%H%M%S)
BK=$BASE/deploy-backups/$TS

echo "=== [1/4] Backup ($TS) ==="
mkdir -p "$BK"
cp -a "$DATA" "$BK/data"                              # DB + keys + settings + logo
cp -a "$APP/docker-compose.yml" "$BK/"      2>/dev/null || true
cp -a "$APP/.env"               "$BK/"      2>/dev/null || true
cp -a "$APP/rust-src/Cargo.toml"             "$BK/Cargo.toml.bak"
cp -a "$APP/rust-src/src/bin/bal-pusher.rs"  "$BK/bal-pusher.rs.bak"
echo "Backup saved to: $BK"

cd "$APP"
IMG=$(grep -m1 -E 'image:.*rust' docker-compose.yml | sed -E 's/.*image:[[:space:]]*//' | tr -d '\r"')
IMG=${IMG:-bal-umbrel-rust:latest}
echo "Current image: $IMG"
if docker image inspect "$IMG" >/dev/null 2>&1; then
  docker tag "$IMG" "${IMG%:*}:rollback-$TS"
  echo "Tagged fast-rollback image: ${IMG%:*}:rollback-$TS"
else
  echo "WARN: current image not found locally; rollback will rely on rebuild."
fi

echo "=== [2/4] Apply version fix (idempotent) ==="
# Target scheme: <upstream bal-server version>-umbrel.<packaging build>.
# Matches whatever "version = ..." is currently on the box (0.2.3, the
# previously-broken 0.2.13-umbrel, or an already-correct string) and
# rewrites it to the target below. Update TARGET_VERSION by hand when a
# new Umbrel-packaging build or upstream sync happens.
TARGET_VERSION="0.2.3-umbrel.13"
sed -i -E "s/^version = \".*\"/version = \"${TARGET_VERSION}\"/" rust-src/Cargo.toml
sed -i -E 's|^const VERSION: &str = "[^"]*";|const VERSION: \&str = env!("CARGO_PKG_VERSION");|' rust-src/src/bin/bal-pusher.rs
echo "Cargo.toml version now: $(grep -m1 '^version' rust-src/Cargo.toml)"

echo "=== [3/4] Rebuild + recreate ==="
docker compose --env-file .env -f docker-compose.yml build bal-server
docker compose --env-file .env -f docker-compose.yml up -d --force-recreate bal-server bal-pusher bal-ui

echo "=== [4/4] Smoke test ==="
sleep 6
echo -n "version   : "; curl -s --max-time 8 http://localhost:9140/api/version || echo "(no answer)"; echo
echo -n "zmq-status: "; curl -s --max-time 8 http://localhost:9140/zmq-status.json || echo "(no answer)"; echo
echo -n "btc info  : "; curl -s --max-time 8 http://localhost:9140/api/bitcoin/info | head -c 200 || echo "(no answer)"; echo
echo
docker compose -f docker-compose.yml ps

echo
echo "============================================================"
echo "DONE. Backup timestamp: $TS"
echo "If the app misbehaves, roll back with:"
echo "   sudo bash rollback-update.sh $TS"
echo "============================================================"
