Build a Rust proof-of-concept for a gate control system. Work in /Users/egill/Documents/GitHub/Avero-inspect/gateway-poc/

Read REQUIREMENTS.md for full specification. Summary:
- Subscribe to MQTT topic xovis/sensor, parse Xovis JSON events
- Track person dwell time in POS zones (1001-1005), authorize when >= 7s
- When authorized person enters GATE_1 (1007), send HTTP GET to open gate
- Poll RS485 at 250ms intervals, log door status changes
- Log everything with microsecond precision for latency analysis

## Phases

### Phase 1: Project Setup
- Initialize Cargo project with dependencies from REQUIREMENTS.md
- Create file structure: main.rs, config.rs, mqtt.rs, tracker.rs, gate.rs, rs485.rs, metrics.rs, types.rs
- Verify: cargo build succeeds

### Phase 2: Types & Config
- Define Event, Person, Journey structs per REQUIREMENTS.md
- Load config from environment variables
- Verify: cargo build succeeds

### Phase 3: MQTT Client
- Connect to broker, subscribe to xovis/sensor
- Parse JSON, extract track_id, events, geometry_id
- Send parsed events to bounded channel
- Verify: unit tests pass, cargo test

### Phase 4: Person Tracker
- Maintain HashMap of tracked persons
- Accumulate dwell time across POS zones
- Mark authorized when dwell >= 7000ms
- Track journey events with timestamps
- Verify: unit tests for dwell logic pass

### Phase 5: Gate Control
- HTTP GET on GATE_1 entry for authorized persons
- Log latency (target < 10ms from event to command)
- Single attempt, log errors, no retry
- Verify: unit tests pass, mock HTTP in tests

### Phase 6: RS485 Monitoring
- Mock RS485 for macOS development
- Poll every 250ms, log status changes
- Track poll timing accuracy
- Verify: mock tests pass

### Phase 7: Integration
- Wire all components in main.rs
- Periodic metrics logging (every 10s)
- Journey completion on EXIT_1 line cross
- Journey abandonment on TRACK_DELETE
- Verify: cargo build --release succeeds, cargo test all pass

### Phase 8: Verification
- Run cargo clippy - no warnings
- Run cargo test - all tests pass
- Verify log output format matches REQUIREMENTS.md examples

## Completion Criteria
- All phases complete
- cargo build --release succeeds
- cargo test passes with 0 failures
- cargo clippy has no warnings

When ALL criteria are met, output exactly:
<promise>COMPLETE</promise>

If stuck on a phase for more than 2 iterations, add TODO comments explaining the blocker and proceed to next phase.
