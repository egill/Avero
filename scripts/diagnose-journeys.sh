#!/bin/bash
# Journey Diagnostic Script
# Scans journeys for anomalies and tracks issues for review
#
# Usage: ./diagnose-journeys.sh [options]
#   --since <timestamp>   Override the last check timestamp (ISO8601 or "2h ago")
#   --site <netto|grandi> Site to analyze (default: netto)
#   --dry-run             Show what would be analyzed without writing issues
#   --verbose             Show detailed output
#   --help                Show this help

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ISSUES_DIR="${SCRIPT_DIR}/issues"
LAST_CHECK_FILE="${ISSUES_DIR}/.last-check"
ISSUES_FILE="${ISSUES_DIR}/issues-to-review.jsonl"
COUNTERS_FILE="${ISSUES_DIR}/issue-counters.json"
TASKLIST_FILE="${ISSUES_DIR}/issue-tasklist.jsonl"

# Hosts
NETTO_HOST="avero@100.80.187.3"
GRANDI_HOST="avero@100.80.187.4"
DB_HOST="e18n.net"

# Default paths on RPi
JOURNEY_FILE="/opt/avero/gateway-poc/journeys.jsonl"
LOG_UNIT="gateway-poc"

# Config
MIN_DWELL_MS=7000  # From config - min time at POS for authorization

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# Options
SITE="netto"
SINCE=""
DRY_RUN=false
VERBOSE=false

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
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        --verbose|-v)
            VERBOSE=true
            shift
            ;;
        --help|-h)
            head -n 10 "$0" | tail -n 8
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
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[OK]${NC} $1"
}

verbose() {
    if $VERBOSE; then
        echo -e "${CYAN}[DEBUG]${NC} $1"
    fi
}

# Get epoch ms from ISO timestamp or relative time
parse_time() {
    local input="$1"
    if [[ "$input" =~ ^[0-9]+$ ]]; then
        echo "$input"
    elif [[ "$input" =~ ago$ ]]; then
        # Handle "2h ago", "30m ago", etc.
        local num="${input%% *}"
        local unit="${input#* }"
        unit="${unit%% ago}"
        local now_ms=$(date +%s)000
        case "$unit" in
            h|hour|hours) echo $((now_ms - num * 3600000)) ;;
            m|min|minutes) echo $((now_ms - num * 60000)) ;;
            d|day|days) echo $((now_ms - num * 86400000)) ;;
            *) echo "$now_ms" ;;
        esac
    else
        # ISO timestamp
        local ts=$(date -j -f "%Y-%m-%dT%H:%M:%S" "$input" +%s 2>/dev/null || date -d "$input" +%s 2>/dev/null)
        echo "${ts}000"
    fi
}

# Get host for site
get_host() {
    case "$SITE" in
        netto) echo "$NETTO_HOST" ;;
        grandi) echo "$GRANDI_HOST" ;;
        *) log_error "Unknown site: $SITE"; exit 1 ;;
    esac
}

# ============================================================================
# Issue type definitions
# ============================================================================
# Issue categories:
#   - acc_match_failure: Customer at POS but ACC didn't match
#   - tracking_lost_with_pos: Tracking lost after customer spent time at POS
#   - tracking_lost_early: Tracking lost before meaningful interaction
#   - exit_no_acc: Exited (crossed exit) with dwell but no ACC match
#   - gate_blocked_with_pos: Gate blocked for customer who was at POS
#   - stitch_failure: Track stitching failed after gap

# Create issue JSON
create_issue() {
    local type="$1"
    local severity="$2"
    local journey_id="$3"
    local description="$4"
    local context="$5"

    local now=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
    local id=$(uuidgen | tr '[:upper:]' '[:lower:]')

    jq -n \
        --arg id "$id" \
        --arg type "$type" \
        --arg severity "$severity" \
        --arg jid "$journey_id" \
        --arg desc "$description" \
        --arg created "$now" \
        --arg status "new" \
        --argjson context "$context" \
        '{
            id: $id,
            type: $type,
            severity: $severity,
            journey_id: $jid,
            description: $desc,
            context: $context,
            status: $status,
            created_at: $created,
            reviewed_at: null,
            notes: null
        }'
}

# Update counter for issue type
update_counter() {
    local type="$1"

    if [ ! -f "$COUNTERS_FILE" ]; then
        echo '{}' > "$COUNTERS_FILE"
    fi

    local count=$(jq -r --arg t "$type" '.[$t] // 0' "$COUNTERS_FILE")
    count=$((count + 1))

    jq --arg t "$type" --argjson c "$count" '.[$t] = $c' "$COUNTERS_FILE" > "${COUNTERS_FILE}.tmp"
    mv "${COUNTERS_FILE}.tmp" "$COUNTERS_FILE"

    verbose "Counter for $type: $count"
}

# ============================================================================
# Journey analysis
# ============================================================================
analyze_journey() {
    local journey="$1"
    local issues_found=0

    local jid=$(echo "$journey" | jq -r '.jid')
    local outcome=$(echo "$journey" | jq -r '.out')
    local auth=$(echo "$journey" | jq -r '.auth')
    local acc=$(echo "$journey" | jq -r '.acc')
    local dwell=$(echo "$journey" | jq -r '.dwell')
    local gate_cmd=$(echo "$journey" | jq -r '.gate_cmd // "null"')
    local t0=$(echo "$journey" | jq -r '.t0')
    local t1=$(echo "$journey" | jq -r '.t1 // "null"')
    local events=$(echo "$journey" | jq -c '.ev // []')

    # Extract POS zone visits from events
    local pos_zones=$(echo "$events" | jq -r '[.[] | select(.t == "zone_entry" or .t == "zone_exit") | select(.z | test("POS")) | .z] | unique | join(",")')
    local has_exit_cross=$(echo "$events" | jq '[.[] | select(.t == "exit_cross")] | length > 0')

    verbose "Analyzing journey $jid: outcome=$outcome auth=$auth acc=$acc dwell=${dwell}ms"

    # Issue 1: Exit with POS dwell but no ACC match
    # Customer spent time at POS, exited, but payment wasn't matched
    if [[ "$outcome" == "exit" && "$acc" == "false" && "$dwell" -ge "$MIN_DWELL_MS" ]]; then
        local context=$(jq -n \
            --arg dwell "$dwell" \
            --arg pos "$pos_zones" \
            --arg auth "$auth" \
            --arg gate_cmd "$gate_cmd" \
            '{
                dwell_ms: ($dwell | tonumber),
                pos_zones: $pos,
                was_authorized: ($auth == "true"),
                gate_cmd_sent: ($gate_cmd != "null"),
                potential_causes: [
                    "ACC event arrived after journey ended",
                    "Payment at different POS than tracked",
                    "ACC terminal communication failure",
                    "Customer paid with cash (no ACC event)",
                    "Tracking assigned to wrong person"
                ]
            }')

        if ! $DRY_RUN; then
            create_issue "exit_no_acc" "medium" "$jid" \
                "Customer exited with ${dwell}ms POS dwell but no ACC match" \
                "$context" >> "$ISSUES_FILE"
            update_counter "exit_no_acc"
        fi
        log_warn "Issue: exit_no_acc - jid=$jid dwell=${dwell}ms zones=$pos_zones"
        issues_found=$((issues_found + 1))
    fi

    # Issue 2: Abandoned (tracking lost) with significant POS time
    # Customer was at POS but tracking was lost before exit
    if [[ "$outcome" == "abandoned" && "$dwell" -ge "$MIN_DWELL_MS" ]]; then
        local context=$(jq -n \
            --arg dwell "$dwell" \
            --arg pos "$pos_zones" \
            --arg acc "$acc" \
            --arg t0 "$t0" \
            --arg t1 "$t1" \
            '{
                dwell_ms: ($dwell | tonumber),
                pos_zones: $pos,
                acc_matched: ($acc == "true"),
                started_at: ($t0 | tonumber),
                ended_at: (if $t1 == "null" then null else ($t1 | tonumber) end),
                potential_causes: [
                    "Xovis lost track in crowd",
                    "Stitch window expired",
                    "Customer left via emergency exit",
                    "Sensor blind spot",
                    "Track ID collision"
                ]
            }')

        if ! $DRY_RUN; then
            create_issue "tracking_lost_with_pos" "high" "$jid" \
                "Tracking lost after ${dwell}ms at POS zones: $pos_zones" \
                "$context" >> "$ISSUES_FILE"
            update_counter "tracking_lost_with_pos"
        fi
        log_warn "Issue: tracking_lost_with_pos - jid=$jid dwell=${dwell}ms zones=$pos_zones acc=$acc"
        issues_found=$((issues_found + 1))
    fi

    # Issue 3: Abandoned with ACC match but no exit
    # Payment was matched but customer tracking lost before gate
    if [[ "$outcome" == "abandoned" && "$acc" == "true" ]]; then
        local context=$(jq -n \
            --arg dwell "$dwell" \
            --arg pos "$pos_zones" \
            --arg gate_cmd "$gate_cmd" \
            '{
                dwell_ms: ($dwell | tonumber),
                pos_zones: $pos,
                gate_cmd_sent: ($gate_cmd != "null"),
                potential_causes: [
                    "Track lost after ACC match, before gate",
                    "Stitch failed after payment",
                    "Multiple tracks for same person",
                    "Gate area sensor issue"
                ]
            }')

        if ! $DRY_RUN; then
            create_issue "acc_match_tracking_lost" "high" "$jid" \
                "ACC matched but tracking lost before exit" \
                "$context" >> "$ISSUES_FILE"
            update_counter "acc_match_tracking_lost"
        fi
        log_warn "Issue: acc_match_tracking_lost - jid=$jid dwell=${dwell}ms"
        issues_found=$((issues_found + 1))
    fi

    # Issue 4: Gate command sent but tracking lost (no exit confirmation)
    if [[ "$outcome" == "abandoned" && "$gate_cmd" != "null" ]]; then
        local context=$(jq -n \
            --arg dwell "$dwell" \
            --arg gate_cmd "$gate_cmd" \
            --arg acc "$acc" \
            '{
                dwell_ms: ($dwell | tonumber),
                gate_cmd_at: ($gate_cmd | tonumber),
                acc_matched: ($acc == "true"),
                potential_causes: [
                    "Gate opened but exit not detected",
                    "Track lost during gate transit",
                    "Exit line sensor issue",
                    "Tailgating detection false positive"
                ]
            }')

        if ! $DRY_RUN; then
            create_issue "gate_cmd_no_exit" "medium" "$jid" \
                "Gate command sent but no exit confirmation" \
                "$context" >> "$ISSUES_FILE"
            update_counter "gate_cmd_no_exit"
        fi
        log_warn "Issue: gate_cmd_no_exit - jid=$jid gate_cmd=$gate_cmd"
        issues_found=$((issues_found + 1))
    fi

    echo "$issues_found"
}

# ============================================================================
# Log analysis
# ============================================================================
analyze_logs() {
    local host="$1"
    local since_ts="$2"
    local tmp_log="/tmp/gateway_diag_$$.log"

    log_info "Fetching logs from $host since $(date -r $((since_ts / 1000)) 2>/dev/null || date -d "@$((since_ts / 1000))" 2>/dev/null || echo "$since_ts")..."

    # Convert epoch ms to journalctl format
    local since_date=$(date -r $((since_ts / 1000)) +"%Y-%m-%d %H:%M:%S" 2>/dev/null || \
                       date -d "@$((since_ts / 1000))" +"%Y-%m-%d %H:%M:%S" 2>/dev/null)

    ssh "$host" "journalctl -u $LOG_UNIT --since '$since_date' --no-pager 2>/dev/null" > "$tmp_log" 2>/dev/null || true

    if [ ! -s "$tmp_log" ]; then
        log_warn "No logs found or connection failed"
        rm -f "$tmp_log"
        return
    fi

    # Count key events
    local acc_unmatched=$(grep -c 'acc_unmatched' "$tmp_log" 2>/dev/null || echo 0)
    local stitch_expired=$(grep -c 'stitch_expired_lost' "$tmp_log" 2>/dev/null || echo 0)
    local gate_blocked=$(grep -c 'gate_entry_not_authorized' "$tmp_log" 2>/dev/null || echo 0)

    echo ""
    echo -e "${YELLOW}=== Log Analysis ===${NC}"
    echo -e "  ACC Unmatched Events: ${RED}$acc_unmatched${NC}"
    echo -e "  Stitch Expired (Lost): ${RED}$stitch_expired${NC}"
    echo -e "  Gate Blocked: ${YELLOW}$gate_blocked${NC}"

    # Look for patterns that might indicate systemic issues
    if [ "$acc_unmatched" -gt 10 ]; then
        if ! $DRY_RUN; then
            local context=$(jq -n --argjson count "$acc_unmatched" '{
                count: $count,
                analysis: "High ACC unmatched rate may indicate timing issues or network problems"
            }')
            create_issue "high_acc_unmatched_rate" "medium" "system" \
                "$acc_unmatched unmatched ACC events detected" \
                "$context" >> "$ISSUES_FILE"
            update_counter "high_acc_unmatched_rate"
        fi
        log_warn "Issue: high_acc_unmatched_rate - count=$acc_unmatched"
    fi

    if [ "$stitch_expired" -gt 20 ]; then
        if ! $DRY_RUN; then
            local context=$(jq -n --argjson count "$stitch_expired" '{
                count: $count,
                analysis: "High stitch expiry rate may indicate sensor coverage gaps or timing issues"
            }')
            create_issue "high_stitch_expiry_rate" "medium" "system" \
                "$stitch_expired stitch expiry events detected" \
                "$context" >> "$ISSUES_FILE"
            update_counter "high_stitch_expiry_rate"
        fi
        log_warn "Issue: high_stitch_expiry_rate - count=$stitch_expired"
    fi

    rm -f "$tmp_log"
}

# ============================================================================
# Fetch and analyze journeys
# ============================================================================
fetch_and_analyze_journeys() {
    local host="$1"
    local since_ts="$2"
    local tmp_journeys="/tmp/journeys_diag_$$.jsonl"

    log_info "Fetching journeys from $host..."

    # Fetch journey file
    ssh "$host" "cat $JOURNEY_FILE 2>/dev/null" > "$tmp_journeys" 2>/dev/null || true

    if [ ! -s "$tmp_journeys" ]; then
        log_warn "No journey file found at $JOURNEY_FILE"
        rm -f "$tmp_journeys"
        return
    fi

    # Filter journeys since last check and analyze
    local total=0
    local analyzed=0
    local issues=0

    while IFS= read -r line; do
        [ -z "$line" ] && continue

        # Parse journey timestamp
        local t0=$(echo "$line" | jq -r '.t0 // 0' 2>/dev/null)
        if [ -z "$t0" ] || [ "$t0" == "null" ]; then
            continue
        fi

        total=$((total + 1))

        # Skip if before our check window
        if [ "$t0" -lt "$since_ts" ]; then
            continue
        fi

        analyzed=$((analyzed + 1))

        # Analyze this journey
        local found=$(analyze_journey "$line")
        issues=$((issues + found))

    done < "$tmp_journeys"

    echo ""
    echo -e "${YELLOW}=== Journey Analysis ===${NC}"
    echo -e "  Total journeys in file: ${BLUE}$total${NC}"
    echo -e "  Journeys since last check: ${BLUE}$analyzed${NC}"
    echo -e "  Issues found: ${RED}$issues${NC}"

    rm -f "$tmp_journeys"
}

# ============================================================================
# Main
# ============================================================================
main() {
    echo ""
    echo -e "${CYAN}========================================${NC}"
    echo -e "${CYAN}  Journey Diagnostic Tool${NC}"
    echo -e "${CYAN}========================================${NC}"
    echo ""

    # Ensure issues directory exists
    mkdir -p "$ISSUES_DIR"

    # Initialize counters file if needed
    if [ ! -f "$COUNTERS_FILE" ]; then
        echo '{}' > "$COUNTERS_FILE"
    fi

    # Determine since timestamp
    local since_ts
    if [ -n "$SINCE" ]; then
        since_ts=$(parse_time "$SINCE")
        log_info "Using provided timestamp: $SINCE (${since_ts}ms)"
    elif [ -f "$LAST_CHECK_FILE" ]; then
        since_ts=$(cat "$LAST_CHECK_FILE")
        log_info "Last check: $(date -r $((since_ts / 1000)) 2>/dev/null || date -d "@$((since_ts / 1000))" 2>/dev/null || echo "$since_ts")"
    else
        # Default to 24 hours ago
        since_ts=$(($(date +%s)000 - 86400000))
        log_info "No previous check found, analyzing last 24 hours"
    fi

    local host=$(get_host)
    log_info "Site: $SITE ($host)"

    if $DRY_RUN; then
        log_warn "DRY RUN - no issues will be written"
    fi

    # Run analysis
    fetch_and_analyze_journeys "$host" "$since_ts"
    analyze_logs "$host" "$since_ts"

    # Update last check timestamp
    if ! $DRY_RUN; then
        echo "$(date +%s)000" > "$LAST_CHECK_FILE"
        log_success "Updated last check timestamp"
    fi

    # Show summary
    echo ""
    echo -e "${CYAN}=== Issue Counters ===${NC}"
    if [ -f "$COUNTERS_FILE" ]; then
        jq -r 'to_entries | .[] | "  \(.key): \(.value)"' "$COUNTERS_FILE"
    fi

    echo ""
    echo -e "${CYAN}=== Files ===${NC}"
    echo "  Issues file: $ISSUES_FILE"
    echo "  Counters: $COUNTERS_FILE"
    echo "  Tasklist: $TASKLIST_FILE"

    if [ -f "$ISSUES_FILE" ]; then
        local new_issues=$(grep -c '"status":"new"' "$ISSUES_FILE" 2>/dev/null || echo 0)
        echo ""
        echo -e "  ${YELLOW}$new_issues new issues awaiting review${NC}"
    fi

    echo ""
    log_success "Diagnostic complete"
}

main
