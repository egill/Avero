# ACC to Person/Track Matching

## Overview

When an ACC (payment) event arrives, we need to determine which person(s) made the payment. This document describes the matching approach validated through replay analysis.

## Key Insight

**Use sensor time (`frame_time`), not receive time (`ts_recv`)** for determining occupancy state.

| Analysis Method | Unmatched Rate |
|-----------------|----------------|
| ts_recv + interval-based | 22.6% |
| frame_time + replay | 0.8% |

The flawed analysis led to misguided assumptions about a "homeless ACC" problem that largely didn't exist.

## Matching Algorithm

### On ACC Arrival

```
1. Get current occupancy for the POS zone
2. Filter to tracks with accumulated_dwell >= min_dwell (7s)
3. If candidates found → match (prefer longest dwell as primary)
4. If zone empty → check recent exits within TTL
5. If recent exit found with sufficient dwell → match (late auth)
6. Otherwise → no match
```

### State to Track Per Zone

**Current Occupancy:**
- Track IDs currently in the zone (is_present = true)
- Include both person tracks AND group tracks

**Recent Exits (TTL-based):**
- Track ID
- Exit time
- Accumulated dwell (for qualification check)
- Auto-expires after TTL (2500-3000ms sufficient)

### Data Structure

Use a TTL cache (e.g., `moka`) for unified tracking:

```rust
struct ZoneOccupant {
    track_id: TrackId,
    accumulated_dwell_ms: u64,
    is_present: bool,
    last_activity: Instant,
}

// Per zone - auto-expires entries after TTL
zone_occupants: Cache<(ZoneId, TrackId), ZoneOccupant>
```

**On ZONE_ENTRY:**
- Set `is_present = true`
- Update `last_activity`
- Reset TTL

**On ZONE_EXIT:**
- Add dwell to `accumulated_dwell_ms`
- Set `is_present = false`
- Update `last_activity`
- Entry remains for TTL duration (grace window for late ACC)

**On ACC:**
```rust
let candidates: Vec<_> = zone_occupants
    .iter()
    .filter(|(_, o)| {
        let within_ttl = o.is_present || now.duration_since(o.last_activity) < TTL;
        within_ttl && o.accumulated_dwell_ms >= min_dwell
    })
    .collect();
```

## Why Include Group Tracks?

Xovis creates group tracks (track_id >= 2^31) when people are close together. Including them:
- Provides backup candidates when person track exits just outside grace window
- Group track often lingers slightly longer than person track
- Helps catch edge cases

## Late Auth Cases

From analysis of 261 ACC events on 2026-01-12:
- 259 matched via current occupancy
- 2 matched via exit grace window (customer already at gate)
- 0 truly unmatched

The 2 "late auth" cases:
- Customer left POS 2-2.8 seconds before ACC arrived
- Customer was already in GATE zone when ACC fired
- Exit grace window of 2500ms caught both

## Configuration

| Parameter | Value | Purpose |
|-----------|-------|---------|
| `min_dwell_ms` | 7000 | Minimum accumulated dwell to qualify |
| `exit_grace_ms` | 2500-3000 | TTL for recent exits |
| `max_occupants_per_zone` | 50 | Hard cap for memory safety |

## Receipt Tracking

Receipts can be invalidated by either:
- **Gate exit** (auth-based) - person walks through
- **Barcode scan** - manual exit

Use bidirectional lookup with TTL:
```rust
receipt_cache: Cache<ReceiptId, ReceiptInfo>  // barcode lookup
track_to_receipt: Cache<TrackId, ReceiptId>   // gate exit lookup
```

Both invalidate together. 1-hour max TTL.
