# Gateway PoC — Whole Codebase Review & Architecture Vision

Scope: `gateway-poc/src/*.rs` and existing per-file reviews (`*.rs.review.md`)

## Executive Summary
This codebase is already a coherent end-to-end pipeline (MQTT → ParsedEvent → Tracker → JourneyManager → JSONL egress + gate/door correlation). The biggest improvement opportunities are **structural cohesion** and **operational correctness**:

- **Cohesion**: there are two separate “Journey” models (`journey.rs` vs `types.rs`), and `Tracker` orchestrates many responsibilities.
- **Operational robustness**: shutdown is not end-to-end, and logging volume is likely too high for production.
- **Performance**: avoidable hot-path overhead exists (per-event async lock, per-event allocations, O(n) front-drains / Vec::remove shifts, per-command HTTP client creation).

The recommended direction is **targeted refactor + operational hardening**, done incrementally with minimal behavior change, while keeping room for optional breaking changes once metrics/logging validate the gains.

## Operating Assumptions (confirmed)
- **Embedded MQTT broker is production**: `rumqttd` is part of the deployed system and should be treated as a first-class, observable, well-bounded component.
- **Durability is best-effort**: journey egress should be reliable under normal operation, but occasional loss on crash/power failure is acceptable (no need for fsync/transactional guarantees at this stage).

## Current Architecture (as implemented)

### Data flow
- **Ingress**:
  - `mqtt.rs` parses Xovis JSON into `types::ParsedEvent` and pushes into an `mpsc` channel.
  - `rs485.rs` polls door state and emits `EventType::DoorStateChange(DoorStatus)` into the same event channel.
- **Processing**:
  - `tracker.rs` is the main event loop:
    - maintains active `types::Person` state,
    - handles stitching (`stitcher.rs`),
    - manages journeys (`journey_manager.rs` + `journey.rs`),
    - correlates gate cmd → door open (`door_correlator.rs`),
    - detects re-entry (`reentry_detector.rs`),
    - egresses completed journeys (`egress.rs`),
    - sends gate open commands (`gate.rs` + `cloudplus.rs`).
- **Egress**:
  - `egress.rs` appends JSONL to the configured file.

### Observability & ops
- Tracing logs throughout; default log level is `INFO`.
- Metrics are collected via `metrics.rs` but currently logged in `main.rs` as a heartbeat; hot-path updates are behind an async mutex.

## Cross-Cutting Issues & Opportunities

### 1) Domain model duplication: two Journeys
**Problem**
- `journey.rs` defines the canonical journey that is egressed (UUIDs, tids, outcome, JSON encoding).
- `types.rs` defines an additional `Journey`/`JourneyEvent` embedded in `types::Person`, used for some logging/short-term tracking.

**Risks**
- Inconsistencies (events recorded in one journey but not the other).
- Cognitive overhead (ambiguous type names, easy to misuse imports).
- Bugs when evolving behavior (two places to update).

**Direction**
- Near-term (non-breaking): remove ambiguity with import aliases and naming conventions; reduce reliance on `types::Journey` for anything beyond debugging.
- Optional breaking: unify into one canonical journey model and delete the duplicate.

### 2) Tracker as an orchestration “god object”
**Problem**
- `Tracker` owns: person state, stitching, journey state machine, re-entry detection, door correlation, gate IO, metrics, egress ticking.

**Risks**
- Hard to test in isolation; small changes can have wide impact.
- Hard to reason about invariants (e.g., when is a journey created vs stitched vs ended vs egressed).

**Direction**
- Introduce internal “service” boundaries without changing public APIs:
  - `tracker/state.rs`: person + dwell + authorization transitions.
  - `tracker/journeys.rs`: mapping from events → journey_manager calls.
  - `tracker/side_effects.rs`: gate commands + door correlation hooks.
  - Keep `Tracker` as façade that wires these.

### 3) End-to-end shutdown is incomplete (operational correctness)
**Problem**
- `main.rs` creates a shutdown signal, but:
  - `rs485.rs` ignores shutdown.
  - `mqtt.rs` runs forever with no cancellation.
  - `tracker.rs` stops only when the channel closes (which is not tied to shutdown).

**Direction**
- Non-breaking operational hardening:
  - propagate shutdown to MQTT + tracker;
  - close/drop senders on shutdown so tracker exits;
  - ensure spawned tasks are awaited or aborted for clean shutdown semantics.

### 4) Logging volume is likely the dominant performance & cost issue
**Problem**
- Many modules log per-event/per-poll/per-heartbeat at `info` (cloudplus, rs485 noise paths, tracker event logs, egress logs).

**Direction**
- Move high-frequency logs to `debug`.
- Keep `info` for state transitions and periodic summaries.
- Add counters for “dropped events”, “stitch expired”, “no valid rs485 frame”, etc., so the signal remains without log storms.

### 5) Hot-path performance improvements (without changing behavior)
Key hotspots and safe improvements:
- **Per-event async metrics lock** (`tracker.rs`):
  - consider atomics or a lock-free channel to aggregate metrics off-thread.
- **Per-command HTTP client creation** (`gate.rs`):
  - cache `reqwest::Client` in the controller.
- **Vec front-drains / Vec::remove shifting**:
  - `cloudplus.rs` uses `acc.drain(..consumed)` repeatedly; use a cursor/ring buffer (`BytesMut`) to avoid memmoves.
  - `stitcher.rs` / `reentry_detector.rs` / `door_correlator.rs` use `Vec::remove(idx)`; `swap_remove` can reduce shifting (if ordering is irrelevant).
- **Config hot paths**:
  - `Config::is_pos_zone` is `Vec::contains`; add an internal `HashSet` cache while keeping the public API stable.
- **Timestamps**:
  - `mqtt.rs` currently sets `event_time` to `0`; either implement parsing or make “timestamp disabled” explicit and keep using `Instant` for durations.

## Target Structure (recommended)
Goal: make boundaries explicit and reduce coupling while preserving behavior.

### Proposed module grouping (within the existing crate)
- `domain/`
  - `journey.rs` (canonical)
  - `event.rs` (ParsedEvent/EventType + time semantics)
  - `person.rs` (Person state machine) *(optional, depends on duplication removal)*
- `io/`
  - `mqtt.rs`
  - `rs485.rs`
  - `cloudplus.rs`
  - `egress.rs` (arguably IO)
- `services/`
  - `tracker.rs` (facade)
  - `stitcher.rs`
  - `door_correlator.rs`
  - `reentry_detector.rs`
  - `acc_collector.rs`
- `infra/`
  - `config.rs`
  - `metrics.rs`
  - `broker.rs` (or move under `bin/` if dev-only)
- `bin/`
  - `main.rs` (wires everything + lifecycle management)

This can be done incrementally without a crate split. If this becomes truly production, splitting into a library crate + thin binary is a natural next step.

## Invariants to make explicit (to improve solidity)
- **Event ordering**: what happens if ZONE_ENTRY arrives before TRACK_CREATE? Should `Tracker` overwrite existing `Person` on TRACK_CREATE?
- **Time semantics**:
  - `received_at: Instant` is the authoritative monotonic time for duration comparisons.
  - `event_time` meaning must be defined (sensor epoch ms vs “unknown/0”).
  - `epoch_ms()` is wall-clock and should be treated as “egress/log timestamp”.
- **Journey lifecycle**:
  - created → (stitch)* → end (completed/abandoned) → delayed pending → emit/discard
  - clarify which transitions are allowed and what state must be synced from `Person` into `Journey` at each transition.

## Recommended Roadmap (incremental, low risk)

### Phase 1 — Operational hardening (no behavior change on the happy path)
- **End-to-end shutdown** (MQTT, tracker, rs485) so Ctrl+C behaves predictably.
- **Production broker hardening**:
  - explicit configuration of bind address/port and resource limits (and document rationale),
  - health/liveness signal (log/metric) rather than fixed sleeps for readiness,
  - failure semantics: if broker fails to start, decide whether to exit or continue (best-effort is acceptable, but it must be explicit and observable).
- **Reduce log volume**; keep signal with counters/periodic summaries.
- Add lightweight validation/warnings for config load fallback and misconfigs.

### Phase 2 — Hot-path performance cleanup (still non-breaking)
- Cache HTTP client in `GateController`.
- Reduce buffer/memmove churn in `cloudplus.rs`.
- Reduce per-event lock contention for metrics.

### Phase 3 — Structural refactor (mostly non-breaking; optional breaking)
- Establish explicit module boundaries (domain/io/services/infra).
- De-duplicate Journey models (optional breaking if downstream expects internal types).
- Encapsulate invariants with narrower mutation surfaces (optional breaking).

## Biggest “bang for buck” improvements
1) Logging level + rate limiting (especially cloudplus + rs485 noise paths).
2) End-to-end shutdown correctness.
3) Gate HTTP client reuse.
4) Remove journey-model duplication (even if only via aliases + “use canonical only” policy initially).

