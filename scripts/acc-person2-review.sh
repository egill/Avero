#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PY_SCRIPT="${SCRIPT_DIR}/acc-person2-review.py"

LOG_DIR="${LOG_DIR:-/var/log/gateway-analysis}"
CONFIG_PATH="${CONFIG_PATH:-/opt/avero/gateway-poc/config/netto.toml}"

if [[ ! -f "${PY_SCRIPT}" ]]; then
  echo "Missing ${PY_SCRIPT}" >&2
  exit 1
fi

if [[ ! -f "${CONFIG_PATH}" ]]; then
  echo "Missing config: ${CONFIG_PATH}" >&2
  exit 1
fi

exec python3 "${PY_SCRIPT}" --log-dir "${LOG_DIR}" --config "${CONFIG_PATH}" "$@"
