//! Tests for the Tracker module

use super::*;
use crate::domain::journey::JourneyOutcome;
use crate::domain::types::{EventType, GeometryId, TrackId};
use crate::infra::config::Config;
use crate::infra::metrics::Metrics;
use crate::services::gate_worker::GateCmd;
use std::collections::HashMap;
use tokio::time::Duration;

/// Test harness that keeps channel receivers alive so `try_send` succeeds
struct TestTracker {
    tracker: Tracker,
    #[allow(dead_code)]
    gate_cmd_rx: mpsc::Receiver<GateCmd>,
    #[allow(dead_code)]
    journey_rx: mpsc::Receiver<Journey>,
    #[allow(dead_code)]
    door_tx: watch::Sender<DoorStatus>,
}

impl std::ops::Deref for TestTracker {
    type Target = Tracker;
    fn deref(&self) -> &Self::Target {
        &self.tracker
    }
}

impl std::ops::DerefMut for TestTracker {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.tracker
    }
}

fn create_test_tracker() -> TestTracker {
    create_test_tracker_with_config(Config::default())
}

fn create_test_tracker_with_config(config: Config) -> TestTracker {
    let (gate_cmd_tx, gate_cmd_rx) = mpsc::channel::<GateCmd>(64);
    let (journey_tx, journey_rx) = mpsc::channel::<Journey>(64);
    let (door_tx, door_rx) = watch::channel(DoorStatus::Unknown);
    let metrics = Arc::new(Metrics::new());
    let tracker = Tracker::new(config, gate_cmd_tx, journey_tx, metrics, None, door_rx);
    TestTracker { tracker, gate_cmd_rx, journey_rx, door_tx }
}

fn millis(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

fn acc_ip_mapping() -> HashMap<String, String> {
    HashMap::from([("127.0.0.1".to_string(), "POS_1".to_string())])
}

fn acc_ip_mapping_multi_zone() -> HashMap<String, String> {
    HashMap::from([
        ("127.0.0.1".to_string(), "POS_1".to_string()),
        ("127.0.0.2".to_string(), "POS_2".to_string()),
    ])
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

    // Dwell is now tracked in journey.total_dwell_ms (via PosOccupancyState)
    let journey = tracker.journey_manager.get(TrackId(100)).unwrap();
    assert!(journey.total_dwell_ms >= 100);
    // Not authorized yet (need ACC match)
    let person = tracker.persons.get(&TrackId(100)).unwrap();
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

    // Dwell is now tracked in journey.total_dwell_ms (via PosOccupancyState)
    let journey = tracker.journey_manager.get(TrackId(100)).unwrap();
    assert!(journey.total_dwell_ms >= 50);
    // Person is NOT authorized (no ACC match)
    let person = tracker.persons.get(&TrackId(100)).unwrap();
    assert!(!person.authorized);
}

#[tokio::test]
async fn test_accumulated_dwell_across_zones() {
    // With per-zone tracking, dwell accumulates per-zone (not across zones)
    // This test verifies that dwell is tracked separately per zone
    let config = Config::default().with_min_dwell_ms(100);
    let mut tracker = create_test_tracker_with_config(config);

    tracker.process_event(create_event(EventType::TrackCreate, 100, None));

    // First POS zone visit (POS_1)
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));
    tokio::time::sleep(millis(60)).await;
    tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1001)));

    // Journey tracks total dwell across zones
    let journey = tracker.journey_manager.get(TrackId(100)).unwrap();
    assert!(journey.total_dwell_ms >= 50);
    let person = tracker.persons.get(&TrackId(100)).unwrap();
    assert!(!person.authorized);

    // Second POS zone visit (POS_2)
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1002)));
    tokio::time::sleep(millis(60)).await;
    tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1002)));

    // Journey tracks total dwell (for logging), but ACC matching uses per-zone dwell
    let journey = tracker.journey_manager.get(TrackId(100)).unwrap();
    assert!(journey.total_dwell_ms >= 100);
    let person = tracker.persons.get(&TrackId(100)).unwrap();
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
async fn test_lost_in_exit_corridor_is_stitch_candidate() {
    // When a track enters gate zone but doesn't cross the exit line, it's marked
    // as Lost (stitch candidate) rather than Completed. We don't infer exit -
    // only proven exits (EXIT cross or EXIT zone) are Completed.
    let config = Config::default().with_min_dwell_ms(10).with_approach_line(1008);
    let mut tracker = create_test_tracker_with_config(config);

    tracker.process_event(create_event_with_pos(EventType::TrackCreate, 100, [1.0, 1.0, 1.70]));

    // Enter POS zone for dwell
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));
    tokio::time::sleep(millis(20)).await;
    tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1001)));

    // Manually authorize (simulating ACC match)
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
    tracker.process_event(create_event_with_pos(EventType::TrackDelete, 100, [1.0, 1.0, 1.70]));

    // Track should be gone (pending for stitch)
    assert_eq!(tracker.active_tracks(), 0);

    // Journey should be Lost (stitch candidate) - went deep (GATE) but didn't exit
    let journey = tracker.journey_manager.get_any(TrackId(100)).expect("Journey should exist");
    assert_eq!(journey.outcome, JourneyOutcome::Lost);
    assert!(!journey.exit_inferred, "exit_inferred should be false - no proven exit");
    assert!(journey.authorized);
}

#[tokio::test]
async fn test_no_exit_inferred_without_approach_cross() {
    // Exit is NOT inferred if person didn't cross approach line
    let config = Config::default().with_min_dwell_ms(10).with_approach_line(1008);
    let mut tracker = create_test_tracker_with_config(config);

    tracker.process_event(create_event_with_pos(EventType::TrackCreate, 100, [1.0, 1.0, 1.70]));

    // Enter GATE zone directly (no approach cross)
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1007)));

    // Track deleted in gate area
    tracker.process_event(create_event_with_pos(EventType::TrackDelete, 100, [1.0, 1.0, 1.70]));

    // Should be marked as Lost (no approach cross = uncertain exit)
    let journey = tracker.journey_manager.get_any(TrackId(100)).expect("Journey should exist");
    assert_eq!(journey.outcome, JourneyOutcome::Lost);
    assert!(!journey.exit_inferred);
}

#[tokio::test]
async fn test_stitch_transfers_state() {
    let mut tracker = create_test_tracker();

    // Create track with position
    tracker.process_event(create_event_with_pos(EventType::TrackCreate, 100, [1.0, 1.0, 1.70]));

    // Enter POS zone (makes track a valid stitch candidate - went deep)
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));

    // Manually authorize (simulating ACC match)
    {
        let person = tracker.persons.get_mut(&TrackId(100)).unwrap();
        person.authorized = true;
    }

    // Delete track (goes to stitch pending - Lost because went deep but didn't exit)
    tracker.process_event(create_event_with_pos(EventType::TrackDelete, 100, [1.0, 1.0, 1.70]));
    assert_eq!(tracker.active_tracks(), 0);

    // New track nearby within stitch criteria
    tracker.process_event(create_event_with_pos(EventType::TrackCreate, 200, [1.05, 1.0, 1.71]));
    assert_eq!(tracker.active_tracks(), 1);

    // New track should have inherited authorized state
    let new_person = tracker.persons.get(&TrackId(200)).unwrap();
    assert!(new_person.authorized);
}

#[tokio::test]
async fn test_stitch_fails_too_late() {
    let mut tracker = create_test_tracker();

    // Create and authorize track
    tracker.process_event(create_event_with_pos(EventType::TrackCreate, 100, [1.0, 1.0, 1.70]));

    // Enter POS zone (makes track a valid stitch candidate)
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));

    {
        let person = tracker.persons.get_mut(&TrackId(100)).unwrap();
        person.authorized = true;
    }

    // Delete track
    tracker.process_event(create_event_with_pos(EventType::TrackDelete, 100, [1.0, 1.0, 1.70]));
    assert_eq!(tracker.active_tracks(), 0);

    // Wait beyond stitch window (8s for POS zone, base is 4.5s)
    tokio::time::sleep(millis(8100)).await;

    // New track nearby - should NOT stitch (too late)
    tracker.process_event(create_event_with_pos(EventType::TrackCreate, 200, [1.05, 1.0, 1.71]));

    // New track should be fresh (no inherited state)
    let new_person = tracker.persons.get(&TrackId(200)).unwrap();
    assert!(!new_person.authorized);
}

#[tokio::test]
async fn test_stitch_fails_too_far() {
    let mut tracker = create_test_tracker();

    // Create and authorize track
    tracker.process_event(create_event_with_pos(EventType::TrackCreate, 100, [1.0, 1.0, 1.70]));
    {
        let person = tracker.persons.get_mut(&TrackId(100)).unwrap();
        person.authorized = true;
    }

    // Delete track
    tracker.process_event(create_event_with_pos(EventType::TrackDelete, 100, [1.0, 1.0, 1.70]));

    // New track 3m away - should NOT stitch (too far)
    tracker.process_event(create_event_with_pos(EventType::TrackCreate, 200, [4.0, 1.0, 1.70]));

    // New track should be fresh
    let new_person = tracker.persons.get(&TrackId(200)).unwrap();
    assert!(!new_person.authorized);
}

#[tokio::test]
async fn test_no_stitch_without_new_track() {
    let mut tracker = create_test_tracker();

    // Create and authorize track
    tracker.process_event(create_event_with_pos(EventType::TrackCreate, 100, [1.0, 1.0, 1.70]));
    {
        let person = tracker.persons.get_mut(&TrackId(100)).unwrap();
        person.authorized = true;
    }

    // Delete track
    tracker.process_event(create_event_with_pos(EventType::TrackDelete, 100, [1.0, 1.0, 1.70]));
    assert_eq!(tracker.active_tracks(), 0);

    // No new track created - person stays in stitch pending until expired
    // (stitch pending is internal, can't directly verify, but track count stays 0)
    assert_eq!(tracker.active_tracks(), 0);
}

#[tokio::test]
async fn test_absolutely_no_stitch() {
    let mut tracker = create_test_tracker();

    // Create authorized track
    tracker.process_event(create_event_with_pos(EventType::TrackCreate, 100, [0.0, 0.0, 1.50]));
    {
        let person = tracker.persons.get_mut(&TrackId(100)).unwrap();
        person.authorized = true;
    }

    // Delete track
    tracker.process_event(create_event_with_pos(EventType::TrackDelete, 100, [0.0, 0.0, 1.50]));
    assert_eq!(tracker.active_tracks(), 0);

    // New track at opposite corner, completely different height
    // Distance: 14m away (1414cm >> 180cm limit)
    // Height: 50cm different (>> 10cm limit)
    tracker.process_event(create_event_with_pos(EventType::TrackCreate, 999, [10.0, 10.0, 2.00]));

    // New track should be completely fresh - NO state transferred
    let new_person = tracker.persons.get(&TrackId(999)).unwrap();
    assert!(!new_person.authorized);
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

    tracker.process_event(create_event(EventType::AccEvent("127.0.0.1".to_string()), 0, None));

    let summary = tracker.metrics.report(tracker.active_tracks(), tracker.authorized_tracks());
    assert_eq!(summary.gate_commands_sent, 1);
    assert!(tracker.journey_manager.get(TrackId(100)).unwrap().gate_cmd_at.is_some());
}

#[tokio::test]
async fn test_acc_authorization_survives_pending_and_stitch() {
    let config = Config::default().with_min_dwell_ms(50).with_acc_ip_to_pos(acc_ip_mapping());
    let mut tracker = create_test_tracker_with_config(config);

    tracker.process_event(create_event_with_pos(EventType::TrackCreate, 100, [1.0, 1.0, 1.70]));
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));
    tokio::time::sleep(millis(80)).await;
    tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1001)));

    tracker.process_event(create_event_with_pos(EventType::TrackDelete, 100, [1.0, 1.0, 1.70]));

    tracker.process_event(create_event(EventType::AccEvent("127.0.0.1".to_string()), 0, None));

    tracker.process_event(create_event_with_pos(EventType::TrackCreate, 200, [1.05, 1.0, 1.71]));
    tracker.process_event(create_event(EventType::ZoneEntry, 200, Some(1007)));

    let summary = tracker.metrics.report(tracker.active_tracks(), tracker.authorized_tracks());
    assert_eq!(summary.gate_commands_sent, 1);
    assert!(tracker.journey_manager.get(TrackId(200)).unwrap().gate_cmd_at.is_some());
}

// =============================================================================
// Per-Zone Dwell Semantics Tests (POS-009)
// =============================================================================

/// Simulate a POS zone visit with specified dwell time
async fn visit_pos_zone(tracker: &mut TestTracker, track_id: i64, zone_id: i32, dwell_ms: u64) {
    tracker.process_event(create_event(EventType::ZoneEntry, track_id, Some(zone_id)));
    tokio::time::sleep(millis(dwell_ms)).await;
    tracker.process_event(create_event(EventType::ZoneExit, track_id, Some(zone_id)));
}

fn is_authorized(tracker: &TestTracker, track_id: i64) -> bool {
    tracker.persons.get(&TrackId(track_id)).map_or(false, |p| p.authorized)
}

fn send_acc_event(tracker: &mut TestTracker, ip: &str) {
    tracker.process_event(create_event(EventType::AccEvent(ip.to_string()), 0, None));
}

fn enter_gate_zone(tracker: &mut TestTracker, track_id: i64) {
    tracker.process_event(create_event(EventType::ZoneEntry, track_id, Some(1007)));
}

#[tokio::test]
async fn test_per_zone_dwell_does_not_combine_across_zones() {
    // Customer at POS_1 (3s) then POS_2 (5s) does NOT qualify for ACC at either zone
    // With per-zone tracking, dwell doesn't combine across zones
    let config =
        Config::default().with_min_dwell_ms(7000).with_acc_ip_to_pos(acc_ip_mapping_multi_zone());
    let mut tracker = create_test_tracker_with_config(config);

    tracker.process_event(create_event(EventType::TrackCreate, 100, None));
    visit_pos_zone(&mut tracker, 100, 1001, 3100).await; // POS_1: 3s (below 7s threshold)
    visit_pos_zone(&mut tracker, 100, 1002, 5100).await; // POS_2: 5s (below 7s threshold)

    // ACC from POS_1 should NOT match (only 3s dwell at POS_1)
    send_acc_event(&mut tracker, "127.0.0.1");
    assert!(!is_authorized(&tracker, 100), "POS_1 dwell (3s) < threshold (7s)");

    // ACC from POS_2 should also NOT match (only 5s dwell at POS_2)
    send_acc_event(&mut tracker, "127.0.0.2");
    assert!(!is_authorized(&tracker, 100), "POS_2 dwell (5s) < threshold (7s)");
}

#[tokio::test]
async fn test_per_zone_dwell_qualifies_at_correct_zone_only() {
    // Customer at POS_1 for 8s qualifies for ACC at POS_1 only
    let config =
        Config::default().with_min_dwell_ms(7000).with_acc_ip_to_pos(acc_ip_mapping_multi_zone());
    let mut tracker = create_test_tracker_with_config(config);

    tracker.process_event(create_event(EventType::TrackCreate, 100, None));
    visit_pos_zone(&mut tracker, 100, 1001, 8100).await; // POS_1: 8s (above 7s threshold)
    enter_gate_zone(&mut tracker, 100);

    send_acc_event(&mut tracker, "127.0.0.1");
    assert!(is_authorized(&tracker, 100), "POS_1 dwell (8s) >= threshold (7s)");

    let summary = tracker.metrics.report(tracker.active_tracks(), tracker.authorized_tracks());
    assert_eq!(summary.gate_commands_sent, 1, "Gate should have opened");
}

#[tokio::test]
async fn test_per_zone_acc_from_wrong_zone_does_not_match() {
    // Customer at POS_1 for 8s, but ACC comes from POS_2 - no match
    let config =
        Config::default().with_min_dwell_ms(7000).with_acc_ip_to_pos(acc_ip_mapping_multi_zone());
    let mut tracker = create_test_tracker_with_config(config);

    tracker.process_event(create_event(EventType::TrackCreate, 100, None));
    visit_pos_zone(&mut tracker, 100, 1001, 8100).await; // POS_1: 8s (above threshold)

    send_acc_event(&mut tracker, "127.0.0.2"); // ACC from POS_2 (wrong zone)
    assert!(!is_authorized(&tracker, 100), "No dwell at POS_2");
}

#[tokio::test]
async fn test_acc_within_grace_window_matches() {
    // ACC arrives 4s after exit - within 5s grace window, should match
    let config = Config::default().with_min_dwell_ms(50).with_acc_ip_to_pos(acc_ip_mapping());
    let mut tracker = create_test_tracker_with_config(config);

    tracker.process_event(create_event(EventType::TrackCreate, 100, None));
    visit_pos_zone(&mut tracker, 100, 1001, 100).await;
    tokio::time::sleep(millis(4000)).await; // Wait 4s (within 5s grace window)
    enter_gate_zone(&mut tracker, 100);

    send_acc_event(&mut tracker, "127.0.0.1");
    assert!(is_authorized(&tracker, 100), "Exit within grace window");

    let summary = tracker.metrics.report(tracker.active_tracks(), tracker.authorized_tracks());
    assert_eq!(summary.gate_commands_sent, 1, "Gate should have opened");
}

#[tokio::test]
async fn test_acc_beyond_grace_window_does_not_match() {
    // ACC arrives 6s after exit - beyond 5s grace window, should NOT match
    let config = Config::default().with_min_dwell_ms(50).with_acc_ip_to_pos(acc_ip_mapping());
    let mut tracker = create_test_tracker_with_config(config);

    tracker.process_event(create_event(EventType::TrackCreate, 100, None));
    visit_pos_zone(&mut tracker, 100, 1001, 100).await;
    tokio::time::sleep(millis(6000)).await; // Wait 6s (beyond 5s grace window)

    send_acc_event(&mut tracker, "127.0.0.1");
    assert!(!is_authorized(&tracker, 100), "Exit beyond grace window");
}

#[tokio::test]
async fn test_acc_picks_highest_dwell_as_primary() {
    // Multiple tracks in POS, ACC authorizes all tracks in zone
    let config = Config::default().with_min_dwell_ms(50).with_acc_ip_to_pos(acc_ip_mapping());
    let mut tracker = create_test_tracker_with_config(config);

    tracker.process_event(create_event(EventType::TrackCreate, 100, None));
    tracker.process_event(create_event(EventType::TrackCreate, 200, None));

    // Track 100 enters POS first (longer dwell)
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));
    tokio::time::sleep(millis(50)).await;
    // Track 200 enters POS later (shorter dwell)
    tracker.process_event(create_event(EventType::ZoneEntry, 200, Some(1001)));
    tokio::time::sleep(millis(100)).await;

    // Both still in POS zone: track 100 ~150ms, track 200 ~100ms
    send_acc_event(&mut tracker, "127.0.0.1");

    // Both should be authorized
    assert!(is_authorized(&tracker, 100), "Track 100 should be authorized");
    assert!(is_authorized(&tracker, 200), "Track 200 should be authorized");

    // Verify journey events show ACC match for both
    assert!(tracker.journey_manager.get(TrackId(100)).unwrap().acc_matched);
    assert!(tracker.journey_manager.get(TrackId(200)).unwrap().acc_matched);
}
