# Task: Increase Exit Grace Period

## Summary
Increase exit grace period from 1.5s to 3s.

## Problem
Payment terminal may have delay; 1.5s is too short to match people who just left POS.

## Solution
Increase `MAX_TIME_SINCE_EXIT` to 3000ms.

## Files
- `src/services/acc_collector.rs`

## Implementation

```rust
const MAX_TIME_SINCE_EXIT: u64 = 3000;  // Was 1500
```

## Estimated LOC
~1

## Definition of Done
- [ ] `MAX_TIME_SINCE_EXIT` changed from 1500 to 3000
- [ ] Recent exits matched within 3 second window
