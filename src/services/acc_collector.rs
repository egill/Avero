//! ACC (payment) event collection and correlation with journeys
//!
//! Supports group detection: people who enter a POS zone while others
//! are present (within GROUP_WINDOW_MS) are considered a group.
//! When ACC matches any group member, all members are authorized.

use crate::domain::journey::{epoch_ms, JourneyEvent};
use crate::domain::types::TrackId;
use crate::infra::config::Config;
use crate::services::journey_manager::JourneyManager;
use std::collections::HashMap;
use std::time::Instant;
use tracing::{debug, info};

/// Maximum time since leaving POS to still match ACC (1.5 seconds)
const MAX_TIME_SINCE_EXIT: u64 = 1500;
/// Maximum time between entries to be considered same group (10 seconds)
const GROUP_WINDOW_MS: u64 = 10000;

/// A member of a POS group
#[derive(Debug, Clone)]
struct GroupMember {
    track_id: TrackId,
    entered_at: Instant,
}

/// A group of people at a POS zone (co-presence)
#[derive(Debug, Clone)]
struct PosGroup {
    members: Vec<GroupMember>,
    last_entry: Instant,
    min_dwell_for_acc: u64,
}

impl PosGroup {
    fn new(track_id: TrackId, min_dwell_for_acc: u64) -> Self {
        let now = Instant::now();
        Self {
            members: vec![GroupMember { track_id, entered_at: now }],
            last_entry: now,
            min_dwell_for_acc,
        }
    }

    fn add_member(&mut self, track_id: TrackId) {
        let now = Instant::now();
        self.members.push(GroupMember { track_id, entered_at: now });
        self.last_entry = now;
    }

    fn remove_member(&mut self, track_id: TrackId) {
        self.members.retain(|m| m.track_id != track_id);
    }

    fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Check if a new entry should join this group (within time window)
    fn should_join(&self) -> bool {
        self.last_entry.elapsed().as_millis() as u64 <= GROUP_WINDOW_MS
    }

    /// Get all track_ids of members with sufficient dwell
    /// Uses MAX of accumulated dwell and session dwell to handle:
    /// - Zone flicker: accumulated_dwell tracks total time even after brief exit
    /// - Still in zone: session_dwell counts while accumulated hasn't updated yet
    fn members_with_sufficient_dwell(
        &self,
        accumulated_dwells: Option<&HashMap<TrackId, u64>>,
    ) -> Vec<TrackId> {
        self.members
            .iter()
            .filter(|m| {
                let session_dwell = m.entered_at.elapsed().as_millis() as u64;
                let accumulated = accumulated_dwells
                    .and_then(|dwells| dwells.get(&m.track_id).copied())
                    .unwrap_or(0);
                // Use max of accumulated and session dwell
                // - accumulated handles zone flicker (re-entry after brief exit)
                // - session handles currently in zone (accumulated only updates on exit)
                let effective_dwell = session_dwell.max(accumulated);
                effective_dwell >= self.min_dwell_for_acc
            })
            .map(|m| m.track_id)
            .collect()
    }

    /// Get all track_ids in the group
    fn all_members(&self) -> Vec<TrackId> {
        self.members.iter().map(|m| m.track_id).collect()
    }
}

/// Tracks recent zone exits for matching
#[derive(Debug, Clone)]
struct RecentExit {
    track_id: TrackId,
    group_members: Vec<TrackId>, // Other members who were in the same group
    exited_at: Instant,
    dwell_ms: u64,
}

/// Collects ACC events and correlates them with journeys
pub struct AccCollector {
    /// IP to POS name mapping
    ip_to_pos: HashMap<String, String>,
    min_dwell_for_acc: u64,
    /// Current POS groups by zone name (co-presence tracking)
    pos_groups: HashMap<String, PosGroup>,
    /// Recent exits by zone name (for delayed matching)
    recent_exits: HashMap<String, Vec<RecentExit>>,
}

impl AccCollector {
    pub fn new(config: &Config) -> Self {
        Self {
            ip_to_pos: config.acc_ip_to_pos().clone(),
            min_dwell_for_acc: config.min_dwell_ms(),
            pos_groups: HashMap::new(),
            recent_exits: HashMap::new(),
        }
    }

    /// Record that a track entered a POS zone
    /// If others are present (within GROUP_WINDOW_MS), they form a group
    ///
    /// IMPORTANT: We never replace an existing group that has members.
    /// This prevents losing track of people who are still at the POS zone.
    pub fn record_pos_entry(&mut self, track_id: TrackId, pos_zone: &str) {
        // Debug: log current state of all POS groups
        let all_groups: Vec<(&String, Vec<TrackId>)> =
            self.pos_groups.iter().map(|(k, g)| (k, g.all_members())).collect();
        debug!(
            track_id = %track_id,
            pos = %pos_zone,
            current_groups = ?all_groups,
            "acc_pos_entry_state"
        );

        if let Some(group) = self.pos_groups.get_mut(pos_zone) {
            // Group exists - always add to it to preserve existing members
            // The should_join() check is only for logging (co-presence detection)
            let is_cogroup = group.should_join();
            if is_cogroup {
                debug!(track_id = %track_id, pos = %pos_zone, group_size = %(group.members.len() + 1), "acc_pos_entry_join_group");
            } else {
                debug!(track_id = %track_id, pos = %pos_zone, existing_members = %group.members.len(), "acc_pos_entry_add_to_existing");
            }
            group.add_member(track_id);
            return;
        }
        // No group exists - start new group
        debug!(track_id = %track_id, pos = %pos_zone, "acc_pos_entry_new_group");
        self.pos_groups.insert(
            pos_zone.to_string(),
            PosGroup::new(track_id, self.min_dwell_for_acc),
        );
    }

    /// Record that a track exited a POS zone
    pub fn record_pos_exit(&mut self, track_id: TrackId, pos_zone: &str, dwell_ms: u64) {
        // Get group members before removing this track
        let group_members =
            self.pos_groups.get(pos_zone).map(|g| g.all_members()).unwrap_or_default();

        debug!(track_id = %track_id, pos = %pos_zone, dwell_ms = %dwell_ms, group_size = %group_members.len(), "acc_pos_exit");

        // Remove from group
        if let Some(group) = self.pos_groups.get_mut(pos_zone) {
            group.remove_member(track_id);
            if group.is_empty() {
                debug!(track_id = %track_id, pos = %pos_zone, "acc_pos_group_removed_empty");
                self.pos_groups.remove(pos_zone);
            } else {
                let remaining: Vec<TrackId> = group.all_members();
                debug!(track_id = %track_id, pos = %pos_zone, remaining = ?remaining, "acc_pos_exit_group_remains");
            }
        } else {
            debug!(track_id = %track_id, pos = %pos_zone, "acc_pos_exit_no_group_found");
        }

        // Record recent exit with group info for delayed matching
        let exits = self.recent_exits.entry(pos_zone.to_string()).or_default();
        exits.push(RecentExit {
            track_id,
            group_members: group_members
                .into_iter()
                .filter(|&id| id != track_id)
                .collect(),
            exited_at: Instant::now(),
            dwell_ms,
        });
    }

    /// Process an ACC event by IP address and try to match it to a journey
    /// Returns all matched track_ids (group members)
    /// accumulated_dwells: map of track_id -> total accumulated dwell time from Person state
    pub fn process_acc(
        &mut self,
        ip: &str,
        journey_manager: &mut JourneyManager,
        accumulated_dwells: Option<&HashMap<TrackId, u64>>,
    ) -> Vec<TrackId> {
        let Some(pos) = self.ip_to_pos.get(ip).cloned() else {
            return vec![];
        };
        self.process_acc_by_pos(&pos, Some(ip), journey_manager, accumulated_dwells)
    }

    /// Process an ACC event by POS zone directly (used when kiosk_id IS the zone name)
    /// Returns all matched track_ids (entire group if any member qualifies)
    /// accumulated_dwells: map of track_id -> total accumulated dwell time from Person state
    pub fn process_acc_by_pos(
        &mut self,
        pos: &str,
        kiosk_id: Option<&str>,
        journey_manager: &mut JourneyManager,
        accumulated_dwells: Option<&HashMap<TrackId, u64>>,
    ) -> Vec<TrackId> {
        let ts = epoch_ms();
        let kiosk_str = kiosk_id.unwrap_or(pos);

        info!(kiosk = %kiosk_str, pos = %pos, "acc_event_received");

        // First try: current group at POS with at least one member having sufficient dwell
        if let Some(group) = self.pos_groups.get(pos) {
            let all_members = group.all_members();
            // Show both session dwell and accumulated dwell for debugging
            let member_dwells: Vec<(TrackId, u64, Option<u64>)> = group
                .members
                .iter()
                .map(|m| {
                    let session_dwell = m.entered_at.elapsed().as_millis() as u64;
                    let acc_dwell = accumulated_dwells.and_then(|d| d.get(&m.track_id).copied());
                    (m.track_id, session_dwell, acc_dwell)
                })
                .collect();
            debug!(
                pos = %pos,
                members = ?all_members,
                dwells_ms = ?member_dwells,
                min_dwell = %self.min_dwell_for_acc,
                "acc_checking_pos_group"
            );

            let qualified = group.members_with_sufficient_dwell(accumulated_dwells);
            if qualified.is_empty() {
                debug!(
                    pos = %pos,
                    members = ?all_members,
                    dwells_ms = ?member_dwells,
                    min_dwell = %self.min_dwell_for_acc,
                    "acc_group_exists_but_no_qualified_members"
                );
            }
            if !qualified.is_empty() {
                // At least one member qualifies - authorize entire group
                let all_members = group.all_members();
                info!(
                    pos = %pos,
                    qualified = ?qualified,
                    group = ?all_members,
                    "acc_matched_group_present"
                );

                // Update journeys for all group members
                for &track_id in &all_members {
                    if let Some(journey) = journey_manager.get_mut_any(track_id) {
                        journey.acc_matched = true;
                    }
                    journey_manager.add_event(
                        track_id,
                        JourneyEvent::new("acc", ts)
                            .with_zone(pos)
                            .with_extra(&format!("kiosk={kiosk_str},group={}", all_members.len())),
                    );
                }

                return all_members;
            }
        }

        // Second try: recently exited with sufficient dwell AND pos now empty
        if !self.pos_groups.contains_key(pos) {
            debug!(pos = %pos, "acc_pos_empty_checking_recent_exits");
            self.cleanup_old_exits();

            if let Some(exits) = self.recent_exits.get_mut(pos) {
                // Find the most recent (newest) exit with sufficient dwell
                let now = Instant::now();
                let idx = exits
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, e)| {
                        e.dwell_ms >= self.min_dwell_for_acc
                            && now.duration_since(e.exited_at).as_millis() as u64
                                <= MAX_TIME_SINCE_EXIT
                    })
                    .map(|(idx, _)| idx);

                if let Some(idx) = idx {
                    let exit = exits.swap_remove(idx);
                    let track_id = exit.track_id;
                    let time_since = now.duration_since(exit.exited_at).as_millis() as u64;

                    // Include group members who were together
                    let mut all_members = vec![track_id];
                    all_members.extend(exit.group_members.iter().copied());

                    info!(
                        track_id = %track_id,
                        pos = %pos,
                        dwell_ms = %exit.dwell_ms,
                        time_since_exit_ms = %time_since,
                        group = ?all_members,
                        "acc_matched_recent_exit_group"
                    );

                    // Update journeys for all group members
                    for &tid in &all_members {
                        if let Some(journey) = journey_manager.get_mut_any(tid) {
                            journey.acc_matched = true;
                        }
                        journey_manager.add_event(
                            tid,
                            JourneyEvent::new("acc", ts).with_zone(pos).with_extra(&format!(
                                "kiosk={kiosk_str},group={}",
                                all_members.len()
                            )),
                        );
                    }

                    return all_members;
                }
            }
        }

        // Log detailed debug info for unmatched ACC
        let all_pos_zones: Vec<&String> = self.pos_groups.keys().collect();
        let recent_exit_zones: Vec<&String> = self.recent_exits.keys().collect();
        debug!(
            pos = %pos,
            available_pos_groups = ?all_pos_zones,
            recent_exit_zones = ?recent_exit_zones,
            "acc_no_match"
        );
        vec![]
    }

    /// Clean up old exit records
    fn cleanup_old_exits(&mut self) {
        let now = Instant::now();
        for exits in self.recent_exits.values_mut() {
            exits.retain(|e| {
                now.duration_since(e.exited_at).as_millis() as u64 <= MAX_TIME_SINCE_EXIT * 2
            });
        }
    }

    /// Get the POS name for an IP address
    pub fn pos_for_ip(&self, ip: &str) -> Option<&str> {
        self.ip_to_pos.get(ip).map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::journey_manager::JourneyManager;

    fn create_test_collector() -> AccCollector {
        let mut ip_to_pos = std::collections::HashMap::new();
        ip_to_pos.insert("192.168.1.10".to_string(), "POS_1".to_string());
        ip_to_pos.insert("192.168.1.11".to_string(), "POS_2".to_string());
        let config = Config::default().with_acc_ip_to_pos(ip_to_pos);
        AccCollector::new(&config)
    }

    #[test]
    fn test_acc_match_present() {
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        // Create journey
        jm.new_journey(TrackId(100));

        // Record entry to POS with sufficient dwell
        collector.record_pos_entry(TrackId(100), "POS_1");
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Override dwell check for test - access the group's member
        if let Some(group) = collector.pos_groups.get_mut("POS_1") {
            if let Some(member) = group.members.first_mut() {
                member.entered_at = Instant::now() - std::time::Duration::from_secs(8);
            }
        }

        // Process ACC
        let result = collector.process_acc("192.168.1.10", &mut jm, None);

        assert_eq!(result, vec![TrackId(100)]);
        let journey = jm.get(TrackId(100)).unwrap();
        assert!(journey.acc_matched);
    }

    #[test]
    fn test_acc_match_recent_exit() {
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));

        // Record exit with sufficient dwell
        collector.record_pos_exit(TrackId(100), "POS_1", 8000);

        // Process ACC within time window
        let result = collector.process_acc("192.168.1.10", &mut jm, None);

        assert!(result.contains(&TrackId(100)));
    }

    #[test]
    fn test_acc_no_match_insufficient_dwell() {
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));
        collector.record_pos_entry(TrackId(100), "POS_1");

        // Process ACC immediately (insufficient dwell)
        let result = collector.process_acc("192.168.1.10", &mut jm, None);

        assert!(result.is_empty());
    }

    #[test]
    fn test_acc_no_match_unknown_ip() {
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        let result = collector.process_acc("192.168.1.99", &mut jm, None);

        assert!(result.is_empty());
    }

    #[test]
    fn test_pos_for_ip() {
        let collector = create_test_collector();

        assert_eq!(collector.pos_for_ip("192.168.1.10"), Some(&"POS_1".to_string()));
        assert_eq!(collector.pos_for_ip("192.168.1.11"), Some(&"POS_2".to_string()));
        assert_eq!(collector.pos_for_ip("192.168.1.99"), None);
    }

    #[test]
    fn test_acc_group_present() {
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));
        jm.new_journey(TrackId(200));

        // Two people enter same POS (forming a group)
        collector.record_pos_entry(TrackId(100), "POS_1");
        collector.record_pos_entry(TrackId(200), "POS_1"); // joins group

        // Override dwell for first member to qualify
        if let Some(group) = collector.pos_groups.get_mut("POS_1") {
            if let Some(member) = group.members.first_mut() {
                member.entered_at = Instant::now() - std::time::Duration::from_secs(8);
            }
        }

        // Process ACC - should match BOTH group members
        let result = collector.process_acc("192.168.1.10", &mut jm, None);

        assert_eq!(result.len(), 2);
        assert!(result.contains(&TrackId(100)));
        assert!(result.contains(&TrackId(200)));
    }

    #[test]
    fn test_acc_group_recent_exit() {
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));
        jm.new_journey(TrackId(200));

        // Two people enter same POS (forming a group)
        collector.record_pos_entry(TrackId(100), "POS_1");
        collector.record_pos_entry(TrackId(200), "POS_1");

        // Both exit
        collector.record_pos_exit(TrackId(100), "POS_1", 8000); // sufficient dwell
        collector.record_pos_exit(TrackId(200), "POS_1", 5000); // insufficient dwell alone

        // Process ACC - should match both since they were in a group
        let result = collector.process_acc("192.168.1.10", &mut jm, None);

        // The newest exit (200) is matched, along with group members
        assert!(result.contains(&TrackId(200)));
        assert!(result.contains(&TrackId(100)));
    }

    #[test]
    fn test_acc_uses_accumulated_dwell() {
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));
        collector.record_pos_entry(TrackId(100), "POS_1");

        // Without accumulated dwell, should not match (just entered)
        let result = collector.process_acc("192.168.1.10", &mut jm, None);
        assert!(result.is_empty());

        // With accumulated dwell >= min_dwell, should match even with recent entry
        let mut accumulated = HashMap::new();
        accumulated.insert(TrackId(100), 10000); // 10s accumulated dwell
        let result = collector.process_acc("192.168.1.10", &mut jm, Some(&accumulated));
        assert_eq!(result, vec![TrackId(100)]);
    }

    // ============================================================
    // US-001: ACC Collector time boundary tests
    // ============================================================

    #[test]
    fn test_group_window_boundary_within() {
        // Test GROUP_WINDOW_MS boundary: 9.999s should still group
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));
        jm.new_journey(TrackId(200));

        // First person enters
        collector.record_pos_entry(TrackId(100), "POS_1");

        // Simulate 9.999s elapsed (just within GROUP_WINDOW_MS=10000)
        if let Some(group) = collector.pos_groups.get_mut("POS_1") {
            group.last_entry = Instant::now() - std::time::Duration::from_millis(9999);
        }

        // Verify should_join returns true at 9.999s
        assert!(
            collector.pos_groups.get("POS_1").unwrap().should_join(),
            "9.999s should be within GROUP_WINDOW_MS"
        );

        // Second person enters - should join group
        collector.record_pos_entry(TrackId(200), "POS_1");

        let group = collector.pos_groups.get("POS_1").unwrap();
        assert_eq!(group.all_members().len(), 2, "Second person should join group within 9.999s");
    }

    #[test]
    fn test_group_window_boundary_outside() {
        // Test GROUP_WINDOW_MS boundary: 10.001s should NOT group (timing wise)
        // Note: In current implementation, members are still added but should_join returns false
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));
        jm.new_journey(TrackId(200));

        // First person enters
        collector.record_pos_entry(TrackId(100), "POS_1");

        // Simulate 10.001s elapsed (just outside GROUP_WINDOW_MS=10000)
        if let Some(group) = collector.pos_groups.get_mut("POS_1") {
            group.last_entry = Instant::now() - std::time::Duration::from_millis(10001);
        }

        // Verify should_join returns false at 10.001s
        assert!(
            !collector.pos_groups.get("POS_1").unwrap().should_join(),
            "10.001s should be outside GROUP_WINDOW_MS"
        );
    }

    #[test]
    fn test_recent_exit_boundary_within() {
        // Test MAX_TIME_SINCE_EXIT boundary: 1.499s should match
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));

        // Record exit with sufficient dwell
        collector.record_pos_exit(TrackId(100), "POS_1", 8000);

        // Simulate 1.499s since exit (just within MAX_TIME_SINCE_EXIT=1500)
        if let Some(exits) = collector.recent_exits.get_mut("POS_1") {
            if let Some(exit) = exits.first_mut() {
                exit.exited_at = Instant::now() - std::time::Duration::from_millis(1499);
            }
        }

        // Process ACC - should match
        let result = collector.process_acc("192.168.1.10", &mut jm, None);
        assert!(result.contains(&TrackId(100)), "Exit within 1.499s should match ACC");
    }

    #[test]
    fn test_recent_exit_boundary_outside() {
        // Test MAX_TIME_SINCE_EXIT boundary: 1.501s should NOT match
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));

        // Record exit with sufficient dwell
        collector.record_pos_exit(TrackId(100), "POS_1", 8000);

        // Simulate 1.501s since exit (just outside MAX_TIME_SINCE_EXIT=1500)
        if let Some(exits) = collector.recent_exits.get_mut("POS_1") {
            if let Some(exit) = exits.first_mut() {
                exit.exited_at = Instant::now() - std::time::Duration::from_millis(1501);
            }
        }

        // Process ACC - should NOT match
        let result = collector.process_acc("192.168.1.10", &mut jm, None);
        assert!(result.is_empty(), "Exit after 1.501s should NOT match ACC");
    }

    #[test]
    fn test_pos_occupied_blocks_recent_exit_match() {
        // When POS is occupied, recent exits should NOT be matched
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));
        jm.new_journey(TrackId(200));

        // Track 100 exits with sufficient dwell
        collector.record_pos_exit(TrackId(100), "POS_1", 8000);

        // Track 200 enters (POS now occupied)
        collector.record_pos_entry(TrackId(200), "POS_1");

        // Process ACC - should NOT match recent exit because POS is occupied
        // (and 200 doesn't have sufficient dwell yet)
        let result = collector.process_acc("192.168.1.10", &mut jm, None);

        // Current implementation: tries current group first, fails due to insufficient dwell
        // Does NOT fall back to recent exits when POS is occupied
        assert!(result.is_empty(), "Should not match recent exit when POS is currently occupied");
    }

    #[test]
    fn test_newest_exit_selected() {
        // When multiple recent exits exist, the newest one should be matched
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));
        jm.new_journey(TrackId(200));
        jm.new_journey(TrackId(300));

        // Record multiple exits in sequence
        collector.record_pos_exit(TrackId(100), "POS_1", 8000);
        collector.record_pos_exit(TrackId(200), "POS_1", 8000);
        collector.record_pos_exit(TrackId(300), "POS_1", 8000);

        // Make all exits within time window but with different ages
        if let Some(exits) = collector.recent_exits.get_mut("POS_1") {
            // 100 exited 1.4s ago
            exits[0].exited_at = Instant::now() - std::time::Duration::from_millis(1400);
            // 200 exited 1.0s ago
            exits[1].exited_at = Instant::now() - std::time::Duration::from_millis(1000);
            // 300 exited 0.5s ago (newest)
            exits[2].exited_at = Instant::now() - std::time::Duration::from_millis(500);
        }

        // Process ACC - should match newest exit (300)
        let result = collector.process_acc("192.168.1.10", &mut jm, None);
        assert!(result.contains(&TrackId(300)), "Should match newest exit (track 300)");
    }

    #[test]
    fn test_group_authorization_propagates_to_all_members() {
        // When ACC matches, ALL group members should get acc_matched=true
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));
        jm.new_journey(TrackId(200));
        jm.new_journey(TrackId(300));

        // Three people enter same POS (forming a group)
        collector.record_pos_entry(TrackId(100), "POS_1");
        collector.record_pos_entry(TrackId(200), "POS_1");
        collector.record_pos_entry(TrackId(300), "POS_1");

        // Only first member has sufficient dwell
        if let Some(group) = collector.pos_groups.get_mut("POS_1") {
            // Track 100: 8 seconds dwell (sufficient)
            group.members[0].entered_at = Instant::now() - std::time::Duration::from_secs(8);
            // Tracks 200, 300: just entered (insufficient individually)
        }

        // Process ACC - should match ALL group members
        let result = collector.process_acc("192.168.1.10", &mut jm, None);

        assert_eq!(result.len(), 3, "All 3 group members should be matched");
        assert!(result.contains(&TrackId(100)));
        assert!(result.contains(&TrackId(200)));
        assert!(result.contains(&TrackId(300)));

        // Verify all journeys have acc_matched=true
        assert!(jm.get(TrackId(100)).unwrap().acc_matched, "Track 100 should have acc_matched");
        assert!(jm.get(TrackId(200)).unwrap().acc_matched, "Track 200 should have acc_matched");
        assert!(jm.get(TrackId(300)).unwrap().acc_matched, "Track 300 should have acc_matched");
    }

    #[test]
    fn test_group_window_exact_boundary_10000ms() {
        // Test exactly at GROUP_WINDOW_MS=10000ms boundary
        let mut collector = create_test_collector();

        collector.record_pos_entry(TrackId(100), "POS_1");

        // Exactly at 10000ms - the condition is <= so this should pass
        if let Some(group) = collector.pos_groups.get_mut("POS_1") {
            group.last_entry = Instant::now() - std::time::Duration::from_millis(10000);
        }

        assert!(
            collector.pos_groups.get("POS_1").unwrap().should_join(),
            "Exactly 10000ms should be within GROUP_WINDOW_MS (uses <=)"
        );
    }

    #[test]
    fn test_recent_exit_exact_boundary_1500ms() {
        // Test exactly at MAX_TIME_SINCE_EXIT=1500ms boundary
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));
        collector.record_pos_exit(TrackId(100), "POS_1", 8000);

        // Exactly at 1500ms - the condition is <= so this should pass
        if let Some(exits) = collector.recent_exits.get_mut("POS_1") {
            if let Some(exit) = exits.first_mut() {
                exit.exited_at = Instant::now() - std::time::Duration::from_millis(1500);
            }
        }

        let result = collector.process_acc("192.168.1.10", &mut jm, None);
        assert!(result.contains(&TrackId(100)), "Exactly 1500ms should match (uses <=)");
    }
}
