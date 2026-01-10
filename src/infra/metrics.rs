//! Lock-free metrics collection and periodic reporting
//!
//! Uses atomics for hot-path operations to avoid mutex contention.
//! All counter updates are lock-free; reporting is the only operation
//! that needs synchronization (via atomic swap).
//!
//! NOTE: All atomics use Relaxed ordering intentionally—these are statistical
//! counters only. Do NOT use these atomics for coordination or logic decisions.

use rustc_hash::FxHashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tracing::info;

/// Prometheus-style exponential bucket boundaries (microseconds)
/// Buckets: ≤100, ≤200, ≤400, ≤800, ≤1600, ≤3200, ≤6400, ≤12800, ≤25600, ≤51200, >51200
const BUCKET_BOUNDS: [u64; 10] = [100, 200, 400, 800, 1600, 3200, 6400, 12800, 25600, 51200];
const NUM_BUCKETS: usize = 11;

/// Stitch distance bucket boundaries (centimeters)
/// Buckets: ≤10, ≤20, ≤40, ≤80, ≤160, ≤320, ≤640, ≤1280, ≤2560, ≤5120, >5120 cm
const STITCH_DIST_BOUNDS: [u64; 10] = [10, 20, 40, 80, 160, 320, 640, 1280, 2560, 5120];

/// Compute bucket index for a latency value using binary search
#[inline]
fn bucket_index(latency_us: u64) -> usize {
    BUCKET_BOUNDS.partition_point(|&bound| bound < latency_us)
}

/// Compute bucket index for stitch distance (cm) using binary search
#[inline]
fn stitch_dist_bucket_index(dist_cm: u64) -> usize {
    STITCH_DIST_BOUNDS.partition_point(|&bound| bound < dist_cm)
}

/// Update an atomic max value using compare-and-swap loop
#[inline]
fn update_atomic_max(atomic_max: &AtomicU64, new_value: u64) {
    let mut current_max = atomic_max.load(Ordering::Relaxed);
    while new_value > current_max {
        match atomic_max.compare_exchange_weak(
            current_max,
            new_value,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(actual) => current_max = actual,
        }
    }
}

/// Swap all buckets to zero and return their values
#[inline]
fn swap_buckets(buckets: &[AtomicU64; NUM_BUCKETS]) -> [u64; NUM_BUCKETS] {
    let mut result = [0u64; NUM_BUCKETS];
    for (i, bucket) in buckets.iter().enumerate() {
        result[i] = bucket.swap(0, Ordering::Relaxed);
    }
    result
}

/// Load all bucket values without resetting
#[inline]
fn load_buckets(buckets: &[AtomicU64; NUM_BUCKETS]) -> [u64; NUM_BUCKETS] {
    let mut result = [0u64; NUM_BUCKETS];
    for (i, bucket) in buckets.iter().enumerate() {
        result[i] = bucket.load(Ordering::Relaxed);
    }
    result
}

/// Compute percentile from histogram buckets
/// Returns the upper bound of the bucket containing the percentile
fn percentile_from_buckets(buckets: &[u64; NUM_BUCKETS], percentile: f64) -> u64 {
    let total: u64 = buckets.iter().sum();
    if total == 0 {
        return 0;
    }

    let target = (total as f64 * percentile) as u64;
    let mut cumulative = 0u64;

    // Upper bounds for each bucket (last bucket uses 2x the previous bound)
    const BUCKET_UPPER_BOUNDS: [u64; NUM_BUCKETS] =
        [100, 200, 400, 800, 1600, 3200, 6400, 12800, 25600, 51200, 102400];

    for (i, &count) in buckets.iter().enumerate() {
        cumulative += count;
        if cumulative >= target {
            return BUCKET_UPPER_BOUNDS[i];
        }
    }
    BUCKET_UPPER_BOUNDS[NUM_BUCKETS - 1]
}

/// Gate state values for Prometheus gauge
pub const GATE_STATE_CLOSED: u64 = 0;
pub const GATE_STATE_MOVING: u64 = 1;
pub const GATE_STATE_OPEN: u64 = 2;

/// Maximum number of POS zones to track
pub const MAX_POS_ZONES: usize = 10;

/// Lock-free metrics collector
///
/// All recording operations are lock-free using atomics.
/// The `report()` method atomically swaps counters to get a consistent snapshot.
pub struct Metrics {
    /// Total events ever processed (monotonic)
    events_total: AtomicU64,
    /// Events since last report (reset on report)
    events_since_report: AtomicU64,
    /// Sum of latencies in microseconds (reset on report)
    latency_sum_us: AtomicU64,
    /// Max latency in microseconds (reset on report)
    latency_max_us: AtomicU64,
    /// Event processing latency histogram buckets (reset on report)
    latency_buckets: [AtomicU64; NUM_BUCKETS],
    /// Total gate commands sent (monotonic)
    gate_commands_sent: AtomicU64,
    /// Gate command latency histogram buckets (reset on report)
    gate_latency_buckets: [AtomicU64; NUM_BUCKETS],
    /// Sum of gate command latencies (reset on report)
    gate_latency_sum_us: AtomicU64,
    /// Max gate command latency (reset on report)
    gate_latency_max_us: AtomicU64,
    /// Gate commands since last report (reset on report)
    gate_commands_since_report: AtomicU64,
    /// Current gate state (0=closed, 1=moving, 2=open)
    gate_state: AtomicU64,
    /// Total exits through gate (monotonic)
    exits_total: AtomicU64,
    /// ACC events received (monotonic)
    acc_events_total: AtomicU64,
    /// ACC events matched to a track (monotonic)
    acc_matched_total: AtomicU64,
    /// Tracks successfully stitched (monotonic)
    stitch_matched_total: AtomicU64,
    /// Tracks truly lost (expired without stitch) (monotonic)
    stitch_expired_total: AtomicU64,
    /// Stitch distance histogram buckets (centimeters)
    /// Bounds: ≤10, ≤20, ≤40, ≤80, ≤160, ≤320, ≤640, ≤1280, ≤2560, ≤5120, >5120 cm
    stitch_distance_buckets: [AtomicU64; NUM_BUCKETS],
    /// Sum of stitch distances (cm) for average calculation
    stitch_distance_sum: AtomicU64,
    /// Stitch time histogram buckets (milliseconds)
    /// Bounds: ≤100, ≤200, ≤400, ≤800, ≤1600, ≤3200, ≤6400, ≤12800, ≤25600, ≤51200, >51200 ms
    stitch_time_buckets: [AtomicU64; NUM_BUCKETS],
    /// Sum of stitch times (ms) for average calculation
    stitch_time_sum: AtomicU64,
    /// ACC events that arrived late (after person entered gate zone)
    acc_late_total: AtomicU64,
    /// ACC events matched but no journey found
    acc_no_journey_total: AtomicU64,
    /// MQTT events dropped due to channel full (monotonic)
    mqtt_events_dropped: AtomicU64,
    /// ACC events dropped due to channel full (monotonic)
    acc_events_dropped: AtomicU64,
    /// Gate commands dropped due to channel full (monotonic)
    gate_cmds_dropped: AtomicU64,
    /// MQTT events received (monotonic) - before try_send
    mqtt_events_received: AtomicU64,
    /// ACC events received (monotonic) - before try_send
    acc_events_received: AtomicU64,
    /// Journey egress events dropped due to channel full (monotonic)
    journey_egress_dropped: AtomicU64,
    /// Gate command queue delay histogram (time from enqueue to worker pickup)
    /// Same buckets as latency: 100, 200, 400, ... 51200 µs
    gate_queue_delay_buckets: [AtomicU64; NUM_BUCKETS],
    /// Sum of gate queue delays (reset on report)
    gate_queue_delay_sum_us: AtomicU64,
    /// Max gate queue delay (reset on report)
    gate_queue_delay_max_us: AtomicU64,
    /// Current event queue depth (updated by sampler)
    event_queue_depth: AtomicU64,
    /// Current gate command queue depth (updated by sampler)
    gate_queue_depth: AtomicU64,
    /// Current CloudPlus outbound queue depth (updated by sampler)
    cloudplus_queue_depth: AtomicU64,
    /// Gate send latency histogram (time for actual network send)
    gate_send_latency_buckets: [AtomicU64; NUM_BUCKETS],
    /// Sum of gate send latencies (reset on report)
    gate_send_latency_sum_us: AtomicU64,
    /// Max gate send latency (reset on report)
    gate_send_latency_max_us: AtomicU64,
    /// Gate enqueue-to-send histogram (total time from enqueue to send complete)
    gate_enqueue_to_send_buckets: [AtomicU64; NUM_BUCKETS],
    /// Sum of gate enqueue-to-send times (reset on report)
    gate_enqueue_to_send_sum_us: AtomicU64,
    /// Max gate enqueue-to-send time (reset on report)
    gate_enqueue_to_send_max_us: AtomicU64,
    /// POS zone occupancy (number of people in each zone)
    /// Index is determined by order in pos_zones config
    pos_occupancy: [AtomicU64; MAX_POS_ZONES],
    /// Zone IDs for POS zones (set once at init)
    pos_zone_ids: parking_lot::Mutex<Vec<i32>>,
    /// Pre-computed zone ID to index mapping (for O(1) lookup without mutex)
    zone_id_to_index: parking_lot::RwLock<FxHashMap<i32, usize>>,
    /// Last report time (only accessed from reporter, not atomic)
    last_report_time: parking_lot::Mutex<Instant>,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            events_total: AtomicU64::new(0),
            events_since_report: AtomicU64::new(0),
            latency_sum_us: AtomicU64::new(0),
            latency_max_us: AtomicU64::new(0),
            latency_buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            gate_commands_sent: AtomicU64::new(0),
            gate_latency_buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            gate_latency_sum_us: AtomicU64::new(0),
            gate_latency_max_us: AtomicU64::new(0),
            gate_commands_since_report: AtomicU64::new(0),
            gate_state: AtomicU64::new(GATE_STATE_CLOSED),
            exits_total: AtomicU64::new(0),
            acc_events_total: AtomicU64::new(0),
            acc_matched_total: AtomicU64::new(0),
            stitch_matched_total: AtomicU64::new(0),
            stitch_expired_total: AtomicU64::new(0),
            stitch_distance_buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            stitch_distance_sum: AtomicU64::new(0),
            stitch_time_buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            stitch_time_sum: AtomicU64::new(0),
            acc_late_total: AtomicU64::new(0),
            acc_no_journey_total: AtomicU64::new(0),
            mqtt_events_dropped: AtomicU64::new(0),
            acc_events_dropped: AtomicU64::new(0),
            gate_cmds_dropped: AtomicU64::new(0),
            mqtt_events_received: AtomicU64::new(0),
            acc_events_received: AtomicU64::new(0),
            journey_egress_dropped: AtomicU64::new(0),
            gate_queue_delay_buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            gate_queue_delay_sum_us: AtomicU64::new(0),
            gate_queue_delay_max_us: AtomicU64::new(0),
            event_queue_depth: AtomicU64::new(0),
            gate_queue_depth: AtomicU64::new(0),
            cloudplus_queue_depth: AtomicU64::new(0),
            gate_send_latency_buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            gate_send_latency_sum_us: AtomicU64::new(0),
            gate_send_latency_max_us: AtomicU64::new(0),
            gate_enqueue_to_send_buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            gate_enqueue_to_send_sum_us: AtomicU64::new(0),
            gate_enqueue_to_send_max_us: AtomicU64::new(0),
            pos_occupancy: std::array::from_fn(|_| AtomicU64::new(0)),
            pos_zone_ids: parking_lot::Mutex::new(Vec::new()),
            zone_id_to_index: parking_lot::RwLock::new(FxHashMap::default()),
            last_report_time: parking_lot::Mutex::new(Instant::now()),
        }
    }

    /// Set the POS zone IDs (call once at initialization)
    pub fn set_pos_zones(&self, zone_ids: &[i32]) {
        // Update the zone list (for reporting)
        let mut zones = self.pos_zone_ids.lock();
        zones.clear();
        zones.extend(zone_ids.iter().take(MAX_POS_ZONES));

        // Pre-compute the zone ID to index mapping for O(1) lookup
        let mut index_map = self.zone_id_to_index.write();
        index_map.clear();
        for (idx, &zone_id) in zone_ids.iter().take(MAX_POS_ZONES).enumerate() {
            index_map.insert(zone_id, idx);
        }
    }

    /// Get the index for a zone ID, or None if not a POS zone
    /// Uses pre-computed O(1) lookup via FxHashMap (no mutex on hot path)
    #[inline]
    fn zone_index(&self, zone_id: i32) -> Option<usize> {
        let index_map = self.zone_id_to_index.read();
        index_map.get(&zone_id).copied()
    }

    /// Record a person entering a POS zone
    #[inline]
    pub fn pos_zone_enter(&self, zone_id: i32) {
        if let Some(idx) = self.zone_index(zone_id) {
            self.pos_occupancy[idx].fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Record a person exiting a POS zone
    #[inline]
    pub fn pos_zone_exit(&self, zone_id: i32) {
        if let Some(idx) = self.zone_index(zone_id) {
            // Use saturating sub to avoid underflow
            let current = self.pos_occupancy[idx].load(Ordering::Relaxed);
            if current > 0 {
                self.pos_occupancy[idx].fetch_sub(1, Ordering::Relaxed);
            }
        }
    }

    /// Get current POS zone occupancy for all zones
    pub fn pos_occupancy(&self) -> Vec<(i32, u64)> {
        let zones = self.pos_zone_ids.lock();
        zones
            .iter()
            .enumerate()
            .map(|(idx, &zone_id)| {
                let count = self.pos_occupancy[idx].load(Ordering::Relaxed);
                (zone_id, count)
            })
            .collect()
    }

    /// Record an event was processed with given latency (lock-free)
    #[inline]
    pub fn record_event_processed(&self, latency_us: u64) {
        self.events_total.fetch_add(1, Ordering::Relaxed);
        self.events_since_report.fetch_add(1, Ordering::Relaxed);
        self.latency_sum_us.fetch_add(latency_us, Ordering::Relaxed);

        // Update histogram bucket
        let bucket = bucket_index(latency_us);
        self.latency_buckets[bucket].fetch_add(1, Ordering::Relaxed);

        // Update max
        update_atomic_max(&self.latency_max_us, latency_us);
    }

    /// Record a gate command was sent (lock-free)
    #[inline]
    pub fn record_gate_command(&self) {
        self.gate_commands_sent.fetch_add(1, Ordering::Relaxed);
    }

    /// Record gate command end-to-end latency (lock-free)
    ///
    /// This tracks the time from event received to gate command queued.
    #[inline]
    pub fn record_gate_latency(&self, latency_us: u64) {
        self.gate_commands_since_report.fetch_add(1, Ordering::Relaxed);
        self.gate_latency_sum_us.fetch_add(latency_us, Ordering::Relaxed);

        // Update histogram bucket
        let bucket = bucket_index(latency_us);
        self.gate_latency_buckets[bucket].fetch_add(1, Ordering::Relaxed);

        // Update max
        update_atomic_max(&self.gate_latency_max_us, latency_us);
    }

    /// Get total events processed
    #[inline]
    #[allow(dead_code)]
    pub fn events_total(&self) -> u64 {
        self.events_total.load(Ordering::Relaxed)
    }

    /// Set gate state (0=closed, 1=moving, 2=open)
    #[inline]
    pub fn set_gate_state(&self, state: u64) {
        self.gate_state.store(state, Ordering::Relaxed);
    }

    /// Get current gate state
    #[inline]
    #[allow(dead_code)]
    pub fn gate_state(&self) -> u64 {
        self.gate_state.load(Ordering::Relaxed)
    }

    /// Record an exit through the gate (lock-free)
    #[inline]
    pub fn record_exit(&self) {
        self.exits_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Get total exits
    #[inline]
    #[allow(dead_code)]
    pub fn exits_total(&self) -> u64 {
        self.exits_total.load(Ordering::Relaxed)
    }

    /// Record an ACC event received (lock-free)
    #[inline]
    pub fn record_acc_event(&self, matched: bool) {
        self.acc_events_total.fetch_add(1, Ordering::Relaxed);
        if matched {
            self.acc_matched_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Record a successful track stitch (lock-free)
    #[inline]
    pub fn record_stitch_matched(&self) {
        self.stitch_matched_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a track that expired without stitching (truly lost) (lock-free)
    #[inline]
    pub fn record_stitch_expired(&self) {
        self.stitch_expired_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Record stitch distance in centimeters (lock-free)
    #[inline]
    pub fn record_stitch_distance(&self, dist_cm: u64) {
        let bucket = stitch_dist_bucket_index(dist_cm);
        self.stitch_distance_buckets[bucket].fetch_add(1, Ordering::Relaxed);
        self.stitch_distance_sum.fetch_add(dist_cm, Ordering::Relaxed);
    }

    /// Record stitch time in milliseconds (lock-free)
    /// Uses same bucket bounds as latency (100-51200ms)
    #[inline]
    pub fn record_stitch_time(&self, time_ms: u64) {
        let bucket = bucket_index(time_ms);
        self.stitch_time_buckets[bucket].fetch_add(1, Ordering::Relaxed);
        self.stitch_time_sum.fetch_add(time_ms, Ordering::Relaxed);
    }

    /// Record an ACC event that arrived late (after person entered gate zone)
    #[inline]
    pub fn record_acc_late(&self) {
        self.acc_late_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an ACC event that matched but had no journey
    #[inline]
    pub fn record_acc_no_journey(&self) {
        self.acc_no_journey_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an MQTT event dropped due to channel full (lock-free)
    #[inline]
    pub fn record_mqtt_event_dropped(&self) {
        self.mqtt_events_dropped.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an ACC event dropped due to channel full (lock-free)
    #[inline]
    pub fn record_acc_event_dropped(&self) {
        self.acc_events_dropped.fetch_add(1, Ordering::Relaxed);
    }

    /// Get MQTT events dropped total
    #[inline]
    #[allow(dead_code)]
    pub fn mqtt_events_dropped(&self) -> u64 {
        self.mqtt_events_dropped.load(Ordering::Relaxed)
    }

    /// Get ACC events dropped total
    #[inline]
    #[allow(dead_code)]
    pub fn acc_events_dropped(&self) -> u64 {
        self.acc_events_dropped.load(Ordering::Relaxed)
    }

    /// Record a gate command dropped due to channel full (lock-free)
    #[inline]
    pub fn record_gate_cmd_dropped(&self) {
        self.gate_cmds_dropped.fetch_add(1, Ordering::Relaxed);
    }

    /// Get gate commands dropped total
    #[inline]
    #[allow(dead_code)]
    pub fn gate_cmds_dropped(&self) -> u64 {
        self.gate_cmds_dropped.load(Ordering::Relaxed)
    }

    /// Record an MQTT event received (before try_send) (lock-free)
    #[inline]
    pub fn record_mqtt_event_received(&self) {
        self.mqtt_events_received.fetch_add(1, Ordering::Relaxed);
    }

    /// Get MQTT events received total
    #[inline]
    pub fn mqtt_events_received(&self) -> u64 {
        self.mqtt_events_received.load(Ordering::Relaxed)
    }

    /// Record an ACC event received (before try_send) (lock-free)
    #[inline]
    pub fn record_acc_event_received(&self) {
        self.acc_events_received.fetch_add(1, Ordering::Relaxed);
    }

    /// Get ACC events received total
    #[inline]
    pub fn acc_events_received(&self) -> u64 {
        self.acc_events_received.load(Ordering::Relaxed)
    }

    /// Record a journey egress event dropped due to channel full (lock-free)
    #[inline]
    pub fn record_journey_egress_dropped(&self) {
        self.journey_egress_dropped.fetch_add(1, Ordering::Relaxed);
    }

    /// Get journey egress dropped total
    #[inline]
    pub fn journey_egress_dropped(&self) -> u64 {
        self.journey_egress_dropped.load(Ordering::Relaxed)
    }

    /// Record gate command queue delay (time from enqueue to worker pickup)
    #[inline]
    pub fn record_gate_queue_delay(&self, delay_us: u64) {
        // Update histogram bucket
        let bucket = bucket_index(delay_us);
        self.gate_queue_delay_buckets[bucket].fetch_add(1, Ordering::Relaxed);
        self.gate_queue_delay_sum_us.fetch_add(delay_us, Ordering::Relaxed);

        // Update max
        update_atomic_max(&self.gate_queue_delay_max_us, delay_us);
    }

    /// Set current event queue depth (called by sampler)
    #[inline]
    pub fn set_event_queue_depth(&self, depth: u64) {
        self.event_queue_depth.store(depth, Ordering::Relaxed);
    }

    /// Set current gate command queue depth (called by sampler)
    #[inline]
    pub fn set_gate_queue_depth(&self, depth: u64) {
        self.gate_queue_depth.store(depth, Ordering::Relaxed);
    }

    /// Get current event queue depth
    #[inline]
    pub fn event_queue_depth(&self) -> u64 {
        self.event_queue_depth.load(Ordering::Relaxed)
    }

    /// Get current gate queue depth
    #[inline]
    pub fn gate_queue_depth(&self) -> u64 {
        self.gate_queue_depth.load(Ordering::Relaxed)
    }

    /// Set current CloudPlus outbound queue depth (called by sampler)
    #[inline]
    pub fn set_cloudplus_queue_depth(&self, depth: u64) {
        self.cloudplus_queue_depth.store(depth, Ordering::Relaxed);
    }

    /// Get current CloudPlus queue depth
    #[inline]
    pub fn cloudplus_queue_depth(&self) -> u64 {
        self.cloudplus_queue_depth.load(Ordering::Relaxed)
    }

    /// Record gate send latency (time for actual network send)
    #[inline]
    pub fn record_gate_send_latency(&self, latency_us: u64) {
        let bucket = bucket_index(latency_us);
        self.gate_send_latency_buckets[bucket].fetch_add(1, Ordering::Relaxed);
        self.gate_send_latency_sum_us.fetch_add(latency_us, Ordering::Relaxed);
        update_atomic_max(&self.gate_send_latency_max_us, latency_us);
    }

    /// Record gate enqueue-to-send time (total from enqueue to send complete)
    #[inline]
    pub fn record_gate_enqueue_to_send(&self, latency_us: u64) {
        let bucket = bucket_index(latency_us);
        self.gate_enqueue_to_send_buckets[bucket].fetch_add(1, Ordering::Relaxed);
        self.gate_enqueue_to_send_sum_us.fetch_add(latency_us, Ordering::Relaxed);
        update_atomic_max(&self.gate_enqueue_to_send_max_us, latency_us);
    }

    /// Get ACC events total
    #[inline]
    #[allow(dead_code)]
    pub fn acc_events_total(&self) -> u64 {
        self.acc_events_total.load(Ordering::Relaxed)
    }

    /// Get ACC matched total
    #[inline]
    #[allow(dead_code)]
    pub fn acc_matched_total(&self) -> u64 {
        self.acc_matched_total.load(Ordering::Relaxed)
    }

    /// Get stitch matched total
    #[inline]
    #[allow(dead_code)]
    pub fn stitch_matched_total(&self) -> u64 {
        self.stitch_matched_total.load(Ordering::Relaxed)
    }

    /// Get stitch expired total (truly lost)
    #[inline]
    #[allow(dead_code)]
    pub fn stitch_expired_total(&self) -> u64 {
        self.stitch_expired_total.load(Ordering::Relaxed)
    }

    /// Calculate and return metrics summary, then reset periodic counters
    ///
    /// This is the only method that resets counters. It uses atomic swap
    /// to get a consistent snapshot while allowing concurrent updates.
    pub fn report(&self, active_tracks: usize, authorized_tracks: usize) -> MetricsSummary {
        // Swap periodic counters to zero and get their values
        let events_count = self.events_since_report.swap(0, Ordering::Relaxed);
        let latency_sum = self.latency_sum_us.swap(0, Ordering::Relaxed);
        let max_latency = self.latency_max_us.swap(0, Ordering::Relaxed);

        // Swap histogram buckets and collect values
        let lat_buckets = swap_buckets(&self.latency_buckets);

        // Swap gate latency counters
        let gate_count = self.gate_commands_since_report.swap(0, Ordering::Relaxed);
        let gate_latency_sum = self.gate_latency_sum_us.swap(0, Ordering::Relaxed);
        let gate_max_latency = self.gate_latency_max_us.swap(0, Ordering::Relaxed);

        // Swap gate histogram buckets
        let gate_lat_buckets = swap_buckets(&self.gate_latency_buckets);

        // Get monotonic counters (don't reset)
        let events_total = self.events_total.load(Ordering::Relaxed);
        let gate_commands = self.gate_commands_sent.load(Ordering::Relaxed);

        // Calculate elapsed time and reset
        let elapsed = {
            let mut last = self.last_report_time.lock();
            let elapsed = last.elapsed();
            *last = Instant::now();
            elapsed
        };

        // Calculate derived metrics
        let events_per_sec = if elapsed.as_secs_f64() > 0.0 {
            events_count as f64 / elapsed.as_secs_f64()
        } else {
            0.0
        };

        let avg_latency = if events_count > 0 { latency_sum / events_count } else { 0 };

        // Compute percentiles from histogram
        let lat_p50 = percentile_from_buckets(&lat_buckets, 0.50);
        let lat_p95 = percentile_from_buckets(&lat_buckets, 0.95);
        let lat_p99 = percentile_from_buckets(&lat_buckets, 0.99);

        // Gate latency metrics
        let gate_avg_latency = if gate_count > 0 { gate_latency_sum / gate_count } else { 0 };
        let gate_lat_p99 = percentile_from_buckets(&gate_lat_buckets, 0.99);

        // Get current gate state and exits (don't reset)
        let gate_state = self.gate_state.load(Ordering::Relaxed);
        let exits_total = self.exits_total.load(Ordering::Relaxed);

        // Get ACC and stitch counters (don't reset)
        let acc_events_total = self.acc_events_total.load(Ordering::Relaxed);
        let acc_matched_total = self.acc_matched_total.load(Ordering::Relaxed);
        let stitch_matched_total = self.stitch_matched_total.load(Ordering::Relaxed);
        let stitch_expired_total = self.stitch_expired_total.load(Ordering::Relaxed);
        let acc_late_total = self.acc_late_total.load(Ordering::Relaxed);
        let acc_no_journey_total = self.acc_no_journey_total.load(Ordering::Relaxed);

        // Get drop and received counters (don't reset)
        let mqtt_events_dropped = self.mqtt_events_dropped.load(Ordering::Relaxed);
        let acc_events_dropped = self.acc_events_dropped.load(Ordering::Relaxed);
        let gate_cmds_dropped = self.gate_cmds_dropped.load(Ordering::Relaxed);
        let mqtt_events_received = self.mqtt_events_received.load(Ordering::Relaxed);
        let acc_events_received = self.acc_events_received.load(Ordering::Relaxed);
        let journey_egress_dropped = self.journey_egress_dropped.load(Ordering::Relaxed);

        // Compute drop ratios (guard divide-by-zero)
        let mqtt_drop_ratio =
            if mqtt_events_received > 0 { mqtt_events_dropped as f64 / mqtt_events_received as f64 } else { 0.0 };
        let acc_drop_ratio =
            if acc_events_received > 0 { acc_events_dropped as f64 / acc_events_received as f64 } else { 0.0 };

        // Swap gate queue delay histogram (reset on report)
        let gate_queue_delay_buckets = swap_buckets(&self.gate_queue_delay_buckets);
        let gate_queue_delay_sum = self.gate_queue_delay_sum_us.swap(0, Ordering::Relaxed);
        let gate_queue_delay_max = self.gate_queue_delay_max_us.swap(0, Ordering::Relaxed);
        let gate_queue_delay_count: u64 = gate_queue_delay_buckets.iter().sum();
        let gate_queue_delay_avg_us = if gate_queue_delay_count > 0 {
            gate_queue_delay_sum / gate_queue_delay_count
        } else {
            0
        };
        let gate_queue_delay_p99_us = percentile_from_buckets(&gate_queue_delay_buckets, 0.99);

        // Get queue depths (point-in-time, don't reset)
        let event_queue_depth = self.event_queue_depth.load(Ordering::Relaxed);
        let gate_queue_depth = self.gate_queue_depth.load(Ordering::Relaxed);
        let cloudplus_queue_depth = self.cloudplus_queue_depth.load(Ordering::Relaxed);

        // Swap gate send latency histogram (reset on report)
        let gate_send_latency_buckets = swap_buckets(&self.gate_send_latency_buckets);
        let gate_send_latency_sum = self.gate_send_latency_sum_us.swap(0, Ordering::Relaxed);
        let gate_send_latency_max = self.gate_send_latency_max_us.swap(0, Ordering::Relaxed);
        let gate_send_latency_count: u64 = gate_send_latency_buckets.iter().sum();
        let gate_send_latency_avg_us = if gate_send_latency_count > 0 {
            gate_send_latency_sum / gate_send_latency_count
        } else {
            0
        };
        let gate_send_latency_p99_us = percentile_from_buckets(&gate_send_latency_buckets, 0.99);

        // Swap gate enqueue-to-send histogram (reset on report)
        let gate_enqueue_to_send_buckets = swap_buckets(&self.gate_enqueue_to_send_buckets);
        let gate_enqueue_to_send_sum = self.gate_enqueue_to_send_sum_us.swap(0, Ordering::Relaxed);
        let gate_enqueue_to_send_max = self.gate_enqueue_to_send_max_us.swap(0, Ordering::Relaxed);
        let gate_enqueue_to_send_count: u64 = gate_enqueue_to_send_buckets.iter().sum();
        let gate_enqueue_to_send_avg_us = if gate_enqueue_to_send_count > 0 {
            gate_enqueue_to_send_sum / gate_enqueue_to_send_count
        } else {
            0
        };
        let gate_enqueue_to_send_p99_us =
            percentile_from_buckets(&gate_enqueue_to_send_buckets, 0.99);

        // Get stitch histogram buckets (don't reset - cumulative)
        let stitch_distance_buckets = load_buckets(&self.stitch_distance_buckets);
        let stitch_distance_sum = self.stitch_distance_sum.load(Ordering::Relaxed);
        let stitch_distance_count: u64 = stitch_distance_buckets.iter().sum();
        let stitch_distance_avg_cm =
            if stitch_distance_count > 0 { stitch_distance_sum / stitch_distance_count } else { 0 };

        let stitch_time_buckets = load_buckets(&self.stitch_time_buckets);
        let stitch_time_sum = self.stitch_time_sum.load(Ordering::Relaxed);
        let stitch_time_count: u64 = stitch_time_buckets.iter().sum();
        let stitch_time_avg_ms =
            if stitch_time_count > 0 { stitch_time_sum / stitch_time_count } else { 0 };

        MetricsSummary {
            events_total,
            events_per_sec,
            avg_process_latency_us: avg_latency,
            max_process_latency_us: max_latency,
            lat_buckets,
            lat_p50_us: lat_p50,
            lat_p95_us: lat_p95,
            lat_p99_us: lat_p99,
            active_tracks,
            authorized_tracks,
            gate_commands_sent: gate_commands,
            gate_lat_buckets,
            gate_lat_avg_us: gate_avg_latency,
            gate_lat_max_us: gate_max_latency,
            gate_lat_p99_us: gate_lat_p99,
            gate_state,
            exits_total,
            acc_events_total,
            acc_matched_total,
            stitch_matched_total,
            stitch_expired_total,
            stitch_distance_buckets,
            stitch_distance_avg_cm,
            stitch_time_buckets,
            stitch_time_avg_ms,
            acc_late_total,
            acc_no_journey_total,
            mqtt_events_dropped,
            acc_events_dropped,
            gate_cmds_dropped,
            mqtt_events_received,
            acc_events_received,
            journey_egress_dropped,
            mqtt_drop_ratio,
            acc_drop_ratio,
            gate_queue_delay_buckets,
            gate_queue_delay_avg_us,
            gate_queue_delay_max_us: gate_queue_delay_max,
            gate_queue_delay_p99_us,
            event_queue_depth,
            gate_queue_depth,
            cloudplus_queue_depth,
            gate_send_latency_buckets,
            gate_send_latency_avg_us,
            gate_send_latency_max_us: gate_send_latency_max,
            gate_send_latency_p99_us,
            gate_enqueue_to_send_buckets,
            gate_enqueue_to_send_avg_us,
            gate_enqueue_to_send_max_us: gate_enqueue_to_send_max,
            gate_enqueue_to_send_p99_us,
        }
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Number of histogram buckets (exported for egress)
pub const METRICS_NUM_BUCKETS: usize = NUM_BUCKETS;

/// Exported bucket bounds for Prometheus formatting
pub const METRICS_BUCKET_BOUNDS: [u64; 10] = BUCKET_BOUNDS;
pub const METRICS_STITCH_DIST_BOUNDS: [u64; 10] = STITCH_DIST_BOUNDS;

#[derive(Debug)]
#[allow(dead_code)]
pub struct MetricsSummary {
    pub events_total: u64,
    pub events_per_sec: f64,
    pub avg_process_latency_us: u64,
    pub max_process_latency_us: u64,
    /// Event processing latency histogram buckets
    /// Bounds: ≤100, ≤200, ≤400, ≤800, ≤1600, ≤3200, ≤6400, ≤12800, ≤25600, ≤51200, >51200 µs
    pub lat_buckets: [u64; NUM_BUCKETS],
    /// 50th percentile latency (µs)
    pub lat_p50_us: u64,
    /// 95th percentile latency (µs)
    pub lat_p95_us: u64,
    /// 99th percentile latency (µs)
    pub lat_p99_us: u64,
    pub active_tracks: usize,
    pub authorized_tracks: usize,
    pub gate_commands_sent: u64,
    /// Gate command E2E latency histogram buckets (same bounds)
    pub gate_lat_buckets: [u64; NUM_BUCKETS],
    /// Average gate command latency (µs)
    pub gate_lat_avg_us: u64,
    /// Max gate command latency (µs)
    pub gate_lat_max_us: u64,
    /// 99th percentile gate command latency (µs)
    pub gate_lat_p99_us: u64,
    /// Current gate state (0=closed, 1=moving, 2=open)
    pub gate_state: u64,
    /// Total exits through gate
    pub exits_total: u64,
    /// Total ACC events received
    pub acc_events_total: u64,
    /// Total ACC events matched to tracks
    pub acc_matched_total: u64,
    /// Total tracks successfully stitched
    pub stitch_matched_total: u64,
    /// Total tracks truly lost (expired without stitch)
    pub stitch_expired_total: u64,
    /// Stitch distance histogram buckets (cm)
    /// Bounds: ≤10, ≤20, ≤40, ≤80, ≤160, ≤320, ≤640, ≤1280, ≤2560, ≤5120, >5120 cm
    pub stitch_distance_buckets: [u64; NUM_BUCKETS],
    /// Average stitch distance (cm)
    pub stitch_distance_avg_cm: u64,
    /// Stitch time histogram buckets (ms)
    pub stitch_time_buckets: [u64; NUM_BUCKETS],
    /// Average stitch time (ms)
    pub stitch_time_avg_ms: u64,
    /// ACC events that arrived late (after person entered gate zone)
    pub acc_late_total: u64,
    /// ACC events matched but no journey found
    pub acc_no_journey_total: u64,
    /// MQTT events dropped due to channel full
    pub mqtt_events_dropped: u64,
    /// ACC events dropped due to channel full
    pub acc_events_dropped: u64,
    /// Gate commands dropped due to channel full
    pub gate_cmds_dropped: u64,
    /// MQTT events received (before try_send)
    pub mqtt_events_received: u64,
    /// ACC events received (before try_send)
    pub acc_events_received: u64,
    /// Journey egress events dropped due to channel full
    pub journey_egress_dropped: u64,
    /// MQTT drop ratio (dropped / received)
    pub mqtt_drop_ratio: f64,
    /// ACC drop ratio (dropped / received)
    pub acc_drop_ratio: f64,
    /// Gate queue delay histogram buckets (time from enqueue to worker pickup)
    pub gate_queue_delay_buckets: [u64; NUM_BUCKETS],
    /// Average gate queue delay (µs)
    pub gate_queue_delay_avg_us: u64,
    /// Max gate queue delay (µs)
    pub gate_queue_delay_max_us: u64,
    /// 99th percentile gate queue delay (µs)
    pub gate_queue_delay_p99_us: u64,
    /// Current event queue depth (snapshot)
    pub event_queue_depth: u64,
    /// Current gate command queue depth (snapshot)
    pub gate_queue_depth: u64,
    /// Current CloudPlus outbound queue depth (snapshot)
    pub cloudplus_queue_depth: u64,
    /// Gate send latency histogram (time for actual network send)
    pub gate_send_latency_buckets: [u64; NUM_BUCKETS],
    /// Average gate send latency (µs)
    pub gate_send_latency_avg_us: u64,
    /// Max gate send latency (µs)
    pub gate_send_latency_max_us: u64,
    /// 99th percentile gate send latency (µs)
    pub gate_send_latency_p99_us: u64,
    /// Gate enqueue-to-send histogram (total time from enqueue to send complete)
    pub gate_enqueue_to_send_buckets: [u64; NUM_BUCKETS],
    /// Average gate enqueue-to-send time (µs)
    pub gate_enqueue_to_send_avg_us: u64,
    /// Max gate enqueue-to-send time (µs)
    pub gate_enqueue_to_send_max_us: u64,
    /// 99th percentile gate enqueue-to-send time (µs)
    pub gate_enqueue_to_send_p99_us: u64,
}

impl MetricsSummary {
    pub fn log(&self) {
        info!(
            events_total = %self.events_total,
            events_per_sec = format!("{:.1}", self.events_per_sec),
            avg_latency_us = %self.avg_process_latency_us,
            max_latency_us = %self.max_process_latency_us,
            p50_us = %self.lat_p50_us,
            p95_us = %self.lat_p95_us,
            p99_us = %self.lat_p99_us,
            active_tracks = %self.active_tracks,
            authorized_tracks = %self.authorized_tracks,
            gate_cmds = %self.gate_commands_sent,
            gate_p99_us = %self.gate_lat_p99_us,
            mqtt_recv = %self.mqtt_events_received,
            mqtt_drop = %self.mqtt_events_dropped,
            mqtt_drop_pct = format!("{:.2}", self.mqtt_drop_ratio * 100.0),
            acc_recv = %self.acc_events_received,
            acc_drop = %self.acc_events_dropped,
            acc_drop_pct = format!("{:.2}", self.acc_drop_ratio * 100.0),
            egress_drop = %self.journey_egress_dropped,
            gate_drop = %self.gate_cmds_dropped,
            "metrics"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_new() {
        let metrics = Metrics::new();
        assert_eq!(metrics.events_total(), 0);
        assert_eq!(metrics.gate_commands_sent.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_record_event() {
        let metrics = Metrics::new();

        metrics.record_event_processed(100);
        assert_eq!(metrics.events_total(), 1);
        assert_eq!(metrics.latency_sum_us.load(Ordering::Relaxed), 100);

        metrics.record_event_processed(200);
        assert_eq!(metrics.events_total(), 2);
        assert_eq!(metrics.latency_sum_us.load(Ordering::Relaxed), 300);
    }

    #[test]
    fn test_record_gate_command() {
        let metrics = Metrics::new();

        metrics.record_gate_command();
        assert_eq!(metrics.gate_commands_sent.load(Ordering::Relaxed), 1);

        metrics.record_gate_command();
        assert_eq!(metrics.gate_commands_sent.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_report() {
        let metrics = Metrics::new();

        metrics.record_event_processed(100);
        metrics.record_event_processed(200);
        metrics.record_event_processed(300);
        metrics.record_gate_command();

        let summary = metrics.report(5, 2);

        assert_eq!(summary.events_total, 3);
        assert_eq!(summary.avg_process_latency_us, 200); // (100+200+300)/3
        assert_eq!(summary.max_process_latency_us, 300);
        assert_eq!(summary.active_tracks, 5);
        assert_eq!(summary.authorized_tracks, 2);
        assert_eq!(summary.gate_commands_sent, 1);

        // Periodic counters should be reset
        assert_eq!(metrics.events_since_report.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.latency_sum_us.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.latency_max_us.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_report_empty() {
        let metrics = Metrics::new();
        let summary = metrics.report(0, 0);

        assert_eq!(summary.events_total, 0);
        assert_eq!(summary.avg_process_latency_us, 0);
        assert_eq!(summary.max_process_latency_us, 0);
    }

    #[test]
    fn test_max_latency_tracking() {
        let metrics = Metrics::new();

        metrics.record_event_processed(100);
        metrics.record_event_processed(500);
        metrics.record_event_processed(200);
        metrics.record_event_processed(50);

        assert_eq!(metrics.latency_max_us.load(Ordering::Relaxed), 500);
    }

    #[test]
    fn test_concurrent_updates() {
        use std::sync::Arc;
        use std::thread;

        let metrics = Arc::new(Metrics::new());
        let mut handles = vec![];

        // Spawn 10 threads, each recording 1000 events
        for _ in 0..10 {
            let m = metrics.clone();
            handles.push(thread::spawn(move || {
                for i in 0..1000 {
                    m.record_event_processed(i as u64);
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(metrics.events_total(), 10_000);
    }

    #[test]
    fn test_bucket_index() {
        // Test bucket boundaries
        assert_eq!(bucket_index(0), 0);
        assert_eq!(bucket_index(100), 0);
        assert_eq!(bucket_index(101), 1);
        assert_eq!(bucket_index(200), 1);
        assert_eq!(bucket_index(201), 2);
        assert_eq!(bucket_index(400), 2);
        assert_eq!(bucket_index(51200), 9);
        assert_eq!(bucket_index(51201), 10); // overflow
        assert_eq!(bucket_index(100000), 10);
    }

    #[test]
    fn test_histogram_buckets() {
        let metrics = Metrics::new();

        // Record events in different buckets
        metrics.record_event_processed(50); // bucket 0 (≤100)
        metrics.record_event_processed(150); // bucket 1 (≤200)
        metrics.record_event_processed(350); // bucket 2 (≤400)
        metrics.record_event_processed(60000); // bucket 10 (overflow)

        let summary = metrics.report(0, 0);

        assert_eq!(summary.lat_buckets[0], 1);
        assert_eq!(summary.lat_buckets[1], 1);
        assert_eq!(summary.lat_buckets[2], 1);
        assert_eq!(summary.lat_buckets[10], 1);
    }

    #[test]
    fn test_percentile_computation() {
        let metrics = Metrics::new();

        // Record 100 events, all at 150µs (bucket 1, ≤200)
        for _ in 0..100 {
            metrics.record_event_processed(150);
        }

        let summary = metrics.report(0, 0);

        // All percentiles should be 200 (upper bound of bucket 1)
        assert_eq!(summary.lat_p50_us, 200);
        assert_eq!(summary.lat_p95_us, 200);
        assert_eq!(summary.lat_p99_us, 200);
    }

    #[test]
    fn test_gate_latency_tracking() {
        let metrics = Metrics::new();

        metrics.record_gate_latency(100);
        metrics.record_gate_latency(500);
        metrics.record_gate_latency(200);

        let summary = metrics.report(0, 0);

        assert_eq!(summary.gate_lat_avg_us, 266); // (100+500+200)/3
        assert_eq!(summary.gate_lat_max_us, 500);
        // All in lower buckets, p99 should be upper bound of highest occupied
        assert!(summary.gate_lat_p99_us <= 800);
    }
}
