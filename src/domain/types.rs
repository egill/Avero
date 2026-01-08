//! Shared types for the gateway PoC

use serde::{Deserialize, Deserializer, Serialize};
use std::time::Instant;

/// Newtype wrapper for track IDs to provide type safety
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct TrackId(pub i64);

impl std::fmt::Display for TrackId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Newtype wrapper for geometry IDs to provide type safety
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct GeometryId(pub i32);

impl std::fmt::Display for GeometryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Xovis message structure for parsing
#[derive(Debug, Deserialize)]
pub struct XovisMessage {
    pub live_data: Option<LiveData>,
}

#[derive(Debug, Deserialize)]
pub struct LiveData {
    pub frames: Vec<Frame>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Frame {
    /// Timestamp - can be ISO 8601 string or epoch milliseconds integer
    #[serde(default, deserialize_with = "deserialize_timestamp")]
    pub time: TimestampValue,
    #[serde(default)]
    pub tracked_objects: Vec<TrackedObject>,
    #[serde(default)]
    pub events: Vec<XovisEvent>,
}

/// Timestamp that can be either ISO 8601 string or epoch milliseconds
#[derive(Debug, Clone, Default)]
pub enum TimestampValue {
    #[default]
    None,
    IsoString(String),
    EpochMs(u64),
}

fn deserialize_timestamp<'de, D>(deserializer: D) -> Result<TimestampValue, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::{self, Visitor};

    struct TimestampVisitor;

    impl<'de> Visitor<'de> for TimestampVisitor {
        type Value = TimestampValue;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string or integer timestamp")
        }

        fn visit_str<E>(self, value: &str) -> Result<TimestampValue, E>
        where
            E: de::Error,
        {
            Ok(TimestampValue::IsoString(value.to_string()))
        }

        fn visit_string<E>(self, value: String) -> Result<TimestampValue, E>
        where
            E: de::Error,
        {
            Ok(TimestampValue::IsoString(value))
        }

        fn visit_u64<E>(self, value: u64) -> Result<TimestampValue, E>
        where
            E: de::Error,
        {
            Ok(TimestampValue::EpochMs(value))
        }

        fn visit_i64<E>(self, value: i64) -> Result<TimestampValue, E>
        where
            E: de::Error,
        {
            // Use TryFrom to safely convert i64 to u64, defaulting to 0 for negative values
            let epoch_ms = u64::try_from(value).unwrap_or(0);
            Ok(TimestampValue::EpochMs(epoch_ms))
        }
    }

    deserializer.deserialize_any(TimestampVisitor)
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct TrackedObject {
    pub track_id: i64,
    #[serde(rename = "type")]
    pub obj_type: String,
    #[serde(default)]
    pub position: Vec<f64>,
}

#[derive(Debug, Deserialize)]
pub struct XovisEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub attributes: Option<EventAttributes>,
}

#[derive(Debug, Deserialize)]
pub struct EventAttributes {
    pub track_id: Option<i64>,
    pub geometry_id: Option<i32>,
    pub direction: Option<String>,
}

/// Parsed event for internal processing
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ParsedEvent {
    pub event_type: EventType,
    pub track_id: TrackId,
    pub geometry_id: Option<GeometryId>,
    pub direction: Option<String>,
    pub event_time: u64,
    pub received_at: Instant,
    pub position: Option<[f64; 3]>, // [x, y, height] for stitching
}

#[derive(Debug, Clone, PartialEq)]
pub enum EventType {
    TrackCreate,
    TrackDelete,
    ZoneEntry,
    ZoneExit,
    LineCrossForward,
    LineCrossBackward,
    DoorStateChange(DoorStatus),
    /// ACC (payment terminal) event with kiosk IP
    AccEvent(String),
    Unknown(String),
}

impl std::str::FromStr for EventType {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "TRACK_CREATE" => EventType::TrackCreate,
            "TRACK_DELETE" => EventType::TrackDelete,
            "ZONE_ENTRY" => EventType::ZoneEntry,
            "ZONE_EXIT" => EventType::ZoneExit,
            "LINE_CROSS_FORWARD" => EventType::LineCrossForward,
            "LINE_CROSS_BACKWARD" => EventType::LineCrossBackward,
            other => EventType::Unknown(other.to_string()),
        })
    }
}

impl EventType {
    #[allow(dead_code)]
    pub fn as_str(&self) -> &str {
        match self {
            EventType::TrackCreate => "track_create",
            EventType::TrackDelete => "track_delete",
            EventType::ZoneEntry => "zone_entry",
            EventType::ZoneExit => "zone_exit",
            EventType::LineCrossForward => "line_cross_forward",
            EventType::LineCrossBackward => "line_cross_backward",
            EventType::DoorStateChange(_) => "door_state_change",
            EventType::AccEvent(_) => "acc_event",
            EventType::Unknown(s) => s,
        }
    }
}

/// Tracked person state
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Person {
    pub track_id: TrackId,
    pub current_zone: Option<GeometryId>,
    pub zone_entered_at: Option<Instant>,
    pub accumulated_dwell_ms: u64,
    pub authorized: bool,
    pub last_position: Option<[f64; 3]>, // [x, y, height] for stitching
}

impl Person {
    #[inline]
    pub fn new(track_id: TrackId) -> Self {
        Self {
            track_id,
            current_zone: None,
            zone_entered_at: None,
            accumulated_dwell_ms: 0,
            authorized: false,
            last_position: None,
        }
    }
}

/// RS485 door status
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DoorStatus {
    Closed,
    Moving,
    Open,
    Unknown,
}

impl DoorStatus {
    pub fn as_str(&self) -> &str {
        match self {
            DoorStatus::Closed => "closed",
            DoorStatus::Moving => "moving",
            DoorStatus::Open => "open",
            DoorStatus::Unknown => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_type_from_str() {
        assert_eq!(
            "ZONE_ENTRY".parse::<EventType>().unwrap(),
            EventType::ZoneEntry
        );
        assert_eq!(
            "TRACK_DELETE".parse::<EventType>().unwrap(),
            EventType::TrackDelete
        );
        assert!(matches!(
            "UNKNOWN_TYPE".parse::<EventType>().unwrap(),
            EventType::Unknown(_)
        ));
    }
}
