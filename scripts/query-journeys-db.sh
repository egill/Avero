#!/bin/bash
# Query TimescaleDB for journey data
# Establishes SSH tunnel to e18n.net and queries the Docker-hosted TimescaleDB
#
# Usage: ./query-journeys-db.sh [options]
#   --since <timestamp>   Filter journeys since this time (ISO8601 or epoch ms)
#   --site <netto|grandi> Filter by site
#   --outcome <outcome>   Filter by outcome (exit, abandoned, etc.)
#   --no-acc              Filter journeys with no ACC match
#   --has-dwell           Filter journeys with dwell > 0
#   --query <sql>         Run custom SQL query
#   --count               Just count matching journeys
#   --export <file>       Export results to JSONL file
#   --help                Show this help
#
# Prerequisites:
#   - SSH access to e18n.net
#   - psql client installed locally
#   - Docker container 'timescaledb' running on e18n.net

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Connection settings
DB_HOST="e18n.net"
DB_USER="postgres"
DB_NAME="avero"
DB_PORT="5432"
DOCKER_CONTAINER="timescaledb"

# Tunnel settings
LOCAL_PORT="15432"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# Options
SINCE=""
SITE=""
OUTCOME=""
NO_ACC=false
HAS_DWELL=false
CUSTOM_QUERY=""
COUNT_ONLY=false
EXPORT_FILE=""

# ============================================================================
# Argument parsing
# ============================================================================
while [[ $# -gt 0 ]]; do
    case "$1" in
        --since)
            SINCE="$2"
            shift 2
            ;;
        --site)
            SITE="$2"
            shift 2
            ;;
        --outcome)
            OUTCOME="$2"
            shift 2
            ;;
        --no-acc)
            NO_ACC=true
            shift
            ;;
        --has-dwell)
            HAS_DWELL=true
            shift
            ;;
        --query)
            CUSTOM_QUERY="$2"
            shift 2
            ;;
        --count)
            COUNT_ONLY=true
            shift
            ;;
        --export)
            EXPORT_FILE="$2"
            shift 2
            ;;
        --help|-h)
            head -n 18 "$0" | tail -n 16
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# ============================================================================
# Helper functions
# ============================================================================
log_info() {
    echo -e "${BLUE}[INFO]${NC} $1" >&2
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1" >&2
}

# Parse time to postgres timestamp
parse_time_to_pg() {
    local input="$1"
    if [[ "$input" =~ ^[0-9]+$ ]]; then
        # Epoch ms to timestamp
        echo "to_timestamp($input / 1000.0)"
    elif [[ "$input" =~ ago$ ]]; then
        # Relative time
        local num="${input%% *}"
        local unit="${input#* }"
        unit="${unit%% ago}"
        case "$unit" in
            h|hour|hours) echo "NOW() - INTERVAL '$num hours'" ;;
            m|min|minutes) echo "NOW() - INTERVAL '$num minutes'" ;;
            d|day|days) echo "NOW() - INTERVAL '$num days'" ;;
            *) echo "NOW() - INTERVAL '24 hours'" ;;
        esac
    else
        # ISO timestamp
        echo "'$input'::timestamptz"
    fi
}

# ============================================================================
# SSH Tunnel management
# ============================================================================
TUNNEL_PID=""

start_tunnel() {
    log_info "Starting SSH tunnel to $DB_HOST..."

    # Check if tunnel already exists
    if nc -z localhost "$LOCAL_PORT" 2>/dev/null; then
        log_info "Tunnel already active on port $LOCAL_PORT"
        return
    fi

    # Start tunnel in background
    ssh -f -N -L "${LOCAL_PORT}:localhost:${DB_PORT}" "$DB_HOST" \
        -o ExitOnForwardFailure=yes \
        -o ServerAliveInterval=30 \
        -o ServerAliveCountMax=3

    # Wait for tunnel to be ready
    for i in {1..10}; do
        if nc -z localhost "$LOCAL_PORT" 2>/dev/null; then
            log_info "Tunnel established"
            return
        fi
        sleep 0.5
    done

    log_error "Failed to establish tunnel"
    exit 1
}

cleanup_tunnel() {
    # Find and kill the SSH tunnel process
    local pid=$(pgrep -f "ssh.*-L.*${LOCAL_PORT}:localhost:${DB_PORT}.*${DB_HOST}" 2>/dev/null || true)
    if [ -n "$pid" ]; then
        kill "$pid" 2>/dev/null || true
        log_info "Tunnel closed"
    fi
}

# ============================================================================
# Database queries
# ============================================================================
run_query() {
    local query="$1"
    local format="${2:-}"

    # Connect through tunnel to Docker container
    # Note: We use the tunnel to reach the server, then the query runs against the docker container's port
    PGPASSWORD="${DB_PASSWORD:-}" psql \
        -h localhost \
        -p "$LOCAL_PORT" \
        -U "$DB_USER" \
        -d "$DB_NAME" \
        -t -A \
        ${format:+-F "$format"} \
        -c "$query"
}

build_journey_query() {
    local select_cols="*"
    local where_clauses=""

    if $COUNT_ONLY; then
        select_cols="COUNT(*)"
    fi

    # Build WHERE clause
    local conditions=()

    if [ -n "$SINCE" ]; then
        conditions+=("time >= $(parse_time_to_pg "$SINCE")")
    fi

    if [ -n "$SITE" ]; then
        conditions+=("site = '$SITE'")
    fi

    if [ -n "$OUTCOME" ]; then
        conditions+=("outcome = '$OUTCOME'")
    fi

    if $NO_ACC; then
        # This depends on how ACC is stored - adjust as needed
        conditions+=("(data->>'acc_matched' = 'false' OR data->>'acc_matched' IS NULL)")
    fi

    if $HAS_DWELL; then
        conditions+=("total_pos_dwell_ms > 0")
    fi

    # Combine conditions
    if [ ${#conditions[@]} -gt 0 ]; then
        where_clauses="WHERE $(IFS=' AND '; echo "${conditions[*]}")"
    fi

    echo "SELECT $select_cols FROM person_journeys $where_clauses ORDER BY time DESC LIMIT 1000"
}

# ============================================================================
# Main
# ============================================================================
main() {
    # Handle custom query
    if [ -n "$CUSTOM_QUERY" ]; then
        start_tunnel
        trap cleanup_tunnel EXIT
        run_query "$CUSTOM_QUERY"
        exit 0
    fi

    # Build and run journey query
    local query=$(build_journey_query)

    log_info "Query: $query"

    start_tunnel
    trap cleanup_tunnel EXIT

    if $COUNT_ONLY; then
        local count=$(run_query "$query")
        echo "Count: $count"
    elif [ -n "$EXPORT_FILE" ]; then
        log_info "Exporting to $EXPORT_FILE..."
        # Export as JSONL
        run_query "SELECT row_to_json(t) FROM ($query) t" > "$EXPORT_FILE"
        local lines=$(wc -l < "$EXPORT_FILE" | tr -d ' ')
        log_info "Exported $lines journeys"
    else
        # Pretty print results
        run_query "$query" "|" | while IFS='|' read -r line; do
            echo "$line"
        done
    fi
}

main
