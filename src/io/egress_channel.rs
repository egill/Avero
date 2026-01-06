//! Typed channel for MQTT egress messages
//!
//! Provides a non-blocking way to send events to the MQTT publisher.
//! Uses bounded mpsc channels to prevent unbounded memory growth.

use crate::domain::journey::{epoch_ms, Journey};
use crate::infra::metrics::MetricsSummary;
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
    /// Track ID from Xovis
    pub tid: i32,
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
}

/// Payload for metrics snapshot
#[derive(Debug, Serialize)]
pub struct MetricsPayload {
    /// Timestamp (epoch ms)
    pub ts: u64,
    /// Total events processed
    pub events_total: u64,
    /// Events per second
    pub events_per_sec: f64,
    /// Average processing latency (microseconds)
    pub avg_latency_us: u64,
    /// Max processing latency (microseconds)
    pub max_latency_us: u64,
    /// Current active tracks
    pub active_tracks: usize,
    /// Authorized tracks
    pub authorized_tracks: usize,
    /// Total gate commands sent
    pub gate_cmds: u64,
}

impl From<MetricsSummary> for MetricsPayload {
    fn from(summary: MetricsSummary) -> Self {
        Self {
            ts: epoch_ms(),
            events_total: summary.events_total,
            events_per_sec: summary.events_per_sec,
            avg_latency_us: summary.avg_process_latency_us,
            max_latency_us: summary.max_process_latency_us,
            active_tracks: summary.active_tracks,
            authorized_tracks: summary.authorized_tracks,
            gate_cmds: summary.gate_commands_sent,
        }
    }
}

/// Payload for gate state changes
#[derive(Debug, Clone, Serialize)]
pub struct GateStatePayload {
    /// Timestamp (epoch ms)
    pub ts: u64,
    /// Gate state (cmd_sent, open, closed, moving)
    pub state: String,
    /// Associated track ID (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tid: Option<i32>,
    /// Source of the state change (rs485, tcp, cmd)
    pub src: String,
}

/// Payload for track lifecycle events
#[derive(Debug, Clone, Serialize)]
pub struct TrackEventPayload {
    /// Timestamp (epoch ms)
    pub ts: u64,
    /// Event type: create, delete, stitch, lost, reentry
    pub t: String,
    /// Primary track ID
    pub tid: i32,
    /// Previous track ID (for stitch events)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev_tid: Option<i32>,
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

/// Payload for ACC (payment terminal) events
#[derive(Debug, Clone, Serialize)]
pub struct AccEventPayload {
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
    pub tid: Option<i32>,
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
}

/// Sender handle for egress messages
///
/// Clone this to share across multiple producers.
/// Non-blocking - if the channel is full, messages are dropped.
#[derive(Clone)]
pub struct EgressSender {
    tx: mpsc::Sender<EgressMessage>,
}

impl EgressSender {
    /// Create a new sender from an mpsc sender
    pub fn new(tx: mpsc::Sender<EgressMessage>) -> Self {
        Self { tx }
    }

    /// Send a completed journey for publishing
    pub fn send_journey(&self, journey: &Journey) {
        let json = journey.to_json();
        let payload = JourneyPayload { json };
        // Use try_send to avoid blocking - drop if channel full
        let _ = self.tx.try_send(EgressMessage::Journey(payload));
    }

    /// Send a zone event for live display
    pub fn send_zone_event(&self, payload: ZoneEventPayload) {
        let _ = self.tx.try_send(EgressMessage::ZoneEvent(payload));
    }

    /// Send a metrics snapshot
    pub fn send_metrics(&self, summary: MetricsSummary) {
        let payload = MetricsPayload::from(summary);
        let _ = self.tx.try_send(EgressMessage::Metrics(payload));
    }

    /// Send a gate state change
    pub fn send_gate_state(&self, payload: GateStatePayload) {
        let _ = self.tx.try_send(EgressMessage::GateState(payload));
    }

    /// Send a track lifecycle event
    pub fn send_track_event(&self, payload: TrackEventPayload) {
        let _ = self.tx.try_send(EgressMessage::TrackEvent(payload));
    }

    /// Send an ACC (payment terminal) event
    pub fn send_acc_event(&self, payload: AccEventPayload) {
        let _ = self.tx.try_send(EgressMessage::AccEvent(payload));
    }
}

/// Create a new egress channel pair
///
/// Returns (sender, receiver) where sender can be cloned and shared.
/// Buffer size determines how many messages can be queued.
pub fn create_egress_channel(buffer_size: usize) -> (EgressSender, mpsc::Receiver<EgressMessage>) {
    let (tx, rx) = mpsc::channel(buffer_size);
    (EgressSender::new(tx), rx)
}
