#!/bin/bash
# Usage: ./loop.sh [plan] [max_iterations]
# Examples:
#   ./loop.sh              # Build mode, unlimited iterations
#   ./loop.sh 20           # Build mode, max 20 iterations
#   ./loop.sh plan         # Plan mode, unlimited iterations
#   ./loop.sh plan 5       # Plan mode, max 5 iterations

set -uo pipefail

# Directory paths
RALPH_DIR=".ralph"
RUNS_DIR="$RALPH_DIR/runs"
ACTIVITY_LOG="$RALPH_DIR/activity.log"
ERRORS_LOG="$RALPH_DIR/errors.log"
GUARDRAILS_FILE="$RALPH_DIR/guardrails.md"

# Initialize .ralph directory structure
mkdir -p "$RALPH_DIR" "$RUNS_DIR"

# Generate unique run tag for this execution
RUN_TAG="$(date +%Y%m%d-%H%M%S)-$$"

# Logging functions
log_activity() {
    local message="$1"
    local timestamp
    timestamp=$(date '+%Y-%m-%d %H:%M:%S')
    echo "[$timestamp] $message" >> "$ACTIVITY_LOG"
}

log_error() {
    local message="$1"
    local timestamp
    timestamp=$(date '+%Y-%m-%d %H:%M:%S')
    echo "[$timestamp] ERROR: $message" >> "$ERRORS_LOG"
    echo "[$timestamp] ERROR: $message" >> "$ACTIVITY_LOG"
}

# Git helper functions
git_head() {
    if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
        git rev-parse HEAD 2>/dev/null || echo ""
    else
        echo ""
    fi
}

# Parse arguments
if [ "${1:-}" = "plan" ]; then
    MODE="plan"
    PROMPT_FILE=".agents/ralph/PROMPT_plan.md"
    MAX_ITERATIONS=${2:-0}
elif [[ "${1:-}" =~ ^[0-9]+$ ]]; then
    MODE="build"
    PROMPT_FILE=".agents/ralph/PROMPT_build.md"
    MAX_ITERATIONS=$1
else
    MODE="build"
    PROMPT_FILE=".agents/ralph/PROMPT_build.md"
    MAX_ITERATIONS=0
fi

ITERATION=0
CURRENT_BRANCH=$(git branch --show-current 2>/dev/null || echo "main")

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Mode:   $MODE"
echo "Prompt: $PROMPT_FILE"
echo "Branch: $CURRENT_BRANCH"
echo "Run:    $RUN_TAG"
[ $MAX_ITERATIONS -gt 0 ] && echo "Max:    $MAX_ITERATIONS iterations"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Verify prompt file exists
if [ ! -f "$PROMPT_FILE" ]; then
    log_error "Prompt file not found: $PROMPT_FILE"
    echo "Error: $PROMPT_FILE not found"
    exit 1
fi

# Log loop start
log_activity "LOOP START run=$RUN_TAG mode=$MODE branch=$CURRENT_BRANCH"

while true; do
    ITERATION=$((ITERATION + 1))

    if [ $MAX_ITERATIONS -gt 0 ] && [ $ITERATION -gt $MAX_ITERATIONS ]; then
        log_activity "LOOP END run=$RUN_TAG reason=max_iterations count=$((ITERATION - 1))"
        echo "Reached max iterations: $MAX_ITERATIONS"
        break
    fi

    echo -e "\n\n═══════════════════════════════════════════════════════"
    echo "  Ralph Iteration $ITERATION (run: $RUN_TAG)"
    echo "═══════════════════════════════════════════════════════"

    # Capture timing
    ITER_START=$(date +%s)
    HEAD_BEFORE="$(git_head)"

    # Set up run log file
    LOG_FILE="$RUNS_DIR/run-$RUN_TAG-iter-$ITERATION.log"

    log_activity "ITERATION $ITERATION start mode=$MODE"

    # Run Ralph iteration with selected prompt
    set +e
    cat "$PROMPT_FILE" | claude -p \
        --dangerously-skip-permissions \
        --output-format=stream-json \
        --model opus \
        --verbose 2>&1 | tee "$LOG_FILE"
    CMD_STATUS=$?
    set -e

    # Capture end timing
    ITER_END=$(date +%s)
    ITER_DURATION=$((ITER_END - ITER_START))

    if [ "$CMD_STATUS" -ne 0 ]; then
        log_error "ITERATION $ITERATION command failed status=$CMD_STATUS"
    fi

    log_activity "ITERATION $ITERATION end duration=${ITER_DURATION}s"

    # Check for completion signal
    if grep -q "<promise>COMPLETE</promise>" "$LOG_FILE"; then
        echo ""
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo "ALL STORIES COMPLETE!"
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        log_activity "LOOP END run=$RUN_TAG reason=all_complete iterations=$ITERATION"
        break
    fi

    # Push changes after each iteration
    git push origin "$CURRENT_BRANCH" 2>/dev/null || {
        echo "Failed to push. Creating remote branch..."
        git push -u origin "$CURRENT_BRANCH" || log_error "ITERATION $ITERATION git push failed"
    }

    echo ""
done

log_activity "LOOP FINISHED run=$RUN_TAG total_iterations=$ITERATION"
