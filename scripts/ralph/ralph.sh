#!/bin/bash
set -e

MAX_ITERATIONS=${1:-10}
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOG_FILE="$SCRIPT_DIR/ralph.log"

echo "🚀 Ralph - Autonomous Coding Agent"
echo "   Branch: ralph/perf-fixes"
echo "   Max iterations: $MAX_ITERATIONS"
echo "   Log file: $LOG_FILE"
echo ""

# Ensure we're on the right branch
CURRENT_BRANCH=$(git branch --show-current)
if [[ "$CURRENT_BRANCH" != "ralph/perf-fixes" ]]; then
  echo "📌 Creating branch ralph/perf-fixes..."
  git checkout -b ralph/perf-fixes 2>/dev/null || git checkout ralph/perf-fixes
fi

for i in $(seq 1 $MAX_ITERATIONS); do
  echo "═══════════════════════════════════════"
  echo "  Iteration $i of $MAX_ITERATIONS"
  echo "═══════════════════════════════════════"

  # Run claude with prompt
  echo "🤖 Running Claude..."
  OUTPUT=$(claude --dangerously-skip-permissions \
    --print \
    "$SCRIPT_DIR/prompt.md" 2>&1 \
    | tee -a "$LOG_FILE") || true

  echo ""

  # Check for completion
  if echo "$OUTPUT" | grep -q "COMPLETE"; then
    echo "✅ All stories complete!"
    echo ""
    echo "Summary:"
    grep '"passes": true' "$SCRIPT_DIR/prd.json" | wc -l | xargs echo "  - Completed stories:"
    exit 0
  fi

  # Brief pause between iterations
  echo "⏳ Waiting 2s before next iteration..."
  sleep 2
done

echo ""
echo "⚠️  Max iterations ($MAX_ITERATIONS) reached"
echo "    Check $LOG_FILE for details"
exit 1
