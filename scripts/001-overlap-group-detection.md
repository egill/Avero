# Task: Overlap-Based Group Detection

## Summary
Replace 10-second entry time window with overlap-based grouping logic.

## Problem
Current: People entering POS within 10 seconds are grouped together.
Issue: Two unrelated shoppers entering 9 seconds apart get incorrectly grouped.

## Solution
Group people whose **time at POS overlapped** (both present simultaneously at any point).

## Files
- `src/services/acc_collector.rs`

## Implementation

```rust
struct GroupMember {
    track_id: i64,
    entered_at: Instant,
    exited_at: Option<Instant>,  // NEW: track when they left
}

impl GroupMember {
    fn overlapped_with(&self, other: &GroupMember) -> bool {
        let self_end = self.exited_at.unwrap_or(Instant::now());
        let other_end = other.exited_at.unwrap_or(Instant::now());
        // Overlap: A.start < B.end AND B.start < A.end
        self.entered_at < other_end && other.entered_at < self_end
    }
}
```

Remove `GROUP_WINDOW_MS` constant and `should_join()` time-based check.

## Estimated LOC
~30

## Definition of Done
- [ ] `GroupMember` tracks `exited_at` timestamp
- [ ] `overlapped_with()` method correctly detects time overlap
- [ ] New entries only join group if they overlap with existing members
- [ ] Existing `acc_pos_entry_state` debug logging still works
