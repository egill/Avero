# Simctl Test Scenarios TODO

Additional test scenarios to implement for the gateway simulation system.

## Current Tests (Passing)

- [x] `happy_path` - Customer pays and exits through gate
- [x] `no_payment` - Customer dwells but doesn't pay (gate blocked)
- [x] `fast_exit` - Customer doesn't dwell long enough (gate blocked)
- [x] `gate_zone_exit` - Customer enters gate zone after paying
- [x] `multiple_pos_zones` - Customer visits multiple POS zones before paying

## Authorization Edge Cases

- [ ] `acc_before_dwell` - ACC event arrives before dwell threshold met, then customer continues dwelling. Should authorize when threshold reached.
- [ ] `acc_wrong_zone` - Customer dwells at POS_1, ACC arrives for POS_2. Gate should be blocked.
- [ ] `multiple_acc_events` - ACC flicker merge - multiple ACC events within 10s window. Should count as one authorization.

## Track Continuity

- [ ] `track_stitch` - Track ID changes mid-journey (sensor loses then re-acquires). Authorization should persist across stitched tracks.
- [ ] `track_delete_before_exit` - Track deleted before reaching gate. Journey ends as incomplete.

## Re-entry Scenarios

- [ ] `reentry_after_exit` - Customer exits store, re-enters within 30s window. New journey should have parent reference.
- [ ] `pos_zone_reentry` - Customer exits POS zone briefly, re-enters within grace window. Dwell should continue accumulating.

## Gate Behavior

- [ ] `gate_entry_no_exit` - Authorized customer enters gate but never crosses exit line. Verify gate opened on entry.
- [ ] `multiple_gate_entries` - Customer enters/exits gate zone multiple times. Only first entry should trigger gate command.
- [ ] `unauthorized_gate_entry` - Customer enters gate zone without any POS visit. Should be blocked.

## Anomaly Detection

- [ ] `backward_entry` - Customer crosses entry line in wrong direction. Should create incident.
- [ ] `gate_tailgating` - Second customer enters gate zone while door is open from first customer.

## Implementation Notes

### Adding a New Scenario

1. Add scenario to `SCENARIOS` array in `src/bin/simctl.rs`
2. Define steps using `ScenarioStep` enum variants:
   - `CreateTrack { in_store: bool }` - Create new track
   - `DeleteTrack` - Delete current track
   - `ZoneEntry("ZONE_NAME")` - Enter a zone
   - `ZoneExit("ZONE_NAME")` - Exit a zone
   - `LineCross { line: "LINE_NAME", forward: bool }` - Cross a line
   - `Acc("POS_NAME")` - Trigger ACC event for POS zone
   - `Wait(ms)` - Wait specified milliseconds

3. Set expected outcome:
   - `ExpectedOutcome::GateOpen` - Gate should open
   - `ExpectedOutcome::GateBlocked` - Gate should block

### Key Thresholds

| Threshold | Value | Description |
|-----------|-------|-------------|
| `min_dwell_ms` | 7000ms | Minimum dwell time at POS for authorization |
| `exit_grace_ms` | 5000ms | Grace window for POS zone re-entry |
| `acc_flicker_merge_s` | 10s | Window for merging multiple ACC events |
| `recent_exit_window_ms` | 3000ms | Window for ACC matching after POS exit |
| `reentry_window_ms` | 30000ms | Window for detecting re-entry after exit |

### Running Tests

```bash
# Run all tests
cargo run --bin simctl -- --test all

# Run specific test
cargo run --bin simctl -- --test happy_path

# Run with verbose output
cargo run --bin simctl -- --test all --verbose
```
