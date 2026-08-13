#!/bin/bash
# reinstall-umbrel.sh — Re-deploy Will Executor after Umbrel OS update
# Run this ON the Umbrel box (via SSH):  bash reinstall-umbrel.sh
# =============================================================================
set -euo pipefail

cd /home/umbrel/bitcoin-after-life-will-executor

# Recreate .env if missing
if [ ! -f .env ]; then
  echo "Creazione .env..."
  cat > .env << 'ENVEOF'
APP_DATA_DIR=/home/umbrel/umbrel/app-data/bal-umbrel
APP_BITCOIN_NODE_IP=10.21.21.8
APP_BITCOIN_RPC_PORT=8332
APP_BITCOIN_RPC_USER=umbrel
APP_BITCOIN_RPC_PASS="$(grep APP_BITCOIN_RPC_PASS /home/umbrel/umbrel/app-data/bitcoin/.env | cut -d= -f2-)"
BAL_ZMQ_PORT=28334
BAL_PUSHER_BITCOIN_ZMQ_HASHBLOCK=tcp://10.21.21.8:28334
APP_BITCOIN_DATA_DIR=/home/umbrel/umbrel/app-data/bitcoin/data/bitcoin
BAL_PUSHER_BITCOIN_COOKIE_FILE=/bitcoin-data/.cookie
BAL_BITCOIN_ADDRESS=
BAL_BITCOIN_FEE=11011
BAL_LOG_LEVEL=info
BAL_PUBLIC_URL=
APP_PORT=9140
ENVEOF
  echo ".env creato"
fi

# Build UI image (includes latest index.html fixes)
echo "Build UI image..."
echo "$SUDO_PASSWORD" | sudo -S docker compose build ui

# Start all services
echo "Avvio container..."
echo "$SUDO_PASSWORD" | sudo -S docker compose up -d

echo ""
echo "=== Verifica ==="
echo "$SUDO_PASSWORD" | sudo -S docker ps --format 'table {{.Names}}\t{{.Status}}' | grep bal-will
echo ""
echo "App disponibile su: http://umbrel.local:9140/"
