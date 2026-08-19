#!/bin/bash
# =============================================================================
# entrypoint-server.sh — Startup script for bal-server container
# =============================================================================
# Responsibilities:
#   1. Create persistent data and key directories
#   2. Generate an Ed25519 keypair on first run (used for signed stats reports)
#   3. Wait until the data directory is writable (Umbrel volume timing)
#   4. Export all required environment variables for the Rust binary
#   5. Exec into bal-server (replaces this shell process)
# =============================================================================

set -euo pipefail

# --- Paths (all inside the persistent volume mounted at /data) ---
DATA_DIR="${BAL_SERVER_DB_FILE%/*}"   # strip filename from DB path → directory
DATA_DIR="${DATA_DIR:-/data}"
KEY_DIR="/data/keys"

# Fallback: if BAL_SERVER_DB_FILE is just a filename with no path, use /data
if [[ "$DATA_DIR" == "" || "$DATA_DIR" == "." ]]; then
  DATA_DIR="/data"
fi

log() { echo "[entrypoint-server] $*"; }

# --- 1. Wait for /data to be writable (Docker volume may lag on slow ARM) ---
log "Waiting for data volume at ${DATA_DIR}…"
MAX_WAIT=30
elapsed=0
while ! mkdir -p "${DATA_DIR}" 2>/dev/null; do
  if (( elapsed >= MAX_WAIT )); then
    echo "[entrypoint-server] ERROR: /data volume never became writable after ${MAX_WAIT}s" >&2
    exit 1
  fi
  sleep 1
  (( elapsed++ )) || true
done
mkdir -p "${KEY_DIR}"
log "Data directory ready: ${DATA_DIR}"

# --- 2. Generate Ed25519 keypair on first run ---
# The private key is used by bal-pusher to sign stats reports.
# The public key is served at GET /.pub_key.pem for clients to verify.
PRIV_KEY="${KEY_DIR}/private_key.pem"
PUB_KEY="${KEY_DIR}/public_key.pem"

if [[ ! -f "${PRIV_KEY}" ]]; then
  log "Generating Ed25519 keypair (first run)…"
  openssl genpkey -algorithm ED25519 -out "${PRIV_KEY}"
  openssl pkey   -in "${PRIV_KEY}" -pubout -out "${PUB_KEY}"
  chmod 600 "${PRIV_KEY}"
  log "Keys written to ${KEY_DIR}/"
else
  log "Keypair already exists — skipping generation."
fi

# Sanity-check: both files must exist before starting bal-server
if [[ ! -f "${PUB_KEY}" ]]; then
  echo "[entrypoint-server] ERROR: Public key not found at ${PUB_KEY}" >&2
  exit 1
fi

# --- 3. Export all config as environment variables for the Rust binary ---
# The Rust binary reads these at startup via std::env::var().
# Most are already in the docker-compose env; we set defaults here for safety.

export BAL_SERVER_DB_FILE="${BAL_SERVER_DB_FILE:-/data/bal.db}"
export BAL_SERVER_PUB_KEY_PATH="${PUB_KEY}"
export BAL_SERVER_BIND_ADDRESS="${BAL_SERVER_BIND_ADDRESS:-0.0.0.0}"
export BAL_SERVER_BIND_PORT="${BAL_SERVER_BIND_PORT:-9137}"
export BAL_SERVER_INFO="${BAL_SERVER_INFO:-Bitcoin After Life — Will Executor (Umbrel)}"
export BAL_SERVER_EXPOSE_STATS="${BAL_SERVER_EXPOSE_STATS:-true}"

# Actix global rate limiter (upstream 0.3.x DoS protection). NOTE: despite the
# name, PER_SEC maps to Actix's `seconds_per_request` — i.e. SECONDS BETWEEN
# requests, not requests/second. So keep it at 1 (= 1 req/s sustained, the
# floor) and instead raise the BURST: the limiter keys on peer IP, and behind
# nginx/cloudflared every request shares ONE bucket, while the SAFE21 dashboard
# fires ~15-20 requests per page load (it does not poll). A large burst absorbs
# several page loads; nginx still rate-limits the sensitive public endpoints
# (pushtxs/searchtx) at the proxy layer, so DoS protection is preserved.
export BAL_SERVER_ACTIX_PUSHTXS_PER_SEC="${BAL_SERVER_ACTIX_PUSHTXS_PER_SEC:-1}"
export BAL_SERVER_ACTIX_PUSHTXS_BURST="${BAL_SERVER_ACTIX_PUSHTXS_BURST:-300}"

# Per-network: map from generic Umbrel env vars to BAL-specific ones
# Mainnet
export BAL_SERVER_BITCOIN_ADDRESS="${BAL_SERVER_BITCOIN_ADDRESS:-}"
export BAL_SERVER_BITCOIN_FIXED_FEE="${BAL_SERVER_BITCOIN_FIXED_FEE:-50000}"
# Signet
export BAL_SERVER_SIGNET_ADDRESS="${BAL_SERVER_SIGNET_ADDRESS:-}"
export BAL_SERVER_SIGNET_FIXED_FEE="${BAL_SERVER_SIGNET_FIXED_FEE:-50000}"
# Testnet
export BAL_SERVER_TESTNET_ADDRESS="${BAL_SERVER_TESTNET_ADDRESS:-}"
export BAL_SERVER_TESTNET_FIXED_FEE="${BAL_SERVER_TESTNET_FIXED_FEE:-50000}"
# Regtest
export BAL_SERVER_REGTEST_ADDRESS="${BAL_SERVER_REGTEST_ADDRESS:-}"
export BAL_SERVER_REGTEST_FIXED_FEE="${BAL_SERVER_REGTEST_FIXED_FEE:-0}"

# --- 3b. Bridge dashboard settings (settings.json) over the env config ---
# The dashboard (umbrel_api set_settings) persists the user's bitcoin address,
# fee and server description to settings.json next to the database. Upstream's
# server config is immutable at runtime, so here we translate those persisted
# values into the env vars the server reads at startup. Consequence: a settings
# change made in the UI takes effect on the next container restart.
SETTINGS_FILE="$(dirname "${BAL_SERVER_DB_FILE}")/settings.json"
if [ -f "${SETTINGS_FILE}" ] && command -v jq >/dev/null 2>&1; then
  s_addr="$(jq -r '.address // empty' "${SETTINGS_FILE}" 2>/dev/null)"
  s_fee="$(jq -r '.fee // empty'     "${SETTINGS_FILE}" 2>/dev/null)"
  s_info="$(jq -r '.info // empty'   "${SETTINGS_FILE}" 2>/dev/null)"
  [ -n "${s_addr}" ] && export BAL_SERVER_BITCOIN_ADDRESS="${s_addr}"
  [ -n "${s_fee}" ] && [ "${s_fee}" != "0" ] && export BAL_SERVER_BITCOIN_FIXED_FEE="${s_fee}"
  [ -n "${s_info}" ] && export BAL_SERVER_INFO="${s_info}"
  log "Applied settings.json over env (address/fee/info)"
fi

log "Starting bal-server on ${BAL_SERVER_BIND_ADDRESS}:${BAL_SERVER_BIND_PORT}…"
log "Database: ${BAL_SERVER_DB_FILE}"

# --- 4. Exec into the Rust binary ---
# Using exec replaces the shell with the binary, so Docker signals (SIGTERM etc.)
# go directly to bal-server instead of being caught by bash.
exec /usr/local/bin/bal-server
