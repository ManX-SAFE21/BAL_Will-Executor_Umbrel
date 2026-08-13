#!/usr/bin/env bash
# =============================================================================
# install-service.sh — Installs the systemd service
#
# This ensures the Docker containers start automatically after
# every Umbrel / system reboot, without any manual intervention.
#
# Usage (from the app root directory):
#   sudo bash scripts/install-service.sh
# =============================================================================

set -euo pipefail

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SERVICE_FILE=/etc/systemd/system/bal-will.service

echo "[install-service] App directory: $APP_DIR"

# Write the service unit
cat > "$SERVICE_FILE" << EOF
[Unit]
Description=Bitcoin After Life Will Executor
Requires=umbrel.service docker.service
After=umbrel.service docker.service

[Service]
Type=oneshot
RemainAfterExit=yes
WorkingDirectory=${APP_DIR}
ExecStart=/usr/bin/docker compose up -d
ExecStop=/usr/bin/docker compose down

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable bal-will.service
systemctl start bal-will.service

echo "[install-service] Done."
systemctl status bal-will.service --no-pager
