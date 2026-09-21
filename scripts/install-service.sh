#!/usr/bin/env bash
# =============================================================================
# install-service.sh — Installs the boot auto-start hook
#
# Ensures the Docker containers start automatically after every reboot
# (power cut, umbrelOS update), without any manual intervention.
#
# IMPORTANT — why this is NOT a systemd unit:
# umbrelOS runs its root filesystem on an overlay whose upper layer is
# discarded on reboot, so a unit written to /etc/systemd/system disappears.
# That approach was tried on 12 Sep 2026 and was gone after the next reboot.
# The supported, persistent mechanism is umbrelOS's custom pre-start hook:
# /opt/umbrel-custom-hooks/run-pre-start (shipped in the OS image, so it
# survives OS updates) runs a user script from the persistent /home path.
# See scripts/umbrel-pre-start-hook.sh and AGENTS.md.
#
# Usage (from the app root directory, on the device):
#   sudo bash scripts/install-service.sh
# =============================================================================

set -euo pipefail

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HOOK_DIR=/home/umbrel/umbrel/custom-hooks
HOOK="${HOOK_DIR}/pre-start"
SRC="${APP_DIR}/scripts/umbrel-pre-start-hook.sh"

echo "[install-service] App directory: $APP_DIR"

if [[ ! -f "$SRC" ]]; then
  echo "[install-service] ERROR: $SRC not found" >&2
  exit 1
fi

# Refuse to clobber an unrelated hook someone else installed: this path is a
# single shared extension point, not ours by right.
if [[ -f "$HOOK" ]] && ! grep -q "bal-umbrel" "$HOOK"; then
  echo "[install-service] ERROR: $HOOK already exists and is not ours." >&2
  echo "[install-service] Inspect it and merge by hand; refusing to overwrite." >&2
  exit 1
fi

mkdir -p "$HOOK_DIR"
# Strip CR: a \r in the shebang makes the kernel look for '/bin/bash\r'.
tr -d '\r' < "$SRC" > "$HOOK"
chmod +x "$HOOK"

echo "[install-service] Installed: $HOOK"
echo

# Verify the OS-side wrapper that will call us is actually present.
if [[ -x /opt/umbrel-custom-hooks/run-pre-start ]]; then
  echo "[install-service] OK: umbrelOS pre-start wrapper found."
else
  echo "[install-service] WARNING: /opt/umbrel-custom-hooks/run-pre-start is missing." >&2
  echo "[install-service] This umbrelOS version may not support custom hooks." >&2
fi

echo
echo "[install-service] Done. The stack will come up automatically on next boot."
echo "[install-service] Boot log: /home/umbrel/umbrel/app-data/bal-umbrel/autostart.log"
