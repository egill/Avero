#!/usr/bin/env bash
# Export journey data from TimescaleDB and run ACC group evaluation
set -euo pipefail

# Default configuration
SITE="netto"
HOURS="48"
MIN_DWELL_MS="7000"
ENTRY_SPREAD_S="10"
OTHER_POS_WINDOW_S="30"
OTHER_POS_MIN_S="0"
MERGE_GAP_S="10"
SAMPLE_SIZE="15"

# Database connection
SSH_HOST="root@e18n.net"
DB_CONTAINER="avero-timescaledb"
DB_USER="avero"
DB_NAME="avero_command"

# Output paths (set later based on site/hours if not provided)
OUT_JSONL=""
OUT_SAMPLES=""

usage() {
  cat <<EOF
Usage: $(basename "$0") [options]

Export journey data and evaluate ACC grouping variants.

Options:
  --site <site>                 Site name (default: $SITE)
  --hours <hours>               Lookback window in hours (default: $HOURS)
  --min-dwell-ms <ms>           Min dwell threshold (default: $MIN_DWELL_MS)
  --entry-spread-s <sec>        Entry spread threshold (default: $ENTRY_SPREAD_S)
  --other-pos-window-s <sec>    Other POS activity window (default: $OTHER_POS_WINDOW_S)
  --other-pos-min-s <sec>       Min other POS seconds to count (default: $OTHER_POS_MIN_S)
  --merge-gap-s <sec>           POS flicker merge gap (default: $MERGE_GAP_S)
  --sample-size <n>             Samples per variant (default: $SAMPLE_SIZE)
  --out <file>                  Output JSONL path
  --samples <file>              Samples JSONL path
  --ssh <host>                  SSH host (default: $SSH_HOST)
  --container <name>            DB container name (default: $DB_CONTAINER)
  --db-user <user>              DB user (default: $DB_USER)
  --db-name <name>              DB name (default: $DB_NAME)
  -h, --help                    Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --site)             SITE="$2"; shift 2 ;;
    --hours)            HOURS="$2"; shift 2 ;;
    --min-dwell-ms)     MIN_DWELL_MS="$2"; shift 2 ;;
    --entry-spread-s)   ENTRY_SPREAD_S="$2"; shift 2 ;;
    --other-pos-window-s) OTHER_POS_WINDOW_S="$2"; shift 2 ;;
    --other-pos-min-s)  OTHER_POS_MIN_S="$2"; shift 2 ;;
    --merge-gap-s)      MERGE_GAP_S="$2"; shift 2 ;;
    --sample-size)      SAMPLE_SIZE="$2"; shift 2 ;;
    --out)              OUT_JSONL="$2"; shift 2 ;;
    --samples)          OUT_SAMPLES="$2"; shift 2 ;;
    --ssh)              SSH_HOST="$2"; shift 2 ;;
    --container)        DB_CONTAINER="$2"; shift 2 ;;
    --db-user)          DB_USER="$2"; shift 2 ;;
    --db-name)          DB_NAME="$2"; shift 2 ;;
    -h|--help)          usage; exit 0 ;;
    *)                  echo "Unknown option: $1" >&2; usage; exit 1 ;;
  esac
done

# Set default output paths if not provided
: "${OUT_JSONL:=/tmp/acc-group-${SITE}-${HOURS}h.jsonl}"
: "${OUT_SAMPLES:=/tmp/acc-group-samples-${SITE}-${HOURS}h.jsonl}"

echo "Exporting journeys to ${OUT_JSONL}..."
ssh "${SSH_HOST}" "docker exec ${DB_CONTAINER} psql -U ${DB_USER} -d ${DB_NAME} -t -A -c \"
  SELECT row_to_json(t) FROM (
    SELECT time, person_id, site, member_count, group_member_ids, acc_matched, payment_zone, events
    FROM person_journeys
    WHERE site='${SITE}' AND time > now() - interval '${HOURS} hours'
  ) t;
\"" > "${OUT_JSONL}"

echo "Running eval..."
python3 scripts/acc-group-eval.py \
  --input "${OUT_JSONL}" \
  --min-dwell-ms "${MIN_DWELL_MS}" \
  --entry-spread-s "${ENTRY_SPREAD_S}" \
  --other-pos-window-s "${OTHER_POS_WINDOW_S}" \
  --other-pos-min-s "${OTHER_POS_MIN_S}" \
  --merge-gap-s "${MERGE_GAP_S}" \
  --sample-size "${SAMPLE_SIZE}" \
  --samples "${OUT_SAMPLES}"

echo "Running report..."
python3 scripts/acc-group-report.py \
  --samples "${OUT_SAMPLES}" \
  --journeys "${OUT_JSONL}" \
  --min-dwell-ms "${MIN_DWELL_MS}" \
  --entry-spread-s "${ENTRY_SPREAD_S}" \
  --sample-size 3

echo "Samples written to ${OUT_SAMPLES}"
