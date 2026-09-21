#!/bin/bash
# =============================================================================
# umbrel-pre-start-hook.sh — installed as /home/umbrel/umbrel/custom-hooks/pre-start
#
# Brings the Will Executor stack up on every boot, and keeps it up.
#
# THE ROOT PROBLEM: umbreld WIPES CONTAINERS AT STARTUP
# -----------------------------------------------------
# This app is deployed manually, not through Umbrel's App Store flow, so it is
# not in umbreld's app registry. Early in its startup umbreld logs
#   [apps] Cleaning up old containers...
# and then removes **every** container and prunes **every** network — including
# umbrel_main_network — before recreating only the apps it knows about. Ours is
# not one of them, so it is destroyed and never comes back.
#
# That is why a stack started too early is pointless: it is wiped seconds later.
# Boot of 21 Sep 2026: our waiter had the stack up at 05:14:29, umbreld's
# cleanup ran at 05:14:31, the networks were pruned at 05:15:04, and the app was
# down. Waiting for umbrel_main_network is NOT a sufficient signal — the network
# that satisfied the check was the previous boot's, still present pre-cleanup.
#
# So we wait for umbreld to finish bringing its OWN apps up, and then supervise:
# if the stack disappears anyway, we put it back.
#
# WHY A HOOK, AND WHY systemd-run
# -------------------------------
# A unit in /etc/systemd/system does not survive — umbrelOS runs / on an overlay
# whose upper layer is discarded on reboot; only /home and /data persist.
# umbrelOS's supported entry point is /opt/umbrel-custom-hooks/run-pre-start
# (shipped in the OS image, so it returns after every OS update), which executes
# this script from the persistent /home path.
#
# That wrapper is Type=oneshot and runs Before=umbrel.service, so:
#   * we must return immediately — blocking it would stall umbreld, and what we
#     are waiting for is umbreld itself;
#   * a plain `… &` (even with setsid, which changes session but not cgroup) is
#     killed when systemd tears the oneshot unit's control group down.
# Hence systemd-run: the waiter becomes its own transient unit, in its own
# cgroup, untouched by that teardown.
# =============================================================================

APP_DIR=/home/umbrel/umbrel/app-data/bal-umbrel/bal-umbrel
LOG=/home/umbrel/umbrel/app-data/bal-umbrel/autostart.log
UNIT=bal-umbrel-autostart

# Container that proves umbreld has finished its cleanup and is starting apps:
# it is one of umbreld's own, so it only exists on the far side of the wipe.
UMBRELD_READY_CONTAINER=auth
OUR_CONTAINER=bal-umbrel-ui

# ---------------------------------------------------------------------------
# Waiter branch — runs inside the transient unit, detached from the hook.
# ---------------------------------------------------------------------------
if [ "${1:-}" = "--waiter" ]; then
    exec >>"${LOG}" 2>&1
    echo "=== $(date -Is) waiter started (unit ${UNIT}) ==="

    running() {
        [ "$(docker inspect -f '{{.State.Running}}' "$1" 2>/dev/null)" = "true" ]
    }

    start_stack() {
        cd "${APP_DIR}" || { echo "$(date -Is) ERROR: ${APP_DIR} missing"; return 1; }
        docker compose --env-file .env -f docker-compose.yml up -d
    }

    # --- 1. Wait for umbreld to be past its wipe and running its own apps ----
    # 180 x 5s = 15 min, generous for a slow boot but bounded.
    ready=0
    for i in $(seq 1 180); do
        if running "${UMBRELD_READY_CONTAINER}" \
           && docker network inspect umbrel_main_network >/dev/null 2>&1; then
            echo "$(date -Is) umbreld ready after ${i} check(s) (${UMBRELD_READY_CONTAINER} up)"
            ready=1
            break
        fi
        sleep 5
    done
    [ "$ready" = "1" ] || echo "$(date -Is) WARNING: umbreld never looked ready; starting anyway"

    # Let umbreld settle: it starts its apps over several seconds, and the tail
    # of that work can still prune networks that have no containers attached.
    sleep 30

    echo "$(date -Is) starting stack"
    start_stack
    echo "$(date -Is) initial start done (exit $?)"

    # --- 2. Supervise ------------------------------------------------------
    # Belt and braces: if anything still removes our containers after this
    # point (a late umbreld pass, an app install, a manual prune), put them
    # back instead of leaving the executor down until a human notices.
    for i in $(seq 1 30); do   # 30 x 30s = 15 min
        sleep 30
        if ! running "${OUR_CONTAINER}"; then
            echo "$(date -Is) ${OUR_CONTAINER} gone (check ${i}) — restarting stack"
            start_stack
            echo "$(date -Is) restart done (exit $?)"
        fi
    done

    echo "$(date -Is) supervision window closed; stack state: $(running "${OUR_CONTAINER}" && echo up || echo DOWN)"
    exit 0
fi

# ---------------------------------------------------------------------------
# Hook branch — what umbrelOS actually calls at boot.
# ---------------------------------------------------------------------------
if ! command -v systemd-run >/dev/null 2>&1; then
    echo "$(date -Is) ERROR: systemd-run not found; cannot start stack" >>"${LOG}" 2>&1
    exit 0   # never fail umbreld startup
fi

# Clear a leftover failed unit from a previous boot, or systemd-run refuses the
# name. --collect then reaps this run's unit once the waiter exits.
systemctl reset-failed "${UNIT}" 2>/dev/null

if ! systemd-run --unit="${UNIT}" --collect --no-block --quiet \
        --description="Bitcoin After Life Will Executor boot autostart" \
        /bin/bash "$0" --waiter 2>>"${LOG}"; then
    echo "$(date -Is) ERROR: systemd-run failed to start ${UNIT}" >>"${LOG}" 2>&1
fi

# Always succeed: this hook must never hold up or fail umbreld startup.
exit 0
