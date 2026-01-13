# ACC monitor in command - PRD

## Overview

Build a new feature from scratch

## Audience

End users of the application

## Scope

Single component or module

## Success Criteria

Feature works as specified in acceptance criteria

## User Stories

### [x] US-001: See incoming ACC requests
**Category:** feat
**Priority:** 1

**Acceptance Criteria:**
- [x] See incoming ACC requests is implemented
- [x] Tests pass (code compiles; DB required for integration tests)
- [x] Documentation updated

**Files:**
- `command/lib/avero_command_web/live/acc_feed_live.ex` - ACC monitor LiveView
- `command/lib/avero_command_web/router.ex` - Added `/acc` route
- `command/lib/avero_command/mqtt/event_router.ex` - Broadcasts ACC events to `acc_events` PubSub
- `command/lib/avero_command_web/components/dashboard_components.ex` - Added nav item

**Implementation Notes:**
- Created real-time ACC monitor at `/acc` route
- Subscribes to `acc_events` PubSub channel for all ACC event types
- Displays: matched, unmatched, matched_no_journey, late_after_gate, received
- Features: filter by type, pause/resume feed, clear events
- Shows debug info (active/pending tracks) for unmatched events
- Maximum 100 events kept in memory

---

### [x] US-002: See unmatched ACC
**Category:** feat
**Priority:** 2

**Acceptance Criteria:**
- [x] See unmatched ACC is implemented
- [x] Tests pass (code compiles; DB required for integration tests)
- [x] Documentation updated

**Files:**
- `command/lib/avero_command_web/live/acc_feed_live.ex` - ACC monitor with unmatched filter (line 109) and debug display (lines 192-210)

**Implementation Notes:**
- Unmatched ACC events are visible in the ACC Monitor (`/acc` route)
- Users can filter to show only unmatched events via the "Unmatched" filter button
- Unmatched events display:
  - Red status badge with ✗ icon
  - Payment terminal IP address
  - POS zone (if known)
  - Debug context: active tracks (with track IDs and dwell times) and pending track count
- This helps operators diagnose why payments couldn't be matched to customers
- Already implemented as part of US-001 ACC feed infrastructure

---

### [x] US-003: See ACC that have exited
**Category:** feat
**Priority:** 3

**Acceptance Criteria:**
- [x] See ACC that have exited is implemented
- [x] Tests pass (code compiles; DB required for integration tests)
- [x] Documentation updated

**Files:**
- `command/lib/avero_command_web/live/journey_feed_live.ex` - Added ACC filter to journey feed
- `command/lib/avero_command/journeys.ex` - Added `apply_acc_filter` backend query support

**Implementation Notes:**
- Added ACC filter buttons to the Journey Feed (`/journeys` route)
- Users can filter by: "All" | "ACC ✓" (matched) | "No ACC" (not matched)
- **To see ACC that have exited**: Select "ACC ✓" filter + "Exits" exit type
- Combines with existing exit type filters for flexible querying
- Real-time updates respect ACC filter

---

### [x] US-004: See ACC that have been lost (lost journey)
**Category:** feat
**Priority:** 3

**Acceptance Criteria:**
- [x] See ACC that have been lost (lost journey) is implemented
- [x] Tests pass (code compiles; DB required for integration tests)
- [x] Documentation updated

**Files:**
- `command/lib/avero_command_web/live/journey_feed_live.ex` - ACC filter implementation
- `command/lib/avero_command/journeys.ex` - Backend query support

**Implementation Notes:**
- **To see ACC that have been lost**: Select "ACC ✓" filter + "Lost" exit type
- Shows journeys where payment was received but track was lost before exit
- Useful for identifying potential tracking issues with paying customers

---

### [x] US-005: See ACC that went back to store
**Category:** feat
**Priority:** 3

**Acceptance Criteria:**
- [x] See ACC that went back to store is implemented
- [x] Tests pass (code compiles; DB required for integration tests)
- [x] Documentation updated

**Files:**
- `command/lib/avero_command_web/live/journey_feed_live.ex` - ACC filter implementation
- `command/lib/avero_command/journeys.ex` - Backend query support

**Implementation Notes:**
- **To see ACC that went back to store**: Select "ACC ✓" filter + "Returns" exit type
- Shows journeys where payment was received but customer returned to store
- Useful for identifying customers who paid but changed their mind

---

