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

// =============================================================================
// Group Track Tests (POS-010)
// =============================================================================
// Group tracks have the 0x80000000 bit set and were previously filtered.
// Now they flow through all handlers like regular tracks.

const XOVIS_GROUP_BIT: i64 = 0x80000000;

fn group_track_id(base_id: i64) -> i64 {
    base_id | XOVIS_GROUP_BIT
}

#[tokio::test]
async fn test_group_track_is_tracked() {
    // Group track (0x80000000 bit set) is tracked as a regular person
    let config = Config::default().with_min_dwell_ms(50).with_acc_ip_to_pos(acc_ip_mapping());
    let mut tracker = create_test_tracker_with_config(config);

    let gid = group_track_id(100);
    tracker.process_event(create_event(EventType::TrackCreate, gid, None));
    tracker.process_event(create_event(EventType::ZoneEntry, gid, Some(1001)));

    assert_eq!(tracker.active_tracks(), 1);
    assert!(tracker.persons.contains_key(&TrackId(gid)));
}

#[tokio::test]
async fn test_group_track_full_journey() {
    // Group track completes full journey: dwell -> ACC match -> gate open
    let config = Config::default().with_min_dwell_ms(50).with_acc_ip_to_pos(acc_ip_mapping());
    let mut tracker = create_test_tracker_with_config(config);

    let gid = group_track_id(100);
    tracker.process_event(create_event(EventType::TrackCreate, gid, None));
    visit_pos_zone(&mut tracker, gid, 1001, 100).await;
    enter_gate_zone(&mut tracker, gid);
    send_acc_event(&mut tracker, "127.0.0.1");

    // Verify authorization
    assert!(is_authorized(&tracker, gid));

    // Verify gate command was sent
    let summary = tracker.metrics.report(tracker.active_tracks(), tracker.authorized_tracks());
    assert_eq!(summary.gate_commands_sent, 1);

    // Verify journey state
    let journey = tracker.journey_manager.get(TrackId(gid)).unwrap();
    assert!(journey.total_dwell_ms >= 50);
    assert!(journey.authorized);
    assert!(journey.acc_matched);
    assert!(journey.gate_cmd_at.is_some());
    assert!(journey.tids.contains(&TrackId(gid)));
    assert!(journey.events.len() >= 4);
}

// =============================================================================
// Position-Based Exit Detection Tests (Task 7)
// =============================================================================
// These tests verify the position-based exit detection feature, which infers
// journey completion when a track disappears in the exit region without a
// LINE_CROSS event.

// -----------------------------------------------------------------------------
// Core Detection Tests
// -----------------------------------------------------------------------------

#[tokio::test]
async fn test_position_detected_exit() {
    // No LINE_CROSS, last_pos=(2.0, 2.5), has_zone_events=true
    // Should result in JourneyOutcome::Completed (position-detected exit)
    let config = Config::default().with_min_dwell_ms(50);
    let mut tracker = create_test_tracker_with_config(config);

    // Create track at position in exit region (y=2.5 > 2.3 threshold, x=2.0 in [1.5, 3.0])
    tracker.process_event(create_event_with_pos(EventType::TrackCreate, 100, [2.0, 2.5, 1.70]));

    // Enter POS zone to set has_zone_events = true
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));
    tokio::time::sleep(millis(60)).await;
    tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1001)));

    // Delete track at exit region position (no LINE_CROSS)
    tracker.process_event(create_event_with_pos(EventType::TrackDelete, 100, [2.0, 2.5, 1.70]));

    // Journey should be Completed (position-detected exit)
    let journey = tracker.journey_manager.get_any(TrackId(100)).expect("Journey should exist");
    assert_eq!(journey.outcome, JourneyOutcome::Completed);
    assert!(journey.exit_inferred, "exit_inferred should be true for position-detected exit");
}

#[tokio::test]
async fn test_pass_through() {
    // No LINE_CROSS, last_pos=(2.0, 2.5), has_zone_events=false
    // Should result in JourneyOutcome::PassThrough
    let config = Config::default().with_min_dwell_ms(50);
    let mut tracker = create_test_tracker_with_config(config);

    // Create track at position in exit region
    tracker.process_event(create_event_with_pos(EventType::TrackCreate, 100, [2.0, 2.5, 1.70]));

    // NO zone events - person just passed through

    // Delete track at exit region position (no LINE_CROSS, no zone events)
    tracker.process_event(create_event_with_pos(EventType::TrackDelete, 100, [2.0, 2.5, 1.70]));

    // Journey should be PassThrough (in exit region but no zone engagement)
    let journey = tracker.journey_manager.get_any(TrackId(100)).expect("Journey should exist");
    assert_eq!(journey.outcome, JourneyOutcome::PassThrough);
    assert!(!journey.exit_inferred, "exit_inferred should be false for pass-through");
}

#[tokio::test]
async fn test_lost_low_y() {
    // No LINE_CROSS, last_pos=(2.0, 1.5), has_zone_events=true
    // Should result in JourneyOutcome::Lost (below y threshold of 2.3)
    let config = Config::default().with_min_dwell_ms(50);
    let mut tracker = create_test_tracker_with_config(config);

    // Create track at position with low y (below exit threshold)
    tracker.process_event(create_event_with_pos(EventType::TrackCreate, 100, [2.0, 1.5, 1.70]));

    // Enter POS zone to set has_zone_events = true
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));
    tokio::time::sleep(millis(60)).await;
    tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1001)));

    // Delete track at low y position (not in exit region)
    tracker.process_event(create_event_with_pos(EventType::TrackDelete, 100, [2.0, 1.5, 1.70]));

    // Journey should be Lost (not in exit region)
    let journey = tracker.journey_manager.get_any(TrackId(100)).expect("Journey should exist");
    assert_eq!(journey.outcome, JourneyOutcome::Lost);
    assert!(!journey.exit_inferred);
}

// -----------------------------------------------------------------------------
// X-Bounds False Positive Prevention Tests
// -----------------------------------------------------------------------------

#[tokio::test]
async fn test_false_positive_prevention_store_zone() {
    // No LINE_CROSS, last_pos=(-3.0, 2.5), has_zone_events=true
    // Should result in JourneyOutcome::Lost (x too far left, in store area)
    let config = Config::default().with_min_dwell_ms(50);
    let mut tracker = create_test_tracker_with_config(config);

    // Create track far in store zone (x=-3.0, way left of exit region [1.5, 3.0])
    tracker.process_event(create_event_with_pos(EventType::TrackCreate, 100, [-3.0, 2.5, 1.70]));

    // Enter POS zone to set has_zone_events = true
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));
    tokio::time::sleep(millis(60)).await;
    tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1001)));

    // Delete track at store zone position
    tracker.process_event(create_event_with_pos(EventType::TrackDelete, 100, [-3.0, 2.5, 1.70]));

    // Journey should be Lost (x outside exit region bounds)
    let journey = tracker.journey_manager.get_any(TrackId(100)).expect("Journey should exist");
    assert_eq!(journey.outcome, JourneyOutcome::Lost);
    assert!(!journey.exit_inferred);
}

#[tokio::test]
async fn test_false_positive_prevention_pos_zone() {
    // No LINE_CROSS, last_pos=(-1.0, 2.5), has_zone_events=true
    // Should result in JourneyOutcome::Lost (x in POS area, not near exit)
    let config = Config::default().with_min_dwell_ms(50);
    let mut tracker = create_test_tracker_with_config(config);

    // Create track in POS area (x=-1.0, left of exit region [1.5, 3.0])
    tracker.process_event(create_event_with_pos(EventType::TrackCreate, 100, [-1.0, 2.5, 1.70]));

    // Enter POS zone to set has_zone_events = true
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));
    tokio::time::sleep(millis(60)).await;
    tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1001)));

    // Delete track at POS area position
    tracker.process_event(create_event_with_pos(EventType::TrackDelete, 100, [-1.0, 2.5, 1.70]));

    // Journey should be Lost (x outside exit region bounds)
    let journey = tracker.journey_manager.get_any(TrackId(100)).expect("Journey should exist");
    assert_eq!(journey.outcome, JourneyOutcome::Lost);
    assert!(!journey.exit_inferred);
}

#[tokio::test]
async fn test_correct_detection_exit_region() {
    // No LINE_CROSS, last_pos=(2.0, 2.5), has_zone_events=true
    // Should result in JourneyOutcome::Completed (x within exit bounds)
    let config = Config::default().with_min_dwell_ms(50);
    let mut tracker = create_test_tracker_with_config(config);

    // Create track in exit region (x=2.0 in [1.5, 3.0], y=2.5 > 2.3)
    tracker.process_event(create_event_with_pos(EventType::TrackCreate, 100, [2.0, 2.5, 1.70]));

    // Enter POS zone to set has_zone_events = true
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));
    tokio::time::sleep(millis(60)).await;
    tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1001)));

    // Delete track at exit region position
    tracker.process_event(create_event_with_pos(EventType::TrackDelete, 100, [2.0, 2.5, 1.70]));

    // Journey should be Completed (position-detected exit)
    let journey = tracker.journey_manager.get_any(TrackId(100)).expect("Journey should exist");
    assert_eq!(journey.outcome, JourneyOutcome::Completed);
    assert!(journey.exit_inferred);
}

// -----------------------------------------------------------------------------
// Boundary Tests
// -----------------------------------------------------------------------------

#[tokio::test]
async fn test_y_threshold_boundary() {
    // Test boundary at y=2.3 threshold
    // last_pos=(2.0, 2.29) → Lost (just below threshold)
    // last_pos=(2.0, 2.31) → Completed (just above threshold)

    // Test: y=2.29, just below threshold
    {
        let config = Config::default().with_min_dwell_ms(50);
        let mut tracker = create_test_tracker_with_config(config);

        tracker.process_event(create_event_with_pos(
            EventType::TrackCreate,
            100,
            [2.0, 2.29, 1.70],
        ));
        tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));
        tokio::time::sleep(millis(60)).await;
        tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1001)));
        tracker.process_event(create_event_with_pos(
            EventType::TrackDelete,
            100,
            [2.0, 2.29, 1.70],
        ));

        let journey = tracker.journey_manager.get_any(TrackId(100)).expect("Journey should exist");
        assert_eq!(
            journey.outcome,
            JourneyOutcome::Lost,
            "y=2.29 should be Lost (below threshold 2.3)"
        );
    }

    // Test: y=2.31, just above threshold
    {
        let config = Config::default().with_min_dwell_ms(50);
        let mut tracker = create_test_tracker_with_config(config);

        tracker.process_event(create_event_with_pos(
            EventType::TrackCreate,
            200,
            [2.0, 2.31, 1.70],
        ));
        tracker.process_event(create_event(EventType::ZoneEntry, 200, Some(1001)));
        tokio::time::sleep(millis(60)).await;
        tracker.process_event(create_event(EventType::ZoneExit, 200, Some(1001)));
        tracker.process_event(create_event_with_pos(
            EventType::TrackDelete,
            200,
            [2.0, 2.31, 1.70],
        ));

        let journey = tracker.journey_manager.get_any(TrackId(200)).expect("Journey should exist");
        assert_eq!(
            journey.outcome,
            JourneyOutcome::Completed,
            "y=2.31 should be Completed (above threshold 2.3)"
        );
        assert!(journey.exit_inferred);
    }
}

#[tokio::test]
async fn test_x_min_boundary() {
    // Test boundary at x_min=1.5 threshold
    // last_pos=(1.49, 2.5) → Lost (just below x_min)
    // last_pos=(1.51, 2.5) → Completed (just above x_min)

    // Test: x=1.49, just below x_min threshold
    {
        let config = Config::default().with_min_dwell_ms(50);
        let mut tracker = create_test_tracker_with_config(config);

        tracker.process_event(create_event_with_pos(
            EventType::TrackCreate,
            100,
            [1.49, 2.5, 1.70],
        ));
        tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));
        tokio::time::sleep(millis(60)).await;
        tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1001)));
        tracker.process_event(create_event_with_pos(
            EventType::TrackDelete,
            100,
            [1.49, 2.5, 1.70],
        ));

        let journey = tracker.journey_manager.get_any(TrackId(100)).expect("Journey should exist");
        assert_eq!(
            journey.outcome,
            JourneyOutcome::Lost,
            "x=1.49 should be Lost (below x_min 1.5)"
        );
    }

    // Test: x=1.51, just above x_min threshold
    {
        let config = Config::default().with_min_dwell_ms(50);
        let mut tracker = create_test_tracker_with_config(config);

        tracker.process_event(create_event_with_pos(
            EventType::TrackCreate,
            200,
            [1.51, 2.5, 1.70],
        ));
        tracker.process_event(create_event(EventType::ZoneEntry, 200, Some(1001)));
        tokio::time::sleep(millis(60)).await;
        tracker.process_event(create_event(EventType::ZoneExit, 200, Some(1001)));
        tracker.process_event(create_event_with_pos(
            EventType::TrackDelete,
            200,
            [1.51, 2.5, 1.70],
        ));

        let journey = tracker.journey_manager.get_any(TrackId(200)).expect("Journey should exist");
        assert_eq!(
            journey.outcome,
            JourneyOutcome::Completed,
            "x=1.51 should be Completed (at or above x_min 1.5)"
        );
        assert!(journey.exit_inferred);
    }
}

#[tokio::test]
async fn test_x_max_boundary() {
    // Test boundary at x_max=3.0 threshold
    // last_pos=(2.99, 2.5) → Completed (within x_max)
    // last_pos=(3.01, 2.5) → Lost (just above x_max)

    // Test: x=2.99, within x_max threshold
    {
        let config = Config::default().with_min_dwell_ms(50);
        let mut tracker = create_test_tracker_with_config(config);

        tracker.process_event(create_event_with_pos(
            EventType::TrackCreate,
            100,
            [2.99, 2.5, 1.70],
        ));
        tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));
        tokio::time::sleep(millis(60)).await;
        tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1001)));
        tracker.process_event(create_event_with_pos(
            EventType::TrackDelete,
            100,
            [2.99, 2.5, 1.70],
        ));

        let journey = tracker.journey_manager.get_any(TrackId(100)).expect("Journey should exist");
        assert_eq!(
            journey.outcome,
            JourneyOutcome::Completed,
            "x=2.99 should be Completed (within x_max 3.0)"
        );
        assert!(journey.exit_inferred);
    }

    // Test: x=3.01, just above x_max threshold
    {
        let config = Config::default().with_min_dwell_ms(50);
        let mut tracker = create_test_tracker_with_config(config);

        tracker.process_event(create_event_with_pos(
            EventType::TrackCreate,
            200,
            [3.01, 2.5, 1.70],
        ));
        tracker.process_event(create_event(EventType::ZoneEntry, 200, Some(1001)));
        tokio::time::sleep(millis(60)).await;
        tracker.process_event(create_event(EventType::ZoneExit, 200, Some(1001)));
        tracker.process_event(create_event_with_pos(
            EventType::TrackDelete,
            200,
            [3.01, 2.5, 1.70],
        ));

        let journey = tracker.journey_manager.get_any(TrackId(200)).expect("Journey should exist");
        assert_eq!(
            journey.outcome,
            JourneyOutcome::Lost,
            "x=3.01 should be Lost (above x_max 3.0)"
        );
    }
}

// -----------------------------------------------------------------------------
// Person Field Tests
// -----------------------------------------------------------------------------

#[tokio::test]
async fn test_max_y_tracking() {
    // Verify max_y updates on position events
    let config = Config::default().with_min_dwell_ms(50);
    let mut tracker = create_test_tracker_with_config(config);

    // Create track at y=1.0
    tracker.process_event(create_event_with_pos(EventType::TrackCreate, 100, [2.0, 1.0, 1.70]));

    // Verify initial max_y
    let person = tracker.persons.get(&TrackId(100)).unwrap();
    assert!((person.max_y - 1.0).abs() < f32::EPSILON, "Initial max_y should be 1.0");

    // Process position events with higher y values
    // Zone entry will update position implicitly via event handling
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));

    // Now delete track at higher y position - this should update max_y
    tracker.process_event(create_event_with_pos(EventType::TrackDelete, 100, [2.0, 2.5, 1.70]));

    // The person is removed on delete, but max_y was updated before removal
    // We can verify this by checking the journey outcome (position-detected exit requires high y)
    let journey = tracker.journey_manager.get_any(TrackId(100)).expect("Journey should exist");
    // Since we had zone events and ended at y=2.5 > 2.3, should be Completed
    assert_eq!(journey.outcome, JourneyOutcome::Completed);
}

#[tokio::test]
async fn test_has_zone_events_tracking() {
    // Verify has_zone_events is set on zone events
    let config = Config::default().with_min_dwell_ms(50);
    let mut tracker = create_test_tracker_with_config(config);

    // Create track
    tracker.process_event(create_event_with_pos(EventType::TrackCreate, 100, [2.0, 2.5, 1.70]));

    // Verify has_zone_events is false initially
    let person = tracker.persons.get(&TrackId(100)).unwrap();
    assert!(!person.has_zone_events, "has_zone_events should be false initially");

    // Enter a zone
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));

    // Verify has_zone_events is now true
    let person = tracker.persons.get(&TrackId(100)).unwrap();
    assert!(person.has_zone_events, "has_zone_events should be true after ZoneEntry");

    // Exit the zone
    tokio::time::sleep(millis(60)).await;
    tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1001)));

    // Verify has_zone_events is still true
    let person = tracker.persons.get(&TrackId(100)).unwrap();
    assert!(person.has_zone_events, "has_zone_events should remain true after ZoneExit");
}

// -----------------------------------------------------------------------------
// Edge Cases and Overrides
// -----------------------------------------------------------------------------

#[tokio::test]
async fn test_line_cross_overrides_position() {
    // LINE_CROSS should take precedence over position detection
    // Even if last_pos is outside exit region, line cross = Exit
    let config = Config::default().with_min_dwell_ms(50);
    let mut tracker = create_test_tracker_with_config(config);

    // Create track at position outside exit region
    tracker.process_event(create_event_with_pos(EventType::TrackCreate, 100, [0.0, 0.0, 1.70]));

    // Enter POS zone
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));
    tokio::time::sleep(millis(60)).await;
    tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1001)));

    // Enter GATE zone
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1007)));

    // Cross EXIT line forward (this should complete the journey regardless of position)
    let mut exit_event = create_event(EventType::LineCrossForward, 100, Some(1006));
    exit_event.direction = Some("forward".to_string());
    tracker.process_event(exit_event);

    // Person should be removed (journey complete via line cross)
    assert_eq!(tracker.active_tracks(), 0);

    // Journey should be Completed (via line cross, not position)
    let journey = tracker.journey_manager.get_any(TrackId(100)).expect("Journey should exist");
    assert_eq!(journey.outcome, JourneyOutcome::Completed);
    assert!(!journey.exit_inferred, "exit_inferred should be false for line cross exit");
}

#[tokio::test]
async fn test_no_last_position() {
    // Person with no last_position should be Lost (can't determine exit region)
    let config = Config::default().with_min_dwell_ms(50);
    let mut tracker = create_test_tracker_with_config(config);

    // Create track without position
    tracker.process_event(create_event(EventType::TrackCreate, 100, None));

    // Enter POS zone to set has_zone_events
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));
    tokio::time::sleep(millis(60)).await;
    tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1001)));

    // Delete track without position
    tracker.process_event(create_event(EventType::TrackDelete, 100, None));

    // Journey should be Lost (no position to determine exit region)
    let journey = tracker.journey_manager.get_any(TrackId(100)).expect("Journey should exist");
    assert_eq!(journey.outcome, JourneyOutcome::Lost);
    assert!(!journey.exit_inferred);
}

#[tokio::test]
async fn test_normal_exit_line_cross() {
    // LINE_CROSS_FORWARD received → JourneyOutcome::Completed (existing behavior)
    // Verify position detection doesn't interfere with normal line cross
    let config = Config::default().with_min_dwell_ms(50);
    let mut tracker = create_test_tracker_with_config(config);

    tracker.process_event(create_event_with_pos(EventType::TrackCreate, 100, [2.0, 2.0, 1.70]));
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1001)));
    tokio::time::sleep(millis(60)).await;
    tracker.process_event(create_event(EventType::ZoneExit, 100, Some(1001)));
    tracker.process_event(create_event(EventType::ZoneEntry, 100, Some(1007)));

    // Cross EXIT line forward
    let mut exit_event = create_event(EventType::LineCrossForward, 100, Some(1006));
    exit_event.direction = Some("forward".to_string());
    tracker.process_event(exit_event);

    // Person should be removed
    assert_eq!(tracker.active_tracks(), 0);

    let journey = tracker.journey_manager.get_any(TrackId(100)).expect("Journey should exist");
    assert_eq!(journey.outcome, JourneyOutcome::Completed);
    assert!(!journey.exit_inferred, "exit_inferred should be false for line cross");
}
