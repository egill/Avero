//! MQTT client for receiving Xovis sensor data

use crate::domain::types::{
    EventAttributes, EventType, Frame, GeometryId, ParsedEvent, TimestampValue, TrackId,
    XovisMessage,
};
use crate::infra::config::Config;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use std::time::{Duration, Instant};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, warn};

/// Start the MQTT client and send parsed events to the channel
pub async fn start_mqtt_client(
    config: &Config,
    event_tx: mpsc::Sender<ParsedEvent>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut mqttoptions = MqttOptions::new("gateway-poc", config.mqtt_host(), config.mqtt_port());
    mqttoptions.set_keep_alive(Duration::from_secs(30));

    // Set credentials if configured
    if let (Some(username), Some(password)) = (config.mqtt_username(), config.mqtt_password()) {
        mqttoptions.set_credentials(username, password);
    }

    let (client, mut eventloop) = AsyncClient::new(mqttoptions, 100);
    client
        .subscribe(config.mqtt_topic(), QoS::AtMostOnce)
        .await?;

    info!(topic = %config.mqtt_topic(), host = %config.mqtt_host(), port = %config.mqtt_port(), "MQTT client subscribed");

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
                                let events = parse_xovis_message(json_str, received_at);
                                // Only log if there are actual events (not just position updates)
                                if !events.is_empty() {
                                    debug!(topic = %topic, event_count = %events.len(), "MQTT message with events");
                                }
                                for event in events {
                                    debug!(track_id = %event.track_id, event_type = ?event.event_type, "Parsed event");
                                    if event_tx.send(event).await.is_err() {
                                        warn!("Event channel closed");
                                        return Ok(());
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
    let mut parsed_events = Vec::with_capacity(8);

    let message: XovisMessage = match serde_json::from_str(json_str) {
        Ok(m) => m,
        Err(e) => {
            debug!(error = %e, "Failed to parse Xovis message");
            return parsed_events;
        }
    };

    let Some(live_data) = message.live_data else {
        return parsed_events;
    };

    for frame in live_data.frames {
        parsed_events.extend(parse_frame(&frame, received_at));
    }

    parsed_events
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

    // Build position map from tracked_objects
    let mut positions: std::collections::HashMap<i64, [f64; 3]> = std::collections::HashMap::new();
    for obj in &frame.tracked_objects {
        if obj.position.len() >= 3 {
            positions.insert(
                obj.track_id,
                [obj.position[0], obj.position[1], obj.position[2]],
            );
        }
    }

    // Extract event time from frame timestamp (handles both ISO string and epoch ms)
    let event_time = timestamp_to_epoch_ms(&frame.time);

    for xovis_event in &frame.events {
        let event_type: EventType = xovis_event.event_type.parse().unwrap();

        let attrs = xovis_event.attributes.as_ref().unwrap_or(&EventAttributes {
            track_id: None,
            geometry_id: None,
            direction: None,
        });

        if let Some(track_id) = attrs.track_id {
            // Get position for this track if available
            let position = positions.get(&track_id).copied();
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
        assert!(
            events[0].event_time > 0,
            "event_time should be parsed from ISO timestamp"
        );
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
