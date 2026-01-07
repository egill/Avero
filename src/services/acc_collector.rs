//! ACC (payment) event collection and correlation with journeys
//!
//! Supports group detection: people who enter a POS zone while others
//! are present (within GROUP_WINDOW_MS) are considered a group.
//! When ACC matches any group member, all members are authorized.

use crate::domain::journey::{epoch_ms, JourneyEvent};
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
    track_id: i64,
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
    fn new(track_id: i64, min_dwell_for_acc: u64) -> Self {
        let now = Instant::now();
        Self {
            members: vec![GroupMember {
                track_id,
                entered_at: now,
            }],
            last_entry: now,
            min_dwell_for_acc,
        }
    }

    fn add_member(&mut self, track_id: i64) {
        let now = Instant::now();
        self.members.push(GroupMember {
            track_id,
            entered_at: now,
        });
        self.last_entry = now;
    }

    fn remove_member(&mut self, track_id: i64) {
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
    fn members_with_sufficient_dwell(&self) -> Vec<i64> {
        self.members
            .iter()
            .filter(|m| m.entered_at.elapsed().as_millis() as u64 >= self.min_dwell_for_acc)
            .map(|m| m.track_id)
            .collect()
    }

    /// Get all track_ids in the group
    fn all_members(&self) -> Vec<i64> {
        self.members.iter().map(|m| m.track_id).collect()
    }
}

/// Tracks recent zone exits for matching
#[derive(Debug, Clone)]
struct RecentExit {
    track_id: i64,
    group_members: Vec<i64>, // Other members who were in the same group
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
    pub fn record_pos_entry(&mut self, track_id: i64, pos_zone: &str) {
        // Debug: log current state of all POS groups
        let all_groups: Vec<(&String, Vec<i64>)> = self
            .pos_groups
            .iter()
            .map(|(k, g)| (k, g.all_members()))
            .collect();
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
        self.pos_groups
            .insert(pos_zone.to_string(), PosGroup::new(track_id, self.min_dwell_for_acc));
    }

    /// Record that a track exited a POS zone
    pub fn record_pos_exit(&mut self, track_id: i64, pos_zone: &str, dwell_ms: u64) {
        // Get group members before removing this track
        let group_members = self
            .pos_groups
            .get(pos_zone)
            .map(|g| g.all_members())
            .unwrap_or_default();

        debug!(track_id = %track_id, pos = %pos_zone, dwell_ms = %dwell_ms, group_size = %group_members.len(), "acc_pos_exit");

        // Remove from group
        if let Some(group) = self.pos_groups.get_mut(pos_zone) {
            group.remove_member(track_id);
            if group.is_empty() {
                debug!(track_id = %track_id, pos = %pos_zone, "acc_pos_group_removed_empty");
                self.pos_groups.remove(pos_zone);
            } else {
                let remaining: Vec<i64> = group.all_members();
                debug!(track_id = %track_id, pos = %pos_zone, remaining = ?remaining, "acc_pos_exit_group_remains");
            }
        } else {
            debug!(track_id = %track_id, pos = %pos_zone, "acc_pos_exit_no_group_found");
        }

        // Record recent exit with group info for delayed matching
        let exits = self.recent_exits.entry(pos_zone.to_string()).or_default();
        exits.push(RecentExit {
            track_id,
            group_members: group_members.into_iter().filter(|&id| id != track_id).collect(),
            exited_at: Instant::now(),
            dwell_ms,
        });
    }

    /// Process an ACC event by IP address and try to match it to a journey
    /// Returns all matched track_ids (group members)
    pub fn process_acc(&mut self, ip: &str, journey_manager: &mut JourneyManager) -> Vec<i64> {
        let Some(pos) = self.ip_to_pos.get(ip).cloned() else {
            return vec![];
        };
        self.process_acc_by_pos(&pos, Some(ip), journey_manager)
    }

    /// Process an ACC event by POS zone directly (used when kiosk_id IS the zone name)
    /// Returns all matched track_ids (entire group if any member qualifies)
    pub fn process_acc_by_pos(
        &mut self,
        pos: &str,
        kiosk_id: Option<&str>,
        journey_manager: &mut JourneyManager,
    ) -> Vec<i64> {
        let ts = epoch_ms();
        let kiosk_str = kiosk_id.unwrap_or(pos);

        info!(kiosk = %kiosk_str, pos = %pos, "acc_event_received");

        // First try: current group at POS with at least one member having sufficient dwell
        if let Some(group) = self.pos_groups.get(pos) {
            let all_members = group.all_members();
            let member_dwells: Vec<(i64, u64)> = group.members
                .iter()
                .map(|m| (m.track_id, m.entered_at.elapsed().as_millis() as u64))
                .collect();
            debug!(
                pos = %pos,
                members = ?all_members,
                dwells_ms = ?member_dwells,
                min_dwell = %self.min_dwell_for_acc,
                "acc_checking_pos_group"
            );

            let qualified = group.members_with_sufficient_dwell();
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
        if self.pos_groups.get(pos).is_none() {
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
                    let exit = exits.remove(idx);
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
                            JourneyEvent::new("acc", ts)
                                .with_zone(pos)
                                .with_extra(&format!("kiosk={kiosk_str},group={}", all_members.len())),
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
    pub fn pos_for_ip(&self, ip: &str) -> Option<&String> {
        self.ip_to_pos.get(ip)
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
        jm.new_journey(100);

        // Record entry to POS with sufficient dwell
        collector.record_pos_entry(100, "POS_1");
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Override dwell check for test - access the group's member
        if let Some(group) = collector.pos_groups.get_mut("POS_1") {
            if let Some(member) = group.members.first_mut() {
                member.entered_at = Instant::now() - std::time::Duration::from_secs(8);
            }
        }

        // Process ACC
        let result = collector.process_acc("192.168.1.10", &mut jm);

        assert_eq!(result, vec![100]);
        let journey = jm.get(100).unwrap();
        assert!(journey.acc_matched);
    }

    #[test]
    fn test_acc_match_recent_exit() {
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(100);

        // Record exit with sufficient dwell
        collector.record_pos_exit(100, "POS_1", 8000);

        // Process ACC within time window
        let result = collector.process_acc("192.168.1.10", &mut jm);

        assert!(result.contains(&100));
    }

    #[test]
    fn test_acc_no_match_insufficient_dwell() {
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(100);
        collector.record_pos_entry(100, "POS_1");

        // Process ACC immediately (insufficient dwell)
        let result = collector.process_acc("192.168.1.10", &mut jm);

        assert!(result.is_empty());
    }

    #[test]
    fn test_acc_no_match_unknown_ip() {
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        let result = collector.process_acc("192.168.1.99", &mut jm);

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

        jm.new_journey(100);
        jm.new_journey(200);

        // Two people enter same POS (forming a group)
        collector.record_pos_entry(100, "POS_1");
        collector.record_pos_entry(200, "POS_1"); // joins group

        // Override dwell for first member to qualify
        if let Some(group) = collector.pos_groups.get_mut("POS_1") {
            if let Some(member) = group.members.first_mut() {
                member.entered_at = Instant::now() - std::time::Duration::from_secs(8);
            }
        }

        // Process ACC - should match BOTH group members
        let result = collector.process_acc("192.168.1.10", &mut jm);

        assert_eq!(result.len(), 2);
        assert!(result.contains(&100));
        assert!(result.contains(&200));
    }

    #[test]
    fn test_acc_group_recent_exit() {
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(100);
        jm.new_journey(200);

        // Two people enter same POS (forming a group)
        collector.record_pos_entry(100, "POS_1");
        collector.record_pos_entry(200, "POS_1");

        // Both exit
        collector.record_pos_exit(100, "POS_1", 8000); // sufficient dwell
        collector.record_pos_exit(200, "POS_1", 5000); // insufficient dwell alone

        // Process ACC - should match both since they were in a group
        let result = collector.process_acc("192.168.1.10", &mut jm);

        // The newest exit (200) is matched, along with group members
        assert!(result.contains(&200));
        assert!(result.contains(&100));
    }
}
