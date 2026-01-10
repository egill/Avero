//! Gateway TUI - Performance Dashboard for checkout gateway
//!
//! Two views (Tab to toggle):
//! - Activity View: Live events + zone status + issues
//! - Performance View: Histograms, latency stats, throughput
//!
//! Keyboard shortcuts:
//! - Tab: Toggle Activity/Performance view
//! - o: Manual gate open test
//! - r: Reset stats
//! - q: Quit

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{BarChart, Block, Borders, List, ListItem, Paragraph},
    Frame, Terminal,
};
use rumqttc::{AsyncClient, Event as MqttEvent, MqttOptions, Packet, QoS};
use serde::Deserialize;
use std::collections::{HashMap, VecDeque};
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

// ============================================================================
// Constants
// ============================================================================

const MAX_TRACK_EVENTS: usize = 15;
const MAX_STITCH_EVENTS: usize = 12;
const MAX_GATE_EVENTS: usize = 10;
const MAX_ACC_EVENTS: usize = 10;
const MAX_ISSUES: usize = 10;
const MAX_LATENCY_SAMPLES: usize = 500;

// Zone timing thresholds (in ms)
const POS_WARNING_THRESHOLD_MS: u64 = 60_000; // 60s - show warning
const POS_OVERDUE_THRESHOLD_MS: u64 = 120_000; // 120s - show overdue

// ============================================================================
// View Mode
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq)]
enum ViewMode {
    Activity,
    Performance,
}

// ============================================================================
// Event Structures (from MQTT)
// ============================================================================

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct ZoneEvent {
    tid: i64,
    t: String,
    z: Option<String>,
    ts: u64,
    auth: bool,
    dwell_ms: Option<u64>,
    total_dwell_ms: Option<u64>,
    event_time: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct TrackEvent {
    ts: u64,
    t: String,
    tid: i64,
    prev_tid: Option<i64>,
    auth: bool,
    dwell_ms: u64,
    stitch_dist_cm: Option<u64>,
    stitch_time_ms: Option<u64>,
    parent_jid: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct Metrics {
    ts: u64,
    events_total: u64,
    events_per_sec: f64,
    avg_latency_us: u64,
    max_latency_us: u64,
    active_tracks: usize,
    authorized_tracks: usize,
    gate_cmds: u64,
    // Latency histogram fields (optional for backward compatibility)
    lat_buckets: Vec<u64>,
    lat_p50_us: u64,
    lat_p95_us: u64,
    lat_p99_us: u64,
    // Gate latency fields
    gate_lat_buckets: Vec<u64>,
    gate_lat_avg_us: u64,
    gate_lat_max_us: u64,
    gate_lat_p99_us: u64,
    // Queue depth and utilization
    event_queue_depth: u64,
    gate_queue_depth: u64,
    cloudplus_queue_depth: u64,
    event_queue_utilization_pct: u64,
    gate_queue_utilization_pct: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct GateState {
    ts: u64,
    state: String,
    tid: Option<i64>,
    src: String,
    /// Queue delay in microseconds (time from enqueue to processing start)
    queue_delay_us: Option<u64>,
    /// Send latency in microseconds (time for actual network send)
    send_latency_us: Option<u64>,
    /// Total enqueue-to-send time in microseconds
    enqueue_to_send_us: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct Journey {
    jid: String,
    pid: String,
    auth: bool,
    dwell: u64,
    out: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct AccDebugTrack {
    tid: i64,
    zone: Option<String>,
    dwell_ms: u64,
    auth: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct AccDebugPending {
    tid: i64,
    last_zone: Option<String>,
    dwell_ms: u64,
    auth: bool,
    pending_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct AccEvent {
    ts: u64,
    t: String,
    ip: String,
    pos: Option<String>,
    tid: Option<i64>,
    dwell_ms: Option<u64>,
    gate_zone: Option<String>,
    gate_entry_ts: Option<u64>,
    delta_ms: Option<u64>,
    gate_cmd_at: Option<u64>,
    debug_active: Option<Vec<AccDebugTrack>>,
    debug_pending: Option<Vec<AccDebugPending>>,
}

// ============================================================================
// Latency Statistics
// ============================================================================

#[derive(Debug, Clone, Default)]
struct LatencyStats {
    samples: VecDeque<u64>,
    min: Option<u64>,
    max: Option<u64>,
    sum: u64,
}

impl LatencyStats {
    fn add(&mut self, value: u64) {
        self.samples.push_back(value);
        if self.samples.len() > MAX_LATENCY_SAMPLES {
            if let Some(old) = self.samples.pop_front() {
                self.sum = self.sum.saturating_sub(old);
            }
        }
        self.sum += value;

        self.min = Some(self.min.map_or(value, |m| m.min(value)));
        self.max = Some(self.max.map_or(value, |m| m.max(value)));
    }

    fn count(&self) -> usize {
        self.samples.len()
    }

    fn avg(&self) -> Option<f64> {
        if self.samples.is_empty() {
            None
        } else {
            Some(self.sum as f64 / self.samples.len() as f64)
        }
    }

    fn p95(&self) -> Option<u64> {
        if self.samples.is_empty() {
            return None;
        }
        let mut sorted: Vec<u64> = self.samples.iter().copied().collect();
        sorted.sort_unstable();
        let idx = (sorted.len() as f64 * 0.95) as usize;
        sorted.get(idx.min(sorted.len() - 1)).copied()
    }

    /// Returns histogram buckets (auto-scaled)
    fn histogram(&self, num_buckets: usize) -> Vec<u64> {
        if self.samples.is_empty() || num_buckets == 0 {
            return vec![0; num_buckets];
        }

        let min = self.min.unwrap_or(0);
        let max = self.max.unwrap_or(0);

        if min == max {
            let mut buckets = vec![0; num_buckets];
            buckets[num_buckets / 2] = self.samples.len() as u64;
            return buckets;
        }

        let range = max - min;
        let bucket_size = (range as f64 / num_buckets as f64).max(1.0);

        let mut buckets = vec![0u64; num_buckets];
        for &sample in &self.samples {
            let bucket = ((sample - min) as f64 / bucket_size) as usize;
            let bucket = bucket.min(num_buckets - 1);
            buckets[bucket] += 1;
        }

        buckets
    }

    fn reset(&mut self) {
        self.samples.clear();
        self.min = None;
        self.max = None;
        self.sum = 0;
    }
}

// ============================================================================
// Performance Statistics
// ============================================================================

#[derive(Debug, Clone, Default)]
struct GateTimingStats {
    entry_to_cmd: LatencyStats,
    cmd_to_moving: LatencyStats,
    moving_to_open: LatencyStats,
    total_entry_to_open: LatencyStats,
}

impl GateTimingStats {
    fn reset(&mut self) {
        self.entry_to_cmd.reset();
        self.cmd_to_moving.reset();
        self.moving_to_open.reset();
        self.total_entry_to_open.reset();
    }
}

#[derive(Debug, Clone, Default)]
struct TrackProcessingStats {
    network_latency: LatencyStats, // ts - event_time = sensor-to-tracker latency
    processing_latency: LatencyStats, // internal processing
}

impl TrackProcessingStats {
    fn reset(&mut self) {
        self.network_latency.reset();
        self.processing_latency.reset();
    }
}

#[derive(Debug, Clone, Default)]
struct AccStats {
    match_latency: LatencyStats,
    matched_count: u64,
    orphaned_count: u64,
    late_count: u64,
    no_journey_count: u64,
}

impl AccStats {
    fn reset(&mut self) {
        self.match_latency.reset();
        self.matched_count = 0;
        self.orphaned_count = 0;
        self.late_count = 0;
        self.no_journey_count = 0;
    }

    fn total(&self) -> u64 {
        self.matched_count + self.orphaned_count
    }

    fn match_rate(&self) -> f64 {
        let total = self.total();
        if total == 0 {
            0.0
        } else {
            self.matched_count as f64 / total as f64 * 100.0
        }
    }
}

#[derive(Debug, Clone, Default)]
struct StitchingStats {
    success_count: u64,
    lost_count: u64,
    pending_count: u64,
    stitch_distance: LatencyStats, // cm
    stitch_time: LatencyStats,     // ms
}

impl StitchingStats {
    fn reset(&mut self) {
        self.success_count = 0;
        self.lost_count = 0;
        self.pending_count = 0;
        self.stitch_distance.reset();
        self.stitch_time.reset();
    }

    fn total(&self) -> u64 {
        self.success_count + self.lost_count
    }

    fn success_rate(&self) -> f64 {
        let total = self.total();
        if total == 0 {
            0.0
        } else {
            self.success_count as f64 / total as f64 * 100.0
        }
    }
}

#[derive(Debug, Clone, Default)]
struct ThroughputStats {
    events_per_sec_samples: VecDeque<f64>,
    peak_events_per_sec: f64,
    total_events: u64,
    session_start: Option<Instant>,
}

impl ThroughputStats {
    fn add_sample(&mut self, eps: f64) {
        self.events_per_sec_samples.push_back(eps);
        if self.events_per_sec_samples.len() > 60 {
            self.events_per_sec_samples.pop_front();
        }
        if eps > self.peak_events_per_sec {
            self.peak_events_per_sec = eps;
        }
    }

    fn current(&self) -> f64 {
        self.events_per_sec_samples.back().copied().unwrap_or(0.0)
    }

    fn avg(&self) -> f64 {
        if self.events_per_sec_samples.is_empty() {
            0.0
        } else {
            self.events_per_sec_samples.iter().sum::<f64>()
                / self.events_per_sec_samples.len() as f64
        }
    }

    fn session_duration(&self) -> Duration {
        self.session_start.map(|s| s.elapsed()).unwrap_or_default()
    }

    fn reset(&mut self) {
        self.events_per_sec_samples.clear();
        self.peak_events_per_sec = 0.0;
        self.total_events = 0;
        self.session_start = Some(Instant::now());
    }
}

// ============================================================================
// Zone Status Tracking
// ============================================================================

#[derive(Debug, Clone, Default)]
enum ZoneStatus {
    #[default]
    Empty,
    Pending {
        tid: i64,
        dwell_ms: u64,
    },
    Authorized {
        tid: i64,
        _dwell_ms: u64,
    },
    Waiting {
        tid: i64,
        dwell_ms: u64,
    },
    Overdue {
        tid: i64,
        dwell_ms: u64,
    },
}

#[derive(Debug, Clone, Default)]
enum GateZoneStatus {
    #[default]
    Empty,
    Authorized {
        tid: i64,
        door_state: String,
    },
    Blocked {
        tid: i64,
    },
    Exiting {
        tid: i64,
        door_state: String,
    },
}

#[derive(Debug, Clone, Default)]
struct ZoneState {
    name: String,
    occupant_tid: Option<i64>,
    occupant_auth: bool,
    occupant_dwell_ms: u64,
    entered_at: Option<Instant>,
    acc_matched_at: Option<Instant>,
}

impl ZoneState {
    fn status(&self) -> ZoneStatus {
        match self.occupant_tid {
            None => ZoneStatus::Empty,
            Some(tid) => {
                let dwell_ms = self.live_dwell_ms();

                if self.occupant_auth {
                    ZoneStatus::Authorized { tid, _dwell_ms: dwell_ms }
                } else if dwell_ms > POS_OVERDUE_THRESHOLD_MS {
                    ZoneStatus::Overdue { tid, dwell_ms }
                } else if dwell_ms > POS_WARNING_THRESHOLD_MS {
                    ZoneStatus::Waiting { tid, dwell_ms }
                } else {
                    ZoneStatus::Pending { tid, dwell_ms }
                }
            }
        }
    }

    fn live_dwell_ms(&self) -> u64 {
        self.entered_at
            .map(|t| self.occupant_dwell_ms + t.elapsed().as_millis() as u64)
            .unwrap_or(self.occupant_dwell_ms)
    }
}

// ============================================================================
// Gate Flow Tracking
// ============================================================================

#[derive(Debug, Clone, Default)]
struct GateFlow {
    tid: Option<i64>,
    _gate_entry_event_ts: Option<u64>,
    gate_entry_received: Option<Instant>,
    cmd_sent_ts: Option<u64>,
    cmd_sent_received: Option<Instant>,
    moving_ts: Option<u64>,
    moving_received: Option<Instant>,
    open_ts: Option<u64>,
    open_received: Option<Instant>,
    exit_ts: Option<u64>,
    exit_received: Option<Instant>,
}

impl GateFlow {
    fn entry_to_cmd_ms(&self) -> Option<u64> {
        match (self.gate_entry_received, self.cmd_sent_received) {
            (Some(e), Some(c)) => Some(c.duration_since(e).as_millis() as u64),
            _ => None,
        }
    }

    fn cmd_to_moving_ms(&self) -> Option<u64> {
        match (self.cmd_sent_received, self.moving_received) {
            (Some(c), Some(m)) => Some(m.duration_since(c).as_millis() as u64),
            _ => None,
        }
    }

    fn moving_to_open_ms(&self) -> Option<u64> {
        match (self.moving_received, self.open_received) {
            (Some(m), Some(o)) => Some(o.duration_since(m).as_millis() as u64),
            _ => None,
        }
    }

    fn total_entry_to_open_ms(&self) -> Option<u64> {
        match (self.gate_entry_received, self.open_received) {
            (Some(e), Some(o)) => Some(o.duration_since(e).as_millis() as u64),
            _ => None,
        }
    }
}

// ============================================================================
// Manual Gate Test
// ============================================================================

#[derive(Debug, Clone, Default)]
struct ManualGateTest {
    active: bool,
    cmd_sent: Option<Instant>,
    moving_at: Option<Instant>,
    open_at: Option<Instant>,
    last_cmd_to_moving: Option<u64>,
    last_moving_to_open: Option<u64>,
    last_total: Option<u64>,
}

impl ManualGateTest {
    fn start(&mut self) {
        self.active = true;
        self.cmd_sent = Some(Instant::now());
        self.moving_at = None;
        self.open_at = None;
    }

    fn on_moving(&mut self) {
        if self.active && self.moving_at.is_none() {
            let now = Instant::now();
            self.moving_at = Some(now);
            if let Some(cmd) = self.cmd_sent {
                self.last_cmd_to_moving = Some(now.duration_since(cmd).as_millis() as u64);
            }
        }
    }

    fn on_open(&mut self) {
        if self.active && self.open_at.is_none() {
            let now = Instant::now();
            self.open_at = Some(now);
            if let Some(moving) = self.moving_at {
                self.last_moving_to_open = Some(now.duration_since(moving).as_millis() as u64);
            }
            if let Some(cmd) = self.cmd_sent {
                self.last_total = Some(now.duration_since(cmd).as_millis() as u64);
            }
            self.active = false;
        }
    }
}

// ============================================================================
// Issues Tracking
// ============================================================================

#[derive(Debug, Clone)]
enum IssueSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone)]
struct Issue {
    timestamp: Instant,
    severity: IssueSeverity,
    message: String,
}

// ============================================================================
// Session Counters
// ============================================================================

#[derive(Debug, Clone, Default)]
struct SessionCounters {
    entries: u64,
    exits: u64,
    gate_opens: u64,
}

impl SessionCounters {
    fn reset(&mut self) {
        self.entries = 0;
        self.exits = 0;
        self.gate_opens = 0;
    }
}

// ============================================================================
// Dashboard State
// ============================================================================

#[derive(Debug)]
struct DashboardState {
    // View mode
    view_mode: ViewMode,

    // Zone tracking (5 POS + 1 GATE)
    pos_zones: HashMap<String, ZoneState>,
    gate_zone: ZoneState,
    gate_zone_status: GateZoneStatus,
    last_gate_state: String,

    // Event feeds
    track_events: VecDeque<TrackEvent>,
    stitch_events: VecDeque<TrackEvent>,
    gate_events: VecDeque<GateState>,
    acc_events: VecDeque<AccEvent>,

    // Issues
    issues: VecDeque<Issue>,

    // Gate flow tracking
    current_gate_flow: GateFlow,
    last_gate_flow: Option<GateFlow>,
    gate_cmd_count: u64,

    // Manual gate test
    manual_test: ManualGateTest,

    // Performance statistics
    gate_timing: GateTimingStats,
    track_processing: TrackProcessingStats,
    acc_stats: AccStats,
    stitching_stats: StitchingStats,
    throughput: ThroughputStats,

    // Session counters
    counters: SessionCounters,

    // Metrics from gateway
    metrics: Metrics,

    // Journey tracking
    journeys_completed: u64,
    journeys_authorized: u64,
    last_journey: Option<Journey>,

    // Connection status
    connected: bool,
    last_message: Option<Instant>,

    // Debug counters for MQTT messages received
    _msg_count_gate: u64,
    _msg_count_metrics: u64,
    _msg_count_events: u64,

    // Gate controller (for manual test)
    gate_host: Option<String>,
}

impl Default for DashboardState {
    fn default() -> Self {
        let mut state = Self {
            view_mode: ViewMode::Activity,
            pos_zones: HashMap::new(),
            gate_zone: ZoneState { name: "GATE".to_string(), ..Default::default() },
            gate_zone_status: GateZoneStatus::Empty,
            last_gate_state: "closed".to_string(),
            track_events: VecDeque::new(),
            stitch_events: VecDeque::new(),
            gate_events: VecDeque::new(),
            acc_events: VecDeque::new(),
            issues: VecDeque::new(),
            current_gate_flow: GateFlow::default(),
            last_gate_flow: None,
            gate_cmd_count: 0,
            manual_test: ManualGateTest::default(),
            gate_timing: GateTimingStats::default(),
            track_processing: TrackProcessingStats::default(),
            acc_stats: AccStats::default(),
            stitching_stats: StitchingStats::default(),
            throughput: ThroughputStats {
                session_start: Some(Instant::now()),
                ..Default::default()
            },
            counters: SessionCounters::default(),
            metrics: Metrics::default(),
            journeys_completed: 0,
            journeys_authorized: 0,
            last_journey: None,
            connected: false,
            last_message: None,
            _msg_count_gate: 0,
            _msg_count_metrics: 0,
            _msg_count_events: 0,
            gate_host: None,
        };

        // Initialize 5 POS zones
        for i in 1..=5 {
            let name = format!("POS_{}", i);
            state.pos_zones.insert(name.clone(), ZoneState { name, ..Default::default() });
        }

        state
    }
}

impl DashboardState {
    fn reset_stats(&mut self) {
        self.gate_timing.reset();
        self.track_processing.reset();
        self.acc_stats.reset();
        self.stitching_stats.reset();
        self.throughput.reset();
        self.counters.reset();
        self.issues.clear();
        self.journeys_completed = 0;
        self.journeys_authorized = 0;
        self.gate_cmd_count = 0;
    }

    fn add_issue(&mut self, severity: IssueSeverity, message: String) {
        self.issues.push_front(Issue { timestamp: Instant::now(), severity, message });
        if self.issues.len() > MAX_ISSUES {
            self.issues.pop_back();
        }
    }

    fn handle_zone_event(&mut self, event: ZoneEvent) {
        let now = Instant::now();

        // Track network latency if available (ts - event_time = sensor-to-tracker latency)
        if let Some(event_time) = event.event_time {
            if event.ts > event_time {
                self.track_processing.network_latency.add(event.ts - event_time);
            }
        }

        if let Some(zone_name) = &event.z {
            // Handle POS zones
            if zone_name.starts_with("POS_") {
                if let Some(zone) = self.pos_zones.get_mut(zone_name) {
                    match event.t.as_str() {
                        "zone_entry" => {
                            zone.occupant_tid = Some(event.tid);
                            zone.occupant_auth = event.auth;
                            zone.occupant_dwell_ms = event.total_dwell_ms.unwrap_or(0);
                            zone.entered_at = Some(now);
                            zone.acc_matched_at = None;
                            self.counters.entries += 1;
                        }
                        "zone_exit" => {
                            if zone.occupant_tid == Some(event.tid) {
                                // Clear zone state (no warning for leaving without ACC - normal browsing)
                                zone.occupant_tid = None;
                                zone.occupant_auth = false;
                                zone.occupant_dwell_ms = 0;
                                zone.entered_at = None;
                                zone.acc_matched_at = None;
                            }
                            self.counters.exits += 1;
                        }
                        _ => {}
                    }
                }
            }

            // Handle GATE zone
            if zone_name == "GATE_1" {
                match event.t.as_str() {
                    "zone_entry" => {
                        // Save previous flow if it had activity
                        if self.current_gate_flow.gate_entry_received.is_some() {
                            self.complete_gate_flow();
                        }
                        // Start new flow
                        self.current_gate_flow = GateFlow {
                            tid: Some(event.tid),
                            _gate_entry_event_ts: Some(event.ts),
                            gate_entry_received: Some(now),
                            ..Default::default()
                        };

                        // Update gate zone state
                        self.gate_zone.occupant_tid = Some(event.tid);
                        self.gate_zone.occupant_auth = event.auth;
                        self.gate_zone.entered_at = Some(now);

                        // Update gate zone status
                        self.gate_zone_status = if event.auth {
                            GateZoneStatus::Authorized {
                                tid: event.tid,
                                door_state: self.last_gate_state.clone(),
                            }
                        } else {
                            GateZoneStatus::Blocked { tid: event.tid }
                        };
                    }
                    "zone_exit" => {
                        if self.gate_zone.occupant_tid == Some(event.tid) {
                            self.gate_zone.occupant_tid = None;
                            self.gate_zone.occupant_auth = false;
                            self.gate_zone.entered_at = None;
                            self.gate_zone_status = GateZoneStatus::Empty;
                        }
                    }
                    _ => {}
                }
            }

            // Track EXIT line crossing
            if zone_name == "EXIT_1"
                && event.t == "line_cross"
                && self.current_gate_flow.tid == Some(event.tid)
            {
                self.current_gate_flow.exit_ts = Some(event.ts);
                self.current_gate_flow.exit_received = Some(now);
            }
        }

        self.last_message = Some(now);
    }

    fn complete_gate_flow(&mut self) {
        let flow = &self.current_gate_flow;

        // Record timing stats if we have complete data
        if let Some(ms) = flow.entry_to_cmd_ms() {
            self.gate_timing.entry_to_cmd.add(ms);
        }
        if let Some(ms) = flow.cmd_to_moving_ms() {
            self.gate_timing.cmd_to_moving.add(ms);
        }
        if let Some(ms) = flow.moving_to_open_ms() {
            self.gate_timing.moving_to_open.add(ms);
        }
        if let Some(ms) = flow.total_entry_to_open_ms() {
            self.gate_timing.total_entry_to_open.add(ms);
        }

        self.last_gate_flow = Some(self.current_gate_flow.clone());
    }

    fn handle_track_event(&mut self, event: TrackEvent) {
        // Update POS zone auth status when track gets authorized
        if event.auth {
            for zone in self.pos_zones.values_mut() {
                if zone.occupant_tid == Some(event.tid) && !zone.occupant_auth {
                    zone.occupant_auth = true;
                    zone.acc_matched_at = Some(Instant::now());
                }
            }
        }

        // Track stitching stats
        match event.t.as_str() {
            "stitch" => {
                self.stitching_stats.success_count += 1;
                if let Some(dist) = event.stitch_dist_cm {
                    self.stitching_stats.stitch_distance.add(dist);
                }
                if let Some(time) = event.stitch_time_ms {
                    self.stitching_stats.stitch_time.add(time);
                }

                // Transfer occupancy from old track to new track
                if let Some(prev_tid) = event.prev_tid {
                    for zone in self.pos_zones.values_mut() {
                        if zone.occupant_tid == Some(prev_tid) {
                            zone.occupant_tid = Some(event.tid);
                            zone.occupant_auth = event.auth;
                        }
                    }
                }
            }
            "lost" => {
                self.stitching_stats.lost_count += 1;
                self.add_issue(
                    IssueSeverity::Error,
                    format!(
                        "TRACK LOST T{} dwell:{:.1}s",
                        event.tid,
                        event.dwell_ms as f64 / 1000.0
                    ),
                );

                // Remove from zones
                for zone in self.pos_zones.values_mut() {
                    if zone.occupant_tid == Some(event.tid) {
                        zone.occupant_tid = None;
                        zone.occupant_auth = false;
                        zone.occupant_dwell_ms = 0;
                        zone.entered_at = None;
                    }
                }
            }
            "pending" => {
                self.stitching_stats.pending_count += 1;

                // Clear from zones temporarily
                for zone in self.pos_zones.values_mut() {
                    if zone.occupant_tid == Some(event.tid) {
                        zone.occupant_tid = None;
                        zone.occupant_auth = false;
                        zone.occupant_dwell_ms = 0;
                        zone.entered_at = None;
                    }
                }
            }
            _ => {}
        }

        // Add to dedicated stitch/lost queue
        if matches!(event.t.as_str(), "stitch" | "lost" | "pending") {
            self.stitch_events.push_front(event.clone());
            if self.stitch_events.len() > MAX_STITCH_EVENTS {
                self.stitch_events.pop_back();
            }
        }

        // Add to track events
        self.track_events.push_front(event);
        if self.track_events.len() > MAX_TRACK_EVENTS {
            self.track_events.pop_back();
        }

        self.last_message = Some(Instant::now());
    }

    fn handle_gate_event(&mut self, event: GateState) {
        let now = Instant::now();
        self.last_gate_state = event.state.clone();

        match event.state.as_str() {
            "cmd_sent" => {
                self.gate_cmd_count += 1;
                self.current_gate_flow.cmd_sent_ts = Some(event.ts);
                self.current_gate_flow.cmd_sent_received = Some(now);
                if self.current_gate_flow.tid.is_none() {
                    self.current_gate_flow.tid = event.tid;
                }

                // Update gate zone status
                if let Some(tid) = event.tid.or(self.gate_zone.occupant_tid) {
                    self.gate_zone_status =
                        GateZoneStatus::Exiting { tid, door_state: "cmd_sent".to_string() };
                }
            }
            "cmd_dropped" => {
                if let Some(tid) = event.tid.or(self.gate_zone.occupant_tid) {
                    self.gate_zone_status =
                        GateZoneStatus::Exiting { tid, door_state: "cmd_dropped".to_string() };
                }
            }
            "moving" => {
                self.current_gate_flow.moving_ts = Some(event.ts);
                self.current_gate_flow.moving_received = Some(now);
                self.manual_test.on_moving();

                // Update gate zone status
                if let GateZoneStatus::Exiting { tid, .. } = self.gate_zone_status {
                    self.gate_zone_status =
                        GateZoneStatus::Exiting { tid, door_state: "moving".to_string() };
                }
            }
            "open" => {
                self.current_gate_flow.open_ts = Some(event.ts);
                self.current_gate_flow.open_received = Some(now);
                self.manual_test.on_open();
                self.counters.gate_opens += 1;

                // Update gate zone status
                if let GateZoneStatus::Exiting { tid, .. } = self.gate_zone_status {
                    self.gate_zone_status =
                        GateZoneStatus::Exiting { tid, door_state: "open".to_string() };
                }
            }
            "closed" => {
                // Complete gate flow
                if self.current_gate_flow.cmd_sent_received.is_some()
                    || self.current_gate_flow.gate_entry_received.is_some()
                {
                    self.complete_gate_flow();
                    self.current_gate_flow = GateFlow::default();
                }

                // If someone still in gate zone, update status
                if let Some(tid) = self.gate_zone.occupant_tid {
                    self.gate_zone_status = if self.gate_zone.occupant_auth {
                        GateZoneStatus::Authorized { tid, door_state: "closed".to_string() }
                    } else {
                        GateZoneStatus::Blocked { tid }
                    };
                } else {
                    self.gate_zone_status = GateZoneStatus::Empty;
                }
            }
            "blocked" => {
                if let Some(tid) = event.tid {
                    self.add_issue(IssueSeverity::Warning, format!("Unauthorized exit T{}", tid));
                }
            }
            _ => {}
        }

        self.gate_events.push_front(event);
        if self.gate_events.len() > MAX_GATE_EVENTS {
            self.gate_events.pop_back();
        }

        self.last_message = Some(now);
    }

    fn handle_acc_event(&mut self, event: AccEvent) {
        match event.t.as_str() {
            "matched" => {
                self.acc_stats.matched_count += 1;
                // Track match latency if available
                if let Some(delta) = event.delta_ms {
                    self.acc_stats.match_latency.add(delta);
                }

                // Update POS zone auth status
                if let Some(tid) = event.tid {
                    for zone in self.pos_zones.values_mut() {
                        if zone.occupant_tid == Some(tid) {
                            zone.occupant_auth = true;
                            zone.acc_matched_at = Some(Instant::now());
                        }
                    }
                }
            }
            "unmatched" => {
                self.acc_stats.orphaned_count += 1;
                let pos = event.pos.as_deref().unwrap_or("?");

                // Build a more informative message showing nearby candidates
                let mut msg = format!("ACC no match at {}", pos);

                // Show candidate tracks if available
                if let Some(ref active) = event.debug_active {
                    if !active.is_empty() {
                        let candidates: Vec<String> = active
                            .iter()
                            .filter(|t| {
                                t.zone.as_ref().map(|z| z.starts_with("POS")).unwrap_or(false)
                            })
                            .take(3)
                            .map(|t| format!("T{}@{}", t.tid, t.zone.as_deref().unwrap_or("?")))
                            .collect();
                        if !candidates.is_empty() {
                            msg.push_str(&format!(" (nearby: {})", candidates.join(", ")));
                        }
                    }
                }

                self.add_issue(IssueSeverity::Error, msg);
            }
            "late_after_gate" => {
                self.acc_stats.late_count += 1;
                if let Some(delta) = event.delta_ms {
                    self.add_issue(
                        IssueSeverity::Warning,
                        format!(
                            "ACC LATE {} - {}ms after gate",
                            event.pos.as_deref().unwrap_or("-"),
                            delta
                        ),
                    );
                }
            }
            "matched_no_journey" => {
                self.acc_stats.no_journey_count += 1;
            }
            _ => {}
        }

        self.acc_events.push_front(event);
        if self.acc_events.len() > MAX_ACC_EVENTS {
            self.acc_events.pop_back();
        }

        self.last_message = Some(Instant::now());
    }

    fn update_metrics(&mut self, metrics: Metrics) {
        self.throughput.add_sample(metrics.events_per_sec);
        self.throughput.total_events = metrics.events_total;
        self.metrics = metrics;
        self.last_message = Some(Instant::now());
    }

    fn add_journey(&mut self, journey: Journey) {
        self.journeys_completed += 1;
        if journey.auth {
            self.journeys_authorized += 1;
        }
        self.last_journey = Some(journey);
        self.last_message = Some(Instant::now());
    }

    fn pos_occupancy_count(&self) -> usize {
        self.pos_zones.values().filter(|z| z.occupant_tid.is_some()).count()
    }
}

type SharedState = Arc<Mutex<DashboardState>>;

// ============================================================================
// Main
// ============================================================================

// Config file support for TUI
// Uses serde(default) to ignore unknown fields from the full gateway config
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct TuiConfig {
    mqtt: Option<TuiMqttConfig>,
    gate: Option<TuiGateConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct TuiMqttConfig {
    host: Option<String>,
    port: Option<u16>,
    username: Option<String>,
    password: Option<String>,
    // Ignored fields from full config
    #[serde(default, rename = "topic")]
    _topic: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct TuiGateConfig {
    host: Option<String>,
    // Ignored fields from full config
    #[serde(default, rename = "mode")]
    _mode: Option<String>,
    #[serde(default)]
    tcp_addr: Option<String>,
    #[serde(default, rename = "http_url")]
    _http_url: Option<String>,
    #[serde(default, rename = "timeout_ms")]
    _timeout_ms: Option<u64>,
}

fn load_config(path: &str) -> Option<TuiConfig> {
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to read config file: {}", e);
            return None;
        }
    };

    // Parse as generic Value first to handle unknown sections
    let value: toml::Value = match toml::from_str(&contents) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Failed to parse TOML: {}", e);
            return None;
        }
    };

    // Extract mqtt section
    let mqtt = value.get("mqtt").map(|m| TuiMqttConfig {
        host: m.get("host").and_then(|v| v.as_str().map(String::from)),
        port: m.get("port").and_then(|v| v.as_integer().map(|p| p as u16)),
        username: m.get("username").and_then(|v| v.as_str().map(String::from)),
        password: m.get("password").and_then(|v| v.as_str().map(String::from)),
        _topic: m.get("topic").and_then(|v| v.as_str().map(String::from)),
    });

    // Extract gate section (for gate_host used in manual test)
    let gate = value.get("gate").map(|g| TuiGateConfig {
        host: g.get("host").and_then(|v| v.as_str().map(String::from)),
        _mode: g.get("mode").and_then(|v| v.as_str().map(String::from)),
        tcp_addr: g.get("tcp_addr").and_then(|v| v.as_str().map(String::from)),
        _http_url: g.get("http_url").and_then(|v| v.as_str().map(String::from)),
        _timeout_ms: g.get("timeout_ms").and_then(|v| v.as_integer().map(|t| t as u64)),
    });

    Some(TuiConfig { mqtt, gate })
}

fn print_usage() {
    eprintln!("Usage: gateway-tui [OPTIONS] [HOST PORT USER PASS [GATE_HOST]]");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  -c, --config <FILE>  Load configuration from TOML file");
    eprintln!("  -h, --help           Show this help message");
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  gateway-tui --config /opt/avero/gateway-poc.toml");
    eprintln!("  gateway-tui localhost 1883 avero avero");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    // Parse config file or CLI args
    let (broker_host, broker_port, mqtt_user, mqtt_pass, gate_host) =
        if args.iter().any(|a| a == "-h" || a == "--help") {
            print_usage();
            return Ok(());
        } else if args.iter().any(|a| a == "--test-config") {
            // Test config parsing without starting TUI
            if let Some(config_idx) = args.iter().position(|a| a == "-c" || a == "--config") {
                let config_path = args.get(config_idx + 1).ok_or("Missing config file path")?;
                if let Some(config) = load_config(config_path) {
                    let mqtt = config.mqtt.unwrap_or_default();
                    eprintln!("Config parsed successfully:");
                    eprintln!("  MQTT host: {:?}", mqtt.host);
                    eprintln!("  MQTT port: {:?}", mqtt.port);
                    eprintln!("  MQTT user: {:?}", mqtt.username);
                    eprintln!("  MQTT pass: {:?}", mqtt.password.as_ref().map(|_| "***"));
                    if let Some(gate) = config.gate {
                        eprintln!("  Gate tcp_addr: {:?}", gate.tcp_addr);
                    }
                } else {
                    eprintln!("Failed to parse config");
                }
            } else {
                eprintln!("--test-config requires --config <FILE>");
            }
            return Ok(());
        } else if let Some(config_idx) = args.iter().position(|a| a == "-c" || a == "--config") {
            // Config file mode
            let config_path = args.get(config_idx + 1).ok_or("Missing config file path")?;
            let config = load_config(config_path)
                .ok_or_else(|| format!("Failed to load config from {}", config_path))?;

            let mqtt = config.mqtt.unwrap_or_default();
            // For gate host, use tcp_addr (e.g., "10.120.48.9:8000") -> extract host part
            let gate_host = config.gate.and_then(|g| {
                g.tcp_addr.map(|addr| addr.split(':').next().unwrap_or(&addr).to_string())
            });
            (
                mqtt.host.unwrap_or_else(|| "localhost".to_string()),
                mqtt.port.unwrap_or(1883),
                mqtt.username,
                mqtt.password,
                gate_host,
            )
        } else {
            // Positional args mode (backward compatible)
            (
                args.get(1).cloned().unwrap_or_else(|| "localhost".to_string()),
                args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1883),
                args.get(3).cloned(),
                args.get(4).cloned(),
                args.get(5).cloned(),
            )
        };

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let state = Arc::new(Mutex::new(DashboardState::default()));

    // Set gate host for manual test
    if let Some(host) = gate_host {
        state.lock().await.gate_host = Some(host);
    }

    let mqtt_state = state.clone();
    let mqtt_handle = tokio::spawn(async move {
        run_mqtt_subscriber(&broker_host, broker_port, mqtt_user, mqtt_pass, mqtt_state).await;
    });

    let result = run_ui(&mut terminal, state).await;

    mqtt_handle.abort();
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    result
}

// ============================================================================
// MQTT Subscriber
// ============================================================================

async fn run_mqtt_subscriber(
    host: &str,
    port: u16,
    username: Option<String>,
    password: Option<String>,
    state: SharedState,
) {
    let client_id = format!("gateway-tui-{}", std::process::id());
    let mut mqttoptions = MqttOptions::new(client_id, host, port);
    mqttoptions.set_keep_alive(Duration::from_secs(30));
    mqttoptions.set_clean_session(true);

    if let (Some(user), Some(pass)) = (username, password) {
        mqttoptions.set_credentials(user, pass);
    }

    let (client, mut eventloop) = AsyncClient::new(mqttoptions, 100);
    let _ = client.subscribe("gateway/#", QoS::AtMostOnce).await;

    loop {
        match eventloop.poll().await {
            Ok(MqttEvent::Incoming(Packet::ConnAck(_))) => {
                let mut s = state.lock().await;
                s.connected = true;
            }
            Ok(MqttEvent::Incoming(Packet::Publish(publish))) => {
                let topic = publish.topic.as_str();
                let payload = std::str::from_utf8(&publish.payload).unwrap_or("");

                let mut s = state.lock().await;

                match topic {
                    "gateway/events" => {
                        if let Ok(event) = serde_json::from_str::<ZoneEvent>(payload) {
                            s.handle_zone_event(event);
                        }
                    }
                    "gateway/tracks" => {
                        if let Ok(event) = serde_json::from_str::<TrackEvent>(payload) {
                            s.handle_track_event(event);
                        }
                    }
                    "gateway/gate" => {
                        if let Ok(event) = serde_json::from_str::<GateState>(payload) {
                            s.handle_gate_event(event);
                        }
                    }
                    "gateway/metrics" => {
                        if let Ok(metrics) = serde_json::from_str::<Metrics>(payload) {
                            s.update_metrics(metrics);
                        }
                    }
                    "gateway/journeys" => {
                        if let Ok(journey) = serde_json::from_str::<Journey>(payload) {
                            s.add_journey(journey);
                        }
                    }
                    "gateway/acc" => {
                        if let Ok(event) = serde_json::from_str::<AccEvent>(payload) {
                            s.handle_acc_event(event);
                        }
                    }
                    _ => {}
                }
            }
            Ok(_) => {}
            Err(_) => {
                let mut s = state.lock().await;
                s.connected = false;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

// ============================================================================
// UI Loop
// ============================================================================

async fn run_ui(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: SharedState,
) -> Result<(), Box<dyn std::error::Error>> {
    let tick_rate = Duration::from_millis(100);
    let mut last_tick = Instant::now();

    loop {
        let s = state.lock().await;
        terminal.draw(|f| draw_ui(f, &s))?;
        drop(s);

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    let mut s = state.lock().await;
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Tab => {
                            s.view_mode = match s.view_mode {
                                ViewMode::Activity => ViewMode::Performance,
                                ViewMode::Performance => ViewMode::Activity,
                            };
                        }
                        KeyCode::Char('r') => {
                            s.reset_stats();
                        }
                        KeyCode::Char('o') => {
                            // Manual gate test - send open command
                            if let Some(ref host) = s.gate_host.clone() {
                                s.manual_test.start();
                                let host = host.clone();
                                drop(s);
                                // Send HTTP request to gate controller
                                tokio::spawn(async move {
                                    let _ =
                                        reqwest::get(format!("http://{}:8080/open", host)).await;
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }
    }
}

// ============================================================================
// Main UI Router
// ============================================================================

fn draw_ui(f: &mut Frame, state: &DashboardState) {
    match state.view_mode {
        ViewMode::Activity => draw_activity_view(f, state),
        ViewMode::Performance => draw_performance_view(f, state),
    }
}

// ============================================================================
// Activity View
// ============================================================================

fn draw_activity_view(f: &mut Frame, state: &DashboardState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Length(4), // Zone status
            Constraint::Length(3), // Live counters
            Constraint::Min(10),   // Event feeds
            Constraint::Length(6), // Issues panel
        ])
        .split(f.area());

    draw_header(f, chunks[0], state);
    draw_zone_status(f, chunks[1], state);
    draw_live_counters(f, chunks[2], state);
    draw_event_feeds(f, chunks[3], state);
    draw_issues_panel(f, chunks[4], state);
}

// ============================================================================
// Performance View
// ============================================================================

fn draw_performance_view(f: &mut Frame, state: &DashboardState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Header
            Constraint::Length(10), // Gate timing
            Constraint::Length(8),  // Track processing + ACC
            Constraint::Length(6),  // Stitching + Throughput
            Constraint::Length(4),  // Zone status
        ])
        .split(f.area());

    draw_header(f, chunks[0], state);
    draw_gate_timing_panel(f, chunks[1], state);
    draw_processing_panels(f, chunks[2], state);
    draw_bottom_panels(f, chunks[3], state);
    draw_zone_status(f, chunks[4], state);
}

// ============================================================================
// Shared Components
// ============================================================================

fn draw_header(f: &mut Frame, area: Rect, state: &DashboardState) {
    let status_color = if state.connected { Color::Green } else { Color::Red };
    let status_text = if state.connected { "CONNECTED" } else { "DISCONNECTED" };

    let last_msg = state
        .last_message
        .map(|t| format!("{}s ago", t.elapsed().as_secs()))
        .unwrap_or_else(|| "never".to_string());

    let view_text = match state.view_mode {
        ViewMode::Activity => "ACTIVITY",
        ViewMode::Performance => "PERFORMANCE",
    };

    let header = Paragraph::new(Line::from(vec![
        Span::styled("Gateway TUI ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw("| "),
        Span::styled(
            format!("[{}]", view_text),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" | "),
        Span::styled(status_text, Style::default().fg(status_color)),
        Span::raw(" | Last: "),
        Span::raw(last_msg),
        Span::raw(" | "),
        Span::styled("Tab", Style::default().fg(Color::DarkGray)),
        Span::raw(":view "),
        Span::styled("o", Style::default().fg(Color::DarkGray)),
        Span::raw(":gate "),
        Span::styled("r", Style::default().fg(Color::DarkGray)),
        Span::raw(":reset "),
        Span::styled("q", Style::default().fg(Color::DarkGray)),
        Span::raw(":quit"),
    ]))
    .block(Block::default().borders(Borders::ALL));

    f.render_widget(header, area);
}

fn draw_zone_status(f: &mut Frame, area: Rect, state: &DashboardState) {
    // Build zone status row: POS1-5 + GATE
    let mut cells: Vec<Span> =
        vec![Span::styled("ZONES ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))];

    // Sort POS zones
    let mut zones: Vec<_> = state.pos_zones.values().collect();
    zones.sort_by(|a, b| a.name.cmp(&b.name));

    for zone in zones {
        let (icon, color, detail) = match zone.status() {
            ZoneStatus::Empty => ("○", Color::DarkGray, zone.name.replace("POS_", "P")),
            ZoneStatus::Pending { tid, dwell_ms } => (
                "?",
                Color::Yellow,
                format!(
                    "{}:T{} {:.0}s",
                    zone.name.replace("POS_", "P"),
                    tid,
                    dwell_ms as f64 / 1000.0
                ),
            ),
            ZoneStatus::Authorized { tid, .. } => {
                ("✓", Color::Green, format!("{}:T{}", zone.name.replace("POS_", "P"), tid))
            }
            ZoneStatus::Waiting { tid, dwell_ms } => (
                "⚠",
                Color::Yellow,
                format!(
                    "{}:T{} {:.0}s!",
                    zone.name.replace("POS_", "P"),
                    tid,
                    dwell_ms as f64 / 1000.0
                ),
            ),
            ZoneStatus::Overdue { tid, dwell_ms } => (
                "✗",
                Color::Red,
                format!(
                    "{}:T{} {:.0}s!!",
                    zone.name.replace("POS_", "P"),
                    tid,
                    dwell_ms as f64 / 1000.0
                ),
            ),
        };

        cells.push(Span::styled(format!("{} ", icon), Style::default().fg(color)));
        cells.push(Span::styled(format!("{} ", detail), Style::default().fg(color)));
        cells.push(Span::raw(" "));
    }

    // GATE zone
    let (gate_icon, gate_color, gate_detail) = match &state.gate_zone_status {
        GateZoneStatus::Empty => ("○", Color::DarkGray, "GATE".to_string()),
        GateZoneStatus::Authorized { tid, door_state } => {
            ("✓", Color::Green, format!("GATE:T{} {}", tid, door_state))
        }
        GateZoneStatus::Blocked { tid } => ("⚠", Color::Red, format!("T{} UNAUTH", tid)),
        GateZoneStatus::Exiting { tid, door_state } => {
            let (icon, color) = if door_state == "cmd_dropped" {
                ("⚠", Color::Red)
            } else {
                ("→", Color::Cyan)
            };
            (icon, color, format!("GATE:T{} {}", tid, door_state))
        }
    };

    cells.push(Span::styled(format!("{} ", gate_icon), Style::default().fg(gate_color)));
    cells.push(Span::styled(gate_detail, Style::default().fg(gate_color)));

    let zones_para = Paragraph::new(Line::from(cells)).block(
        Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Blue)),
    );

    f.render_widget(zones_para, area);
}

fn draw_live_counters(f: &mut Frame, area: Rect, state: &DashboardState) {
    let counters = Paragraph::new(Line::from(vec![
        Span::styled("LIVE ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw("│ Entries: "),
        Span::styled(format!("{}", state.counters.entries), Style::default().fg(Color::Green)),
        Span::raw(" │ Exits: "),
        Span::styled(format!("{}", state.counters.exits), Style::default().fg(Color::Yellow)),
        Span::raw(" │ Gate Opens: "),
        Span::styled(format!("{}", state.counters.gate_opens), Style::default().fg(Color::Cyan)),
        Span::raw(" │ POS Occ: "),
        Span::styled(
            format!("{}", state.pos_occupancy_count()),
            Style::default().fg(Color::Magenta),
        ),
        Span::raw(" │ Events/s: "),
        Span::styled(
            format!("{:.1}", state.throughput.current()),
            Style::default().fg(Color::Blue),
        ),
    ]))
    .block(Block::default().borders(Borders::ALL));

    f.render_widget(counters, area);
}

fn draw_event_feeds(f: &mut Frame, area: Rect, state: &DashboardState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25), // Track
            Constraint::Percentage(25), // Gate
            Constraint::Percentage(25), // ACC
            Constraint::Percentage(25), // Stitch/Lost
        ])
        .split(area);

    draw_track_feed(f, chunks[0], state);
    draw_gate_feed(f, chunks[1], state);
    draw_acc_feed(f, chunks[2], state);
    draw_stitch_feed(f, chunks[3], state);
}

fn draw_track_feed(f: &mut Frame, area: Rect, state: &DashboardState) {
    let items: Vec<ListItem> = state
        .track_events
        .iter()
        .map(|e| {
            let (icon, color) = match e.t.as_str() {
                "create" => ("+", Color::Green),
                "stitch" => ("↔", Color::Cyan),
                "pending" => ("?", Color::Yellow),
                "reentry" => ("↩", Color::Magenta),
                "lost" => ("✗", Color::Red),
                _ => ("·", Color::White),
            };

            let auth_icon = if e.auth { "A" } else { " " };

            ListItem::new(Line::from(vec![
                Span::styled(icon, Style::default().fg(color)),
                Span::raw(format!(" T{:<4} ", e.tid)),
                Span::styled(
                    auth_icon,
                    Style::default().fg(if e.auth { Color::Green } else { Color::DarkGray }),
                ),
                Span::raw(format!(" {:.1}s", e.dwell_ms as f64 / 1000.0)),
            ]))
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .title(" Track Feed ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );

    f.render_widget(list, area);
}

fn draw_gate_feed(f: &mut Frame, area: Rect, state: &DashboardState) {
    // Split for timing + events
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5), // Timing
            Constraint::Min(0),    // Events
        ])
        .split(area);

    // Gate timing summary
    let flow = if state.current_gate_flow.cmd_sent_received.is_some() {
        &state.current_gate_flow
    } else if let Some(ref last) = state.last_gate_flow {
        if last.cmd_sent_received.is_some() {
            last
        } else {
            &state.current_gate_flow
        }
    } else {
        &state.current_gate_flow
    };

    let tid_str = flow.tid.map(|t| format!("T{}", t)).unwrap_or("-".to_string());

    // Build timing breakdown lines
    let entry_to_cmd =
        flow.entry_to_cmd_ms().map(|ms| format!("{}ms", ms)).unwrap_or("-".to_string());
    let cmd_to_moving =
        flow.cmd_to_moving_ms().map(|ms| format!("{}ms", ms)).unwrap_or("-".to_string());
    let moving_to_open =
        flow.moving_to_open_ms().map(|ms| format!("{}ms", ms)).unwrap_or("-".to_string());
    let total =
        flow.total_entry_to_open_ms().map(|ms| format!("{}ms", ms)).unwrap_or("-".to_string());

    let timing = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(tid_str, Style::default().fg(Color::Cyan)),
            Span::raw(format!(" Cmds:{}", state.gate_cmd_count)),
        ]),
        Line::from(vec![
            Span::styled("Entry→Cmd: ", Style::default().fg(Color::DarkGray)),
            Span::styled(entry_to_cmd, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::styled("Cmd→Move:  ", Style::default().fg(Color::DarkGray)),
            Span::styled(cmd_to_moving, Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::styled("Move→Open: ", Style::default().fg(Color::DarkGray)),
            Span::styled(moving_to_open, Style::default().fg(Color::Green)),
        ]),
        Line::from(vec![
            Span::styled(
                "TOTAL: ",
                Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
            ),
            Span::styled(total, Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
        ]),
    ])
    .block(
        Block::default()
            .title(format!(" Gate: {} ", state.last_gate_state.to_uppercase()))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Magenta)),
    );

    f.render_widget(timing, chunks[0]);

    // Gate events
    let events: Vec<ListItem> = state
        .gate_events
        .iter()
        .map(|e| {
            let color = match e.state.as_str() {
                "open" => Color::Green,
                "closed" => Color::DarkGray,
                "moving" => Color::Yellow,
                "cmd_sent" => Color::Cyan,
                "blocked" => Color::Red,
                _ => Color::White,
            };
            let tid = e.tid.map(|t| format!("T{}", t)).unwrap_or("-".to_string());

            // Show timing for cmd_sent events if available
            let timing = if e.state == "cmd_sent" {
                e.enqueue_to_send_us.map(|us| format!(" {}µs", us)).unwrap_or_default()
            } else {
                String::new()
            };

            ListItem::new(Line::from(vec![
                Span::styled(format!("{:<8}", e.state), Style::default().fg(color)),
                Span::raw(format!("{}{}", tid, timing)),
            ]))
        })
        .collect();

    let list = List::new(events).block(
        Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Magenta)),
    );

    f.render_widget(list, chunks[1]);
}

fn draw_acc_feed(f: &mut Frame, area: Rect, state: &DashboardState) {
    // Split for stats + events
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Stats
            Constraint::Min(0),    // Events
        ])
        .split(area);

    // ACC stats
    let stats = Paragraph::new(Line::from(vec![
        Span::styled("✓", Style::default().fg(Color::Green)),
        Span::styled(
            format!("{} ", state.acc_stats.matched_count),
            Style::default().fg(Color::Green),
        ),
        Span::styled("✗", Style::default().fg(Color::Red)),
        Span::styled(
            format!("{} ", state.acc_stats.orphaned_count),
            Style::default().fg(Color::Red),
        ),
        Span::raw(format!("({:.0}%)", state.acc_stats.match_rate())),
    ]))
    .block(
        Block::default()
            .title(" ACC ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow)),
    );

    f.render_widget(stats, chunks[0]);

    // ACC events
    let events: Vec<ListItem> = state
        .acc_events
        .iter()
        .map(|e| {
            let (icon, color) = match e.t.as_str() {
                "matched" => ("✓", Color::Green),
                "unmatched" => ("✗", Color::Red),
                "late_after_gate" => ("!", Color::Yellow),
                "matched_no_journey" => ("?", Color::Magenta),
                _ => ("·", Color::White),
            };

            let pos = e.pos.as_deref().unwrap_or("-");
            let tid = e.tid.map(|t| format!("T{}", t)).unwrap_or("-".to_string());

            ListItem::new(Line::from(vec![
                Span::styled(icon, Style::default().fg(color)),
                Span::raw(format!(" {} {}", pos, tid)),
            ]))
        })
        .collect();

    let list = List::new(events).block(
        Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Yellow)),
    );

    f.render_widget(list, chunks[1]);
}

fn draw_stitch_feed(f: &mut Frame, area: Rect, state: &DashboardState) {
    let items: Vec<ListItem> = state
        .stitch_events
        .iter()
        .map(|e| {
            let (icon, color) = match e.t.as_str() {
                "stitch" => ("↔", Color::Cyan),
                "pending" => ("?", Color::Yellow),
                "lost" => ("✗", Color::Red),
                _ => ("·", Color::DarkGray),
            };

            let extra = match e.t.as_str() {
                "stitch" => {
                    let prev = e.prev_tid.map(|p| format!("←T{}", p)).unwrap_or_default();
                    let dist = e.stitch_dist_cm.map(|d| format!(" {}cm", d)).unwrap_or_default();
                    format!("{}{}", prev, dist)
                }
                "lost" | "pending" => {
                    format!("{:.1}s", e.dwell_ms as f64 / 1000.0)
                }
                _ => String::new(),
            };

            ListItem::new(Line::from(vec![
                Span::styled(icon, Style::default().fg(color)),
                Span::raw(format!(" T{:<4} ", e.tid)),
                Span::styled(extra, Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .title(" Stitch/Lost ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Magenta)),
    );

    f.render_widget(list, area);
}

fn draw_issues_panel(f: &mut Frame, area: Rect, state: &DashboardState) {
    let items: Vec<ListItem> = state
        .issues
        .iter()
        .map(|issue| {
            let (icon, color) = match issue.severity {
                IssueSeverity::Warning => ("⚠", Color::Yellow),
                IssueSeverity::Error => ("✗", Color::Red),
            };

            let age = issue.timestamp.elapsed().as_secs();
            let age_str = if age < 60 { format!("{}s", age) } else { format!("{}m", age / 60) };

            ListItem::new(Line::from(vec![
                Span::styled(format!("{} ", icon), Style::default().fg(color)),
                Span::styled(format!("{:>4} ", age_str), Style::default().fg(Color::DarkGray)),
                Span::styled(&issue.message, Style::default().fg(color)),
            ]))
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .title(" Issues ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Red)),
    );

    f.render_widget(list, area);
}

// ============================================================================
// Performance View Components
// ============================================================================

fn draw_gate_timing_panel(f: &mut Frame, area: Rect, state: &DashboardState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ])
        .split(area);

    draw_latency_histogram(
        f,
        chunks[0],
        "Entry→Cmd",
        &state.gate_timing.entry_to_cmd,
        "ms",
        Color::Cyan,
    );
    draw_latency_histogram(
        f,
        chunks[1],
        "Cmd→Moving",
        &state.gate_timing.cmd_to_moving,
        "ms",
        Color::Yellow,
    );
    draw_latency_histogram(
        f,
        chunks[2],
        "Moving→Open",
        &state.gate_timing.moving_to_open,
        "ms",
        Color::Green,
    );
    draw_latency_histogram(
        f,
        chunks[3],
        "TOTAL",
        &state.gate_timing.total_entry_to_open,
        "ms",
        Color::Magenta,
    );
}

fn draw_latency_histogram(
    f: &mut Frame,
    area: Rect,
    title: &str,
    stats: &LatencyStats,
    unit: &str,
    color: Color,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(4),    // Histogram
            Constraint::Length(4), // Stats
        ])
        .split(area);

    // Draw histogram
    let histogram = stats.histogram(8);

    let data: Vec<(&str, u64)> = histogram
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            // We need static strings for the labels
            let labels = ["", "", "", "", "", "", "", ""];
            (labels[i], v)
        })
        .collect();

    let bar_chart = BarChart::default()
        .block(
            Block::default()
                .title(format!(" {} ", title))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(color)),
        )
        .data(&data)
        .bar_width(2)
        .bar_gap(1)
        .bar_style(Style::default().fg(color))
        .value_style(Style::default().fg(Color::DarkGray));

    f.render_widget(bar_chart, chunks[0]);

    // Draw stats
    let min_str = stats.min.map(|v| format!("{}", v)).unwrap_or("-".to_string());
    let max_str = stats.max.map(|v| format!("{}", v)).unwrap_or("-".to_string());
    let avg_str = stats.avg().map(|v| format!("{:.0}", v)).unwrap_or("-".to_string());
    let p95_str = stats.p95().map(|v| format!("{}", v)).unwrap_or("-".to_string());

    let stats_text = Paragraph::new(vec![
        Line::from(format!("min:{} max:{}", min_str, max_str)),
        Line::from(format!("avg:{} p95:{} {}", avg_str, p95_str, unit)),
        Line::from(format!("n={}", stats.count())),
    ])
    .block(
        Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::DarkGray)),
    );

    f.render_widget(stats_text, chunks[1]);
}

fn draw_processing_panels(f: &mut Frame, area: Rect, state: &DashboardState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(50), // Track processing
            Constraint::Percentage(50), // ACC correlation
        ])
        .split(area);

    // Track processing panel
    let track_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[0]);

    draw_latency_histogram(
        f,
        track_chunks[0],
        "Network",
        &state.track_processing.network_latency,
        "ms",
        Color::Blue,
    );
    draw_latency_histogram(
        f,
        track_chunks[1],
        "Process",
        &state.track_processing.processing_latency,
        "ms",
        Color::Cyan,
    );

    // ACC correlation panel
    let acc_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(60), // Histogram
            Constraint::Percentage(40), // Stats box
        ])
        .split(chunks[1]);

    draw_latency_histogram(
        f,
        acc_chunks[0],
        "ACC Match",
        &state.acc_stats.match_latency,
        "ms",
        Color::Yellow,
    );

    // ACC stats box
    let match_rate = state.acc_stats.match_rate();
    let rate_color = if match_rate > 90.0 {
        Color::Green
    } else if match_rate > 70.0 {
        Color::Yellow
    } else {
        Color::Red
    };

    let acc_stats = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("✓ Matched: ", Style::default().fg(Color::Green)),
            Span::raw(format!("{}", state.acc_stats.matched_count)),
        ]),
        Line::from(vec![
            Span::styled("✗ Orphaned: ", Style::default().fg(Color::Red)),
            Span::raw(format!("{}", state.acc_stats.orphaned_count)),
        ]),
        Line::from(vec![
            Span::styled("! Late: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{}", state.acc_stats.late_count)),
        ]),
        Line::from(vec![
            Span::raw("Rate: "),
            Span::styled(
                format!("{:.1}%", match_rate),
                Style::default().fg(rate_color).add_modifier(Modifier::BOLD),
            ),
        ]),
    ])
    .block(
        Block::default()
            .title(" ACC Status ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow)),
    );

    f.render_widget(acc_stats, acc_chunks[1]);
}

fn draw_bottom_panels(f: &mut Frame, area: Rect, state: &DashboardState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(40), // Stitching
            Constraint::Percentage(60), // Throughput
        ])
        .split(area);

    // Stitching panel
    let success_rate = state.stitching_stats.success_rate();
    let rate_color = if success_rate > 80.0 {
        Color::Green
    } else if success_rate > 60.0 {
        Color::Yellow
    } else {
        Color::Red
    };

    let avg_dist = state
        .stitching_stats
        .stitch_distance
        .avg()
        .map(|v| format!("{:.1}cm", v))
        .unwrap_or("-".to_string());
    let avg_time = state
        .stitching_stats
        .stitch_time
        .avg()
        .map(|v| format!("{:.1}s", v / 1000.0))
        .unwrap_or("-".to_string());

    let stitch_stats = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("↔ Stitched: ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{}", state.stitching_stats.success_count)),
            Span::raw("  "),
            Span::styled("✗ Lost: ", Style::default().fg(Color::Red)),
            Span::raw(format!("{}", state.stitching_stats.lost_count)),
        ]),
        Line::from(vec![
            Span::raw("Success: "),
            Span::styled(
                format!("{:.0}%", success_rate),
                Style::default().fg(rate_color).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(format!("Avg dist: {}  time: {}", avg_dist, avg_time)),
    ])
    .block(
        Block::default()
            .title(" Stitching ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Magenta)),
    );

    f.render_widget(stitch_stats, chunks[0]);

    // Throughput panel
    let session_dur = state.throughput.session_duration();
    let hours = session_dur.as_secs() / 3600;
    let mins = (session_dur.as_secs() % 3600) / 60;
    let session_str =
        if hours > 0 { format!("{}h {}m", hours, mins) } else { format!("{}m", mins) };

    // Manual test results
    let _manual_test_str = if state.manual_test.last_total.is_some() {
        format!(
            "Manual: cmd→mov:{}ms mov→opn:{}ms TOTAL:{}ms",
            state.manual_test.last_cmd_to_moving.unwrap_or(0),
            state.manual_test.last_moving_to_open.unwrap_or(0),
            state.manual_test.last_total.unwrap_or(0)
        )
    } else {
        "Manual: press 'o' to test".to_string()
    };

    let throughput_stats = Paragraph::new(vec![
        Line::from(vec![
            Span::raw("Events/s: "),
            Span::styled(
                format!("{:.1}", state.throughput.current()),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw(format!(
                "  avg:{:.1}  peak:{:.1}",
                state.throughput.avg(),
                state.throughput.peak_events_per_sec
            )),
        ]),
        Line::from(format!(
            "Session: {}  Total: {} events",
            session_str, state.throughput.total_events
        )),
    ])
    .block(
        Block::default()
            .title(" Throughput ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Blue)),
    );

    f.render_widget(throughput_stats, chunks[1]);
}
