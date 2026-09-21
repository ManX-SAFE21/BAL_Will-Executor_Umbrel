#!/bin/bash
# =============================================================================
# umbrel-pre-start-hook.sh — installed as /home/umbrel/umbrel/custom-hooks/pre-start
#
# WHY THIS EXISTS
# ---------------
# This app is deployed manually, not through Umbrel's App Store install flow,
# so it is not in umbreld's app registry and umbreld never starts it. After any
# reboot (power cut, OS update) its containers are simply gone.
#
# A systemd unit in /etc/systemd/system does NOT work here: umbrelOS runs its
# root filesystem on an overlay whose upper layer is discarded on reboot, so
# anything written under /etc evaporates. (Learned the hard way: the unit
# installed on 12 Sep 2026 was missing after the 17 Sep power-cut reboot.)
# Only /home and /data are persistent bind mounts.
#
# umbrelOS provides exactly one supported, persistent extension point:
# /opt/umbrel-custom-hooks/run-pre-start (part of the OS image, so it returns
# with every OS update) executes this script from the persistent /home path.
#
# WHY IT BACKGROUNDS ITSELF
# -------------------------
# The wrapper runs us with `Before=umbrel.service`, i.e. it BLOCKS umbreld from
# starting until we return. But the network we need (umbrel_main_network) is
# created BY umbreld — waiting for it inline would deadlock until the 5-minute
# timeout. So we detach a waiter with setsid and return 0 immediately; umbreld
# boots normally and the waiter brings our stack up as soon as the shared
# network appears.
# =============================================================================

APP_DIR=/home/umbrel/umbrel/app-data/bal-umbrel/bal-umbrel
LOG=/home/umbrel/umbrel/app-data/bal-umbrel/autostart.log

# Detached so returning does not kill it, and so umbreld is never blocked.
setsid nohup bash -c '
  APP_DIR="'"${APP_DIR}"'"
  echo "=== $(date -Is) pre-start hook: waiting for umbrel_main_network ==="

  # umbreld creates the shared network during its own startup. 120 x 5s = 10min,
  # generous for a slow boot; give up rather than loop forever.
  for i in $(seq 1 120); do
    if docker network inspect umbrel_main_network >/dev/null 2>&1; then
      echo "$(date -Is) network present after ${i} attempt(s); starting stack"
      cd "${APP_DIR}" || { echo "$(date -Is) ERROR: ${APP_DIR} missing"; exit 1; }
      docker compose --env-file .env -f docker-compose.yml up -d
      echo "$(date -Is) done (exit $?)"
      exit 0
    fi
    sleep 5
  done

  echo "$(date -Is) ERROR: umbrel_main_network never appeared; giving up"
  exit 1
' >>"${LOG}" 2>&1 &

# Never block or fail umbreld startup, whatever happened above.
exit 0
