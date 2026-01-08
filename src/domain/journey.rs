//! Journey data model for tracking customer paths through the store

use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Generate a new UUIDv7 (time-sortable)
pub fn new_uuid_v7() -> String {
    Uuid::now_v7().to_string()
}

/// Get current epoch milliseconds
pub fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Journey outcome
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum JourneyOutcome {
    InProgress,
    Completed, // crossed EXIT
    Abandoned, // track deleted, never crossed EXIT
}

impl JourneyOutcome {
    pub fn as_str(&self) -> &str {
        match self {
            JourneyOutcome::InProgress => "in_progress",
            JourneyOutcome::Completed => "exit",
            JourneyOutcome::Abandoned => "abandoned",
        }
    }
}

/// A single event in a journey
#[derive(Debug, Clone)]
pub struct JourneyEvent {
    pub t: String,             // event type
    pub z: Option<String>,     // zone or line name
    pub ts: u64,               // epoch ms
    pub extra: Option<String>, // additional data
}

impl JourneyEvent {
    pub fn new(event_type: &str, ts: u64) -> Self {
        Self {
            t: event_type.to_string(),
            z: None,
            ts,
            extra: None,
        }
    }

    pub fn with_zone(mut self, zone: &str) -> Self {
        self.z = Some(zone.to_string());
        self
    }

    pub fn with_extra(mut self, extra: &str) -> Self {
        self.extra = Some(extra.to_string());
        self
    }

    /// Convert to JSON value for short-key format
    fn to_json_value(&self) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        obj.insert("t".to_string(), serde_json::Value::String(self.t.clone()));
        if let Some(z) = &self.z {
            obj.insert("z".to_string(), serde_json::Value::String(z.clone()));
        }
        obj.insert("ts".to_string(), serde_json::Value::Number(self.ts.into()));
        if let Some(x) = &self.extra {
            obj.insert("x".to_string(), serde_json::Value::String(x.clone()));
        }
        serde_json::Value::Object(obj)
    }
}

/// Complete journey for a tracked person
#[derive(Debug, Clone)]
pub struct Journey {
    pub jid: String,            // UUIDv7 journey ID
    pub pid: String,            // UUIDv7 person ID (stable across stitches)
    pub tids: Vec<i64>,         // Xovis track_ids (stitch history)
    pub parent: Option<String>, // Previous journey's jid (for re-entry)
    pub outcome: JourneyOutcome,
    pub authorized: bool,
    pub total_dwell_ms: u64,
    pub acc_matched: bool,
    pub gate_cmd_at: Option<u64>,    // epoch ms
    pub gate_opened_at: Option<u64>, // epoch ms from RS485
    pub gate_was_open: bool,
    pub started_at: u64,       // epoch ms
    pub ended_at: Option<u64>, // epoch ms
    pub crossed_entry: bool,
    pub events: Vec<JourneyEvent>,
}

impl Journey {
    /// Create a new journey for a track
    pub fn new(track_id: i64) -> Self {
        let now = epoch_ms();
        Self {
            jid: new_uuid_v7(),
            pid: new_uuid_v7(),
            tids: vec![track_id],
            parent: None,
            outcome: JourneyOutcome::InProgress,
            authorized: false,
            total_dwell_ms: 0,
            acc_matched: false,
            gate_cmd_at: None,
            gate_opened_at: None,
            gate_was_open: false,
            started_at: now,
            ended_at: None,
            crossed_entry: false,
            events: Vec::with_capacity(16),
        }
    }

    /// Create a new journey that continues from a previous one (re-entry)
    pub fn new_with_parent(track_id: i64, parent_jid: &str, parent_pid: &str) -> Self {
        let mut journey = Self::new(track_id);
        journey.parent = Some(parent_jid.to_string());
        journey.pid = parent_pid.to_string();
        journey
    }

    /// Add a track ID when stitching
    pub fn add_track_id(&mut self, track_id: i64) {
        self.tids.push(track_id);
    }

    /// Add an event to the journey
    pub fn add_event(&mut self, event: JourneyEvent) {
        self.events.push(event);
    }

    /// Mark the journey as completed
    pub fn complete(&mut self, outcome: JourneyOutcome) {
        self.outcome = outcome;
        self.ended_at = Some(epoch_ms());
    }

    /// Get the current/last track ID
    pub fn current_track_id(&self) -> i64 {
        *self.tids.last().unwrap_or(&0)
    }

    /// Convert to short-key JSON string (without site)
    pub fn to_json(&self) -> String {
        self.to_json_with_site_opt(None)
    }

    /// Convert to short-key JSON string with site_id included
    pub fn to_json_with_site(&self, site_id: &str) -> String {
        self.to_json_with_site_opt(Some(site_id))
    }

    /// Internal method for JSON serialization with optional site
    fn to_json_with_site_opt(&self, site_id: Option<&str>) -> String {
        let mut obj = serde_json::Map::new();

        // Include site_id if provided
        if let Some(site) = site_id {
            obj.insert(
                "site".to_string(),
                serde_json::Value::String(site.to_string()),
            );
        }

        obj.insert(
            "jid".to_string(),
            serde_json::Value::String(self.jid.clone()),
        );
        obj.insert(
            "pid".to_string(),
            serde_json::Value::String(self.pid.clone()),
        );
        obj.insert("tids".to_string(), serde_json::json!(self.tids));

        if let Some(parent) = &self.parent {
            obj.insert(
                "parent".to_string(),
                serde_json::Value::String(parent.clone()),
            );
        } else {
            obj.insert("parent".to_string(), serde_json::Value::Null);
        }

        obj.insert(
            "out".to_string(),
            serde_json::Value::String(self.outcome.as_str().to_string()),
        );
        obj.insert("auth".to_string(), serde_json::Value::Bool(self.authorized));
        obj.insert(
            "dwell".to_string(),
            serde_json::Value::Number(self.total_dwell_ms.into()),
        );
        obj.insert("acc".to_string(), serde_json::Value::Bool(self.acc_matched));

        if let Some(gate_cmd) = self.gate_cmd_at {
            obj.insert(
                "gate_cmd".to_string(),
                serde_json::Value::Number(gate_cmd.into()),
            );
        }
        if let Some(gate_open) = self.gate_opened_at {
            obj.insert(
                "gate_open".to_string(),
                serde_json::Value::Number(gate_open.into()),
            );
        }
        obj.insert(
            "gate_was_open".to_string(),
            serde_json::Value::Bool(self.gate_was_open),
        );

        obj.insert(
            "t0".to_string(),
            serde_json::Value::Number(self.started_at.into()),
        );
        if let Some(ended) = self.ended_at {
            obj.insert("t1".to_string(), serde_json::Value::Number(ended.into()));
        }

        let events: Vec<serde_json::Value> =
            self.events.iter().map(|e| e.to_json_value()).collect();
        obj.insert("ev".to_string(), serde_json::Value::Array(events));

        serde_json::Value::Object(obj).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_journey() {
        let journey = Journey::new(100);

        assert!(!journey.jid.is_empty());
        assert!(!journey.pid.is_empty());
        assert_eq!(journey.tids, vec![100]);
        assert!(journey.parent.is_none());
        assert_eq!(journey.outcome, JourneyOutcome::InProgress);
        assert!(!journey.authorized);
        assert_eq!(journey.total_dwell_ms, 0);
        assert!(!journey.acc_matched);
        assert!(!journey.crossed_entry);
        assert!(journey.events.is_empty());
    }

    #[test]
    fn test_journey_with_parent() {
        let journey = Journey::new_with_parent(200, "parent-jid-123", "pid-456");

        assert_eq!(journey.tids, vec![200]);
        assert_eq!(journey.parent, Some("parent-jid-123".to_string()));
        assert_eq!(journey.pid, "pid-456");
    }

    #[test]
    fn test_add_track_id() {
        let mut journey = Journey::new(100);
        journey.add_track_id(200);
        journey.add_track_id(300);

        assert_eq!(journey.tids, vec![100, 200, 300]);
        assert_eq!(journey.current_track_id(), 300);
    }

    #[test]
    fn test_journey_event() {
        let event = JourneyEvent::new("zone_entry", 1736012345678)
            .with_zone("POS_1")
            .with_extra("dwell=7500");

        assert_eq!(event.t, "zone_entry");
        assert_eq!(event.z, Some("POS_1".to_string()));
        assert_eq!(event.ts, 1736012345678);
        assert_eq!(event.extra, Some("dwell=7500".to_string()));
    }

    #[test]
    fn test_journey_to_json() {
        let mut journey = Journey::new(100);
        journey.authorized = true;
        journey.total_dwell_ms = 7500;
        journey.acc_matched = true;
        journey.crossed_entry = true;
        journey.gate_cmd_at = Some(1736012345678);
        journey.gate_opened_at = Some(1736012345890);

        journey.add_event(JourneyEvent::new("entry_cross", 1736012340000));
        journey.add_event(JourneyEvent::new("zone_entry", 1736012341000).with_zone("POS_1"));
        journey.add_event(
            JourneyEvent::new("zone_exit", 1736012348500)
                .with_zone("POS_1")
                .with_extra("dwell=7500"),
        );

        journey.complete(JourneyOutcome::Completed);

        let json = journey.to_json();

        // Parse and verify
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["tids"], serde_json::json!([100]));
        assert_eq!(parsed["out"], "exit");
        assert_eq!(parsed["auth"], true);
        assert_eq!(parsed["dwell"], 7500);
        assert_eq!(parsed["acc"], true);
        assert_eq!(parsed["gate_cmd"], 1736012345678_u64);
        assert_eq!(parsed["gate_open"], 1736012345890_u64);

        let events = parsed["ev"].as_array().unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0]["t"], "entry_cross");
        assert_eq!(events[1]["t"], "zone_entry");
        assert_eq!(events[1]["z"], "POS_1");
        assert_eq!(events[2]["t"], "zone_exit");
        assert_eq!(events[2]["x"], "dwell=7500");
    }

    #[test]
    fn test_uuid_v7_generation() {
        let uuid1 = new_uuid_v7();
        let uuid2 = new_uuid_v7();

        assert!(!uuid1.is_empty());
        assert!(!uuid2.is_empty());
        assert_ne!(uuid1, uuid2);
        // UUIDv7 should be 36 chars with hyphens
        assert_eq!(uuid1.len(), 36);
    }

    #[test]
    fn test_outcome_as_str() {
        assert_eq!(JourneyOutcome::InProgress.as_str(), "in_progress");
        assert_eq!(JourneyOutcome::Completed.as_str(), "exit");
        assert_eq!(JourneyOutcome::Abandoned.as_str(), "abandoned");
    }
}
