# Task: Unit Tests for Overlap Detection

## Summary
Add unit tests for the new overlap-based group detection logic.

## Files
- `src/services/acc_collector.rs` (add tests module)

## Test Cases

### Overlap Detection
```rust
#[test]
fn test_overlap_same_time() {
    // A: 0-10s, B: 0-10s -> overlap
}

#[test]
fn test_overlap_partial() {
    // A: 0-10s, B: 5-15s -> overlap
}

#[test]
fn test_no_overlap_sequential() {
    // A: 0-5s, B: 6-10s -> no overlap
}

#[test]
fn test_overlap_one_still_present() {
    // A: 0-?, B: 5-10s -> overlap (A still present)
}
```

### Group Size Cap
```rust
#[test]
fn test_group_cap_at_4() {
    // 6 people at POS, only top 4 by dwell authorized
}

#[test]
fn test_group_cap_priority_by_dwell() {
    // Verify longest dwell selected first
}
```

### ACC Buffering
```rust
#[test]
fn test_acc_buffered_then_matched() {
    // ACC arrives, no one at POS, person enters 1s later -> match
}

#[test]
fn test_acc_buffer_expires() {
    // ACC arrives, no one at POS, person enters 3s later -> no match
}
```

## Estimated LOC
~100

## Definition of Done
- [ ] Overlap detection tests pass
- [ ] Group cap tests pass
- [ ] ACC buffering tests pass
- [ ] `cargo test` all green
