# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Gateway-poc is a Rust application for automated retail gate control. It processes sensor data from Xovis people-counting cameras via MQTT, tracks customer journeys through the store, and opens gates for authorized customers who have spent sufficient time at point-of-sale zones.

**Target hardware**: Raspberry Pi 5 (aarch64-unknown-linux-gnu)

## Build and Development Commands

**Rust Version**: Pinned to stable via `rust-toolchain.toml`. This ensures consistent Rust version across all developers and deployment targets (RPi5).

```bash
# Build
cargo build --release

# Run with config
cargo run -- --config config/dev.toml
RUST_LOG=debug cargo run -- --config config/dev.toml

# Run TUI monitoring dashboard
cargo run --bin gateway-tui -- --config config/dev.toml

# Run gate testing utility
cargo run --bin gate_test

# Tests
cargo test                          # All tests
cargo test tracker::tests           # Specific module
cargo test test_name                # Single test by name
cargo test -- --nocapture           # With stdout

# Format and lint
cargo fmt
cargo clippy
```

### Cross-compilation for Raspberry Pi

```bash
# Option 1: Zig-based (recommended)
cargo install cargo-zigbuild
cargo zigbuild --release --target aarch64-unknown-linux-gnu

# Option 2: Remote build on target
rsync -avz --exclude target --exclude .git ./ avero@HOST:~/gateway-poc/
ssh avero@HOST "cd ~/gateway-poc && cargo build --release"
```

### Deployment

```bash
# Deploy main service to Netto
./scripts/deploy-netto.sh

# Deploy TUI
./scripts/deploy-tui-netto.sh
./scripts/deploy-tui-avero.sh

# Manual service control
ssh avero@HOST "sudo systemctl stop gateway-poc"
ssh avero@HOST "sudo systemctl start gateway-poc"
ssh avero@HOST "sudo journalctl -u gateway-poc -f"  # Live logs
```

## Architecture

### Data Flow Pipeline

```
Ingress → Processing → Egress

Ingress:
  - MQTT client receives Xovis JSON → ParsedEvent
  - RS485 polls door state → DoorStateChange events
  - ACC TCP listener receives payment terminal events

Processing (Tracker - central orchestrator):
  - Person state (HashMap<track_id, Person>)
  - Stitcher: track identity continuity across sensor gaps
  - JourneyManager: journey lifecycle
  - DoorCorrelator: gate command ↔ door state matching
  - AccCollector: payment event correlation

Egress:
  - Completed journeys → JSONL file
  - Metrics → Prometheus HTTP endpoint
```

### Module Structure

```
src/
├── main.rs                 # Entry point, orchestrates startup
├── bin/
│   ├── tui.rs             # Terminal UI dashboard
│   └── gate_test.rs       # Gate testing utility
├── domain/                 # Business models
│   ├── types.rs           # Core types (ParsedEvent, Person, EventType)
│   └── journey.rs         # Journey model for customer paths
├── io/                     # External interfaces
│   ├── mqtt.rs            # MQTT client (Xovis ingestion)
│   ├── rs485.rs           # Serial communication for door state
│   ├── cloudplus.rs       # TCP client for CloudPlus gate protocol
│   ├── acc_listener.rs    # TCP listener for payment terminals
│   ├── prometheus.rs      # Prometheus metrics endpoint
│   └── egress.rs          # Journey output to JSONL
├── services/              # Business logic
│   ├── tracker/           # Core event orchestrator
│   │   ├── mod.rs
│   │   ├── handlers.rs
│   │   └── tests.rs       # Primary test file
│   ├── journey_manager.rs
│   ├── stitcher.rs        # Track stitching across gaps
│   ├── door_correlator.rs
│   └── acc_collector.rs
└── infra/                 # Infrastructure
    ├── config.rs          # TOML configuration
    ├── metrics.rs         # Lock-free metrics collection
    └── broker.rs          # Embedded MQTT broker
```

### Key Domain Concepts

- **Journey**: Customer path from entry to exit (UUID, track IDs, events, outcome)
- **Person**: Active customer state (track_id, dwell_ms, authorized flag)
- **ParsedEvent**: Xovis sensor event (TrackCreate, TrackDelete, ZoneEntry, ZoneExit)
- **Stitching**: Reconnecting track identity when sensor temporarily loses a person

### Configuration

Config files in `config/` directory (TOML format):
- `dev.toml` - Local development
- `netto.toml` - Netto production
- `grandi.toml` - Grandi production

Key settings:
- `zones.pos_zones` - POS area geometry IDs for dwell tracking
- `zones.gate_zone` - Gate entry trigger zone
- `authorization.min_dwell_ms` - Required dwell time (default 7000ms)
- `gate.mode` - "tcp" (CloudPlus) or "http"

## Key Behaviors

- **Authorization**: Customer is authorized after spending `min_dwell_ms` in any POS zone
- **Gate trigger**: When authorized customer enters gate zone, send open command
- **Journey completion**: On EXIT line cross or TRACK_DELETE
- **Stitching**: Maintains identity across temporary track loss (gap detection)

## Testing

Primary tests are in `src/services/tracker/tests.rs`. Test helpers:
- `create_test_tracker()` - Factory with default config
- `create_event()` - Test event builder

All tests use `#[tokio::test]` for async testing.

## Code Quality Requirements

**Before committing, always run:**
```bash
cargo fmt --check      # Verify formatting
cargo clippy -- -D warnings   # Lint with warnings as errors
cargo test             # Run all tests
```

## Rust Coding Guidelines

### Avoid Common Pitfalls

| Don't | Do | Why |
|-------|-----|-----|
| `.unwrap()` on Mutex locks | Use `parking_lot::Mutex` (no poisoning) | Panics on poisoned lock |
| `Option<&String>` return type | Return `Option<&str>` | Unnecessary indirection |
| `.is_none()` then `.unwrap()` | Use `if let Some(val) = x` | Fragile pattern |
| Custom `from_str` method | Implement `std::str::FromStr` trait | Idiomatic Rust |
| `vec.remove(idx)` when order doesn't matter | Use `vec.swap_remove(idx)` | O(1) vs O(n) |
| `Vec::new()` when size is known | Use `Vec::with_capacity(n)` | Avoids reallocations |
| `HashMap<i64, T>` for integer keys | Use `FxHashMap` from `rustc-hash` | Faster hashing |
| Casting type to itself (`x as u64` when x is u64) | Remove the cast | Unnecessary |

### Performance Patterns

- **Pre-allocate collections**: Use `Vec::with_capacity()` when typical size is known
- **Use `#[cold]` on error paths**: Helps optimizer focus on hot path
- **Use `#[inline]` on small cross-crate functions**: Especially getters and epoch_ms-style helpers
- **Use `.copied()` for iterators of Copy types**: `iter().copied()` instead of `iter().cloned()`
- **Avoid holding locks across await points**: Can cause deadlocks in async code

### Type System Best Practices

- **Add `#[derive(Copy)]` to small fieldless enums**: Eliminates unnecessary `.clone()` calls
- **Consider newtype pattern for IDs**: `struct TrackId(i64)` prevents accidentally mixing ID types
- **Use enums over strings for fixed value sets**: Compile-time validation instead of runtime
- **Add size assertions for key types**:
  ```rust
  const _: () = assert!(std::mem::size_of::<EventType>() <= 32);
  ```

### Clippy Lints to Watch

These are common issues flagged by clippy in this codebase:
- `declare_interior_mutable_const`: Use `static` for `AtomicU64`, not `const`
- `too_many_arguments`: Functions should have ≤7 arguments
- `unnecessary_get_then_check`: Use `!map.contains_key()` directly
- `if_same_then_else`: Don't duplicate if/else blocks

### Safety Pitfalls

| Pitfall | Fix |
|---------|-----|
| `Box::leak()` for static strings | Use owned `String` instead |
| Multiple locks in different orders | Consolidate into single state struct |
| `value as u64` for i64→u64 | Use `TryFrom` to handle negatives |
| Silent config fallback to defaults | Validate config and fail loudly on errors |
| HTTP Response builder `.unwrap()` | Use `.expect("static response")` |

### Dependencies Available

These are already in `Cargo.toml` - use them:
- `parking_lot` - Better Mutex (no poisoning, no `.unwrap()` needed)
- `rustc-hash` - Fast `FxHashMap` for integer keys
- `clap` - CLI argument parsing (derive macro)
- `anyhow` - Error handling with context
- `smallvec` - Stack-allocated small vectors

## Testing Guidelines

### Test Philosophy

Write tests for **business logic correctness**, not code coverage metrics:
- Time boundary tests at critical authorization thresholds
- State machine transitions for door/journey lifecycle
- Error handling for network/protocol failures
- Config validation to prevent production misconfigurations

**Avoid**:
- Testing trivial getters/setters
- Duplicating existing integration coverage
- Tests that just exercise code without verifying business behavior

### Critical Time Boundaries to Test

These thresholds determine customer authorization - always test boundary conditions:

| Boundary | Module | Value | Test Pattern |
|----------|--------|-------|--------------|
| ACC group window | acc_collector | 10,000ms | 9,999ms (pass) vs 10,001ms (fail) |
| ACC recent exit | acc_collector | 1,500ms | 1,499ms (pass) vs 1,501ms (fail) |
| Stitch grace time (base) | stitcher | 4,500ms | 4,499ms (pass) vs 4,501ms (fail) |
| Stitch grace time (POS) | stitcher | 8,000ms | 7,999ms (pass) vs 8,001ms (fail) |
| Stitch distance (base) | stitcher | 180cm | 179cm (pass) vs 181cm (fail) |
| Stitch distance (same zone) | stitcher | 300cm | 299cm (pass) vs 301cm (fail) |
| Height tolerance | stitcher/reentry | 10cm | 9.9cm (pass) vs 10.1cm (fail) |
| Door correlation | door_correlator | 5,000ms | 4,999ms (pass) vs 5,001ms (fail) |
| Re-entry window | reentry_detector | 30,000ms | 29,999ms (pass) vs 30,001ms (fail) |

### Key Scenarios to Test

These are common production scenarios that need test coverage:

1. **Gate Blocked**: Unauthorized customer enters gate zone → logs "blocked" event, no gate open
2. **Chain Stitching**: Track A→B→C through multiple sensor gaps → all track IDs preserved
3. **GROUP Bit Filtering**: Xovis group tracks (track_id with 0x80000000 bit) → skipped
4. **Re-entry Flow**: Customer exits and re-enters → new journey has parent reference
5. **ACC Group Authorization**: Multiple customers at POS → all group members authorized
6. **Door State Cycle**: Closed→Moving→Open→Moving→Closed → flow track preserved then cleared

### Test Patterns

```rust
// Boundary test pattern
#[tokio::test]
async fn test_stitch_grace_time_boundary() {
    let mut stitcher = Stitcher::new();

    // Just within threshold - should match
    let within = create_pending_track(now_ms - 4499);
    assert!(stitcher.find_match(...).is_some());

    // Just outside threshold - should NOT match
    let outside = create_pending_track(now_ms - 4501);
    assert!(stitcher.find_match(...).is_none());
}

// State machine test pattern
#[tokio::test]
async fn test_door_state_cycle() {
    let mut correlator = DoorCorrelator::new();
    correlator.record_gate_cmd(track_id, now);

    // Verify state preserved through cycle
    correlator.process_door_state(DoorStatus::Moving, now + 100);
    assert!(correlator.current_flow_track_id().is_some());

    correlator.process_door_state(DoorStatus::Open, now + 200);
    assert!(correlator.current_flow_track_id().is_some());

    correlator.process_door_state(DoorStatus::Closed, now + 1000);
    assert!(correlator.current_flow_track_id().is_none()); // Cleared on close
}
```

### Protocol/IO Testing

For modules accepting external input (MQTT, RS485, ACC, CloudPlus):
- Test malformed input returns graceful error, not panic
- Test partial data handling (waits for more vs rejects)
- Test checksum/validation failures are detected
- Test boundary values (empty, max length, invalid UTF-8)
