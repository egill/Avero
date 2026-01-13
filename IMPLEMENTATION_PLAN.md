# Implementation Plan: ACC Monitor in Command

## Overview
Add ACC (payment terminal) monitoring to the command app. This creates an ACC entity GenServer to track payment terminal state, patterns, and health alongside the existing Person and Gate entities.

## Current State
- ACC events ARE received via MQTT on `gateway/acc` topic
- Events are normalized and stored in TimescaleDB (event_router.ex lines 301-344)
- Payment events broadcast to PubSub for zone visualization
- **DONE**: ACC entity, registry, supervisor, and event routing implemented

## Implementation Tasks

### Phase 1: Core Infrastructure - DONE
- [x] Create ACC entity GenServer (`command/lib/avero_command/entities/acc.ex`)
  - Tracks received payments, matched/unmatched counts
  - Tracks POS zone associations
  - Tracks terminal health (time since last event)
  - 30-minute inactivity timeout (like Gate)

- [x] Create ACC registry (`command/lib/avero_command/entities/acc_registry.ex`)
  - Key: {site, pos_zone} - one ACC entity per POS zone
  - Methods: get_or_create/2, get/2, list_all/0

- [x] Add ACC supervisor to application.ex
  - DynamicSupervisor named AccSupervisor

### Phase 2: Event Routing - DONE
- [x] Update EventRouter to route ACC events to ACC entity
  - Routes acc.received, acc.matched, acc.unmatched to ACC GenServer
  - Extracts POS zone from event for entity lookup

### Phase 3: Dashboard Integration - DONE
- [x] Add ACC monitoring LiveView (`command/lib/avero_command_web/live/acc_live.ex`)
  - Dedicated /acc route with navigation link in sidebar
  - Show active POS zones with health status (Active/Stale/Offline)
  - Display received/matched/unmatched counts per zone
  - Show match rate percentage
  - Show time since last payment per zone
  - Summary statistics across all zones
  - Auto-refresh every 5 seconds
  - Dark mode support

### Phase 4: Quality Gates - DONE
- [x] Verify build (`mix compile --warnings-as-errors`)
- [x] Rust tests pass (154 tests)

## Learnings
- Elixir underscore prefix on function names does NOT suppress unused warnings (unlike variables)
- Dashboard currently redirects to Grafana iframe, so many components are unused (removed dead code)
- UIComponents imports `empty_state/1` which conflicts with local functions - rename local versions to avoid collision
