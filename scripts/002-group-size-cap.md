# Task: Group Size Cap

## Summary
Limit maximum group size to 4 people per payment.

## Problem
One payment could authorize unlimited people if many are at POS.

## Solution
Cap at 4 people, prioritize by longest dwell time.

## Files
- `src/services/acc_collector.rs`

## Implementation

```rust
const MAX_GROUP_SIZE: usize = 4;

fn get_qualified_members(&self) -> Vec<i64> {
    let mut qualified: Vec<_> = self.members
        .iter()
        .filter(|m| m.dwell_ms() >= self.min_dwell_for_acc)
        .collect();

    // Sort by dwell descending, take top 4
    qualified.sort_by(|a, b| b.dwell_ms().cmp(&a.dwell_ms()));
    qualified.truncate(MAX_GROUP_SIZE);
    qualified.iter().map(|m| m.track_id).collect()
}
```

## Estimated LOC
~10

## Definition of Done
- [ ] `MAX_GROUP_SIZE = 4` constant added
- [ ] Qualified members sorted by dwell time (descending)
- [ ] Only top 4 members returned for authorization
- [ ] Log includes which members were capped out (if any)
