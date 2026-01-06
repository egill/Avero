# Journey Management System Requirements

## Overview

Implement a journey management system for the gateway-poc that tracks people through a retail store, correlates payment events, monitors gate commands, and persists completed journeys.

## Context

- **Codebase**: Rust async application using tokio
- **Location**: `/Users/egill/Documents/GitHub/Avero-inspect/gateway-poc/`
- **Existing modules**: tracker.rs, stitcher.rs, config.rs, types.rs, mqtt.rs, gate.rs, rs485.rs

## Architecture

```
MQTT (Xovis) --> Parser --> Tracker --> JourneyManager --> Egress (file)
                              |               ^
TCP (ACC) --> AccCollector ---+---------------+
                                              |
RS485 (Door) --> DoorMonitor -----------------+
```

---

## Phase 1: Journey Data Model

### Requirements

1. Create `src/journey.rs` with the following types:

```rust
pub struct Journey {
    pub jid: String,           // UUIDv7 journey ID
    pub pid: String,           // UUIDv7 person ID (stable across stitches)
    pub tids: Vec<i32>,        // Xovis track_ids (stitch history)
    pub parent: Option<String>, // Previous journey's jid (for re-entry)
    pub outcome: JourneyOutcome,
    pub authorized: bool,
    pub total_dwell_ms: u64,
    pub acc_matched: bool,
    pub gate_cmd_at: Option<u64>,      // epoch ms
    pub gate_opened_at: Option<u64>,   // epoch ms from RS485
    pub gate_was_open: bool,
    pub started_at: u64,               // epoch ms
    pub ended_at: Option<u64>,         // epoch ms
    pub crossed_entry: bool,
    pub events: Vec<JourneyEvent>,
}

pub enum JourneyOutcome {
    InProgress,
    Completed,  // crossed EXIT
    Abandoned,  // track deleted, never crossed EXIT
}

pub struct JourneyEvent {
    pub t: String,              // event type
    pub z: Option<String>,      // zone or line name
    pub ts: u64,                // epoch ms
    pub extra: Option<String>,  // additional data
}
```

2. Implement `Journey::to_json()` that outputs short-key format:
```json
{
  "jid": "019a...",
  "pid": "019a...",
  "tids": [100, 200],
  "parent": null,
  "out": "exit",
  "auth": true,
  "dwell": 7500,
  "acc": true,
  "gate_cmd": 1736012345678,
  "gate_open": 1736012345890,
  "gate_was_open": false,
  "t0": 1736012340000,
  "t1": 1736012350000,
  "ev": [
    {"t": "entry_cross", "ts": 1736012340000},
    {"t": "zone_entry", "z": "POS_1", "ts": 1736012341000},
    {"t": "zone_exit", "z": "POS_1", "ts": 1736012348500, "x": "dwell=7500"}
  ]
}
```

3. Add UUIDv7 generation (use `uuid` crate with v7 feature)

### Success Criteria
- [ ] `Journey` and `JourneyEvent` structs compile
- [ ] `to_json()` produces valid JSON with short keys
- [ ] UUIDv7 generation works
- [ ] Unit tests pass for serialization

---

## Phase 2: JourneyManager Core

### Requirements

1. Create `src/journey_manager.rs`:

```rust
pub struct JourneyManager {
    active: HashMap<i32, Journey>,      // track_id -> Journey
    pending_egress: Vec<PendingEgress>, // journeys waiting 10s before emit
    pid_by_track: HashMap<i32, String>, // track_id -> person_id mapping
}

struct PendingEgress {
    journey: Journey,
    eligible_at: Instant,  // 10s after journey ended
}
```

2. Implement core methods:
   - `new_journey(track_id: i32) -> Journey` - creates new journey with new pid
   - `stitch_journey(old_track_id: i32, new_track_id: i32)` - merges journeys, appends to tids
   - `add_event(track_id: i32, event: JourneyEvent)` - appends event to journey
   - `end_journey(track_id: i32, outcome: JourneyOutcome)` - moves to pending_egress
   - `tick()` - checks pending_egress, emits journeys past 10s window

3. Stitch handling:
   - When stitch occurs, remove journey from pending_egress if present
   - Inherit pid from old journey
   - Append new track_id to tids array
   - Continue adding events

4. Filtering:
   - Only emit journeys where `crossed_entry == true`
   - Discard journeys that never crossed ENTRY line

### Success Criteria
- [ ] JourneyManager compiles
- [ ] Can create, stitch, and end journeys
- [ ] Pending egress respects 10s delay
- [ ] Stitch within 10s window merges correctly
- [ ] Journeys without entry crossing are discarded
- [ ] Unit tests for all scenarios

---

## Phase 3: Integration with Tracker

### Requirements

1. Modify `Tracker` to use `JourneyManager`:
   - On track_create: call `journey_manager.new_journey()` or handle stitch
   - On zone_entry/exit: call `journey_manager.add_event()`
   - On line_cross (ENTRY): set `crossed_entry = true`
   - On line_cross (EXIT forward): call `end_journey(Completed)`
   - On track_delete: call `end_journey(Abandoned)`
   - On gate_command: record `gate_cmd_at`

2. Periodic tick:
   - Call `journey_manager.tick()` every 1 second
   - Tick returns Vec of journeys ready for egress

3. Update stitch flow:
   - When `Stitcher::find_match()` succeeds, call `journey_manager.stitch_journey()`

### Success Criteria
- [ ] Tracker creates journeys on track_create
- [ ] Events are recorded in journey
- [ ] Entry line crossing is tracked
- [ ] Exit triggers journey completion
- [ ] Stitch properly merges journeys
- [ ] Integration tests pass

---

## Phase 4: ACC Correlation

### Requirements

1. Add IP-to-POS mapping in config.toml:
```toml
[acc_mapping]
"192.168.1.10" = "POS_1"
"192.168.1.11" = "POS_2"
"192.168.1.12" = "POS_3"
```

2. Create `src/acc_collector.rs`:
   - Parse ACC events from TCP (already exists partially)
   - Store recent ACC events with timestamp and POS

3. ACC matching rules:
   - Match ACC to person who:
     - Had dwell time >= 7000ms at that POS, AND
     - Either: currently at POS, OR left < 1500ms ago AND POS is now empty
   - When matched: set `journey.acc_matched = true`, add event

4. Integrate with JourneyManager:
   - JourneyManager receives ACC events
   - Correlates with active journeys based on rules

### Success Criteria
- [ ] Config parses IP-to-POS mapping
- [ ] ACC events are collected and stored
- [ ] Matching logic correctly identifies person
- [ ] Journey is updated when ACC matches
- [ ] Unit tests for correlation logic

---

## Phase 5: Gate/Door Correlation

### Requirements

1. RS485 door state tracking:
   - Already polling at 250ms intervals
   - Emit on state change (closed/moving/open)

2. Gate command correlation:
   - When door transitions to "open" after we sent gate_command:
     - Calculate time delta
     - Set `journey.gate_opened_at`
     - If door was already open when command sent: `gate_was_open = true`

3. Match gate_open to journey:
   - Find journey with recent `gate_cmd_at` (within 5s)
   - Same track should be in GATE zone

### Success Criteria
- [ ] Door state changes are detected
- [ ] Gate open is correlated to command
- [ ] `gate_was_open` correctly identifies pre-opened gates
- [ ] Journey updated with gate timing
- [ ] Unit tests pass

---

## Phase 6: Egress Implementation

### Requirements

1. Create `src/egress.rs`:
   - Write journeys to file (simulating MQTT egress)
   - File path from config: `egress_file = "journeys.jsonl"`
   - One JSON object per line (JSONL format)

2. Egress format:
   - Use short-key JSON from Phase 1
   - Append to file (don't overwrite)
   - Include newline after each record

3. Wire up:
   - JourneyManager.tick() returns ready journeys
   - Main loop calls egress for each

### Success Criteria
- [ ] Journeys are written to JSONL file
- [ ] Format matches specification
- [ ] File is appended, not overwritten
- [ ] Integration test: full journey from track_create to file output

---

## Phase 7: Re-entry Detection (Optional)

### Requirements

1. Detect re-entry:
   - Person exits (crosses EXIT line)
   - New track appears at ENTRY within 30s
   - Height matches within +/- 10cm

2. Link journeys:
   - Inherit pid from previous journey
   - Set `parent` to previous journey's jid

### Success Criteria
- [ ] Re-entry detected by height matching
- [ ] Same pid assigned
- [ ] Parent field correctly set
- [ ] Unit tests pass

---

## Event Types Reference

| Event | Type String | Zone Field | Extra |
|-------|-------------|------------|-------|
| Entry line cross | `entry_cross` | - | - |
| Exit line cross | `exit_cross` | - | - |
| Approach line cross | `approach_cross` | - | - |
| Zone entry | `zone_entry` | zone name | - |
| Zone exit | `zone_exit` | zone name | `dwell=X` |
| ACC payment | `acc` | POS name | `kiosk=IP` |
| Gate command sent | `gate_cmd` | - | `latency_us=X` |
| Gate opened | `gate_open` | - | `delta_ms=X` |
| Track stitch | `stitch` | - | `from=track_id,time_ms=X,dist_cm=Y` |
| Track pending | `pending` | last zone | `auth=T/F,dwell=X` |

---

## Completion Signal

When all phases are complete and tests pass:

```
<promise>COMPLETE</promise>
```

## Iteration Guidelines

1. Complete one phase at a time
2. Run `cargo test` after each phase
3. Run `cargo build` to verify compilation
4. Fix any errors before proceeding
5. If stuck on a phase for 3+ iterations, skip to next phase and return later
