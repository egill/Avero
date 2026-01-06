# Gateway PoC — Refactor Roadmap (Non-Breaking First)

This roadmap turns `CODEBASE_REVIEW.md` into a concrete, low-risk sequence of changes that improves structure, correctness, and performance without changing behavior unless explicitly labeled.

## Guiding principles
- **Stability first**: preserve current behavior and external interfaces.
- **Observability before refactor**: change logging/metrics to make behavior measurable, then refactor with confidence.
- **Small PRs**: keep each step independently valuable and reversible.
- **Best-effort durability**: optimize for correct steady-state operation; avoid heavy persistence complexity for now.

## Phase 0 — Baseline and guardrails (1–2 PRs)
- Add a short `gateway-poc/ARCHITECTURE.md` section (or extend `CODEBASE_REVIEW.md`) with:
  - event ordering expectations,
  - time semantics (`received_at` vs `event_time` vs `epoch_ms`),
  - journey lifecycle state machine.
- Add “golden output” tests for `journey::Journey::to_json()` to lock schema.

## Phase 1 — Service lifecycle & production operability (highest ROI)

### 1. End-to-end shutdown
- **Goal**: Ctrl+C results in predictable termination.
- **Changes** (non-breaking on happy path):
  - `mqtt.rs`: accept a shutdown signal/cancellation and exit loop.
  - `rs485.rs`: honor shutdown in the polling loop.
  - `tracker.rs`: exit loop on shutdown (not only channel close).
  - `main.rs`: drop/close event senders on shutdown so consumers unblock.

### 2. Embedded broker as production component
- **Goal**: remove hidden exposure and unknown resource use.
- **Changes**:
  - move broker bind/port to config (default can remain 1883).
  - add explicit log/metric “broker_started” and “broker_failed”.
  - decide failure policy:
    - **Recommended default**: fail-fast if broker is required for operation; otherwise clearly warn and keep running.

### 3. Logging hygiene / rate limiting
- **Goal**: avoid production log storms; keep actionable signal.
- **Changes**:
  - demote per-event tracker logs to `debug` (keep `info` for state transitions).
  - demote high-frequency `cloudplus` logs (especially hex dumps).
  - rate-limit noisy RS485 “no valid frame” hex dumps; replace with periodic summaries + counters.

## Phase 2 — Hot-path performance improvements (still non-breaking)

### 1. Gate HTTP client reuse
- Cache a single `reqwest::Client` per `GateController` instance.

### 2. CloudPlus buffer/memmove improvements
- Replace `Vec<u8> + drain(..consumed)` parsing with a cursor/ring-buffer approach to avoid repeated memmoves under frequent frames.

### 3. Metrics: reduce lock contention
- Replace per-event async mutex updates with either:
  - atomics for counters + bounded histogram, or
  - a bounded channel and a dedicated metrics aggregation task.

## Phase 3 — Structural refactor (improve cohesion and testability)

### 1. Establish module boundaries
- Introduce directories (or module namespaces) aligned to:
  - `domain/` (events, journey model, person state machine),
  - `io/` (mqtt, rs485, cloudplus, egress),
  - `services/` (tracker, stitcher, correlators),
  - `infra/` (config, metrics, broker).
- Keep public entrypoints stable (`main` still wires everything).

### 2. Reduce “god object” Tracker
- Keep `Tracker` as the façade, but split internal responsibilities:
  - person/dwell/authorization state transitions,
  - journey updates,
  - side effects (gate commands + correlation).

### 3. Journey model unification (Optional Breaking)
- **Breaking (Optional)**: remove `types::Journey` in favor of canonical `journey::Journey` (or vice versa).
- Gate this behind replay tests / golden outputs so the behavior impact is measurable.

## “Breaking (Optional)” candidates worth considering later
- Fix “most recent” selection bugs in `door_correlator.rs` and `acc_collector.rs` (behavioral edge cases).
- Implement real timestamp parsing in `mqtt.rs` (changes `event_time` from 0 to real values).
- Make config fields private and validated (requires downstream migration).

## Next decision points
- Define expected peak load: events/sec and heartbeats/sec.
- Define acceptable loss/backpressure semantics for:
  - RS485 door events (currently `try_send` drops on full),
  - MQTT events (QoS 0, plus channel backpressure).

