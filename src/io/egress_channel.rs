//! Typed channel for MQTT egress messages
//!
//! Provides a non-blocking way to send events to the MQTT publisher.
//! Uses bounded mpsc channels to prevent unbounded memory growth.

use crate::domain::journey::{epoch_ms, Journey};
use crate::infra::metrics::{MetricsSummary, METRICS_NUM_BUCKETS};
use serde::Serialize;
use tokio::sync::mpsc;

/// Messages that can be sent to the MQTT publisher
#[derive(Debug)]
pub enum EgressMessage {
    /// Completed journey for persistence
    Journey(JourneyPayload),
    /// Live zone event for real-time display
    ZoneEvent(ZoneEventPayload),
    /// Periodic metrics snapshot
    Metrics(MetricsPayload),
    /// Gate state change
    GateState(GateStatePayload),
    /// Track lifecycle event (create, delete, stitch, lost)
    TrackEvent(TrackEventPayload),
    /// ACC (payment terminal) event
    AccEvent(AccEventPayload),
}

/// Payload for completed journeys
#[derive(Debug, Serialize)]
pub struct JourneyPayload {
    pub json: String,
}

/// Payload for live zone events
#[derive(Debug, Clone, Serialize)]
pub struct ZoneEventPayload {
    /// Site identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    /// Track ID from Xovis
    pub tid: i64,
    /// Event type (zone_entry, zone_exit, line_cross)
    pub t: String,
    /// Zone name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub z: Option<String>,
    /// Timestamp (epoch ms)
    pub ts: u64,
    /// Authorization status at time of event
    pub auth: bool,
    /// Dwell time in this zone (on exit)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dwell_ms: Option<u64>,
    /// Total accumulated dwell across all POS zones
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_dwell_ms: Option<u64>,
    /// Original event timestamp from Xovis sensor (epoch ms).
    /// Compare with `ts` to calculate sensor-to-tracker latency.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_time: Option<u64>,
}

/// Payload for metrics snapshot
#[derive(Debug, Serialize)]
pub struct MetricsPayload {
    /// Site identifier
    pub site: String,
    /// Timestamp (epoch ms)
    pub ts: u64,
    /// Current gate state (open, closed, moving, unknown)
    pub gate_state: String,
    /// Gate ID (default 1)
    pub gate_id: u32,
    /// Total events processed
    pub events_total: u64,
    /// Events per second
    pub events_per_sec: f64,
    /// Average processing latency (microseconds)
    pub avg_latency_us: u64,
    /// Max processing latency (microseconds)
    pub max_latency_us: u64,
    /// Event processing latency histogram buckets (Prometheus-style exponential)
    /// Bounds: ≤100, ≤200, ≤400, ≤800, ≤1600, ≤3200, ≤6400, ≤12800, ≤25600, ≤51200, >51200 µs
    pub lat_buckets: [u64; METRICS_NUM_BUCKETS],
    /// 50th percentile latency (µs)
    pub lat_p50_us: u64,
    /// 95th percentile latency (µs)
    pub lat_p95_us: u64,
    /// 99th percentile latency (µs)
    pub lat_p99_us: u64,
    /// Current active tracks
    pub active_tracks: usize,
    /// Authorized tracks
    pub authorized_tracks: usize,
    /// Total gate commands sent
    pub gate_cmds: u64,
    /// Gate command E2E latency histogram buckets (same bounds)
    pub gate_lat_buckets: [u64; METRICS_NUM_BUCKETS],
    /// Average gate command latency (µs)
    pub gate_lat_avg_us: u64,
    /// Max gate command latency (µs)
    pub gate_lat_max_us: u64,
    /// 99th percentile gate command latency (µs)
    pub gate_lat_p99_us: u64,
    /// Current event queue depth (snapshot)
    pub event_queue_depth: u64,
    /// Current gate command queue depth (snapshot)
    pub gate_queue_depth: u64,
    /// Current CloudPlus outbound queue depth (snapshot)
    pub cloudplus_queue_depth: u64,
    /// Event queue utilization percentage (0-100)
    pub event_queue_utilization_pct: u64,
    /// Gate queue utilization percentage (0-100)
    pub gate_queue_utilization_pct: u64,
}

impl MetricsPayload {
    /// Create a metrics payload from a summary with site and gate info
    pub fn from_summary(summary: MetricsSummary, site: String, gate_state: &str) -> Self {
        Self {
            site,
            ts: epoch_ms(),
            gate_state: gate_state.to_string(),
            gate_id: 1,
            events_total: summary.events_total,
            events_per_sec: summary.events_per_sec,
            avg_latency_us: summary.avg_process_latency_us,
            max_latency_us: summary.max_process_latency_us,
            lat_buckets: summary.lat_buckets,
            lat_p50_us: summary.lat_p50_us,
            lat_p95_us: summary.lat_p95_us,
            lat_p99_us: summary.lat_p99_us,
            active_tracks: summary.active_tracks,
            authorized_tracks: summary.authorized_tracks,
            gate_cmds: summary.gate_commands_sent,
            gate_lat_buckets: summary.gate_lat_buckets,
            gate_lat_avg_us: summary.gate_lat_avg_us,
            gate_lat_max_us: summary.gate_lat_max_us,
            gate_lat_p99_us: summary.gate_lat_p99_us,
            event_queue_depth: summary.event_queue_depth,
            gate_queue_depth: summary.gate_queue_depth,
            cloudplus_queue_depth: summary.cloudplus_queue_depth,
            event_queue_utilization_pct: summary.event_queue_utilization_pct,
            gate_queue_utilization_pct: summary.gate_queue_utilization_pct,
        }
    }
}

/// Payload for gate state changes
#[derive(Debug, Clone, Serialize)]
pub struct GateStatePayload {
    /// Site identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    /// Timestamp (epoch ms)
    pub ts: u64,
    /// Gate state (cmd_enqueued, cmd_sent, cmd_dropped, open, closed, moving)
    pub state: String,
    /// Associated track ID (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tid: Option<i64>,
    /// Source of the state change (rs485, tcp, cmd)
    pub src: String,
    /// Queue delay in microseconds (time from enqueue to processing start)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_delay_us: Option<u64>,
    /// Send latency in microseconds (time for actual network send)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_latency_us: Option<u64>,
    /// Total enqueue-to-send time in microseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enqueue_to_send_us: Option<u64>,
}

impl GateStatePayload {
    /// Create a new gate state payload without timing info
    pub fn new(ts: u64, state: &str, tid: Option<i64>, src: &str) -> Self {
        Self {
            site: None,
            ts,
            state: state.to_string(),
            tid,
            src: src.to_string(),
            queue_delay_us: None,
            send_latency_us: None,
            enqueue_to_send_us: None,
        }
    }

    /// Create a gate state payload with timing info (for cmd_sent events)
    pub fn with_timing(
        ts: u64,
        state: &str,
        tid: Option<i64>,
        src: &str,
        queue_delay_us: u64,
        send_latency_us: u64,
        enqueue_to_send_us: u64,
    ) -> Self {
        Self {
            site: None,
            ts,
            state: state.to_string(),
            tid,
            src: src.to_string(),
            queue_delay_us: Some(queue_delay_us),
            send_latency_us: Some(send_latency_us),
            enqueue_to_send_us: Some(enqueue_to_send_us),
        }
    }
}

/// Payload for track lifecycle events
#[derive(Debug, Clone, Serialize)]
pub struct TrackEventPayload {
    /// Site identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    /// Timestamp (epoch ms)
    pub ts: u64,
    /// Event type: create, delete, stitch, lost, reentry
    pub t: String,
    /// Primary track ID
    pub tid: i64,
    /// Previous track ID (for stitch events)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev_tid: Option<i64>,
    /// Authorization status
    pub auth: bool,
    /// Accumulated dwell time
    pub dwell_ms: u64,
    /// Stitch distance in cm (for stitch events)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stitch_dist_cm: Option<u64>,
    /// Stitch time gap in ms (for stitch events)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stitch_time_ms: Option<u64>,
    /// Parent journey ID (for reentry)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_jid: Option<String>,
}

/// Debug info for a track when ACC unmatched
#[derive(Debug, Clone, Serialize)]
pub struct AccDebugTrack {
    pub tid: i64,
    pub zone: Option<String>,
    pub dwell_ms: u64,
    pub auth: bool,
}

/// Debug info for recently lost/pending tracks
#[derive(Debug, Clone, Serialize)]
pub struct AccDebugPending {
    pub tid: i64,
    pub last_zone: Option<String>,
    pub dwell_ms: u64,
    pub auth: bool,
    pub pending_ms: u64,
}

/// Payload for ACC (payment terminal) events
#[derive(Debug, Clone, Serialize)]
pub struct AccEventPayload {
    /// Site identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    /// Timestamp (epoch ms)
    pub ts: u64,
    /// Event type: received, matched, unmatched
    pub t: String,
    /// Kiosk IP address
    pub ip: String,
    /// POS zone name (resolved from IP)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pos: Option<String>,
    /// Matched track ID (if matched)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tid: Option<i64>,
    /// Dwell time at match (if matched)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dwell_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate_zone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate_entry_ts: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate_cmd_at: Option<u64>,
    /// Debug: active tracks when unmatched
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_active: Option<Vec<AccDebugTrack>>,
    /// Debug: pending/lost tracks when unmatched
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_pending: Option<Vec<AccDebugPending>>,
}

/// Sender handle for egress messages
///
/// Clone this to share across multiple producers.
/// Non-blocking - if the channel is full, messages are dropped.
#[derive(Clone)]
pub struct EgressSender {
    tx: mpsc::Sender<EgressMessage>,
    site_id: String,
}

impl EgressSender {
    /// Create a new sender from an mpsc sender
    pub fn new(tx: mpsc::Sender<EgressMessage>, site_id: String) -> Self {
        Self { tx, site_id }
    }

    /// Send a completed journey for publishing
    /// Includes site_id in the JSON payload
    pub fn send_journey(&self, journey: &Journey) {
        let json = journey.to_json_with_site(&self.site_id);
        let payload = JourneyPayload { json };
        // Use try_send to avoid blocking - drop if channel full
        let _ = self.tx.try_send(EgressMessage::Journey(payload));
    }

    /// Send a zone event for live display
    /// Injects site_id into the payload
    pub fn send_zone_event(&self, mut payload: ZoneEventPayload) {
        payload.site = Some(self.site_id.clone());
        let _ = self.tx.try_send(EgressMessage::ZoneEvent(payload));
    }

    /// Send a metrics snapshot with current gate state
    pub fn send_metrics(&self, summary: MetricsSummary, gate_state: &str) {
        let payload = MetricsPayload::from_summary(summary, self.site_id.clone(), gate_state);
        let _ = self.tx.try_send(EgressMessage::Metrics(payload));
    }

    /// Send a gate state change
    /// Injects site_id into the payload
    pub fn send_gate_state(&self, mut payload: GateStatePayload) {
        payload.site = Some(self.site_id.clone());
        let _ = self.tx.try_send(EgressMessage::GateState(payload));
    }

    /// Send a track lifecycle event
    /// Injects site_id into the payload
    pub fn send_track_event(&self, mut payload: TrackEventPayload) {
        payload.site = Some(self.site_id.clone());
        let _ = self.tx.try_send(EgressMessage::TrackEvent(payload));
    }

    /// Send an ACC (payment terminal) event
    /// Injects site_id into the payload
    pub fn send_acc_event(&self, mut payload: AccEventPayload) {
        payload.site = Some(self.site_id.clone());
        let _ = self.tx.try_send(EgressMessage::AccEvent(payload));
    }
}

/// Create a new egress channel pair
///
/// Returns (sender, receiver) where sender can be cloned and shared.
/// Buffer size determines how many messages can be queued.
/// site_id is included in journey payloads for downstream consumers.
pub fn create_egress_channel(
    buffer_size: usize,
    site_id: String,
) -> (EgressSender, mpsc::Receiver<EgressMessage>) {
    let (tx, rx) = mpsc::channel(buffer_size);
    (EgressSender::new(tx, site_id), rx)
}
