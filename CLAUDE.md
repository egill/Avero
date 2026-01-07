# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Gateway-poc is a Rust application for automated retail gate control. It processes sensor data from Xovis people-counting cameras via MQTT, tracks customer journeys through the store, and opens gates for authorized customers who have spent sufficient time at point-of-sale zones.

**Target hardware**: Raspberry Pi 5 (aarch64-unknown-linux-gnu)

## Build and Development Commands

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
