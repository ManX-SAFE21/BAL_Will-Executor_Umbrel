#!/bin/bash
# =============================================================================
# load-app-env.sh — operator options for the App Store install (sourced)
# =============================================================================
# An app installed from an Umbrel app store has no .env file next to its
# docker-compose.yml: umbreld owns that file and replaces it on every update.
# The few options an operator may want to change therefore live in the app's
# own data volume, in /data/app.env, one KEY=value per line:
#
#   BAL_PUBLIC_URL=https://will.example.org   # public URL reported to welist
#   BAL_SEND_STATS=true                       # report to the welist aggregator
#   BAL_LOG_LEVEL=info                        # RUST_LOG for both binaries
#
# Only these three keys are read; anything else in the file is ignored, and
# the file is parsed, never executed. When the file does not exist (the manual
# docker-compose.umbrel.yml stack, the dev stack) nothing changes.
# =============================================================================

APP_ENV_FILE="${BAL_APP_ENV_FILE:-/data/app.env}"

if [[ -f "${APP_ENV_FILE}" ]]; then
  while IFS='=' read -r _key _value || [[ -n "${_key}" ]]; do
    _key="${_key//[[:space:]]/}"
    case "${_key}" in
      BAL_PUBLIC_URL|BAL_SEND_STATS|BAL_LOG_LEVEL) ;;
      *) continue ;;
    esac
    _value="${_value%$'\r'}"
    _value="${_value#\"}"
    _value="${_value%\"}"
    export "${_key}=${_value}"
  done < "${APP_ENV_FILE}"
  unset _key _value

  if [[ -n "${BAL_PUBLIC_URL:-}" ]]; then
    export BAL_SERVER_URL="${BAL_PUBLIC_URL}"
  fi
  if [[ -n "${BAL_SEND_STATS:-}" ]]; then
    export BAL_PUSHER_SEND_STATS="${BAL_SEND_STATS}"
  fi
  if [[ -n "${BAL_LOG_LEVEL:-}" ]]; then
    export RUST_LOG="${BAL_LOG_LEVEL}"
  fi
  echo "[load-app-env] Applied ${APP_ENV_FILE}"
fi
