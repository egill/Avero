#!/bin/bash
# Gateway Log Analyzer
# Usage: ./analyze_logs.sh /path/to/gateway.log

LOG_FILE="${1:-/tmp/gateway-test.log}"

if [ ! -f "$LOG_FILE" ]; then
    echo "Error: Log file not found: $LOG_FILE"
    exit 1
fi

# Strip ANSI codes for analysis
strip_ansi() {
    sed 's/\x1b\[[0-9;]*m//g'
}

echo "========================================"
echo "Gateway Performance Analysis"
echo "Log file: $LOG_FILE"
echo "========================================"

echo ""
echo "=== JOURNEY SUMMARY ==="
TOTAL_JOURNEYS=$(grep 'journey_complete' "$LOG_FILE" | wc -l)
AUTHORIZED=$(grep 'journey_complete' "$LOG_FILE" | strip_ansi | grep -c 'authorized=true')
UNAUTHORIZED=$(grep 'journey_complete' "$LOG_FILE" | strip_ansi | grep -c 'authorized=false')
GATE_OPENED=$(grep 'journey_complete' "$LOG_FILE" | strip_ansi | grep -c 'gate_opened=true')

echo "Total journeys completed: $TOTAL_JOURNEYS"
echo "  Authorized: $AUTHORIZED"
echo "  Unauthorized: $UNAUTHORIZED"
echo "  Gate opened: $GATE_OPENED"
if [ "$TOTAL_JOURNEYS" -gt 0 ]; then
    AUTH_RATE=$(echo "scale=1; $AUTHORIZED * 100 / $TOTAL_JOURNEYS" | bc)
    echo "  Authorization rate: ${AUTH_RATE}%"
fi

echo ""
echo "=== JOURNEY TIMING (from journey_complete) ==="
grep 'journey_complete' "$LOG_FILE" | strip_ansi | \
    grep -oP 'duration_ms=\K[0-9]+' | \
    awk '{sum+=$1; count++; if($1>max)max=$1; if(min==""||$1<min)min=$1}
         END {if(count>0) printf "  Duration: min=%dms avg=%dms max=%dms (n=%d)\n", min, sum/count, max, count}'

grep 'journey_complete' "$LOG_FILE" | strip_ansi | \
    grep -oP 'dwell_ms=\K[0-9]+' | \
    awk '{sum+=$1; count++; if($1>max)max=$1; if(min==""||$1<min)min=$1}
         END {if(count>0) printf "  Dwell: min=%dms avg=%dms max=%dms (n=%d)\n", min, sum/count, max, count}'

echo ""
echo "=== GATE COMMAND LATENCY ==="
grep 'gate_open_command' "$LOG_FILE" | strip_ansi | \
    grep -oP 'latency_us=\K[0-9]+' | \
    awk '{sum+=$1; count++; if($1>max)max=$1; if(min==""||$1<min)min=$1}
         END {if(count>0) printf "  Command latency: min=%dus avg=%dus max=%dus (n=%d)\n", min, sum/count, max, count
              else print "  No gate commands found"}'

echo ""
echo "=== GATE-TO-DOOR LATENCY ==="
grep 'gate_open_correlated' "$LOG_FILE" | strip_ansi | \
    grep -oP 'delta_ms=\K[0-9]+' | \
    awk '{sum+=$1; count++; if($1>max)max=$1; if(min==""||$1<min)min=$1}
         END {if(count>0) printf "  Gate-to-door: min=%dms avg=%dms max=%dms (n=%d)\n", min, sum/count, max, count
              else print "  No correlations found (door may not have opened)"}'

echo ""
echo "=== EVENT PROCESSING LATENCY ==="
grep 'metrics' "$LOG_FILE" | strip_ansi | \
    grep -oP 'avg_process_latency_us=\K[0-9]+' | \
    awk '{sum+=$1; count++; if($1>max)max=$1}
         END {if(count>0) printf "  Avg processing: %dus (max period avg: %dus)\n", sum/count, max
              else print "  No metrics found"}'

grep 'metrics' "$LOG_FILE" | strip_ansi | \
    grep -oP 'max_process_latency_us=\K[0-9]+' | \
    awk '{if($1>max)max=$1} END {if(max>0) printf "  Peak latency: %dus\n", max}'

echo ""
echo "=== THROUGHPUT ==="
grep 'metrics' "$LOG_FILE" | strip_ansi | \
    grep -oP 'events_per_sec="\K[0-9.]+' | \
    awk '{sum+=$1; count++; if($1>max)max=$1}
         END {if(count>0) printf "  Avg: %.1f events/sec, Peak: %.1f events/sec\n", sum/count, max
              else print "  No metrics found"}'

TOTAL_EVENTS=$(grep 'metrics' "$LOG_FILE" | strip_ansi | tail -1 | grep -oP 'events_total=\K[0-9]+')
echo "  Total events processed: ${TOTAL_EVENTS:-0}"

echo ""
echo "=== STITCHING ==="
STITCH_SUCCESS=$(grep -c 'track_stitched' "$LOG_FILE")
STITCH_LOST=$(grep -c 'stitch_expired_lost' "$LOG_FILE")
STITCH_TOTAL=$((STITCH_SUCCESS + STITCH_LOST))
echo "  Successful stitches: $STITCH_SUCCESS"
echo "  Lost (expired): $STITCH_LOST"
if [ "$STITCH_TOTAL" -gt 0 ]; then
    STITCH_RATE=$(echo "scale=1; $STITCH_SUCCESS * 100 / $STITCH_TOTAL" | bc)
    echo "  Success rate: ${STITCH_RATE}%"
fi

echo ""
echo "=== REENTRY DETECTION ==="
REENTRY_MATCHED=$(grep -c 'reentry_matched' "$LOG_FILE")
REENTRY_NO_MATCH=$(grep -c 'reentry_no_match' "$LOG_FILE")
echo "  Reentries detected: $REENTRY_MATCHED"
echo "  No match (new person): $REENTRY_NO_MATCH"

echo ""
echo "=== ERRORS ==="
ERROR_COUNT=$(grep -c 'ERROR' "$LOG_FILE")
echo "  Total errors: $ERROR_COUNT"
if [ "$ERROR_COUNT" -gt 0 ]; then
    echo "  Recent errors:"
    grep 'ERROR' "$LOG_FILE" | strip_ansi | tail -5 | sed 's/^/    /'
fi

echo ""
echo "=== RS485 DOOR POLLING (sample) ==="
grep 'rs485_status' "$LOG_FILE" | strip_ansi | tail -1 | \
    grep -oP 'poll_duration_us=\K[0-9]+' | \
    awk '{printf "  Last poll: %dus\n", $1}'

echo ""
echo "========================================"
echo "Analysis complete"
echo "========================================"
