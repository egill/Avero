#!/bin/bash
# Issue Review Script
# Interactive tool to review diagnosed issues and promote to tasklist
#
# Usage: ./review-issues.sh [options]
#   --list              List all issues (default)
#   --list-new          List only new issues
#   --list-tasklist     List issues promoted to tasklist
#   --review <id>       Review a specific issue by ID
#   --dismiss <id>      Dismiss an issue (mark as not a problem)
#   --promote <id>      Promote an issue to the tasklist for investigation
#   --stats             Show issue statistics
#   --reset-counters    Reset all issue counters
#   --help              Show this help

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ISSUES_DIR="${SCRIPT_DIR}/issues"
ISSUES_FILE="${ISSUES_DIR}/issues-to-review.jsonl"
COUNTERS_FILE="${ISSUES_DIR}/issue-counters.json"
TASKLIST_FILE="${ISSUES_DIR}/issue-tasklist.jsonl"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# ============================================================================
# Helper functions
# ============================================================================
ensure_files() {
    mkdir -p "$ISSUES_DIR"
    [ ! -f "$ISSUES_FILE" ] && touch "$ISSUES_FILE"
    [ ! -f "$COUNTERS_FILE" ] && echo '{}' > "$COUNTERS_FILE"
    [ ! -f "$TASKLIST_FILE" ] && touch "$TASKLIST_FILE"
}

# Format timestamp
format_time() {
    local ts="$1"
    if [ -n "$ts" ] && [ "$ts" != "null" ]; then
        date -r $((ts / 1000)) +"%Y-%m-%d %H:%M" 2>/dev/null || \
        date -d "@$((ts / 1000))" +"%Y-%m-%d %H:%M" 2>/dev/null || \
        echo "$ts"
    else
        echo "N/A"
    fi
}

# Get severity color
severity_color() {
    case "$1" in
        high) echo -e "${RED}$1${NC}" ;;
        medium) echo -e "${YELLOW}$1${NC}" ;;
        low) echo -e "${BLUE}$1${NC}" ;;
        *) echo "$1" ;;
    esac
}

# Get status color
status_color() {
    case "$1" in
        new) echo -e "${CYAN}$1${NC}" ;;
        reviewed) echo -e "${GREEN}$1${NC}" ;;
        dismissed) echo -e "${BLUE}$1${NC}" ;;
        promoted) echo -e "${YELLOW}$1${NC}" ;;
        *) echo "$1" ;;
    esac
}

# ============================================================================
# List functions
# ============================================================================
list_issues() {
    local filter="${1:-all}"

    ensure_files

    if [ ! -s "$ISSUES_FILE" ]; then
        echo "No issues found."
        return
    fi

    echo ""
    echo -e "${CYAN}=== Issues to Review ===${NC}"
    echo ""

    local count=0
    while IFS= read -r line; do
        [ -z "$line" ] && continue

        local status=$(echo "$line" | jq -r '.status')

        # Apply filter
        case "$filter" in
            new) [ "$status" != "new" ] && continue ;;
            reviewed) [ "$status" != "reviewed" ] && continue ;;
            dismissed) [ "$status" != "dismissed" ] && continue ;;
            promoted) [ "$status" != "promoted" ] && continue ;;
        esac

        local id=$(echo "$line" | jq -r '.id')
        local type=$(echo "$line" | jq -r '.type')
        local severity=$(echo "$line" | jq -r '.severity')
        local jid=$(echo "$line" | jq -r '.journey_id')
        local desc=$(echo "$line" | jq -r '.description')
        local created=$(echo "$line" | jq -r '.created_at')

        printf "%-8s " "$(echo "$id" | cut -c1-8)"
        severity_color "$severity"
        printf "  %-25s " "$type"
        status_color "$status"
        echo ""
        echo "         $desc"
        echo -e "         ${BLUE}jid:${NC} $(echo "$jid" | cut -c1-8)... ${BLUE}created:${NC} $created"
        echo ""

        count=$((count + 1))
    done < "$ISSUES_FILE"

    echo "Total: $count issues"
}

list_tasklist() {
    ensure_files

    if [ ! -s "$TASKLIST_FILE" ]; then
        echo "No issues in tasklist."
        return
    fi

    echo ""
    echo -e "${CYAN}=== Investigation Tasklist ===${NC}"
    echo ""

    local count=0
    while IFS= read -r line; do
        [ -z "$line" ] && continue

        local id=$(echo "$line" | jq -r '.id')
        local type=$(echo "$line" | jq -r '.type')
        local severity=$(echo "$line" | jq -r '.severity')
        local desc=$(echo "$line" | jq -r '.description')
        local notes=$(echo "$line" | jq -r '.notes // "No notes"')
        local promoted_at=$(echo "$line" | jq -r '.promoted_at')

        printf "%-8s " "$(echo "$id" | cut -c1-8)"
        severity_color "$severity"
        printf "  %-25s\n" "$type"
        echo "         $desc"
        echo -e "         ${BLUE}notes:${NC} $notes"
        echo -e "         ${BLUE}promoted:${NC} $promoted_at"
        echo ""

        count=$((count + 1))
    done < "$TASKLIST_FILE"

    echo "Total: $count items in tasklist"
}

# ============================================================================
# Review functions
# ============================================================================
review_issue() {
    local target_id="$1"

    ensure_files

    local found=false
    while IFS= read -r line; do
        [ -z "$line" ] && continue

        local id=$(echo "$line" | jq -r '.id')
        if [[ "$id" == "$target_id"* ]]; then
            found=true

            echo ""
            echo -e "${CYAN}=== Issue Details ===${NC}"
            echo "$line" | jq -C '.'
            echo ""

            # Show context details
            echo -e "${CYAN}=== Potential Causes ===${NC}"
            echo "$line" | jq -r '.context.potential_causes[]? // empty' 2>/dev/null | while read cause; do
                echo "  - $cause"
            done
            echo ""

            # Interactive options
            echo -e "${YELLOW}Options:${NC}"
            echo "  [d] Dismiss - Not a real issue"
            echo "  [p] Promote - Add to investigation tasklist"
            echo "  [n] Add note"
            echo "  [q] Quit review"
            echo ""
            read -p "Choice: " choice

            case "$choice" in
                d) dismiss_issue "$id" ;;
                p) promote_issue "$id" ;;
                n)
                    read -p "Note: " note
                    add_note "$id" "$note"
                    ;;
            esac

            break
        fi
    done < "$ISSUES_FILE"

    if ! $found; then
        echo "Issue not found: $target_id"
    fi
}

dismiss_issue() {
    local target_id="$1"

    local tmp_file="${ISSUES_FILE}.tmp"
    local now=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

    while IFS= read -r line; do
        [ -z "$line" ] && echo "$line" >> "$tmp_file" && continue

        local id=$(echo "$line" | jq -r '.id')
        if [[ "$id" == "$target_id"* ]]; then
            echo "$line" | jq --arg now "$now" '.status = "dismissed" | .reviewed_at = $now' >> "$tmp_file"
            echo -e "${GREEN}Issue dismissed${NC}"
        else
            echo "$line" >> "$tmp_file"
        fi
    done < "$ISSUES_FILE"

    mv "$tmp_file" "$ISSUES_FILE"
}

promote_issue() {
    local target_id="$1"

    local now=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

    while IFS= read -r line; do
        [ -z "$line" ] && continue

        local id=$(echo "$line" | jq -r '.id')
        if [[ "$id" == "$target_id"* ]]; then
            # Add to tasklist with promotion timestamp
            echo "$line" | jq --arg now "$now" '. + {promoted_at: $now, investigation_status: "pending"}' >> "$TASKLIST_FILE"

            # Update status in issues file
            local tmp_file="${ISSUES_FILE}.tmp"
            while IFS= read -r l2; do
                [ -z "$l2" ] && echo "$l2" >> "$tmp_file" && continue
                local id2=$(echo "$l2" | jq -r '.id')
                if [[ "$id2" == "$target_id"* ]]; then
                    echo "$l2" | jq --arg now "$now" '.status = "promoted" | .reviewed_at = $now' >> "$tmp_file"
                else
                    echo "$l2" >> "$tmp_file"
                fi
            done < "$ISSUES_FILE"
            mv "$tmp_file" "$ISSUES_FILE"

            echo -e "${GREEN}Issue promoted to tasklist${NC}"
            break
        fi
    done < "$ISSUES_FILE"
}

add_note() {
    local target_id="$1"
    local note="$2"

    local tmp_file="${ISSUES_FILE}.tmp"

    while IFS= read -r line; do
        [ -z "$line" ] && echo "$line" >> "$tmp_file" && continue

        local id=$(echo "$line" | jq -r '.id')
        if [[ "$id" == "$target_id"* ]]; then
            echo "$line" | jq --arg note "$note" '.notes = ((.notes // "") + "\n" + $note)' >> "$tmp_file"
            echo -e "${GREEN}Note added${NC}"
        else
            echo "$line" >> "$tmp_file"
        fi
    done < "$ISSUES_FILE"

    mv "$tmp_file" "$ISSUES_FILE"
}

# ============================================================================
# Statistics
# ============================================================================
show_stats() {
    ensure_files

    echo ""
    echo -e "${CYAN}=== Issue Statistics ===${NC}"
    echo ""

    # Issue counts by status
    echo -e "${YELLOW}By Status:${NC}"
    if [ -s "$ISSUES_FILE" ]; then
        jq -rs '[.[].status] | group_by(.) | map({status: .[0], count: length}) | .[]' "$ISSUES_FILE" 2>/dev/null | \
        jq -r '"  \(.status): \(.count)"' 2>/dev/null || echo "  No issues"
    else
        echo "  No issues"
    fi
    echo ""

    # Issue counts by type
    echo -e "${YELLOW}By Type:${NC}"
    if [ -s "$ISSUES_FILE" ]; then
        jq -rs '[.[].type] | group_by(.) | map({type: .[0], count: length}) | .[]' "$ISSUES_FILE" 2>/dev/null | \
        jq -r '"  \(.type): \(.count)"' 2>/dev/null || echo "  No issues"
    else
        echo "  No issues"
    fi
    echo ""

    # Lifetime counters
    echo -e "${YELLOW}Lifetime Counters:${NC}"
    if [ -s "$COUNTERS_FILE" ]; then
        jq -r 'to_entries | .[] | "  \(.key): \(.value)"' "$COUNTERS_FILE"
    else
        echo "  No counters"
    fi
    echo ""

    # Tasklist
    echo -e "${YELLOW}Tasklist:${NC}"
    if [ -s "$TASKLIST_FILE" ]; then
        local count=$(wc -l < "$TASKLIST_FILE" | tr -d ' ')
        echo "  $count items pending investigation"
    else
        echo "  Empty"
    fi
}

reset_counters() {
    echo '{}' > "$COUNTERS_FILE"
    echo -e "${GREEN}Counters reset${NC}"
}

# ============================================================================
# Main
# ============================================================================
main() {
    local cmd="${1:-list}"
    shift || true

    case "$cmd" in
        --list|list)
            list_issues "all"
            ;;
        --list-new)
            list_issues "new"
            ;;
        --list-tasklist|tasklist)
            list_tasklist
            ;;
        --review)
            review_issue "${1:-}"
            ;;
        --dismiss)
            dismiss_issue "${1:-}"
            ;;
        --promote)
            promote_issue "${1:-}"
            ;;
        --stats|stats)
            show_stats
            ;;
        --reset-counters)
            reset_counters
            ;;
        --help|-h)
            head -n 14 "$0" | tail -n 12
            ;;
        *)
            echo "Unknown command: $cmd"
            echo "Use --help for usage"
            exit 1
            ;;
    esac
}

main "$@"
