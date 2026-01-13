# Implementation Plan

## Current Status: COMPLETE

All POS per-zone dwell tracking user stories have been implemented and tested.

### Version: v0.0.2

Tagged: 2026-01-13

## Completed User Stories

| ID | Title | Status |
|----|-------|--------|
| POS-001 | Create PosOccupancyState module | Complete |
| POS-002 | Add pos_tracking config section | Complete |
| POS-003 | Remove group track filtering | Complete |
| POS-004 | Integrate PosOccupancyState with tracker | Complete |
| POS-005 | Simplify AccCollector | Complete |
| POS-006 | Simplify Person struct | Complete |
| POS-007 | Verify journey logging captures POS events | Complete |
| POS-008 | Unit tests for PosOccupancyState | Complete |
| POS-009 | Update tracker tests for per-zone semantics | Complete |
| POS-010 | Add group track tests | Complete |

## Quality Gates

- Tests: 145 passing
- Clippy: Clean (no warnings)
- Format: Clean

## Key Learnings

### Per-Zone Dwell Semantics
Dwell time is now tracked per-zone rather than globally across all POS zones. A customer must spend `min_dwell_ms` at the specific zone where the ACC event arrives.

### Group Track Handling
Group tracks (track_id with 0x80000000 bit set) are no longer filtered. They flow through all handlers and can accumulate dwell and trigger gate opens.

### PosOccupancyState
Central state machine for POS zone occupancy. Entry/exit timestamps tracked per track per zone. Dwell accumulates on exit. Grace window allows re-entry reopening.

## No Outstanding Issues
