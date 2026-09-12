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
SERVICE_FILE=/etc/systemd/system/bal-umbrel.service

echo "[install-service] App directory: $APP_DIR"

# WHY THIS EXISTS: this app is deployed manually (not through Umbrel's official
# App Store install flow), so it is not in umbreld's own app registry. On a
# reboot or OS-level update, umbreld reconciles ITS registered apps' containers
# but has no knowledge of this one — its containers can end up fully removed
# (not just stopped) and never recreated. This systemd unit is an independent
# safety net: it brings the compose stack back up on every boot, using the
# exact same flags deploy-update.sh uses, regardless of what umbreld does.
cat > "$SERVICE_FILE" << EOF
[Unit]
Description=Bitcoin After Life Will Executor
Requires=umbrel.service docker.service
After=umbrel.service docker.service

[Service]
Type=oneshot
RemainAfterExit=yes
WorkingDirectory=${APP_DIR}
ExecStart=/usr/bin/docker compose --env-file .env -f docker-compose.yml up -d
ExecStop=/usr/bin/docker compose --env-file .env -f docker-compose.yml down

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable bal-umbrel.service
systemctl start bal-umbrel.service

echo "[install-service] Done."
systemctl status bal-umbrel.service --no-pager
