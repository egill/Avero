# Gateway PoC - Rust MVP Requirements

## Goal

Build a proof of concept in Rust to validate performance characteristics (latency, predictability, no GC pauses) for a gate control system running on Raspberry Pi 5.

## Performance Priorities

**Latency is the top priority.** The system must make gate decisions as fast as possible.

- **Gate decision latency**: < 10ms from GATE_1 zone entry to HTTP command sent (target: < 1ms)
- **Event processing**: < 1ms p99
- **No latency spikes**: > 5ms indicates a problem (GC, blocking, etc.)

## Expected Load

- **Message rate**: ~50+ messages/sec from Xovis sensor
- **Concurrent tracks**: 20-50 people tracked simultaneously at peak
- **Sustained operation**: 24/7 without degradation

## Expected Output

The app should:

1. **Open the gate** for anyone who has stayed in a POS zone for more than 7 seconds (accumulated dwell time)
2. **Send the open command** via HTTP to the gate controller when they enter the gate zone
3. **Measure gate status** via RS485 polling - detect when gate actually opens/closes
4. **Log everything** in a structured format that allows post-analysis of performance (latencies, timing, throughput)
5. **Log completed journeys** with all events and timings when person crosses EXIT_1, or mark as abandoned on TRACK_DELETE

Example log output for a successful journey:
```
2026-01-05T14:30:00.000000Z INFO track_created track_id=100
2026-01-05T14:30:01.123456Z INFO zone_entry track_id=100 zone=POS_1 event_time=1767617601000
2026-01-05T14:30:08.234567Z INFO dwell_threshold_met track_id=100 zone=POS_1 dwell_ms=7111 authorized=true
2026-01-05T14:30:10.345678Z INFO zone_exit track_id=100 zone=POS_1 event_time=1767617610000
2026-01-05T14:30:15.456789Z INFO zone_entry track_id=100 zone=GATE_1 event_time=1767617615000
2026-01-05T14:30:15.457012Z INFO gate_open_command track_id=100 latency_us=223
2026-01-05T14:30:15.650000Z INFO rs485_status door=opening
2026-01-05T14:30:16.100000Z INFO rs485_status door=open
2026-01-05T14:30:20.345678Z INFO line_cross track_id=100 line=EXIT_1 direction=forward event_time=1767617620000
2026-01-05T14:30:20.345700Z INFO journey_complete track_id=100 authorized=true gate_opened=true duration_ms=20345 events=6
2026-01-05T14:30:20.500000Z INFO rs485_status door=closing
2026-01-05T14:30:21.000000Z INFO rs485_status door=closed
```

Example log output for abandoned journey (track deleted):
```
2026-01-05T14:35:00.000000Z INFO track_created track_id=200
2026-01-05T14:35:05.123456Z INFO zone_entry track_id=200 zone=POS_2 event_time=1767617905000
2026-01-05T14:35:10.234567Z INFO track_deleted track_id=200
2026-01-05T14:35:10.234600Z INFO journey_abandoned track_id=200 authorized=false gate_opened=false duration_ms=10234 events=2 last_zone=POS_2
```

From these logs we can analyze:
- Xovis event_time vs received_time (sensor-to-gateway latency)
- Time from zone_entry to dwell_threshold_met
- Time from gate zone entry to gate_open_command (decision latency)
- Time from gate_open_command to rs485 showing door=opening (command latency)
- Time from door=opening to door=open (gate hardware latency)
- Overall journey timing
- How many journeys complete vs abandoned

## Target Hardware

- Raspberry Pi 5 (4 core ARM64, 8GB RAM, 256GB SSD)
- Cross-compile from macOS to `aarch64-unknown-linux-gnu`

## Core Functionality

### 1. MQTT Ingestion

- Connect to MQTT broker as client (broker at `localhost:1883` for testing)
- Subscribe to `xovis/sensor` topic
- Parse Xovis JSON messages containing:
  - `live_data.frames[].tracked_objects[]` - person positions
  - `live_data.frames[].events[]` - zone entry/exit, line cross, track create/delete
- Extract: `track_id`, `type` (PERSON only), `position`, event types, `geometry_id`

Sample Xovis message structure:
```json
{
  "live_data": {
    "frames": [{
      "time": 1767617614000,
      "tracked_objects": [{
        "track_id": 123,
        "type": "PERSON",
        "position": [1.5, 2.0, 1.7]
      }],
      "events": [{
        "type": "ZONE_ENTRY",
        "attributes": {
          "track_id": 123,
          "geometry_id": 1001
        }
      }]
    }]
  }
}
```

### 2. Person State Tracking

- Maintain in-memory state for each tracked person:
  - `track_id: i32`
  - `current_zone: Option<i32>`
  - `zone_entered_at: Option<Instant>`
  - `accumulated_dwell_ms: u64` (accumulated time spent in any POS zone)
  - `authorized: bool` (true when accumulated_dwell >= 7000ms)

- Zone IDs:
  - POS zones: 1001, 1002, 1003, 1004, 1005 (POS_1 through POS_5)
  - Gate zone: 1007 (GATE_1)

- Logic:
  - On `ZONE_ENTRY` to any POS zone: record `zone_entered_at`
  - On `ZONE_EXIT` from any POS zone: add elapsed time to `accumulated_dwell_ms`
  - When `accumulated_dwell_ms >= 7000`: mark `authorized = true`, log `dwell_threshold_met`
  - On `ZONE_ENTRY` to GATE_1: if `authorized`, send gate open command
  - On `TRACK_DELETE`: remove person from state

Note: Dwell is accumulated across all POS zones. If someone spends 4s at POS_1 then 4s at POS_2, they have 8s accumulated and are authorized.

### 2.1 Journey Tracking

For every person, maintain a journey log with events and timings:

```rust
struct JourneyEvent {
    event_type: String,      // "zone_entry", "zone_exit", "line_cross", etc.
    zone_or_line: String,    // "POS_1", "GATE_1", "EXIT_1", etc.
    event_time: u64,         // Xovis event timestamp (ms since epoch)
    received_time: Instant,  // When we received it
}

struct Journey {
    track_id: i32,
    events: Vec<JourneyEvent>,
    authorized: bool,
    gate_command_sent: Option<Instant>,
}
```

**Journey completion:**
- On `LINE_CROSS_FORWARD` on EXIT_1 (geometry_id 1006): journey is complete
- Log the full journey with all events and timings

**Journey abandoned:**
- On `TRACK_DELETE`: log the journey as incomplete with all events so far
- Note: stitching will be implemented in next iteration

**Journey log format:**
```
2026-01-05T14:30:25.000000Z INFO journey_complete track_id=100 authorized=true gate_opened=true events=[
  {type=track_create, time=1767617600000, received=0ms},
  {type=zone_entry, zone=POS_1, time=1767617601000, received=+1001ms},
  {type=zone_exit, zone=POS_1, time=1767617609000, received=+9002ms, dwell=8001ms},
  {type=zone_entry, zone=GATE_1, time=1767617615000, received=+15003ms},
  {type=gate_command, time=+15003ms, latency_us=234},
  {type=line_cross, line=EXIT_1, time=1767617620000, received=+20004ms}
] total_duration_ms=20004
```

This allows post-analysis of:
- Xovis event_time vs received_time (sensor-to-gateway latency)
- Dwell time accuracy
- Gate command latency
- Total journey duration

### 3. Gate Control

**Trigger**: When authorized person enters GATE_1 zone (geometry_id 1007)

**Action**:
- Send HTTP GET: `http://admin:88888888@10.120.48.9/cdor.cgi?door=0&open=1`
- Log the command with timestamp and track_id
- Measure latency from zone entry event to HTTP response received
- **Target**: < 10ms from zone entry to command sent

**Behavior**:
- **Always send command** - even if gate is already open (gate controller handles duplicates)
- **Single attempt** - no retry on failure
- **On HTTP failure**: log error with status code, continue processing (don't block)
- **No pre-opening** - only trigger on actual GATE_1 zone entry, not on approach

**Implementation notes**:
- Simple HTTP GET with basic auth, no persistent connection needed
- Use async HTTP client with short timeout (e.g., 2 seconds)
- Log HTTP response status and timing

Note: CloudPlus TCP protocol (with heartbeat) deferred to next iteration. HTTP is simpler for MVP.

### 4. RS485 Monitoring

- Poll RS485 device at `/dev/ttyAMA4` (or configurable) every 250ms
- Baud rate: 9600
- Send query command, read response
- Parse response for: door_status, fault, alarm
- Log any status changes
- Measure poll timing accuracy (should be exactly 250ms intervals)

- For PoC on macOS: mock the RS485, simulate responses

### 5. Metrics & Observability

All logs should be structured and include timestamps with microsecond precision for post-analysis.

**Key metrics to capture in logs:**

- **Event processing latency**: time from MQTT message received to state updated
- **Gate decision latency**: time from GATE_1 zone entry to HTTP command sent
- **RS485 poll timing**: actual interval between polls (should be ~250ms)
- **Gate response time**: time from command sent to RS485 showing door state change

**Periodic metrics summary** (every 10 seconds):
```
2026-01-05T14:30:10.000000Z INFO metrics events_total=150 events_per_sec=15.0 avg_process_latency_us=45 max_process_latency_us=120 active_tracks=5 authorized_tracks=2 gate_commands_sent=3
```

**The logs should allow us to answer:**
1. What's the p50/p99 event processing latency?
2. How long from "person enters gate zone" to "gate command sent"?
3. How long from "gate command sent" to "RS485 shows door opening"?
4. Are there any latency spikes? (indicates GC or blocking)
5. Is RS485 polling staying on schedule?

### 6. Logging

- Structured logging with timestamps (microsecond precision)
- Log levels: ERROR, WARN, INFO, DEBUG
- Log format:
```
2026-01-05T14:30:00.123456Z INFO [mqtt] received event track_id=123 type=ZONE_ENTRY zone=1001
2026-01-05T14:30:07.234567Z INFO [tracker] dwell_met track_id=123 zone=POS_1 dwell_ms=7012
2026-01-05T14:30:15.345678Z INFO [gate] open_command track_id=123 latency_us=1234
```

## Architecture

```
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│  MQTT Client    │────▶│  Event Channel  │────▶│    Tracker      │
│  (rumqttc)      │     │  (bounded)      │     │  (single task)  │
└─────────────────┘     └─────────────────┘     └────────┬────────┘
                                                         │
                                                         ▼
┌─────────────────┐                              ┌─────────────────┐
│  RS485 Poller   │                              │  Gate Command   │
│  (250ms timer)  │                              │  (HTTP client)  │
└─────────────────┘                              └─────────────────┘
```

- Use `tokio` async runtime
- Single main task for tracker (no concurrency in state management)
- Separate tasks for: MQTT client, RS485 poller, metrics reporter

## Dependencies

```toml
[dependencies]
tokio = { version = "1", features = ["full"] }
rumqttc = "0.24"              # MQTT client
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio-serial = "5.4"          # RS485 serial
reqwest = { version = "0.11", features = ["json"] }  # HTTP client for gate
tracing = "0.1"               # Logging
tracing-subscriber = "0.3"
```

## Configuration

Use environment variables or a simple config file:

```
MQTT_HOST=localhost
MQTT_PORT=1883
MQTT_TOPIC=xovis/sensor
GATE_URL=http://admin:88888888@10.120.48.9/cdor.cgi?door=0&open=1
GATE_TIMEOUT_MS=2000
RS485_DEVICE=/dev/ttyAMA4
RS485_BAUD=9600
MIN_DWELL_MS=7000
METRICS_INTERVAL_SECS=10
```

## Success Criteria

1. **Latency**: Event processing < 1ms p99
2. **Predictability**: No latency spikes > 5ms (no GC)
3. **RS485 timing**: Poll interval within 1ms of 250ms target
4. **Throughput**: Handle 100+ events/sec sustained
5. **Memory**: Stable memory usage under load (no sawtooth)

## Test Scenarios

### Scenario 1: Normal Flow
1. Start system
2. Send MQTT: TRACK_CREATE for track_id=100
3. Send MQTT: ZONE_ENTRY track_id=100 to POS_1 (1001)
4. Wait 8 seconds
5. Send MQTT: ZONE_EXIT track_id=100 from POS_1
6. Verify: track is marked authorized
7. Send MQTT: ZONE_ENTRY track_id=100 to GATE_1 (1007)
8. Verify: gate open command triggered
9. Log latency from step 7 to gate command

### Scenario 2: Burst Load
1. Send 500 MQTT messages in 1 second
2. Verify no latency spike > 5ms
3. Verify RS485 polling stays on schedule

### Scenario 3: Sustained Load
1. Send 100 messages/sec for 60 seconds
2. Monitor memory (should be flat)
3. Monitor latency (should be consistent)

## File Structure

```
gateway-poc/
├── Cargo.toml
├── src/
│   ├── main.rs           # Entry point, setup, run loop
│   ├── config.rs         # Configuration loading
│   ├── mqtt.rs           # MQTT client and message parsing
│   ├── tracker.rs        # Person state tracking logic
│   ├── gate.rs           # Gate control (HTTP commands)
│   ├── rs485.rs          # RS485 polling (mock for now)
│   ├── metrics.rs        # Metrics collection and reporting
│   └── types.rs          # Shared types (Event, Person, etc.)
└── README.md
```

## Out of Scope for PoC

- Embedded MQTT broker (use external broker for testing)
- SQLite persistence
- Egress to central broker
- ACC tag correlation (linking NFC tags to tracked persons)
- Barcode scanning
- Group detection
- Track stitching (person ID reconnection after track loss)
- Full Xovis message parsing (only parse what we need)
- HTTP API endpoints
- CloudPlus TCP protocol (using simple HTTP for MVP)

## Notes

- Focus on measuring and proving the latency/predictability characteristics
- Keep it simple - this is a validation, not production code
- Mock external systems (gate HTTP, RS485) when running on macOS
- Real RS485 only when deployed to RPi
