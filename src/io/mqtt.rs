//! MQTT client for receiving Xovis sensor data

use crate::domain::journey::epoch_ms;
use crate::domain::types::{
    EventType, Frame, GeometryId, ParsedEvent, TimestampValue, TrackId, TrackedObject, XovisMessage,
};
use crate::infra::config::Config;
use crate::infra::metrics::Metrics;
use crate::io::analysis_logger::AnalysisLogger;
use crate::io::egress_channel::{EgressSender, PositionPayload};
use parking_lot::Mutex;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use rustc_hash::FxHashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, warn};

/// Bit flag for Xovis GROUP tracks (high bit set)
const XOVIS_GROUP_BIT: i64 = 0x80000000;

/// Throttles position updates to reduce bandwidth
/// Only publishes when:
/// - At least 100ms since last publish
/// - Position has moved more than 5cm
struct PositionThrottler {
    /// Last published position per track_id [x, y, z]
    last_positions: FxHashMap<i64, [f64; 3]>,
    /// Last publish time
    last_publish: Instant,
    /// Minimum interval between publishes (100ms)
    min_interval: Duration,
    /// Minimum distance to trigger publish (5cm = 0.05m)
    min_distance: f64,
}

impl PositionThrottler {
    fn new() -> Self {
        Self {
            last_positions: FxHashMap::default(),
            last_publish: Instant::now() - Duration::from_millis(200), // Allow immediate first publish
            min_interval: Duration::from_millis(100),
            min_distance: 0.05, // 5cm
        }
    }

    /// Check if enough time has passed for a new batch
    fn should_publish_batch(&self) -> bool {
        self.last_publish.elapsed() >= self.min_interval
    }

    /// Mark batch as published
    fn mark_published(&mut self) {
        self.last_publish = Instant::now();
    }

    /// Check if a specific track has moved enough to publish
    /// Returns true if the track should be published
    fn should_publish_track(&mut self, track_id: i64, pos: [f64; 3]) -> bool {
        if let Some(last_pos) = self.last_positions.get(&track_id) {
            let dx = pos[0] - last_pos[0];
            let dy = pos[1] - last_pos[1];
            let distance = (dx * dx + dy * dy).sqrt();
            if distance < self.min_distance {
                return false;
            }
        }
        // Update last position
        self.last_positions.insert(track_id, pos);
        true
    }

    /// Remove stale tracks (not seen in a while)
    fn cleanup_stale(&mut self, active_track_ids: &[i64]) {
        self.last_positions.retain(|tid, _| active_track_ids.contains(tid));
    }
}

/// Start the MQTT client and send parsed events to the channel
///
/// Events are sent via try_send to avoid blocking the MQTT eventloop.
/// Dropped events are counted in metrics and logged (rate-limited).
/// If egress_sender is provided, position updates are streamed at 10Hz with 5cm threshold.
pub async fn start_mqtt_client(
    config: &Config,
    event_tx: mpsc::Sender<ParsedEvent>,
    metrics: Arc<Metrics>,
    mut shutdown: watch::Receiver<bool>,
    analysis_logger: Option<Arc<Mutex<AnalysisLogger>>>,
    egress_sender: Option<EgressSender>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut mqttoptions = MqttOptions::new("gateway-poc", config.mqtt_host(), config.mqtt_port());
    mqttoptions.set_keep_alive(Duration::from_secs(30));

    // Set credentials if configured
    if let (Some(username), Some(password)) = (config.mqtt_username(), config.mqtt_password()) {
        mqttoptions.set_credentials(username, password);
    }

    let (client, mut eventloop) = AsyncClient::new(mqttoptions, 100);
    client.subscribe(config.mqtt_topic(), QoS::AtMostOnce).await?;

    info!(topic = %config.mqtt_topic(), host = %config.mqtt_host(), port = %config.mqtt_port(), "MQTT client subscribed");

    // Rate-limit drop warnings to 1 per second
    let mut last_drop_warn = Instant::now() - Duration::from_secs(2);

    // Position throttler for streaming positions at 10Hz with 5cm threshold
    let mut position_throttler = PositionThrottler::new();

    loop {
        tokio::select! {
            // Check for shutdown signal
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    info!("mqtt_shutdown");
                    return Ok(());
                }
            }
            // Process MQTT events
            result = eventloop.poll() => {
                match result {
                    Ok(Event::Incoming(Packet::Publish(publish))) => {
                        let received_at = Instant::now();
                        let topic = &publish.topic;
                        let payload = std::str::from_utf8(&publish.payload);

                        match payload {
                            Ok(json_str) => {
                                // Log to analysis file if enabled
                                if let Some(ref logger) = analysis_logger {
                                    // Try to parse for ts_event extraction
                                    let parsed: Option<serde_json::Value> =
                                        serde_json::from_str(json_str).ok();
                                    logger.lock().log_mqtt(topic, json_str, parsed.as_ref());
                                }

                                // Parse and extract tracked objects for position streaming
                                let (events, tracked_objects) = parse_xovis_message_with_positions(json_str, received_at);

                                // Stream positions if egress sender is available and throttle allows
                                if let Some(ref sender) = egress_sender {
                                    if position_throttler.should_publish_batch() && !tracked_objects.is_empty() {
                                        let ts = epoch_ms();
                                        let mut published_any = false;

                                        for obj in &tracked_objects {
                                            // Skip GROUP tracks (high bit set)
                                            if obj.track_id & XOVIS_GROUP_BIT != 0 {
                                                continue;
                                            }
                                            if obj.position.len() >= 3 {
                                                let pos = [obj.position[0], obj.position[1], obj.position[2]];
                                                if position_throttler.should_publish_track(obj.track_id, pos) {
                                                    sender.send_position(PositionPayload {
                                                        site: None,
                                                        ts,
                                                        tid: obj.track_id,
                                                        obj_type: obj.obj_type.clone(),
                                                        x: pos[0],
                                                        y: pos[1],
                                                        z: pos[2],
                                                        zone: None,
                                                        auth: false,
                                                        ctx: Some("continuous".to_string()),
                                                    });
                                                    published_any = true;
                                                }
                                            }
                                        }

                                        if published_any {
                                            position_throttler.mark_published();
                                        }

                                        // Cleanup stale tracks periodically
                                        let active_ids: Vec<i64> = tracked_objects.iter().map(|o| o.track_id).collect();
                                        position_throttler.cleanup_stale(&active_ids);
                                    }
                                }

                                if !events.is_empty() {
                                    debug!(topic = %topic, event_count = %events.len(), "MQTT message with events");
                                }
                                for event in events {
                                    // Skip GROUP tracks (high bit set) - same person as base track
                                    if event.track_id.0 & XOVIS_GROUP_BIT != 0 {
                                        continue;
                                    }
                                    debug!(track_id = %event.track_id, event_type = ?event.event_type, "Parsed event");
                                    metrics.record_mqtt_event_received();
                                    if let Err(e) = event_tx.try_send(event) {
                                        match e {
                                            TrySendError::Full(_) => {
                                                metrics.record_mqtt_event_dropped();
                                                if last_drop_warn.elapsed() > Duration::from_secs(1) {
                                                    warn!("mqtt_event_dropped: channel full");
                                                    last_drop_warn = Instant::now();
                                                }
                                            }
                                            TrySendError::Closed(_) => {
                                                warn!("Event channel closed");
                                                return Ok(());
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                warn!(error = %e, "Invalid UTF-8 in MQTT payload");
                            }
                        }
                    }
                    Ok(Event::Incoming(Packet::ConnAck(_))) => {
                        info!("MQTT connected");
                    }
                    Ok(_) => {}
                    Err(e) => {
                        error!(error = %e, "MQTT error");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        }
    }
}

/// Parse a Xovis JSON message and extract events
pub fn parse_xovis_message(json_str: &str, received_at: Instant) -> Vec<ParsedEvent> {
    let (events, _) = parse_xovis_message_with_positions(json_str, received_at);
    events
}

/// Parse a Xovis JSON message and extract both events and tracked objects
/// Returns (events, tracked_objects) for position streaming
fn parse_xovis_message_with_positions(
    json_str: &str,
    received_at: Instant,
) -> (Vec<ParsedEvent>, Vec<TrackedObject>) {
    let mut parsed_events = Vec::with_capacity(8);
    let mut all_tracked_objects = Vec::with_capacity(16);

    let message: XovisMessage = match serde_json::from_str(json_str) {
        Ok(m) => m,
        Err(e) => {
            debug!(error = %e, "Failed to parse Xovis message");
            return (parsed_events, all_tracked_objects);
        }
    };

    let Some(live_data) = message.live_data else {
        return (parsed_events, all_tracked_objects);
    };

    for frame in live_data.frames {
        parsed_events.extend(parse_frame(&frame, received_at));
        // Collect all tracked objects from all frames
        all_tracked_objects.extend(frame.tracked_objects.into_iter());
    }

    (parsed_events, all_tracked_objects)
}

/// Parse ISO 8601 timestamp to epoch milliseconds
fn parse_iso_time(time_str: &str) -> Option<u64> {
    // Parse "2026-01-05T16:41:30.048+00:00" format (RFC 3339)
    OffsetDateTime::parse(time_str, &Rfc3339)
        .ok()
        .map(|dt| (dt.unix_timestamp_nanos() / 1_000_000) as u64)
}

/// Extract epoch milliseconds from TimestampValue
fn timestamp_to_epoch_ms(ts: &TimestampValue) -> u64 {
    match ts {
        TimestampValue::EpochMs(ms) => *ms,
        TimestampValue::IsoString(s) => parse_iso_time(s).unwrap_or(0),
        TimestampValue::None => 0,
    }
}

fn parse_frame(frame: &Frame, received_at: Instant) -> Vec<ParsedEvent> {
    let mut events = Vec::with_capacity(8);

    // Extract event time from frame timestamp (handles both ISO string and epoch ms)
    let event_time = timestamp_to_epoch_ms(&frame.time);

    for xovis_event in &frame.events {
        let event_type: EventType = xovis_event.event_type.parse().unwrap();

        let Some(attrs) = xovis_event.attributes.as_ref() else { continue };
        let Some(track_id) = attrs.track_id else { continue };

        // Linear search for position - frames typically have <10 tracked objects
        let position = frame
            .tracked_objects
            .iter()
            .find(|obj| obj.track_id == track_id)
            .filter(|obj| obj.position.len() >= 3)
            .map(|obj| [obj.position[0], obj.position[1], obj.position[2]]);

        events.push(ParsedEvent {
            event_type,
            track_id: TrackId(track_id),
            geometry_id: attrs.geometry_id.map(GeometryId),
            direction: attrs.direction.clone(),
            event_time,
            received_at,
            position,
        });
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_zone_entry() {
        let json = r#"{
            "live_data": {
                "frames": [{
                    "time": "2026-01-05T16:41:30.048+00:00",
                    "tracked_objects": [{
                        "track_id": 123,
                        "type": "PERSON",
                        "position": [1.5, 2.0, 1.7]
                    }],
                    "events": [{
                        "type": "ZONE_ENTRY",
                        "attributes": {
                            "track_id": 123,
                            "geometry_id": 1001
                        }
                    }]
                }]
            }
        }"#;

        let events = parse_xovis_message(json, Instant::now());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].track_id, TrackId(123));
        assert_eq!(events[0].event_type, EventType::ZoneEntry);
        assert_eq!(events[0].geometry_id, Some(GeometryId(1001)));
        // event_time should now be parsed from ISO 8601
        assert!(events[0].event_time > 0, "event_time should be parsed from ISO timestamp");
        // Position should be extracted
        assert_eq!(events[0].position, Some([1.5, 2.0, 1.7]));
    }

    #[test]
    fn test_parse_track_create() {
        let json = r#"{
            "live_data": {
                "frames": [{
                    "time": "2026-01-05T16:40:00.000+00:00",
                    "events": [{
                        "type": "TRACK_CREATE",
                        "attributes": {
                            "track_id": 100
                        }
                    }]
                }]
            }
        }"#;

        let events = parse_xovis_message(json, Instant::now());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].track_id, TrackId(100));
        assert_eq!(events[0].event_type, EventType::TrackCreate);
    }

    #[test]
    fn test_parse_line_cross() {
        let json = r#"{
            "live_data": {
                "frames": [{
                    "time": "2026-01-05T16:42:00.000+00:00",
                    "events": [{
                        "type": "LINE_CROSS_FORWARD",
                        "attributes": {
                            "track_id": 100,
                            "geometry_id": 1006,
                            "direction": "forward"
                        }
                    }]
                }]
            }
        }"#;

        let events = parse_xovis_message(json, Instant::now());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::LineCrossForward);
        assert_eq!(events[0].geometry_id, Some(GeometryId(1006)));
    }

    #[test]
    fn test_parse_multiple_events() {
        let json = r#"{
            "live_data": {
                "frames": [{
                    "time": "2026-01-05T16:41:30.000+00:00",
                    "events": [
                        {"type": "ZONE_EXIT", "attributes": {"track_id": 100, "geometry_id": 1001}},
                        {"type": "ZONE_ENTRY", "attributes": {"track_id": 100, "geometry_id": 1007}}
                    ]
                }]
            }
        }"#;

        let events = parse_xovis_message(json, Instant::now());
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, EventType::ZoneExit);
        assert_eq!(events[1].event_type, EventType::ZoneEntry);
    }

    #[test]
    fn test_parse_invalid_json() {
        let events = parse_xovis_message("not json", Instant::now());
        assert!(events.is_empty());
    }

    #[test]
    fn test_parse_empty_frames() {
        let json = r#"{"live_data": {"frames": []}}"#;
        let events = parse_xovis_message(json, Instant::now());
        assert!(events.is_empty());
    }

    #[test]
    fn test_parse_iso_time() {
        // Test RFC 3339 format parsing
        let ts = parse_iso_time("2026-01-05T16:41:30.048+00:00");
        assert!(ts.is_some());
        let ms = ts.unwrap();
        // 2026-01-05T16:41:30.048Z should be around 1767630090048 ms
        assert!(ms > 1767000000000, "timestamp should be in 2026");
        assert!(ms < 1800000000000, "timestamp should be before 2027");

        // Test invalid input
        assert!(parse_iso_time("not a timestamp").is_none());
        assert!(parse_iso_time("").is_none());
    }
}
