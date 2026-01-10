//! Tests for the Tracker module

use super::*;
use crate::domain::journey::JourneyOutcome;
use crate::domain::types::{EventType, GeometryId, TrackId};
use crate::infra::config::Config;
use crate::infra::metrics::Metrics;
use crate::services::gate_worker::GateCmd;
use std::collections::HashMap;
use tokio::time::Duration;

fn create_test_tracker() -> Tracker {
    create_test_tracker_with_config(Config::default())
}

fn create_test_tracker_with_config(config: Config) -> Tracker {
    // Create a channel for gate commands (we won't consume them in tests)
    let (gate_cmd_tx, _gate_cmd_rx) = mpsc::channel::<GateCmd>(64);
    let metrics = Arc::new(Metrics::new());
    Tracker::new(config, gate_cmd_tx, metrics, None)
}

fn millis(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

fn acc_ip_mapping() -> HashMap<String, String> {
    let mut map = HashMap::new();
    map.insert("127.0.0.1".to_string(), "POS_1".to_string());
    map
}

/// Builder for creating test ParsedEvent instances
struct ParsedEventBuilder {
    event_type: EventType,
    track_id: i64,
    geometry_id: Option<i32>,
    position: Option<[f64; 3]>,
}

impl ParsedEventBuilder {
    fn new(event_type: EventType) -> Self {
        Self { event_type, track_id: 0, geometry_id: None, position: None }
    }

    fn with_track_id(mut self, track_id: i64) -> Self {
        self.track_id = track_id;
        self
    }

    fn with_geometry_id(mut self, geometry_id: i32) -> Self {
        self.geometry_id = Some(geometry_id);
        self
    }

    fn with_position(mut self, position: [f64; 3]) -> Self {
        self.position = Some(position);
        self
    }

    fn build(self) -> ParsedEvent {
        ParsedEvent {
            event_type: self.event_type,
            track_id: TrackId(self.track_id),
            geometry_id: self.geometry_id.map(GeometryId),
            direction: None,
            event_time: 1767617600000,
            received_at: Instant::now(),
            position: self.position,
        }
    }
}

fn create_event(event_type: EventType, track_id: i64, geometry_id: Option<i32>) -> ParsedEvent {
    let mut builder = ParsedEventBuilder::new(event_type).with_track_id(track_id);
    if let Some(gid) = geometry_id {
        builder = builder.with_geometry_id(gid);
    }
    builder.build()
}

fn create_event_with_pos(event_type: EventType, track_id: i64, position: [f64; 3]) -> ParsedEvent {
    ParsedEventBuilder::new(event_type).with_track_id(track_id).with_position(position).build()
}

#[tokio::test]
async fn test_track_create() {
    let mut tracker = create_test_tracker();
    let event = create_event(EventType::TrackCreate, 100, None);

    tracker.process_event(event);

    assert_eq!(tracker.active_tracks(), 1);
    assert!(tracker.persons.contains_key(&TrackId(100)));
}

#[tokio::test]
async fn test_track_delete() {
    let mut tracker = create_test_tracker();

    tracker.process_event(create_event(EventType::TrackCreate, 100, None));
    assert_eq!(tracker.active_tracks(), 1);

    tracker.process_event(create_event(EventType::TrackDelete, 100, None));
    assert_eq!(tracker.active_tracks(), 0);
}

#[tokio::test]
async fn test_dwell_accumulation() {
    let mut tracker = create_test_tracker();

    // Create track
    tracker.process_event(create_event(EventType::TrackCreate, 100, None));

    // Enter POS zone
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));

    // Simulate time passing
    tokio::time::sleep(millis(100)).await;

    // Exit POS zone
    tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1001)));

    let person = tracker.persons.get(&TrackId(100)).unwrap();
    assert!(person.accumulated_dwell_ms >= 100);
    // Not authorized yet (need 7000ms)
    assert!(!person.authorized);
}

#[tokio::test]
async fn test_dwell_threshold_without_acc() {
    // Dwell alone no longer grants authorization - ACC match is required
    let config = Config::default().with_min_dwell_ms(50);
    let mut tracker = create_test_tracker_with_config(config);

    tracker.process_event(create_event(EventType::TrackCreate, 100, None));
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));
    tokio::time::sleep(millis(60)).await;
    tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1001)));

    let person = tracker.persons.get(&TrackId(100)).unwrap();
    // Dwell is accumulated but person is NOT authorized (no ACC match)
    assert!(person.accumulated_dwell_ms >= 50);
    assert!(!person.authorized);
}

#[tokio::test]
async fn test_accumulated_dwell_across_zones() {
    // Dwell accumulates across POS zones, but authorization requires ACC match
    let config = Config::default().with_min_dwell_ms(100);
    let mut tracker = create_test_tracker_with_config(config);

    tracker.process_event(create_event(EventType::TrackCreate, 100, None));

    // First POS zone visit
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));
    tokio::time::sleep(millis(60)).await;
    tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1001)));

    let person = tracker.persons.get(&TrackId(100)).unwrap();
    assert!(!person.authorized);
    assert!(person.accumulated_dwell_ms >= 50);

    // Second POS zone visit
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1002)));
    tokio::time::sleep(millis(60)).await;
    tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1002)));

    let person = tracker.persons.get(&TrackId(100)).unwrap();
    // Dwell accumulated but still not authorized (no ACC match)
    assert!(person.accumulated_dwell_ms >= 100);
    assert!(!person.authorized);
}

#[tokio::test]
async fn test_journey_complete_on_exit_line() {
    let config = Config::default().with_min_dwell_ms(10);
    let mut tracker = create_test_tracker_with_config(config);

    tracker.process_event(create_event(EventType::TrackCreate, 100, None));
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));
    tokio::time::sleep(millis(20)).await;
    tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1001)));
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1007)));
    assert_eq!(tracker.active_tracks(), 1);

    // Cross EXIT_1 line
    let mut exit_event = create_event(EventType::LineCrossForward, 100, Some(1006));
    exit_event.direction = Some("forward".to_string());
    tracker.process_event(exit_event);

    // Person should be removed (journey complete)
    assert_eq!(tracker.active_tracks(), 0);
}

#[tokio::test]
async fn test_exit_inferred_when_lost_in_exit_corridor() {
    // When a track crosses approach forward, enters gate zone, then disappears
    // before crossing the exit line, we infer the exit rather than marking as lost.
    let config = Config::default().with_min_dwell_ms(10).with_approach_line(1008);
    let mut tracker = create_test_tracker_with_config(config);

    tracker
        .process_event(create_event_with_pos(EventType::TrackCreate, 100, [1.0, 1.0, 1.70]));

    // Enter POS zone for dwell
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));
    tokio::time::sleep(millis(20)).await;
    tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1001)));

    // Manually authorize (simulating ACC match) - authorization comes from ACC, not dwell
    {
        let person = tracker.persons.get_mut(&TrackId(100)).unwrap();
        person.authorized = true;
    }

    // Cross APPROACH line forward
    let mut approach_event = create_event(EventType::LineCrossForward, 100, Some(1008));
    approach_event.direction = Some("forward".to_string());
    tracker.process_event(approach_event);

    // Enter GATE zone
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1007)));

    // Exit GATE zone (still in exit corridor, before exit line)
    tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1007)));

    // Track deleted - simulating sensor loss in exit corridor
    // Person should still be in gate area (last zone is GATE_1)
    // Set current zone back to gate to simulate being in gate area when deleted
    tracker.persons.get_mut(&TrackId(100)).unwrap().current_zone = Some(GeometryId(1007));

    tracker
        .process_event(create_event_with_pos(EventType::TrackDelete, 100, [1.0, 1.0, 1.70]));

    // Track should be gone (pending for stitch)
    assert_eq!(tracker.active_tracks(), 0);

    // Check journey - should have exit_inferred=true and Completed outcome
    // Journey is in pending_egress, accessible via get_any
    let journey = tracker.journey_manager.get_any(TrackId(100)).expect("Journey should exist");
    assert_eq!(journey.outcome, JourneyOutcome::Completed);
    assert!(journey.exit_inferred, "Expected exit_inferred to be true");
    assert!(journey.authorized);
}

#[tokio::test]
async fn test_no_exit_inferred_without_approach_cross() {
    // Exit is NOT inferred if person didn't cross approach line
    let config = Config::default().with_min_dwell_ms(10).with_approach_line(1008);
    let mut tracker = create_test_tracker_with_config(config);

    tracker
        .process_event(create_event_with_pos(EventType::TrackCreate, 100, [1.0, 1.0, 1.70]));

    // Enter GATE zone directly (no approach cross)
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1007)));

    // Track deleted in gate area
    tracker
        .process_event(create_event_with_pos(EventType::TrackDelete, 100, [1.0, 1.0, 1.70]));

    // Should be marked as Lost (no approach cross = uncertain exit)
    let journey = tracker.journey_manager.get_any(TrackId(100)).expect("Journey should exist");
    assert_eq!(journey.outcome, JourneyOutcome::Lost);
    assert!(!journey.exit_inferred);
}

#[tokio::test]
async fn test_stitch_transfers_state() {
    let mut tracker = create_test_tracker();

    // Create track with position
    tracker
        .process_event(create_event_with_pos(EventType::TrackCreate, 100, [1.0, 1.0, 1.70]));

    // Manually authorize (simulating ACC match) and set dwell
    {
        let person = tracker.persons.get_mut(&TrackId(100)).unwrap();
        person.authorized = true;
        person.accumulated_dwell_ms = 5000;
    }

    let dwell = tracker.persons.get(&TrackId(100)).unwrap().accumulated_dwell_ms;

    // Delete track (goes to stitch pending)
    tracker
        .process_event(create_event_with_pos(EventType::TrackDelete, 100, [1.0, 1.0, 1.70]));
    assert_eq!(tracker.active_tracks(), 0);

    // New track nearby within stitch criteria
    tracker
        .process_event(create_event_with_pos(EventType::TrackCreate, 200, [1.05, 1.0, 1.71]));
    assert_eq!(tracker.active_tracks(), 1);

    // New track should have inherited state
    let new_person = tracker.persons.get(&TrackId(200)).unwrap();
    assert!(new_person.authorized);
    assert!(new_person.accumulated_dwell_ms >= dwell);
}

#[tokio::test]
async fn test_stitch_fails_too_late() {
    let mut tracker = create_test_tracker();

    // Create and authorize track
    tracker
        .process_event(create_event_with_pos(EventType::TrackCreate, 100, [1.0, 1.0, 1.70]));
    {
        let person = tracker.persons.get_mut(&TrackId(100)).unwrap();
        person.authorized = true;
        person.accumulated_dwell_ms = 5000;
    }

    // Delete track
    tracker
        .process_event(create_event_with_pos(EventType::TrackDelete, 100, [1.0, 1.0, 1.70]));
    assert_eq!(tracker.active_tracks(), 0);

    // Wait beyond stitch window (4.5s)
    tokio::time::sleep(millis(4600)).await;

    // New track nearby - should NOT stitch (too late)
    tracker
        .process_event(create_event_with_pos(EventType::TrackCreate, 200, [1.05, 1.0, 1.71]));

    // New track should be fresh (no inherited state)
    let new_person = tracker.persons.get(&TrackId(200)).unwrap();
    assert!(!new_person.authorized);
    assert_eq!(new_person.accumulated_dwell_ms, 0);
}

#[tokio::test]
async fn test_stitch_fails_too_far() {
    let mut tracker = create_test_tracker();

    // Create and authorize track
    tracker
        .process_event(create_event_with_pos(EventType::TrackCreate, 100, [1.0, 1.0, 1.70]));
    {
        let person = tracker.persons.get_mut(&TrackId(100)).unwrap();
        person.authorized = true;
        person.accumulated_dwell_ms = 5000;
    }

    // Delete track
    tracker
        .process_event(create_event_with_pos(EventType::TrackDelete, 100, [1.0, 1.0, 1.70]));

    // New track 3m away - should NOT stitch (too far)
    tracker
        .process_event(create_event_with_pos(EventType::TrackCreate, 200, [4.0, 1.0, 1.70]));

    // New track should be fresh
    let new_person = tracker.persons.get(&TrackId(200)).unwrap();
    assert!(!new_person.authorized);
    assert_eq!(new_person.accumulated_dwell_ms, 0);
}

#[tokio::test]
async fn test_no_stitch_without_new_track() {
    let mut tracker = create_test_tracker();

    // Create and authorize track
    tracker
        .process_event(create_event_with_pos(EventType::TrackCreate, 100, [1.0, 1.0, 1.70]));
    {
        let person = tracker.persons.get_mut(&TrackId(100)).unwrap();
        person.authorized = true;
    }

    // Delete track
    tracker
        .process_event(create_event_with_pos(EventType::TrackDelete, 100, [1.0, 1.0, 1.70]));
    assert_eq!(tracker.active_tracks(), 0);

    // No new track created - person stays in stitch pending until expired
    // (stitch pending is internal, can't directly verify, but track count stays 0)
    assert_eq!(tracker.active_tracks(), 0);
}

#[tokio::test]
async fn test_absolutely_no_stitch() {
    let mut tracker = create_test_tracker();

    // Create authorized track with accumulated dwell
    tracker
        .process_event(create_event_with_pos(EventType::TrackCreate, 100, [0.0, 0.0, 1.50]));
    {
        let person = tracker.persons.get_mut(&TrackId(100)).unwrap();
        person.authorized = true;
        person.accumulated_dwell_ms = 99999;
    }

    // Delete track
    tracker
        .process_event(create_event_with_pos(EventType::TrackDelete, 100, [0.0, 0.0, 1.50]));
    assert_eq!(tracker.active_tracks(), 0);

    // New track at opposite corner, completely different height
    // Distance: 14m away (1414cm >> 180cm limit)
    // Height: 50cm different (>> 10cm limit)
    tracker
        .process_event(create_event_with_pos(EventType::TrackCreate, 999, [10.0, 10.0, 2.00]));

    // New track should be completely fresh - NO state transferred
    let new_person = tracker.persons.get(&TrackId(999)).unwrap();
    assert!(!new_person.authorized);
    assert_eq!(new_person.accumulated_dwell_ms, 0);
}

#[tokio::test]
async fn test_gate_opens_when_acc_after_gate_entry() {
    let config = Config::default().with_min_dwell_ms(50).with_acc_ip_to_pos(acc_ip_mapping());
    let mut tracker = create_test_tracker_with_config(config);

    tracker.process_event(create_event(EventType::TrackCreate, 100, None));
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));
    tokio::time::sleep(millis(80)).await;
    tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1001)));
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1007)));

    tracker
        .process_event(create_event(EventType::AccEvent("127.0.0.1".to_string()), 0, None));

    let summary = tracker.metrics.report(tracker.active_tracks(), tracker.authorized_tracks());
    assert_eq!(summary.gate_commands_sent, 1);
    assert!(tracker.journey_manager.get(TrackId(100)).unwrap().gate_cmd_at.is_some());
}

#[tokio::test]
async fn test_acc_authorization_survives_pending_and_stitch() {
    let config = Config::default().with_min_dwell_ms(50).with_acc_ip_to_pos(acc_ip_mapping());
    let mut tracker = create_test_tracker_with_config(config);

    tracker
        .process_event(create_event_with_pos(EventType::TrackCreate, 100, [1.0, 1.0, 1.70]));
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));
    tokio::time::sleep(millis(80)).await;
    tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1001)));

    tracker
        .process_event(create_event_with_pos(EventType::TrackDelete, 100, [1.0, 1.0, 1.70]));

    tracker
        .process_event(create_event(EventType::AccEvent("127.0.0.1".to_string()), 0, None));

    tracker
        .process_event(create_event_with_pos(EventType::TrackCreate, 200, [1.05, 1.0, 1.71]));
    tracker.process_event(create_event(EventType::ZoneEntry, 200, Some(1007)));

    let summary = tracker.metrics.report(tracker.active_tracks(), tracker.authorized_tracks());
    assert_eq!(summary.gate_commands_sent, 1);
    assert!(tracker.journey_manager.get(TrackId(200)).unwrap().gate_cmd_at.is_some());
}
