#!/bin/bash
# =============================================================================
# check-zmq.sh — ZMQ health check and auto-configuration
# =============================================================================
# Called by entrypoint-pusher.sh before starting bal-pusher.
#
# What it does:
#   1. Probes the ZMQ hashblock port on the Bitcoin node with netcat.
#   2. If the port is OPEN → write zmq-status.json with {"status":"ok"} → done.
#   3. If the port is CLOSED:
#      a. Find bitcoin.conf (from $APP_BITCOIN_DATA_DIR or well-known paths).
#      b. If bitcoin.conf found and writable:
#           - Check if zmqpubhashblock is already configured.
#           - If not: APPEND the directive and write a RESTART_REQUIRED marker.
#           - Write zmq-status.json with {"status":"restart_required"}.
#      c. If bitcoin.conf NOT found or not writable:
#           - Write zmq-status.json with {"status":"error", "message":"..."}.
#           - Print instructions to the container log.
#
# The UI reads /usr/share/nginx/html/zmq-status.json on load and shows
# the appropriate banner to the user.
#
# Exit codes:
#   0 — ZMQ is working (or we successfully added the config and need a restart)
#   1 — Fatal error (ZMQ not working and we cannot help)
# =============================================================================

set -uo pipefail

BITCOIN_HOST="${APP_BITCOIN_NODE_IP:-}"
ZMQ_PORT="${BAL_ZMQ_PORT:-28332}"
ZMQ_DIRECTIVE="zmqpubhashblock=tcp://0.0.0.0:${ZMQ_PORT}"

# Where nginx serves static JSON files for the UI
UI_STATIC_DIR="/usr/share/nginx/html"

# Path to the bitcoin.conf inside the container (bitcoin data dir is mounted read-only)
# Umbrel mounts the Bitcoin data dir at /bitcoin-data in our container
BITCOIN_DATA_DIR="${APP_BITCOIN_DATA_DIR:-/bitcoin-data}"
BITCOIN_CONF_PATHS=(
  "/bitcoin-data/bitcoin.conf"
  "/bitcoin-data/.bitcoin/bitcoin.conf"
)

log()  { echo "[check-zmq] $*"; }
warn() { echo "[check-zmq] WARNING: $*" >&2; }
err()  { echo "[check-zmq] ERROR: $*" >&2; }

# --- Helper: write the zmq-status.json file read by the UI ---
# Writes to BOTH the local nginx dir (legacy) AND the shared /data volume
# (which the UI container mounts read-only at /app-data and serves to clients).
write_status_json() {
  local status="$1"
  local message="${2:-}"
  local json
  if [[ -n "$message" ]]; then
    json=$(printf '{"status":"%s","message":"%s"}\n' "$status" "$message")
  else
    json=$(printf '{"status":"%s"}\n' "$status")
  fi
  # Legacy location (pusher-local nginx dir, not shared)
  mkdir -p "${UI_STATIC_DIR}"
  printf '%s\n' "$json" > "${UI_STATIC_DIR}/zmq-status.json" 2>/dev/null || true
  # Shared volume — this is what the UI container actually serves
  mkdir -p "/data" 2>/dev/null || true
  printf '%s\n' "$json" > "/data/zmq-status.json" 2>/dev/null || true
  log "Wrote zmq-status.json: status=${status}"
}

# --- Helper: find an existing bitcoin.conf file ---
find_bitcoin_conf() {
  for path in "${BITCOIN_CONF_PATHS[@]}"; do
    if [[ -f "$path" ]]; then
      echo "$path"
      return 0
    fi
  done
  return 1
}

# =============================================================================
# Step 1 — Check if ZMQ port is reachable via netcat
# =============================================================================

if [[ -z "$BITCOIN_HOST" ]]; then
  warn "APP_BITCOIN_NODE_IP is not set — cannot probe ZMQ port."
  write_status_json "error" "APP_BITCOIN_NODE_IP not set. Check Umbrel app configuration."
  exit 0
fi

log "Probing ZMQ at ${BITCOIN_HOST}:${ZMQ_PORT}…"

# nc -z = port scan only (no data), -w3 = 3 second timeout
if nc -z -w3 "${BITCOIN_HOST}" "${ZMQ_PORT}" 2>/dev/null; then
  log "ZMQ port ${ZMQ_PORT} is OPEN on ${BITCOIN_HOST} ✓"
  write_status_json "ok"
  exit 0
fi

# Port is NOT open.
log "ZMQ port ${ZMQ_PORT} is NOT open on ${BITCOIN_HOST}."

# =============================================================================
# Step 2 — Find bitcoin.conf
# =============================================================================

BITCOIN_CONF=""
if BITCOIN_CONF=$(find_bitcoin_conf); then
  log "Found bitcoin.conf at: ${BITCOIN_CONF}"
else
  # bitcoin.conf not found in any expected location
  err "Could not find bitcoin.conf in mounted Bitcoin data directory."
  err "Expected it at one of: ${BITCOIN_CONF_PATHS[*]}"
  err ""
  err "To enable ZMQ manually, add this line to your bitcoin.conf:"
  err "  ${ZMQ_DIRECTIVE}"
  err ""
  err "On Umbrel: Settings → Bitcoin Node → Advanced → Edit Config"
  err "Then restart the Bitcoin node."

  write_status_json "error" \
    "ZMQ not detected. Add zmqpubhashblock=tcp://0.0.0.0:${ZMQ_PORT} to bitcoin.conf and restart Bitcoin."
  exit 0
fi

# =============================================================================
# Step 3 — Check if ZMQ is already in bitcoin.conf (but not yet active)
# =============================================================================

if grep -q "zmqpubhashblock" "${BITCOIN_CONF}" 2>/dev/null; then
  # The directive exists but the port is not open — Bitcoin Core needs a restart
  log "zmqpubhashblock already present in bitcoin.conf but port is closed."
  log "Bitcoin Core needs to be restarted to apply the ZMQ configuration."

  write_status_json "restart_required" \
    "ZMQ is configured in bitcoin.conf but Bitcoin Core needs a restart to activate it."
  exit 0
fi

# =============================================================================
# Step 4 — bitcoin.conf found, directive missing → auto-add it
# =============================================================================

# The bitcoin data dir is mounted READ-ONLY in our container (safety measure).
# We check explicitly and give a clear message if we cannot write.
if [[ ! -w "${BITCOIN_CONF}" ]]; then
  warn "bitcoin.conf is mounted read-only — cannot auto-configure ZMQ."
  warn ""
  warn "Add this line to ${BITCOIN_CONF} manually and restart Bitcoin Core:"
  warn "  ${ZMQ_DIRECTIVE}"
  warn ""
  warn "On Umbrel: Settings → Bitcoin Node → Advanced → Edit Config"

  write_status_json "error" \
    "ZMQ not configured. Add zmqpubhashblock=tcp://0.0.0.0:${ZMQ_PORT} to bitcoin.conf (read-only mount — manual action required)."
  exit 0
fi

# ---- Write the directive ----
log "Auto-adding ZMQ configuration to ${BITCOIN_CONF}…"

# Append a clearly delimited block so it's easy to find and remove later
{
  echo ""
  echo "# ----- Added automatically by Bitcoin After Life Will Executor -----"
  echo "# This enables the ZMQ hashblock notification required by bal-pusher."
  echo "# Safe to remove if you uninstall the Bitcoin After Life Will Executor app."
  echo "${ZMQ_DIRECTIVE}"
  echo "# -------------------------------------------------------------------"
} >> "${BITCOIN_CONF}"

log "ZMQ directive added to bitcoin.conf successfully."
log ""
log "╔══════════════════════════════════════════════════════════════════╗"
log "║  ACTION REQUIRED: Restart Bitcoin Core to activate ZMQ          ║"
log "║                                                                  ║"
log "║  The following line was added to bitcoin.conf:                  ║"
log "║    ${ZMQ_DIRECTIVE}"
log "║                                                                  ║"
log "║  On Umbrel: go to Settings → Bitcoin Node → Restart             ║"
log "║  After restart, bal-pusher will connect automatically.           ║"
log "╚══════════════════════════════════════════════════════════════════╝"

# The UI will show a prominent "restart required" banner
write_status_json "restart_required" \
  "ZMQ configuration was added to bitcoin.conf automatically. Please restart Bitcoin Core (Umbrel → Settings → Bitcoin Node → Restart) to activate it."

exit 0
