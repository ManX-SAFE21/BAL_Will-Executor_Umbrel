#!/bin/bash
# =============================================================================
# umbrel-pre-start-hook.sh — installed as /home/umbrel/umbrel/custom-hooks/pre-start
#
# Brings the Will Executor stack up on every boot.
#
# WHY A HOOK AND NOT A SYSTEMD UNIT
# ---------------------------------
# This app is deployed manually, not through Umbrel's App Store flow, so it is
# not in umbreld's app registry and umbreld never starts it: after any reboot
# its containers are simply gone.
#
# A unit in /etc/systemd/system does NOT survive here — umbrelOS runs / on an
# overlay whose upper layer is discarded on reboot (the SSH banner warns about
# exactly this). Only /home and /data persist. umbrelOS's supported extension
# point is /opt/umbrel-custom-hooks/run-pre-start, which ships inside the OS
# image (so it returns after every OS update) and executes this script from the
# persistent /home path.
#
# WHY systemd-run AND NOT A BACKGROUND JOB
# ----------------------------------------
# Two constraints collide:
#   * the wrapper runs us with Before=umbrel.service, so we BLOCK umbreld until
#     we return — but the network we need (umbrel_main_network) is created BY
#     umbreld, so waiting for it inline would deadlock until the 5 min timeout;
#   * the wrapper's unit is Type=oneshot, so when our script returns systemd
#     tears down the unit's whole control group. A plain `... &`, even with
#     setsid, is inside that cgroup and gets killed instantly. (Tried on
#     21 Sep 2026: the journal showed the hook ran and "completed successfully",
#     yet the waiter never wrote a single log line — it was killed on teardown.)
#
# So we hand the work to systemd-run, which starts it as its own transient unit
# with its own cgroup. It is unaffected by our teardown, and we return at once
# so umbreld boots normally.
# =============================================================================

APP_DIR=/home/umbrel/umbrel/app-data/bal-umbrel/bal-umbrel
LOG=/home/umbrel/umbrel/app-data/bal-umbrel/autostart.log
UNIT=bal-umbrel-autostart

# ---------------------------------------------------------------------------
# Waiter branch — runs inside the transient unit, detached from the hook.
# ---------------------------------------------------------------------------
if [ "${1:-}" = "--waiter" ]; then
    exec >>"${LOG}" 2>&1
    echo "=== $(date -Is) waiter started (unit ${UNIT}) ==="

    # umbreld creates the shared network during its own startup.
    # 120 x 5s = 10 min: generous for a slow boot, but bounded.
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

    echo "$(date -Is) ERROR: umbrel_main_network never appeared after 10 min" >&2
    exit 1
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
