# POS Tracking Migration Plan

## Before vs After Summary

### BEFORE (Current Implementation)

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         CURRENT ARCHITECTURE                             │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  Person Struct                       AccCollector                        │
│  ┌────────────────────┐              ┌────────────────────────────────┐ │
│  │ track_id           │              │ pos_sessions:                  │ │
│  │ current_zone       │              │   HashMap<TrackId, Vec<Sess>>  │ │
│  │ zone_entered_at    │◄────────────►│                                │ │
│  │ accumulated_dwell  │  duplicated  │ recent_exits:                  │ │
│  │   (GLOBAL)         │    state     │   HashMap<Zone, Vec<Exit>>     │ │
│  │ authorized         │              │                                │ │
│  └────────────────────┘              │ Manual cleanup (120s, 2×)      │ │
│                                      └────────────────────────────────┘ │
│                                                                          │
│  Group tracks: FILTERED OUT (0x80000000 bit check)                       │
│  Dwell: Global across ALL POS zones                                      │
│  ACC match: Zone-specific sessions, but global dwell threshold           │
│  Cleanup: Manual pruning with retention windows                          │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

**Pain Points:**
- State duplicated between Person and AccCollector
- Global dwell doesn't reflect zone-specific customer behavior
- Manual cleanup code is error-prone
- Group tracks ignored (may miss edge cases)

---

### AFTER (Proposed Implementation)

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         NEW ARCHITECTURE                                 │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  PosOccupancyState (NEW)                                                 │
│  ┌────────────────────────────────────────────────────────────────────┐ │
│  │ zones: HashMap<ZoneId, HashMap<TrackId, PosState>>                 │ │
│  │                                                                    │ │
│  │ PosState:                                                         │ │
│  │   is_present: bool                                                │ │
│  │   entry_time: Option<Instant>                                     │ │
│  │   exit_time: Option<Instant>                                      │ │
│  │   accumulated_dwell_ms: u64  (PER-ZONE)                           │ │
│  │                                                                    │ │
│  │ Grace window: keep exited entries for 5s                          │ │
│  │ Prune only on entry/exit/ACC                                      │ │
│  └────────────────────────────────────────────────────────────────────┘ │
│                                                                          │
│  Person Struct (simplified)          AccCollector (simplified)           │
│  ┌────────────────────┐              ┌────────────────────────────────┐ │
│  │ track_id           │              │ ip_to_pos mapping              │ │
│  │ current_zone       │              │ (delegates to PosOccupancy)    │ │
│  │ authorized         │              └────────────────────────────────┘ │
│  │ (no dwell fields)  │                                                 │
│  └────────────────────┘                                                 │
│                                                                          │
│  Group tracks: TRACKED (same as person tracks)                           │
│  Dwell: Per-zone, zone-specific ACC matching                             │
│  Cleanup: Event-driven prune on entry/exit/ACC                           │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

**Benefits:**
- Single source of truth for POS occupancy
- Per-zone dwell = accurate ACC matching
- No background cleanup or timers
- Group tracks included for edge cases
- Deterministic candidate selection

---

## Key Behavioral Changes

| Behavior | Before | After |
|----------|--------|-------|
| Customer at POS_1 (3s) → POS_2 (5s) | 8s global dwell, qualifies | 3s at POS_1, 5s at POS_2, neither qualifies |
| ACC from POS_1 | Matches anyone with global dwell ≥ 7s | Only matches tracks with POS_1 dwell ≥ 7s |
| Group track enters POS | Ignored | Tracked, can authorize, can open gate |
| Track exits POS, ACC arrives 4s later | 3s window, no match | 5s grace, matches |
| Cleanup of stale entries | Manual 120s pruning | Prune on entry/exit/ACC when exit + grace < now |

---

## Goal

- Target: 100% ACC match rate for customers who complete POS dwell

---

## Implementation Plan

### Phase 1: Add per-zone POS state machine

**Files to modify:**
- Create `src/services/pos_occupancy.rs` - New module

**Responsibilities:**
- Track entry/exit timestamps and accumulated dwell per zone
- Apply a fixed exit grace window
- Prune expired exits only on entry/exit/ACC events

### Phase 2: Remove group track filtering

**Files to modify:**
- `src/services/tracker/handlers.rs` - Remove `is_group_track()` checks from handlers

### Phase 3: Integrate with tracker

**Files to modify:**
- `src/services/tracker/mod.rs` - Add `pos_occupancy` field
- `src/services/tracker/handlers.rs`:
  - Update entry/exit handlers to record POS state
  - Use POS state for ACC matching (prefer present, then recent exits within grace)
  - Prune only on entry/exit/ACC events
  - Use ACC `ts_recv` as the matching timestamp (frame time only for zone ordering)

### Phase 4: Simplify AccCollector

**Files to modify:**
- `src/services/acc_collector.rs`:
  - Remove `pos_sessions` and `recent_exits`
  - Keep only: IP→zone mapping, metrics, logging

### Phase 5: Simplify Person struct

**Files to modify:**
- `src/domain/types.rs`:
  - Remove `zone_entered_at: Option<Instant>`
  - Remove `accumulated_dwell_ms: u64`
  - Keep `current_zone` for gate zone detection

### Phase 6: Verify/update journey logging for new POS events

**Files to modify:**
- `src/services/tracker/handlers.rs` - Append entry/exit events to journey log
- Persist journey via MQTT on exit/track delete

### Phase 7: Tests

**Files to modify:**
- `src/services/tracker/tests.rs` - Update dwell tests for per-zone semantics
- Create `src/services/pos_occupancy/tests.rs` - Unit tests for new module
- Add group track tests (currently missing)

---

## Configuration Changes

```toml
[pos_tracking]
exit_grace_ms = 5000     # How long after exit to keep candidates
min_dwell_ms = 7000      # Minimum dwell to qualify
```

---

## Migration Risks

1. **Semantic change**: Customers who browse multiple POS zones briefly may no longer qualify
   - Mitigation: Could lower `min_dwell_ms` threshold

2. **Grace window ambiguity**: A new shopper could enter shortly after an exit
   - Mitigation: Prefer present occupants, enforce `min_dwell_ms`, adjust grace as needed

3. **Group track side effects**: May cause unexpected authorizations
   - Open question: Do we have data on group-track frequency, or do we deploy and
     monitor before tightening the rules?
   - Mitigation: Monitor metrics, add group-specific logic if needed

---

## Verification

1. Run existing test suite: `cargo test`
2. Deploy to test environment
3. Monitor ACC match rate (should improve)
4. Check for group track authorizations in logs
5. Verify pruning keeps state bounded under normal load
