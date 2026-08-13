#!/bin/bash
# =============================================================================
# entrypoint-pusher.sh — Startup script for bal-pusher container
# =============================================================================
# Responsibilities:
#   1. Wait for bal-server to initialize the SQLite database
#   2. Run check-zmq.sh to probe and optionally configure ZMQ
#   3. Map Umbrel Bitcoin Core credentials to BAL_PUSHER_* env vars
#   4. Exec into bal-pusher with the correct network argument
# =============================================================================

set -euo pipefail

log()  { echo "[entrypoint-pusher] $*"; }
warn() { echo "[entrypoint-pusher] WARNING: $*" >&2; }

DB_FILE="${BAL_PUSHER_DB_FILE:-/data/bal.db}"
NETWORK="${BAL_PUSHER_NETWORK:-bitcoin}"

# =============================================================================
# Step 1 — Wait for the SQLite database to exist
# (bal-server creates it on first startup; bal-pusher must start after it)
# =============================================================================
log "Waiting for database at ${DB_FILE}…"
MAX_WAIT=60
elapsed=0
while [[ ! -f "${DB_FILE}" ]]; do
  if (( elapsed >= MAX_WAIT )); then
    echo "[entrypoint-pusher] ERROR: Database never appeared at ${DB_FILE} after ${MAX_WAIT}s" >&2
    echo "[entrypoint-pusher] Make sure bal-server started correctly." >&2
    exit 1
  fi
  sleep 2
  (( elapsed += 2 )) || true
done
log "Database found (${DB_FILE}) ✓"

# Small additional wait to let bal-server finish schema creation
sleep 2

# =============================================================================
# Step 2 — Map Umbrel's Bitcoin Core credentials to BAL_PUSHER_* vars
# =============================================================================
# Umbrel injects:
#   APP_BITCOIN_NODE_IP   — IP address of the bitcoin node container
#   APP_BITCOIN_RPC_PORT  — RPC port (usually 8332)
#   APP_BITCOIN_RPC_USER  — RPC username
#   APP_BITCOIN_RPC_PASS  — RPC password
#
# bal-pusher reads:
#   BAL_PUSHER_BITCOIN_HOST     — full http:// URL
#   BAL_PUSHER_BITCOIN_PORT     — port number
#   BAL_PUSHER_BITCOIN_RPC_USER — username
#   BAL_PUSHER_BITCOIN_RPC_PASSWORD — password

BITCOIN_IP="${APP_BITCOIN_NODE_IP:-}"
BITCOIN_RPC_PORT="${APP_BITCOIN_RPC_PORT:-8332}"
BITCOIN_RPC_USER="${APP_BITCOIN_RPC_USER:-}"
BITCOIN_RPC_PASS="${APP_BITCOIN_RPC_PASS:-}"

if [[ -z "$BITCOIN_IP" ]]; then
  warn "APP_BITCOIN_NODE_IP is not set."
  warn "bal-pusher may not be able to reach Bitcoin Core."
  warn "Check that the bitcoin dependency is enabled in Umbrel."
fi

# Set the HOST variable with http:// prefix (required by bitcoincore-rpc crate)
export BAL_PUSHER_BITCOIN_HOST="http://${BITCOIN_IP}"
export BAL_PUSHER_BITCOIN_PORT="${BITCOIN_RPC_PORT}"
export BAL_PUSHER_BITCOIN_RPC_USER="${BITCOIN_RPC_USER}"
export BAL_PUSHER_BITCOIN_RPC_PASSWORD="${BITCOIN_RPC_PASS}"

# -----------------------------------------------------------------------------
# Cookie-file authentication (PREFERRED on Umbrel)
# -----------------------------------------------------------------------------
# The bitcoincore-rpc Rust crate (v0.19) can fail HTTP-Basic auth (HTTP 401)
# with Umbrel's auto-generated RPC password (it contains base64 chars like '='
# and '+'). The Bitcoin cookie file is the robust, native fallback.
#
# Umbrel mounts the Bitcoin data dir read-only at /bitcoin-data, so the cookie
# is normally available at /bitcoin-data/.cookie. If found, we expose it via
# BAL_PUSHER_BITCOIN_COOKIE_FILE so the Rust binary uses Auth::CookieFile.
COOKIE_CANDIDATES=(
  "${BAL_PUSHER_BITCOIN_COOKIE_FILE:-}"
  "/bitcoin-data/.cookie"
  "/bitcoin-data/.bitcoin/.cookie"
)
for c in "${COOKIE_CANDIDATES[@]}"; do
  if [[ -n "$c" && -r "$c" ]]; then
    export BAL_PUSHER_BITCOIN_COOKIE_FILE="$c"
    log "Using Bitcoin cookie file: $c"
    break
  fi
done
if [[ -z "${BAL_PUSHER_BITCOIN_COOKIE_FILE:-}" ]]; then
  warn "No readable Bitcoin cookie file found — will rely on RPC user/password."
  warn "If you see HTTP 401 errors, mount the Bitcoin data dir (APP_BITCOIN_DATA_DIR)."
fi

# ZMQ listener endpoint.
# IMPORTANT: the Rust binary reads BAL_PUSHER_BITCOIN_ZMQ_HASHBLOCK for the
# mainnet network params (parse_env_netconfig). We set BOTH variables so the
# value is honored regardless of which one the binary picks up.
ZMQ_HOST="${BITCOIN_IP}"
ZMQ_PORT="${BAL_ZMQ_PORT:-28332}"
export BAL_PUSHER_ZMQ_LISTENER="tcp://${ZMQ_HOST}:${ZMQ_PORT}"
export BAL_PUSHER_BITCOIN_ZMQ_HASHBLOCK="${BAL_PUSHER_BITCOIN_ZMQ_HASHBLOCK:-tcp://${ZMQ_HOST}:${ZMQ_PORT}}"

log "Bitcoin RPC: http://${BITCOIN_IP}:${BITCOIN_RPC_PORT}"
log "ZMQ listener: ${BAL_PUSHER_ZMQ_LISTENER}"
log "Network: ${NETWORK}"

# =============================================================================
# Step 3 — Check ZMQ and auto-configure if needed
# =============================================================================
# This script probes the ZMQ port and optionally adds the config directive.
# It writes /usr/share/nginx/html/zmq-status.json for the UI to read.
# We intentionally DON'T exit on failure here — we still try to start
# bal-pusher so it can process any already-mature transactions on startup.
log "Running ZMQ health check…"
if [[ -x /scripts/check-zmq.sh ]]; then
  /scripts/check-zmq.sh || warn "ZMQ check completed with warnings (see above)."
else
  warn "/scripts/check-zmq.sh not found or not executable."
fi

# =============================================================================
# Step 4 — Exec into bal-pusher
# =============================================================================
log "Starting bal-pusher for network: ${NETWORK}"

# Pass the network as a positional argument (bal-pusher reads args[1])
exec /usr/local/bin/bal-pusher "${NETWORK}"
