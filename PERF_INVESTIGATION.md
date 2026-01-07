# Performance Investigation Notes

Tracking potential optimizations to benchmark/investigate.

---

## Build Configuration

Source: https://nnethercote.github.io/perf-book/build-configuration.html

| Item | Current | To Try | Notes |
|------|---------|--------|-------|
| `lto` | `true` (thin) | `"fat"` | More aggressive cross-crate optimization, worth benchmarking |
| `panic` | unwind | `"abort"` | Skip unwinding overhead, good for embedded/RPi5 |
| `target-cpu` | generic | `cortex-a76` | RPi5-specific optimizations via `.cargo/config.toml` |

### Target CPU Config

Add to `.cargo/config.toml`:
```toml
[target.aarch64-unknown-linux-gnu]
rustflags = ["-C", "target-cpu=cortex-a76"]
```

---

## Inlining

Source: https://nnethercote.github.io/perf-book/inlining.html

**Current state:** Already using `#[inline]` on metrics functions (`src/infra/metrics.rs`) - good.

| Attribute | When to use |
|-----------|-------------|
| `#[inline]` | Small functions, especially cross-crate |
| `#[inline(always)]` | Critical hot-path, single call site |
| `#[inline(never)]` | Large functions called from hot path |
| `#[cold]` | Rarely-executed paths (errors) - helps hot-path code gen |

**Potential opportunities:**
- Add `#[cold]` to error handling paths in tracker/gate code
- Add `#[inline]` to `epoch_ms()`, `Person::new()`, `JourneyOutcome::as_str()`

---

## Hashing

Source: https://nnethercote.github.io/perf-book/hashing.html

**Current state:** All HashMaps use default SipHash (cryptographically secure but slow for integer keys).

**Action:** Add `rustc-hash` crate and replace integer-keyed HashMaps:
```toml
[dependencies]
rustc-hash = "2"
```

| File | Current | Change To |
|------|---------|-----------|
| `tracker/mod.rs` | `HashMap<i64, Person>` | `FxHashMap<i64, Person>` |
| `journey_manager.rs` | `HashMap<i64, Journey>` | `FxHashMap<i64, Journey>` |
| `journey_manager.rs` | `HashMap<i64, String>` | `FxHashMap<i64, String>` |

**Risk:** Low - drop-in replacement. HashDoS not a concern for internal service.

---

## Heap Allocations

Source: https://nnethercote.github.io/perf-book/heap-allocations.html

### High Priority

1. **JourneyEvent String fields** (`domain/journey.rs:48-54`)
   - `t: String` - event types are from fixed set, use `Cow<'static, str>`
   - `z: Option<String>` - zone names from config, could borrow

2. **Vec without pre-allocation** (`io/mqtt.rs`)
   - `parse_xovis_message` creates `Vec<ParsedEvent>` without capacity
   - Add `Vec::with_capacity(8)` (typical frame has 0-10 events)

3. **Journey events Vec** (`domain/journey.rs`)
   - Typical journey has 5-15 events
   - Add `Vec::with_capacity(16)` in `Journey::new()`

### Medium Priority

4. **SmallVec candidates** - Add `smallvec` crate:
   - `Journey.tids: Vec<i64>` → `SmallVec<[i64; 4]>` (rarely >4)
   - `PosGroup.members` → `SmallVec<[GroupMember; 4]>`
   - `DoorCorrelator.pending_cmds` → `SmallVec<[PendingGateCmd; 2]>`

---

## Type Sizes

Source: https://nnethercote.github.io/perf-book/type-sizes.html

### Measure First
```bash
RUSTFLAGS=-Zprint-type-sizes cargo +nightly build --release 2>&1 | grep -E "(Journey|EventType|Person|ParsedEvent)" | head -20
```

### Potential Optimizations

1. **Journey UUIDs** - `jid: String`, `pid: String` are always 36 chars
   - Consider `Box<str>` or `uuid::Uuid` directly (16 bytes vs 24+36)

2. **EventType enum** - `AccEvent(String)` and `Unknown(String)` create size asymmetry
   - Consider `Box<str>` for string variants

3. **Add size assertions** to prevent regressions:
```rust
const _: () = assert!(std::mem::size_of::<EventType>() <= 32);
const _: () = assert!(std::mem::size_of::<Person>() <= 128);
```

---

## Standard Library Types

Source: https://nnethercote.github.io/perf-book/standard-library-types.html

### swap_remove Opportunities

When order doesn't matter, `swap_remove` is O(1) vs `remove` O(n):

| File | Line | Current | Action |
|------|------|---------|--------|
| `door_correlator.rs` | 108 | `.remove(idx)` | Use `swap_remove` |
| `stitcher.rs` | 163 | `.remove(idx)` | Use `swap_remove` |
| `journey_manager.rs` | 80 | `.remove(idx)` | Use `swap_remove` |
| `acc_collector.rs` | 340, 427 | `.remove(idx)` | Use `swap_remove` |

---

## Iterators

Source: https://nnethercote.github.io/perf-book/iterators.html

1. **Double allocation in handlers.rs:702-720**
   - `get_pending_info()` collects, then `into_iter().map().collect()` again
   - Have `get_pending_info()` return iterator instead

2. **Use `.copied()` for i64 iterations**
   - `for tid in pending.journey.tids.iter().copied()` instead of `&`

---

## General Tips / Hot Path Analysis

Sources:
- https://nnethercote.github.io/perf-book/general-tips.html
- https://gist.github.com/jFransham/369a86eff00e5f280ed25121454acec1
- https://leapcell.io/blog/10-rust-performance-tips

### High Priority - Linear Searches in Hot Paths

1. **Stitcher.find_match_with_zone()** (`stitcher.rs:100-179`)
   - Linear scan through all pending tracks on every TrackCreate
   - Consider spatial indexing if pending pool grows large

2. **JourneyManager.stitch_journey()** (`journey_manager.rs:73-86`)
   - Linear scan through `pending_egress` by track_id
   - Add `HashMap<i64, usize>` index for O(1) lookup

3. **Metrics.zone_index()** (`metrics.rs:221-224`)
   - Mutex lock on every `pos_zone_enter()`/`pos_zone_exit()`
   - Pre-compute zone-to-index mapping at init, use fixed array

### Medium Priority

4. **Bucket index functions** (`metrics.rs:29-57`)
   - Linear search through 10 buckets
   - Use binary search or compute mathematically (bounds are exponential)

5. **epoch_ms() called repeatedly**
   - `SystemTime::now()` may involve syscalls
   - Cache at start of `process_event()`, pass through handlers

---

## Wrapper Types

Source: https://nnethercote.github.io/perf-book/wrapper-types.html

**Low priority:** `CloudPlusClient` has separate `Arc<RwLock<>>` for related fields (`heartbeats_rx`/`heartbeats_ack`, `connected`/`request_mode`). Could consolidate, but not in hot path.

---

## CLI Improvements

Source: https://docs.rs/clap/latest/clap/#example

**Current state:** Manual arg parsing with `std::env::args().collect()`, no `--help` or `--version`.

**Already have** `clap = { version = "4", features = ["derive"] }` in Cargo.toml but not using it.

**Improvement:** Add proper CLI with derive macro:
```rust
use clap::Parser;

#[derive(Parser)]
#[command(version, about = "Gateway PoC - gate control system")]
struct Args {
    /// Path to configuration file
    #[arg(short, long, default_value = "config.toml")]
    config: String,
}

fn main() {
    let args = Args::parse();
    let config = Config::load_from_path(&args.config);
    // ...
}
```

**Benefits:** `--help`, `--version`, `-c/--config` flags, better error messages, shell completions.

---

## Code Quality Issues (Common Rust Mistakes)

Sources:
- https://users.rust-lang.org/t/common-newbie-mistakes-or-bad-practices/64821
- https://dev.to/francescoxx/3-common-mistakes-beginners-make-when-learning-rust-4kic

### Issues Found

| Issue | File | Severity | Notes |
|-------|------|----------|-------|
| Manual `unsafe impl Send + Sync` | `metrics.rs:641-642` | Medium | Should derive automatically if fields are Send+Sync. Investigate why needed. |
| `.unwrap()` on Mutex locks | `metrics.rs:215,222,248,491` | Medium | Will panic if lock is poisoned. Use `parking_lot::Mutex` (no poisoning) or `.lock().unwrap_or_else(\|e\| e.into_inner())` |
| `.unwrap()` on HTTP builders | `prometheus.rs:292,298,304` | Low | Unlikely to fail but could panic |
| `Option<&String>` return | `acc_collector.rs:431` | Low | Should return `Option<&str>` |
| `from_str` as inherent method | `types.rs:136-147` | Low | Should implement `std::str::FromStr` trait |
| Fragile `.is_none()` + `.unwrap()` | `bin/tui.rs:540,549,552` | Low | Use `if let Some(val) = x` instead |

### Recommended Fix: Mutex Poisoning

Replace `std::sync::Mutex` with `parking_lot::Mutex` (no poisoning, also faster):

```toml
[dependencies]
parking_lot = "0.12"
```

```rust
// Before
use std::sync::Mutex;
let zones = self.pos_zone_ids.lock().unwrap();

// After
use parking_lot::Mutex;
let zones = self.pos_zone_ids.lock();  // No unwrap needed
```

### Not Issues (Well Done)

- ✓ Proper use of `Path` instead of strings for paths
- ✓ Proper use of enums instead of stringly-typed APIs
- ✓ Excellent use of `?` operator (44 places)
- ✓ Proper `Option<T>` usage (no sentinel values)
- ✓ No `Rc<RefCell<T>>` anti-pattern (uses `Arc` correctly)
- ✓ Clean borrow patterns (no borrows held across await points)
- ✓ No ownership/borrowing confusion

---

## Effective Rust Findings

Sources: https://effective-rust.com/

### Memory Leak: Box::leak

**File:** `bin/gate_test.rs:415`
```rust
results.push((Box::leak(name.into_boxed_str()), r));
```
**Issue:** Deliberately leaks memory to get `&'static str`. Use owned `String` instead.

### Deadlock Risk: Multiple Lock Acquisition

**File:** `io/cloudplus.rs`

| Location | Issue |
|----------|-------|
| Lines 295-298 | Four locks acquired in sequence (read_half, write_half, connected, request_mode) |
| Lines 369-371 | Different ordering than connect (connected first) |
| Lines 487-488 | Two Mutex guards held for entire write_loop duration |

**Recommendation:** Consolidate into single `Arc<Mutex<CloudPlusState>>` struct.

### Missing Copy Trait

**File:** `domain/journey.rs:21-28`

`JourneyOutcome` is a small fieldless enum but doesn't implement `Copy`:
```rust
pub enum JourneyOutcome {
    Completed,
    Abandoned,
    LostWithAcc,
}
```
Add `#[derive(..., Copy)]` to eliminate `.clone()` calls.

### Clippy: 33 Warnings Found

Run `cargo clippy` to see all. Key issues:

| Warning | Location | Fix |
|---------|----------|-----|
| `declare_interior_mutable_const` | `metrics.rs:172` | Change `const ZERO: AtomicU64` to `static` |
| `too_many_arguments` | `cloudplus.rs:378` | `read_loop` has 8 args (max 7) |
| `unnecessary_get_then_check` | `acc_collector.rs:321` | Use `!contains_key()` |
| `if_same_then_else` | `stitcher.rs:153` | Duplicate if/else blocks |
| `unnecessary_cast` | `handlers.rs:41,69` | Casting `u64` to `u64` |

### Missing Tooling Configuration

| File | Purpose | Status |
|------|---------|--------|
| `rust-toolchain.toml` | Pin Rust version | **Missing** |
| `rustfmt.toml` | Formatter config | **Missing** |
| `clippy.toml` | Lint config | **Missing** |
| `.github/workflows/` | CI pipeline | **Missing** |

**Recommended `rust-toolchain.toml`:**
```toml
[toolchain]
channel = "stable"
```

**Recommended CI checks:**
```yaml
- cargo fmt --check
- cargo clippy -- -D warnings
- cargo test
```

### Testing Gaps

| Gap | Recommendation |
|-----|----------------|
| No `tests/` directory | Add integration tests for end-to-end flows |
| No doc tests | Add `///` examples to prevent API drift |
| No `#[should_panic]` tests | Add tests for invalid input handling |

### Type System Improvements

Source: https://effective-rust.com/use-types.html, use-types-2.html, newtype.html

**Newtype Pattern Opportunities (HIGH VALUE):**

| Type | Current | Proposed | Priority | Locations |
|------|---------|----------|----------|-----------|
| Track ID | `i64` | `TrackId(i64)` | **HIGH** | 30+ locations |
| Geometry ID | `i32` | `GeometryId(i32)` | Medium | 5+ locations |
| Dwell time | `u64` | `DwellMs(u64)` | Medium | 15+ locations |
| Epoch time | `u64` | `EpochMs(u64)` | Medium | 10+ locations |

Example implementation:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct TrackId(pub i64);
```

**Benefits:** Prevents accidentally passing geometry_id where track_id expected.

**String Field Should Be Enum:**

`JourneyEvent.t: String` (`domain/journey.rs:48`) stores event types as strings. Should be a typed enum to make invalid event types impossible at compile time.

**Missing Trait Implementations:**

| Item | Location | Issue |
|------|----------|-------|
| `EventType::from_str` | `types.rs:136-147` | Should implement `std::str::FromStr` trait |
| `Egress` | `io/egress.rs` | No trait abstraction - hard to test/mock |
| `GateController` | `services/gate.rs` | Uses `#[cfg(test)]` instead of trait for mocking |

### Error Handling

Source: https://effective-rust.com/errors.html

**Current:** Uses `Box<dyn Error>` (acceptable for application code).

**Missing:** No `thiserror` or `anyhow` crate.

**Opportunity:** Add `anyhow` for better error context:
```toml
[dependencies]
anyhow = "1"
```

Then replace `Result<_, String>` in `config.rs:361` with `Result<_, anyhow::Error>`.

### Type Casting

Source: https://effective-rust.com/casts.html

**75+ `as` casts found.** Most are safe (duration to u64), but:

| Risk | Location | Issue |
|------|----------|-------|
| Medium | `types.rs:78` | `value as u64` converts i64→u64, negative values wrap |
| Low | Protocol parsing | Byte manipulation in cloudplus.rs is safe (widening) |

**Missing:** No `TryFrom`/`TryInto` usage. Consider for user-facing data parsing.

---

## NOT RELEVANT (Reviewed, No Action Needed)

| Topic | Why Not Relevant |
|-------|------------------|
| Bounds Checks | Already using iterators, no tight numeric loops |
| Borrows/Lifetimes | Clean patterns, owned data in structs, no lifetime issues |
| Reflection | Uses enums + pattern matching, no runtime type inspection |
| Macros | No custom macros needed, derive macros used appropriately |
| Bindgen/FFI | Pure Rust project, no C interop |
| Transforms | Already uses Option/Result combinators idiomatically |
| References | Correct Arc/Mutex usage, proper &self vs &mut self |
| Iterators | Good patterns, uses filter/map/collect appropriately |
| Builders | JourneyEvent already uses builder pattern |
| I/O Buffering | Already using BufReader for TCP, async libs handle rest |
| Logging/Debugging | Tracing is already lazy, no expensive log computations |
| Machine Code/SIMD | I/O-bound app, not CPU-bound |
| Parallelism (Rayon) | Already using Tokio async appropriately |
| Compile Times | ~23s is fast enough, dominated by dependencies |

---

## Profiling Before Optimizing

Run these before implementing changes:

```bash
# CPU profiling
cargo flamegraph --bin gateway-poc -- --config config/dev.toml

# Heap allocation profiling
cargo run --features dhat-heap --bin gateway-poc

# Type sizes
RUSTFLAGS=-Zprint-type-sizes cargo +nightly build --release
```

---

## To Benchmark Checklist

- [ ] `lto = "fat"` vs `lto = true`
- [ ] `panic = "abort"` - binary size reduction
- [ ] `target-cpu=cortex-a76` - latency on RPi5
- [ ] `FxHashMap` for integer-keyed maps
- [ ] `#[cold]` on error paths
- [ ] `swap_remove` replacements
- [ ] `SmallVec` for small collections
- [ ] Pre-allocated Vec capacities
