#!/bin/bash
set -e

MAX_ITERATIONS=${1:-10}
SCRIPT_DIR="$(cd "$(dirname \
  "${BASH_SOURCE[0]}")" && pwd)"

echo "🚀 Starting Ralph"

for i in $(seq 1 $MAX_ITERATIONS); do
  echo "═══ Iteration $i ═══"
  
  echo "Running claude..."
  OUTPUT=$(cat "$SCRIPT_DIR/prompt.md" | claude --dangerously-skip-permissions \
    --print \
    --verbose 2>&1 \
    | tee "$SCRIPT_DIR/log.txt") || true
  echo "Claude finished"
  
  if echo "$OUTPUT" | \
    grep -q "<promise>COMPLETE</promise>"
  then
    echo "✅ Done!"
    exit 0
  fi
  
  sleep 2
done

echo "⚠️ Max iterations reached"
exit 1
