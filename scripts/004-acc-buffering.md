# Task: ACC Buffering for Late Arrivals

## Summary
Buffer unmatched ACC events for 2 seconds to handle late POS entry.

## Problem
ACC arrives but person hasn't entered POS zone yet (walking towards it).
Currently: ACC is logged as unmatched and discarded.

## Solution
Buffer the ACC event. When someone enters POS within 2s, trigger matching.

## Files
- `src/services/acc_collector.rs`
- `src/services/tracker/handlers.rs` (minor)

## Implementation

```rust
struct PendingAcc {
    pos_zone: String,
    received_at: Instant,
}

// In AccCollector:
pending_acc: HashMap<String, PendingAcc>,

fn buffer_acc(&mut self, pos_zone: &str) {
    self.pending_acc.insert(pos_zone.to_string(), PendingAcc {
        pos_zone: pos_zone.to_string(),
        received_at: Instant::now(),
    });
}

fn check_pending_acc(&mut self, pos_zone: &str) -> Option<PendingAcc> {
    if let Some(pending) = self.pending_acc.get(pos_zone) {
        if pending.received_at.elapsed().as_millis() < 2000 {
            return self.pending_acc.remove(pos_zone);
        }
    }
    None
}

fn cleanup_expired_acc(&mut self) {
    self.pending_acc.retain(|_, p| p.received_at.elapsed().as_millis() < 2000);
}
```

In `record_pos_entry()`: check for pending ACC and trigger matching if found.

## Estimated LOC
~40

## Definition of Done
- [ ] `PendingAcc` struct and `pending_acc` HashMap added
- [ ] Unmatched ACC buffered instead of discarded
- [ ] POS entry checks for pending ACC within 2s window
- [ ] Expired pending ACC cleaned up (no memory leak)
- [ ] Debug log when buffered ACC is matched
