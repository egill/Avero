#!/bin/bash
# ACC and Lost Track Monitor for Gateway
# Usage: ./monitor_acc.sh [logfile]

LOG="${1:-/tmp/gateway.log}"
REMOTE_HOST="avero@100.80.187.3"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Strip ANSI codes from log
strip_ansi() {
    sed 's/\x1b\[[0-9;]*m//g'
}

# BSD-compatible field extraction
extract_field() {
    local line="$1"
    local field="$2"
    echo "$line" | sed -n "s/.*${field}=\([^ ]*\).*/\1/p"
}

echo -e "${BLUE}=== ACC & Lost Track Monitor ===${NC}"
echo "Analyzing: $LOG on $REMOTE_HOST"
echo ""

# Fetch and analyze log
ssh $REMOTE_HOST "cat $LOG 2>/dev/null" | strip_ansi > /tmp/gateway_analysis.log

if [ ! -s /tmp/gateway_analysis.log ]; then
    echo -e "${RED}Error: Could not fetch log or log is empty${NC}"
    exit 1
fi

# ACC Statistics
echo -e "${YELLOW}=== ACC Event Summary ===${NC}"
ACC_MATCHED=$(grep -c 'acc_matched\|acc_group_authorized' /tmp/gateway_analysis.log)
ACC_UNMATCHED=$(grep -c 'acc_unmatched' /tmp/gateway_analysis.log)
ACC_TOTAL=$((ACC_MATCHED + ACC_UNMATCHED))

if [ $ACC_TOTAL -gt 0 ]; then
    MATCH_RATE=$(echo "scale=1; $ACC_MATCHED * 100 / $ACC_TOTAL" | bc)
    echo -e "Total ACC events: ${BLUE}$ACC_TOTAL${NC}"
    echo -e "  Matched: ${GREEN}$ACC_MATCHED${NC}"
    echo -e "  Unmatched: ${RED}$ACC_UNMATCHED${NC}"
    echo -e "  Match rate: ${YELLOW}${MATCH_RATE}%${NC}"
else
    echo "No ACC events found"
fi
echo ""

# Unmatched ACC Details
echo -e "${YELLOW}=== Unmatched ACC Details (last 10) ===${NC}"
grep 'acc_unmatched' /tmp/gateway_analysis.log | tail -10 | while read line; do
    time=$(echo "$line" | sed -n 's/\([0-9T:-]*\)Z.*/\1/p' | cut -c1-19)
    ip=$(extract_field "$line" "ip")
    pos=$(echo "$line" | sed -n 's/.*pos=Some("\([^"]*\)").*/\1/p')
    active=$(extract_field "$line" "active_tracks")
    pending=$(extract_field "$line" "pending_tracks")
    echo -e "  ${time} | ${pos} (${ip}) | active=${active} pending=${pending}"
done
echo ""

# Lost Track Statistics
echo -e "${YELLOW}=== Lost Tracks (Stitch Expired) ===${NC}"
LOST_COUNT=$(grep -c 'stitch_expired_lost' /tmp/gateway_analysis.log)
LOST_WITH_ZONE=$(grep 'stitch_expired_lost' /tmp/gateway_analysis.log | grep -v 'last_zone=None' | wc -l | tr -d ' ')
LOST_NO_ZONE=$(grep 'stitch_expired_lost' /tmp/gateway_analysis.log | grep 'last_zone=None' | wc -l | tr -d ' ')

echo -e "Total lost tracks: ${RED}$LOST_COUNT${NC}"
echo -e "  With zone context: ${YELLOW}$LOST_WITH_ZONE${NC} (lost after entering zones)"
echo -e "  No zone context: ${BLUE}$LOST_NO_ZONE${NC} (sensor noise/edge detections)"
echo ""

# Recent lost tracks
echo -e "${YELLOW}=== Recent Lost Tracks (last 10) ===${NC}"
grep 'stitch_expired_lost' /tmp/gateway_analysis.log | tail -10 | while read line; do
    time=$(echo "$line" | sed -n 's/\([0-9T:-]*\)Z.*/\1/p' | cut -c1-19)
    tid=$(extract_field "$line" "track_id")
    auth=$(extract_field "$line" "authorized")
    dwell=$(extract_field "$line" "dwell_ms")
    zone=$(extract_field "$line" "last_zone")
    age=$(extract_field "$line" "age_ms")
    echo -e "  ${time} | tid=${tid} auth=${auth} dwell=${dwell}ms zone=${zone} age=${age}ms"
done
echo ""

# Stitch Statistics
echo -e "${YELLOW}=== Track Stitching ===${NC}"
STITCH_MATCHED=$(grep -c 'stitch_match_found' /tmp/gateway_analysis.log)
STITCH_EXPIRED=$(grep -c 'stitch_expired_lost' /tmp/gateway_analysis.log)
STITCH_TOTAL=$((STITCH_MATCHED + STITCH_EXPIRED))

if [ $STITCH_TOTAL -gt 0 ]; then
    STITCH_RATE=$(echo "scale=1; $STITCH_MATCHED * 100 / $STITCH_TOTAL" | bc)
    echo -e "Total stitch attempts: ${BLUE}$STITCH_TOTAL${NC}"
    echo -e "  Successful stitches: ${GREEN}$STITCH_MATCHED${NC}"
    echo -e "  Lost (expired): ${RED}$STITCH_EXPIRED${NC}"
    echo -e "  Stitch rate: ${YELLOW}${STITCH_RATE}%${NC}"
else
    echo "No stitch events found"
fi
echo ""

# Journey Statistics
echo -e "${YELLOW}=== Journey Summary ===${NC}"
JOURNEY_COMPLETE=$(grep -c 'journey_complete' /tmp/gateway_analysis.log)
JOURNEY_EXIT=$(grep 'journey_ended.*outcome=exit' /tmp/gateway_analysis.log | wc -l | tr -d ' ')
JOURNEY_ABANDONED=$(grep 'journey_ended.*outcome=abandoned' /tmp/gateway_analysis.log | wc -l | tr -d ' ')

echo -e "Completed journeys: ${GREEN}$JOURNEY_COMPLETE${NC}"
echo -e "  Exit (successful): ${GREEN}$JOURNEY_EXIT${NC}"
echo -e "  Abandoned: ${YELLOW}$JOURNEY_ABANDONED${NC}"
echo ""

# Gate Commands
echo -e "${YELLOW}=== Gate Activity ===${NC}"
GATE_CMDS=$(grep -c 'gate_open_command' /tmp/gateway_analysis.log)
GATE_BLOCKED=$(grep -c 'gate_entry_not_authorized' /tmp/gateway_analysis.log)

echo -e "Gate open commands: ${GREEN}$GATE_CMDS${NC}"
echo -e "Gate blocked (unauthorized): ${RED}$GATE_BLOCKED${NC}"
echo ""

# POS-specific breakdown
echo -e "${YELLOW}=== ACC by POS Zone ===${NC}"
for pos in POS_1 POS_2 POS_3 POS_4 POS_5; do
    matched=$(grep "acc_group_authorized.*pos=Some(\"$pos\")" /tmp/gateway_analysis.log | wc -l | tr -d ' ')
    unmatched=$(grep "acc_unmatched.*pos=Some(\"$pos\")" /tmp/gateway_analysis.log | wc -l | tr -d ' ')
    total=$((matched + unmatched))
    if [ $total -gt 0 ]; then
        rate=$(echo "scale=0; $matched * 100 / $total" | bc)
        echo -e "  ${pos}: matched=${matched} unmatched=${unmatched} (${rate}%)"
    fi
done
echo ""

# Cleanup
rm -f /tmp/gateway_analysis.log

echo -e "${BLUE}=== Analysis Complete ===${NC}"
