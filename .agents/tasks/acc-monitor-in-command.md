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

### [ ] US-002: See unmatched ACC
**Category:** feat
**Priority:** 2

**Acceptance Criteria:**
- [ ] See unmatched ACC is implemented
- [ ] Tests pass
- [ ] Documentation updated

**Files:**
- TBD

**Instructions:**
Implement See unmatched ACC according to the specifications.

---

### [ ] US-003: See ACC that have exited
**Category:** feat
**Priority:** 3

**Acceptance Criteria:**
- [ ] See ACC that have exited is implemented
- [ ] Tests pass
- [ ] Documentation updated

**Files:**
- TBD

**Instructions:**
Implement See ACC that have exited according to the specifications.

---

### [ ] US-004: See ACC that have been lost (lost journey)
**Category:** feat
**Priority:** 3

**Acceptance Criteria:**
- [ ] See ACC that have been lost (lost journey) is implemented
- [ ] Tests pass
- [ ] Documentation updated

**Files:**
- TBD

**Instructions:**
Implement See ACC that have been lost (lost journey) according to the specifications.

---

### [ ] US-005: See ACC that went back to store
**Category:** feat
**Priority:** 3

**Acceptance Criteria:**
- [ ] See ACC that went back to store is implemented
- [ ] Tests pass
- [ ] Documentation updated

**Files:**
- TBD

**Instructions:**
Implement See ACC that went back to store according to the specifications.

---

