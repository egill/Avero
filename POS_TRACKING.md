# POS Zone Occupancy Tracking

## Purpose

Track who is at each POS zone at any moment, enabling:
- ACC matching (who paid?)
- Late auth (who just left?)
- Dwell qualification (did they stay long enough?)

## State Model

Use `moka` TTL cache for automatic expiration - no manual pruning needed:

```rust
use moka::sync::Cache;

struct PosOccupant {
    is_present: bool,           // in zone right now?
    accumulated_dwell_ms: u64,  // total time spent at this POS
    entry_time: Option<Instant>, // current visit start (if present)
}

// Per POS zone - moka handles TTL expiration automatically
pos_occupants: HashMap<ZoneId, Cache<TrackId, PosOccupant>>

// Initialize each zone's cache
fn create_zone_cache() -> Cache<TrackId, PosOccupant> {
    Cache::builder()
        .time_to_live(Duration::from_secs(5))  // grace window TTL
        .max_capacity(100)
        .build()
}
```

**Why moka:**
- Entries auto-expire after TTL (no pruning code needed)
- Insert/update refreshes the TTL automatically
- Battle-tested (~30M downloads)
- Lightweight, no background threads in sync mode

## Event Handling

### ZONE_ENTRY

```rust
fn on_zone_entry(zone_id: ZoneId, track_id: TrackId) {
    let occupant = pos_occupants[zone_id].get_or_insert(track_id, default);
    occupant.is_present = true;
    occupant.entry_time = Some(now);
    occupant.last_activity = now;
}
```

### ZONE_EXIT

```rust
fn on_zone_exit(zone_id: ZoneId, track_id: TrackId) {
    if let Some(occupant) = pos_occupants[zone_id].get_mut(track_id) {
        // Accumulate dwell from this visit
        if let Some(entry) = occupant.entry_time.take() {
            occupant.accumulated_dwell_ms += now.duration_since(entry).as_millis();
        }
        occupant.is_present = false;
        occupant.last_activity = now;
        // Entry stays in cache with TTL for late auth
    }
}
```

## Occupant Lifecycle

```
┌─────────────────────────────────────────────────────────────────┐
│                        POS Zone State                           │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ZONE_ENTRY          ZONE_EXIT              TTL expires         │
│      │                   │                      │               │
│      ▼                   ▼                      ▼               │
│  ┌────────┐         ┌────────┐            ┌─────────┐          │
│  │present │ ──────► │ grace  │ ─────────► │ removed │          │
│  │        │  exit   │ window │  TTL       │         │          │
│  └────────┘         └────────┘            └─────────┘          │
│      │                   │                                      │
│      │    re-entry       │                                      │
│      ◄───────────────────┘                                      │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

## Include Both Person and Group Tracks

Xovis emits two types of tracks:
- **Person tracks**: Individual people (track_id < 2^31)
- **Group tracks**: Aggregated when people are close (track_id >= 2^31)

**Track both.** Group tracks often linger slightly longer and help catch edge cases where person track exits just outside grace window.

## Accumulated Dwell

Dwell accumulates across multiple visits to the same POS:

```
Track 29718 at POS_1:
  Visit 1: enter → 3s → exit (brief look)
  Visit 2: enter → 8s → exit (actual shopping)

  accumulated_dwell = 11s ✓ qualifies (>= 7s)
```

This handles customers who briefly step away then return.

## Querying Occupancy

### Current + Recent (for ACC matching)

```rust
fn get_candidates(zone_id: ZoneId, min_dwell_ms: u64, grace_ms: u64) -> Vec<TrackId> {
    let now = Instant::now();

    pos_occupants[zone_id]
        .iter()
        .filter(|(_, o)| {
            // Present OR recently exited
            let is_active = o.is_present ||
                now.duration_since(o.last_activity).as_millis() < grace_ms;

            // Has sufficient dwell
            let qualified = o.accumulated_dwell_ms >= min_dwell_ms;

            is_active && qualified
        })
        .map(|(track_id, _)| track_id)
        .collect()
}
```

### Current Only (for real-time display)

```rust
fn get_present(zone_id: ZoneId) -> Vec<TrackId> {
    pos_occupants[zone_id]
        .iter()
        .filter(|(_, o)| o.is_present)
        .map(|(track_id, _)| track_id)
        .collect()
}
```

## TTL and Pruning

Use `moka` cache with TTL for automatic cleanup:

```rust
let cache: Cache<TrackId, PosOccupant> = Cache::builder()
    .time_to_live(Duration::from_secs(5))  // grace window
    .max_capacity(100)                      // hard cap per zone
    .build();
```

**Refresh TTL on activity:**
- ZONE_ENTRY → insert/update (refreshes TTL)
- ZONE_EXIT → update (refreshes TTL, starts grace countdown)
- No activity → auto-expires after TTL

## Example Timeline

```
Time     Event                    POS_1 State
─────────────────────────────────────────────────────────────────
00:00    ENTRY track=100          {100: present, dwell=0}
00:03    EXIT track=100           {100: grace, dwell=3000ms}
00:04    ENTRY track=100          {100: present, dwell=3000ms}
00:12    EXIT track=100           {100: grace, dwell=11000ms}
00:14    ACC arrives              → track 100 found (in grace, dwell=11s ✓)
00:17    TTL expires              {100: removed}
```

## Configuration

| Parameter | Default | Purpose |
|-----------|---------|---------|
| `grace_ms` | 3000 | Time after exit to keep occupant visible |
| `min_dwell_ms` | 7000 | Minimum dwell to qualify for ACC match |
| `max_per_zone` | 100 | Hard cap on occupants per zone |

## Relationship to ACC Matching

POS tracking provides the **foundation** for ACC matching:

```
ACC arrives for POS_1
        │
        ▼
┌───────────────────┐
│ Query POS_1 state │
│ (present + grace) │
└─────────┬─────────┘
          │
          ▼
┌───────────────────┐
│ Filter by dwell   │
│ (>= 7000ms)       │
└─────────┬─────────┘
          │
          ▼
┌───────────────────┐
│ Return candidates │
│ (track IDs)       │
└───────────────────┘
```

The better the POS tracking, the more accurate the ACC matching.
