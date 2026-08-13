#!/bin/bash
# =============================================================================
# entrypoint-ui.sh — Startup script for the nginx UI container
# =============================================================================
# Responsibilities:
#   1. Read the Tor .onion hostname from Umbrel's tor data directory
#   2. Write /usr/share/nginx/html/tor-address.json for the dashboard to read
#   3. Copy the nginx config to the right place
#   4. Start nginx in foreground mode (so Docker can manage the process)
# =============================================================================

set -uo pipefail

UI_STATIC_DIR="/usr/share/nginx/html"
TOR_HOSTNAME_FILE="${TOR_HOSTNAME_PATH:-/tor/hostname}"
TOR_STATUS_FILE="${UI_STATIC_DIR}/tor-address.json"

log() { echo "[entrypoint-ui] $*"; }

# =============================================================================
# Step 1 — Read Tor .onion hostname
# =============================================================================
# Umbrel writes the app's Tor hostname to a file under the Tor data dir.
# The path is mounted as a volume in docker-compose.yml.

mkdir -p "${UI_STATIC_DIR}"

if [[ -f "${TOR_HOSTNAME_FILE}" ]]; then
  TOR_ADDR=$(cat "${TOR_HOSTNAME_FILE}" | tr -d '[:space:]')
  if [[ -n "${TOR_ADDR}" ]]; then
    log "Tor address found: ${TOR_ADDR}"
    printf '{"address":"%s"}\n' "${TOR_ADDR}" > "${TOR_STATUS_FILE}"
  else
    log "Tor hostname file is empty — Tor may not be configured."
    printf '{"address":""}\n' > "${TOR_STATUS_FILE}"
  fi
else
  log "Tor hostname file not found at ${TOR_HOSTNAME_FILE}."
  log "This is normal if Tor is not enabled for this app."
  printf '{"address":""}\n' > "${TOR_STATUS_FILE}"
fi

# =============================================================================
# Step 2 — Ensure zmq-status.json exists (UI reads it on load)
# =============================================================================
# The pusher writes this file; but if the pusher hasn't started yet,
# we pre-create it as "pending" so the UI doesn't show an error.
ZMQ_STATUS_FILE="${UI_STATIC_DIR}/zmq-status.json"
if [[ ! -f "${ZMQ_STATUS_FILE}" ]]; then
  printf '{"status":"pending"}\n' > "${ZMQ_STATUS_FILE}"
fi

# =============================================================================
# Step 3 — Start nginx
# =============================================================================
log "Starting nginx…"
nginx -g 'daemon off;' &
NGINX_PID=$!

# Tor may not be ready yet — watch the hostname file and update tor-address.json
(while true; do
  if [[ -f "${TOR_HOSTNAME_FILE}" ]]; then
    TOR_ADDR=$(cat "${TOR_HOSTNAME_FILE}" 2>/dev/null | tr -d '[:space:]')
    if [[ -n "${TOR_ADDR}" ]]; then
      printf '{"address":"%s"}\n' "${TOR_ADDR}" > "${TOR_STATUS_FILE}"
      log "Tor address updated: ${TOR_ADDR}"
      break
    fi
  fi
  sleep 5
done) &

wait "${NGINX_PID}"
