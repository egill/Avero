# Journey Diagnostic Issue Tracking

This directory contains tools and data for diagnosing journey anomalies.

## Files

- `.last-check` - Timestamp of last diagnostic run (epoch ms)
- `issues-to-review.jsonl` - Detected issues awaiting review
- `issue-counters.json` - Lifetime counters for each issue type
- `issue-tasklist.jsonl` - Issues promoted for investigation

## Scripts

### chat-journeys.py (NEW)

Natural language interface to journey data, powered by Claude.

```bash
# Install dependencies
pip install -r ../requirements.txt

# Set API key
export ANTHROPIC_API_KEY="sk-ant-..."

# Interactive mode
python ../chat-journeys.py

# Single query
python ../chat-journeys.py "show me journeys with lost tracking after POS"
```

**Example queries:**
- "Show me journeys where someone spent time at POS but tracking was lost"
- "What's the ACC match rate for the last 24 hours?"
- "Why did journey abc123 not get authorized?"
- "Find journeys where gate command was sent but no exit detected"
- "Create an issue for this journey"

**Tools available to Claude:**
- `query_journeys` - Search journeys with filters
- `get_journey_detail` - Get full events for a journey
- `get_journey_stats` - Aggregate statistics
- `query_logs` - Search RPi logs
- `create_issue` - Create tracking issue
- `get_issue_stats` - View issue counters

### diagnose-journeys.sh

Main diagnostic script. Fetches journey data and logs from RPi, analyzes for anomalies.

```bash
# First run (analyzes last 24 hours)
./scripts/diagnose-journeys.sh

# Run for specific time window
./scripts/diagnose-journeys.sh --since "2h ago"
./scripts/diagnose-journeys.sh --since "2024-01-08T10:00:00"

# Analyze different site
./scripts/diagnose-journeys.sh --site grandi

# Dry run (don't write issues)
./scripts/diagnose-journeys.sh --dry-run --verbose
```

### review-issues.sh

Interactive tool for reviewing and managing detected issues.

```bash
# List all issues
./scripts/review-issues.sh

# List only new issues
./scripts/review-issues.sh --list-new

# Review specific issue (interactive)
./scripts/review-issues.sh --review abc123

# Dismiss an issue
./scripts/review-issues.sh --dismiss abc123

# Promote to investigation tasklist
./scripts/review-issues.sh --promote abc123

# Show statistics
./scripts/review-issues.sh --stats

# View tasklist
./scripts/review-issues.sh --list-tasklist
```

### query-journeys-db.sh

Query TimescaleDB directly for journey data.

```bash
# Query journeys since timestamp
./scripts/query-journeys-db.sh --since "2h ago" --site netto

# Find journeys with no ACC match
./scripts/query-journeys-db.sh --no-acc --has-dwell

# Count abandoned journeys
./scripts/query-journeys-db.sh --outcome abandoned --count

# Export to file
./scripts/query-journeys-db.sh --since "24h ago" --export journeys.jsonl

# Custom SQL
./scripts/query-journeys-db.sh --query "SELECT COUNT(*) FROM person_journeys WHERE outcome = 'abandoned'"
```

## Issue Types

| Type | Severity | Description |
|------|----------|-------------|
| `exit_no_acc` | medium | Customer exited with POS dwell but no ACC match |
| `tracking_lost_with_pos` | high | Tracking lost after significant POS time |
| `acc_match_tracking_lost` | high | ACC matched but tracking lost before exit |
| `gate_cmd_no_exit` | medium | Gate command sent but no exit confirmation |
| `high_acc_unmatched_rate` | medium | System-wide high ACC unmatched rate |
| `high_stitch_expiry_rate` | medium | System-wide high track loss rate |

## Issue Lifecycle

1. **new** - Just detected, awaiting review
2. **reviewed** - Looked at, notes added
3. **dismissed** - Not a real issue (false positive)
4. **promoted** - Added to investigation tasklist

## Counter Interpretation

Counters track how many times each issue type has been detected across all runs.
High counts for certain types may indicate:

- `exit_no_acc` high → ACC timing issues, network problems, or cash payments
- `tracking_lost_with_pos` high → Sensor coverage gaps, stitch timing too aggressive
- `acc_match_tracking_lost` high → Gate area sensor issues
- `high_acc_unmatched_rate` high → Systemic ACC communication problems

## Suggested Workflow

1. Run `diagnose-journeys.sh` periodically (cron or manual)
2. Review new issues with `review-issues.sh`
3. Dismiss false positives, promote real issues
4. Investigate promoted issues from tasklist
5. Monitor counters for trends
