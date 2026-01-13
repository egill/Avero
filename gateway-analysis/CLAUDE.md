# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a data analysis directory containing JSONL log captures from the gateway-poc system. The logs are used for offline analysis of customer tracking, payment correlation, and gate control events at retail sites (primarily Netto).

**Parent project**: See `../CLAUDE.md` for the main gateway-poc Rust application.

## Data Structure

```
gateway-analysis/
├── acc/           # ACC (payment terminal) events, split per kiosk IP
│   └── {kiosk_ip}-{YYYYMMDD}.jsonl
├── mqtt/          # MQTT topic captures
│   ├── xovis-sensor-{YYYYMMDD}.jsonl  # Xovis people-tracking frames + events
│   └── xovis-status-{YYYYMMDD}.jsonl  # Xovis device status
└── rs485/         # Door state polling (RS485 serial)
    └── rs485-{YYYYMMDD}.jsonl
```

### JSONL Schema (Unified)

All log files use a common envelope:
```json
{
  "ts_recv": "ISO8601 timestamp when received",
  "src": "mqtt|acc|rs485",
  "site": "netto|grandi|...",
  "payload_raw": "original message/frame",
  "fields": { /* parsed fields specific to source */ }
}
```

**ACC fields**: `kiosk_ip`, `pos_zone`, `receipt_id`
**MQTT fields**: `topic`, `parsed` (nested Xovis JSON)
**RS485 fields**: `checksum_ok`, `door_status` (closed/open/moving)

## Analysis Scripts

Analysis scripts live in `../scripts/`. Key ones for this data:

| Script | Purpose |
|--------|---------|
| `acc-person2-review.py` | Core module - loads ACC/zone events, builds intervals, finds candidates |
| `acc-person0-analysis.py` | Analyze "person_0" ACCs (no candidate found) |
| `acc-person0-positions.py` | Position analysis for person_0 events |
| `acc-group-eval.py` | Evaluate ACC group correlation variants |
| `acc-group-report.py` | Summarize group evaluation results |

### Running Analysis

```bash
# From gateway-poc root
python3 scripts/acc-person2-review.py \
  --date 20260112 \
  --log-dir ./gateway-analysis \
  --config config/netto.toml

# Who-was-in query for specific POS at specific time
python3 scripts/acc-person2-review.py \
  --log-dir ./gateway-analysis \
  --who-pos POS_1 \
  --who-time "2026-01-12T14:30:00+00:00"

# Person_0 analysis with CSV output
python3 scripts/acc-person0-analysis.py \
  --date 20260112 \
  --log-dir ./gateway-analysis \
  --csv person0-report.csv
```

## Key Concepts

**Zone Events**: ZONE_ENTRY/ZONE_EXIT from Xovis sensors, keyed by geometry_id
**ACC Events**: Payment completion events from CloudPlus terminals
**Person Count**: Number of track candidates at a POS zone when ACC fires
- person_0: No candidate (missed detection)
- person_1: Single candidate (ideal)
- person_2+: Multiple candidates (ambiguous)

**Interval Merging**: Zone presence intervals are merged with `flicker_merge_s` (default 10s) to handle brief sensor dropouts.

**Grace Window**: Events received up to `grace_ms` after ACC time are included to compensate for network latency.

## Configuration

Analysis scripts read from the same TOML config as the main application:
- `zones.names`: Map geometry_id -> zone name (e.g., "1009" -> "POS_1")
- `acc.ip_to_pos`: Map kiosk IP -> POS zone
- `acc.flicker_merge_s`: Interval merge window (default 10)

## Timeline Viewer Tool

Interactive TUI for viewing POS activity timeline:

```bash
# Run from gateway-analysis directory
python pos_timeline.py --date 20260112

# Or specify log directory
python pos_timeline.py --date 20260112 --log-dir /var/log/gateway-analysis
```

**Keyboard Controls:**
- `j/k` or `↑/↓` - Scroll up/down
- `h/l` or `←/→` - Previous/next hour
- `n/p` - Jump to next/prev ACC event (auto-changes hour)
- `c` - Copy current ACC receipt ID to clipboard (pbcopy)
- `1-5` - Switch to POS_1 through POS_5
- `f` - Toggle filter: zone events only vs all events
- `q` - Quit

**Columns:**
- Time - event timestamp
- Event - `>> ENTER`, `<< EXIT`, or `$$$ ACC $$$`
- Track - track_id that entered/exited
- In Zone (after) - list of track_ids currently in zone after this event
- Details - receipt_id, height, position, +/- change indicators

## Python Dependencies

```bash
pip install polars textual
```
