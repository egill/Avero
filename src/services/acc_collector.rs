//! ACC (payment) event collection and correlation with journeys
//!
//! Supports group detection: people who enter a POS zone while others
//! are present (within GROUP_WINDOW_MS) are considered a group.
//! When ACC matches any group member, all members are authorized.

use crate::domain::journey::{epoch_ms, JourneyEvent, JourneyEventType};
use crate::domain::types::TrackId;
use crate::infra::config::{AccGroupingStrategy, Config};
use crate::infra::metrics::Metrics;
use crate::services::journey_manager::JourneyManager;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info};

/// Maximum time between entries to be considered same group (10 seconds)
const GROUP_WINDOW_MS: u64 = 10000;
/// Retain POS sessions for a short window to evaluate focus
const POS_SESSION_RETENTION_S: u64 = 120;
/// Limit stored sessions per track to avoid unbounded growth
const MAX_POS_SESSIONS_PER_TRACK: usize = 20;

/// A member of a POS group
#[derive(Debug, Clone)]
struct GroupMember {
    track_id: TrackId,
}

/// A group of people at a POS zone (co-presence)
#[derive(Debug, Clone)]
struct PosGroup {
    members: Vec<GroupMember>,
    last_entry: Instant,
}

impl PosGroup {
    fn new(track_id: TrackId) -> Self {
        let now = Instant::now();
        Self { members: vec![GroupMember { track_id }], last_entry: now }
    }

    fn add_member(&mut self, track_id: TrackId) {
        if self.members.iter().any(|m| m.track_id == track_id) {
            self.last_entry = Instant::now();
            return;
        }
        self.members.push(GroupMember { track_id });
        self.last_entry = Instant::now();
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

    /// Get all track_ids in the group
    fn all_members(&self) -> Vec<TrackId> {
        self.members.iter().map(|m| m.track_id).collect()
    }
}

#[derive(Debug, Clone)]
struct PosSession {
    zone: String,
    entered_at: Instant,
    exited_at: Option<Instant>,
}

/// Tracks recent zone exits for matching
#[derive(Debug, Clone)]
struct RecentExit {
    track_id: TrackId,
    exited_at: Instant,
    dwell_ms: u64,
}

/// Exit time info captured when primary is selected from recent_exits.
/// Used for overlap-based expansion of group members.
#[derive(Debug, Clone)]
struct ExitSessionInfo {
    exited_at: Instant,
}

/// Collects ACC events and correlates them with journeys
pub struct AccCollector {
    /// IP to POS name mapping
    ip_to_pos: HashMap<String, String>,
    min_dwell_for_acc: u64,
    grouping_strategy: AccGroupingStrategy,
    grouping_entry_spread_s: u64,
    grouping_other_pos_window_s: u64,
    grouping_other_pos_min_s: u64,
    grouping_flicker_merge_s: u64,
    recent_exit_window_ms: u64,
    grouping_overlap_grace_s: u64,
    grouping_overlap_soft_dwell_ms: u64,
    /// Current POS groups by zone name (co-presence tracking)
    pos_groups: HashMap<String, PosGroup>,
    /// Recent POS sessions per track for focus grouping
    pos_sessions: HashMap<TrackId, Vec<PosSession>>,
    /// Recent exits by zone name (for delayed matching)
    recent_exits: HashMap<String, Vec<RecentExit>>,
    /// Last time each POS zone became empty (for debugging ACC timing)
    last_pos_exit: HashMap<String, Instant>,
    /// Metrics collector for ACC empty POS timing
    metrics: Arc<Metrics>,
}

impl AccCollector {
    pub fn new(config: &Config, metrics: Arc<Metrics>) -> Self {
        Self {
            ip_to_pos: config.acc_ip_to_pos().clone(),
            min_dwell_for_acc: config.min_dwell_ms(),
            grouping_strategy: config.acc_grouping_strategy(),
            grouping_entry_spread_s: config.acc_grouping_entry_spread_s(),
            grouping_other_pos_window_s: config.acc_grouping_other_pos_window_s(),
            grouping_other_pos_min_s: config.acc_grouping_other_pos_min_s(),
            grouping_flicker_merge_s: config.acc_grouping_flicker_merge_s(),
            recent_exit_window_ms: config.acc_recent_exit_window_ms(),
            grouping_overlap_grace_s: config.acc_grouping_overlap_grace_s(),
            grouping_overlap_soft_dwell_ms: config.acc_grouping_overlap_soft_dwell_ms(),
            pos_groups: HashMap::new(),
            pos_sessions: HashMap::new(),
            recent_exits: HashMap::new(),
            last_pos_exit: HashMap::new(),
            metrics,
        }
    }

    /// Record that a track entered a POS zone
    /// If others are present (within GROUP_WINDOW_MS), they form a group
    ///
    /// IMPORTANT: We never replace an existing group that has members.
    /// This prevents losing track of people who are still at the POS zone.
    pub fn record_pos_entry(&mut self, track_id: TrackId, pos_zone: &str) {
        self.record_pos_session_entry(track_id, pos_zone);
        let all_groups: Vec<_> =
            self.pos_groups.iter().map(|(k, g)| (k, g.all_members())).collect();
        debug!(
            track_id = %track_id,
            pos = %pos_zone,
            current_groups = ?all_groups,
            "acc_pos_entry_state"
        );

        match self.pos_groups.get_mut(pos_zone) {
            Some(group) => {
                // Group exists - always add to preserve existing members
                // The should_join() check is only for logging (co-presence detection)
                if group.should_join() {
                    debug!(track_id = %track_id, pos = %pos_zone, group_size = %(group.members.len() + 1), "acc_pos_entry_join_group");
                } else {
                    debug!(track_id = %track_id, pos = %pos_zone, existing_members = %group.members.len(), "acc_pos_entry_add_to_existing");
                }
                group.add_member(track_id);
            }
            None => {
                debug!(track_id = %track_id, pos = %pos_zone, "acc_pos_entry_new_group");
                self.pos_groups.insert(pos_zone.to_string(), PosGroup::new(track_id));
            }
        }
    }

    /// Record that a track exited a POS zone
    pub fn record_pos_exit(&mut self, track_id: TrackId, pos_zone: &str, dwell_ms: u64) {
        self.record_pos_session_exit(track_id, pos_zone);
        let group_size = self.pos_groups.get(pos_zone).map(|g| g.all_members().len()).unwrap_or(0);

        debug!(track_id = %track_id, pos = %pos_zone, dwell_ms = %dwell_ms, group_size = %group_size, "acc_pos_exit");

        match self.pos_groups.get_mut(pos_zone) {
            Some(group) => {
                group.remove_member(track_id);
                if group.is_empty() {
                    debug!(track_id = %track_id, pos = %pos_zone, "acc_pos_group_removed_empty");
                    self.pos_groups.remove(pos_zone);
                    // Track when this POS became empty for ACC timing diagnostics
                    self.last_pos_exit.insert(pos_zone.to_string(), Instant::now());
                } else {
                    let remaining = group.all_members();
                    debug!(track_id = %track_id, pos = %pos_zone, remaining = ?remaining, "acc_pos_exit_group_remains");
                }
            }
            None => {
                debug!(track_id = %track_id, pos = %pos_zone, "acc_pos_exit_no_group_found");
            }
        }

        self.recent_exits.entry(pos_zone.to_string()).or_default().push(RecentExit {
            track_id,
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

        self.cleanup_old_exits();
        let mut primary: Option<TrackId> = None;
        let mut primary_from_present = false;
        let mut exit_session_info: Option<ExitSessionInfo> = None;

        // First try: current group at POS with sufficient dwell
        if let Some(group) = self.pos_groups.get(pos) {
            let now = Instant::now();
            let all_members = group.all_members();
            let member_dwells: Vec<(TrackId, Option<u64>, Option<u64>)> = group
                .members
                .iter()
                .map(|m| {
                    let session_dwell = self.session_dwell_ms(m.track_id, pos, now);
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

            let mut best: Option<(TrackId, u64)> = None;
            for member in &group.members {
                let Some(effective_dwell) =
                    self.effective_dwell_ms(member.track_id, pos, now, accumulated_dwells)
                else {
                    continue;
                };
                if effective_dwell < self.min_dwell_for_acc {
                    continue;
                }
                let dominated =
                    matches!(best, Some((_, best_dwell)) if effective_dwell <= best_dwell);
                if !dominated {
                    best = Some((member.track_id, effective_dwell));
                }
            }

            if let Some((track_id, _)) = best {
                primary = Some(track_id);
                primary_from_present = true;
            } else {
                debug!(
                    pos = %pos,
                    members = ?all_members,
                    dwells_ms = ?member_dwells,
                    min_dwell = %self.min_dwell_for_acc,
                    "acc_group_exists_but_no_qualified_members"
                );
            }
        }

        // Second try: recently exited with sufficient dwell
        if primary.is_none() {
            if !self.pos_groups.contains_key(pos) {
                let empty_since_ms = self
                    .last_pos_exit
                    .get(pos)
                    .map(|t| t.elapsed().as_millis() as u64)
                    .unwrap_or(0);
                info!(pos = %pos, empty_since_ms = %empty_since_ms, "acc_arrived_pos_empty");
                self.metrics.record_acc_empty_pos_time(empty_since_ms);
            }

            // Find best candidate from recent exits (immutable borrow first)
            let now = Instant::now();
            let min_dwell = self.min_dwell_for_acc;
            let recent_exit_window = self.recent_exit_window_ms;
            let mut idx = None;

            if let Some(exits) = self.recent_exits.get(pos) {
                for (candidate_idx, exit) in exits.iter().enumerate().rev() {
                    if exit.dwell_ms < min_dwell {
                        continue;
                    }
                    if now.duration_since(exit.exited_at).as_millis() as u64 > recent_exit_window {
                        continue;
                    }
                    if self.grouping_other_pos_min_s > 0 {
                        let other_pos_total_s = self.other_pos_total_s(exit.track_id, pos, now);
                        if other_pos_total_s >= self.grouping_other_pos_min_s {
                            continue;
                        }
                    }
                    idx = Some(candidate_idx);
                    break;
                }
            }

            // Now mutably remove the selected candidate and capture session info
            if let Some(idx) = idx {
                if let Some(exits) = self.recent_exits.get_mut(pos) {
                    let exit = exits.swap_remove(idx);
                    let time_since = now.duration_since(exit.exited_at).as_millis() as u64;

                    info!(
                        track_id = %exit.track_id,
                        pos = %pos,
                        dwell_ms = %exit.dwell_ms,
                        time_since_exit_ms = %time_since,
                        "acc_matched_recent_exit_primary"
                    );

                    let exited_at = self
                        .last_pos_session(exit.track_id, pos)
                        .and_then(|s| s.exited_at)
                        .unwrap_or(exit.exited_at);
                    exit_session_info = Some(ExitSessionInfo { exited_at });

                    primary = Some(exit.track_id);
                }
            }
        }

        let Some(primary) = primary else {
            let all_pos_zones: Vec<_> = self.pos_groups.keys().collect();
            let recent_exit_zones: Vec<_> = self.recent_exits.keys().collect();
            debug!(
                pos = %pos,
                available_pos_groups = ?all_pos_zones,
                recent_exit_zones = ?recent_exit_zones,
                "acc_no_match"
            );
            return vec![];
        };

        let mut matched = vec![primary];
        if let Some(group) = self.pos_groups.get(pos) {
            let mut expanded = self.expand_group_members(
                pos,
                primary,
                group,
                accumulated_dwells,
                primary_from_present,
                exit_session_info.as_ref(),
            );
            matched.append(&mut expanded);
        }

        let mut seen = HashSet::new();
        matched.retain(|tid| seen.insert(*tid));

        if primary_from_present {
            info!(pos = %pos, primary = %primary, group = ?matched, "acc_matched_primary_present");
        } else {
            info!(pos = %pos, primary = %primary, group = ?matched, "acc_matched_primary_recent_exit");
        }

        for &track_id in &matched {
            if let Some(journey) = journey_manager.get_mut_any(track_id) {
                journey.acc_matched = true;
            }
            journey_manager.add_event(
                track_id,
                JourneyEvent::new(JourneyEventType::Acc, ts)
                    .with_zone(pos)
                    .with_extra(&format!("kiosk={kiosk_str},group={}", matched.len())),
            );
        }

        matched
    }

    fn expand_group_members(
        &self,
        pos: &str,
        primary: TrackId,
        group: &PosGroup,
        accumulated_dwells: Option<&HashMap<TrackId, u64>>,
        primary_from_present: bool,
        exit_session: Option<&ExitSessionInfo>,
    ) -> Vec<TrackId> {
        // For recent-exit primary, use overlap-based expansion
        if !primary_from_present {
            if self.grouping_strategy == AccGroupingStrategy::Legacy {
                return Vec::new();
            }
            // Require exit session info for overlap-based expansion
            let Some(exit_info) = exit_session else {
                return Vec::new();
            };
            return self.members_overlapping_with_exit(pos, group, exit_info, accumulated_dwells);
        }

        // For present primary, use strategy-based expansion
        let mut expanded = match self.grouping_strategy {
            AccGroupingStrategy::Legacy => group.all_members(),
            AccGroupingStrategy::PresentDwell => {
                self.present_dwell_members(pos, group, accumulated_dwells)
            }
            AccGroupingStrategy::FlickerFocusSoft => self.flicker_focus_members(pos, group),
        };

        if !expanded.contains(&primary) {
            expanded.clear();
        }

        expanded
    }

    fn session_dwell_ms(&self, track_id: TrackId, pos: &str, now: Instant) -> Option<u64> {
        self.current_pos_entry(track_id, pos)
            .map(|entry| now.duration_since(entry).as_millis() as u64)
    }

    fn effective_dwell_ms(
        &self,
        track_id: TrackId,
        pos: &str,
        now: Instant,
        accumulated_dwells: Option<&HashMap<TrackId, u64>>,
    ) -> Option<u64> {
        let acc_dwell = accumulated_dwells.and_then(|d| d.get(&track_id).copied()).unwrap_or(0);
        match self.session_dwell_ms(track_id, pos, now) {
            Some(session_dwell) => Some(session_dwell.max(acc_dwell)),
            None if acc_dwell > 0 => Some(acc_dwell),
            None => None,
        }
    }

    fn present_dwell_members(
        &self,
        pos: &str,
        group: &PosGroup,
        accumulated_dwells: Option<&HashMap<TrackId, u64>>,
    ) -> Vec<TrackId> {
        let now = Instant::now();
        let mut candidates: Vec<(TrackId, Instant)> = Vec::new();

        for member in &group.members {
            let Some(entry_at) = self.current_pos_entry(member.track_id, pos) else {
                continue;
            };
            let effective = self.effective_dwell_ms(member.track_id, pos, now, accumulated_dwells);
            let Some(effective) = effective else {
                continue;
            };
            if effective >= self.min_dwell_for_acc {
                candidates.push((member.track_id, entry_at));
            }
        }

        // Single member doesn't need entry-spread check
        if candidates.len() < 2 {
            return candidates.into_iter().map(|(tid, _)| tid).collect();
        }

        // Enforce entry-spread: all qualified members must have entered within the spread window
        let entry_times: Vec<_> = candidates.iter().map(|(_, entry)| *entry).collect();
        let min_entry = *entry_times.iter().min().unwrap();
        let max_entry = *entry_times.iter().max().unwrap();
        let spread_s = max_entry.duration_since(min_entry).as_secs();
        if spread_s > self.grouping_entry_spread_s {
            return vec![];
        }

        candidates.into_iter().map(|(tid, _)| tid).collect()
    }

    fn flicker_focus_members(&self, pos: &str, group: &PosGroup) -> Vec<TrackId> {
        let now = Instant::now();
        let mut candidates: Vec<(TrackId, Instant)> = Vec::new();
        for member in &group.members {
            let Some(entry_at) = self.current_pos_entry(member.track_id, pos) else {
                continue;
            };
            let dwell_ms = now.duration_since(entry_at).as_millis() as u64;
            if dwell_ms < self.min_dwell_for_acc {
                continue;
            }
            let other_pos_total_s = self.other_pos_total_s(member.track_id, pos, now);
            if self.grouping_other_pos_min_s > 0
                && other_pos_total_s >= self.grouping_other_pos_min_s
            {
                continue;
            }
            candidates.push((member.track_id, entry_at));
        }

        if candidates.len() < 2 {
            return vec![];
        }

        let entry_times: Vec<_> = candidates.iter().map(|(_, entry)| *entry).collect();
        let min_entry = *entry_times.iter().min().unwrap();
        let max_entry = *entry_times.iter().max().unwrap();
        let spread_s = max_entry.duration_since(min_entry).as_secs();
        if spread_s > self.grouping_entry_spread_s {
            return vec![];
        }

        candidates.iter().map(|(tid, _)| *tid).collect()
    }

    fn record_pos_session_entry(&mut self, track_id: TrackId, pos_zone: &str) -> Instant {
        let now = Instant::now();
        let sessions = self.pos_sessions.entry(track_id).or_default();

        // Check if we can reuse or merge with existing session
        // Extract result before calling prune to avoid borrow conflict
        let merge_result = if let Some(last) = sessions.last_mut() {
            if last.zone != pos_zone {
                None
            } else if last.exited_at.is_none() {
                // Already in this zone with open session - reuse it
                Some(last.entered_at)
            } else if let Some(exited_at) = last.exited_at {
                let gap_s = now.duration_since(exited_at).as_secs();
                if gap_s <= self.grouping_flicker_merge_s {
                    // Recent exit from same zone - merge by reopening session
                    last.exited_at = None;
                    Some(last.entered_at)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        // Borrow of `last` released here
        let entry_time = match merge_result {
            Some(entered_at) => entered_at,
            None => {
                sessions.push(PosSession {
                    zone: pos_zone.to_string(),
                    entered_at: now,
                    exited_at: None,
                });
                now
            }
        };

        Self::prune_old_sessions(sessions, now);
        entry_time
    }

    fn record_pos_session_exit(&mut self, track_id: TrackId, pos_zone: &str) {
        let now = Instant::now();
        if let Some(sessions) = self.pos_sessions.get_mut(&track_id) {
            for session in sessions.iter_mut().rev() {
                if session.zone == pos_zone && session.exited_at.is_none() {
                    session.exited_at = Some(now);
                    break;
                }
            }
            Self::prune_old_sessions(sessions, now);
        }
    }

    fn prune_old_sessions(sessions: &mut Vec<PosSession>, now: Instant) {
        // Remove sessions that exited more than RETENTION_S ago
        sessions.retain(|session| match session.exited_at {
            Some(exit) => now.duration_since(exit).as_secs() <= POS_SESSION_RETENTION_S,
            None => true,
        });

        // Cap session count by removing oldest (sessions are chronologically ordered)
        if sessions.len() > MAX_POS_SESSIONS_PER_TRACK {
            let excess = sessions.len() - MAX_POS_SESSIONS_PER_TRACK;
            sessions.drain(0..excess);
        }
    }

    fn current_pos_entry(&self, track_id: TrackId, pos_zone: &str) -> Option<Instant> {
        let sessions = self.pos_sessions.get(&track_id)?;
        sessions
            .iter()
            .rev()
            .find(|session| session.zone == pos_zone && session.exited_at.is_none())
            .map(|session| session.entered_at)
    }

    /// Get the most recent closed session for a track at a given zone
    fn last_pos_session(&self, track_id: TrackId, pos_zone: &str) -> Option<&PosSession> {
        let sessions = self.pos_sessions.get(&track_id)?;
        sessions
            .iter()
            .rev()
            .find(|session| session.zone == pos_zone && session.exited_at.is_some())
    }

    /// Get members who overlapped with the exiting primary's session.
    /// Applies soft dwell threshold, other_pos filter, and entry-spread check.
    fn members_overlapping_with_exit(
        &self,
        pos: &str,
        group: &PosGroup,
        exit_info: &ExitSessionInfo,
        accumulated_dwells: Option<&HashMap<TrackId, u64>>,
    ) -> Vec<TrackId> {
        let grace = std::time::Duration::from_secs(self.grouping_overlap_grace_s);
        let primary_exit_with_grace = exit_info.exited_at + grace;
        let now = Instant::now();

        let candidates: Vec<(TrackId, Instant)> = group
            .members
            .iter()
            .filter_map(|m| {
                let member_entry = self.current_pos_entry(m.track_id, pos)?;

                // Check interval overlap: member must have entered before primary exited (with grace)
                if member_entry > primary_exit_with_grace {
                    return None;
                }

                // Check soft dwell (uses accumulated dwell for flickering tracks)
                let dwell = self.effective_dwell_ms(m.track_id, pos, now, accumulated_dwells)?;
                if dwell < self.grouping_overlap_soft_dwell_ms {
                    return None;
                }

                // Check other_pos activity
                if self.grouping_other_pos_min_s > 0
                    && self.other_pos_total_s(m.track_id, pos, now) >= self.grouping_other_pos_min_s
                {
                    return None;
                }

                Some((m.track_id, member_entry))
            })
            .collect();

        if candidates.len() < 2 {
            return candidates.into_iter().map(|(tid, _)| tid).collect();
        }

        // Enforce entry-spread: all candidates must have entered within the spread window
        let min_entry = candidates.iter().map(|(_, e)| *e).min().unwrap();
        let max_entry = candidates.iter().map(|(_, e)| *e).max().unwrap();
        let spread_s = max_entry.duration_since(min_entry).as_secs();
        if spread_s > self.grouping_entry_spread_s {
            return vec![];
        }

        candidates.into_iter().map(|(tid, _)| tid).collect()
    }

    fn other_pos_total_s(&self, track_id: TrackId, pos_zone: &str, now: Instant) -> u64 {
        let Some(sessions) = self.pos_sessions.get(&track_id) else {
            return 0;
        };
        let window_start = now - std::time::Duration::from_secs(self.grouping_other_pos_window_s);

        sessions
            .iter()
            .filter(|s| s.zone != pos_zone)
            .map(|session| {
                let start = session.entered_at.max(window_start);
                let end = session.exited_at.unwrap_or(now).min(now);
                if end > start {
                    end.duration_since(start).as_secs()
                } else {
                    0
                }
            })
            .sum()
    }

    /// Clean up old exit records
    fn cleanup_old_exits(&mut self) {
        let now = Instant::now();
        let retention_ms = self.recent_exit_window_ms * 2;
        for exits in self.recent_exits.values_mut() {
            exits.retain(|e| now.duration_since(e.exited_at).as_millis() as u64 <= retention_ms);
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
        let metrics = Arc::new(Metrics::new());
        AccCollector::new(&config, metrics)
    }

    fn create_test_collector_with_strategy(strategy: AccGroupingStrategy) -> AccCollector {
        let mut ip_to_pos = std::collections::HashMap::new();
        ip_to_pos.insert("192.168.1.10".to_string(), "POS_1".to_string());
        ip_to_pos.insert("192.168.1.11".to_string(), "POS_2".to_string());
        let config = Config::default()
            .with_acc_ip_to_pos(ip_to_pos)
            .with_min_dwell_ms(1000)
            .with_acc_grouping_strategy(strategy)
            .with_acc_grouping_entry_spread_s(10)
            .with_acc_grouping_other_pos_window_s(30)
            .with_acc_grouping_other_pos_min_s(2)
            .with_acc_grouping_flicker_merge_s(10);
        let metrics = Arc::new(Metrics::new());
        AccCollector::new(&config, metrics)
    }

    /// Create a test collector with production-like dwell thresholds
    /// min_dwell = 7000ms (7s), soft_dwell = 3000ms (3s - default)
    fn create_test_collector_for_soft_dwell_tests() -> AccCollector {
        let mut ip_to_pos = std::collections::HashMap::new();
        ip_to_pos.insert("192.168.1.10".to_string(), "POS_1".to_string());
        ip_to_pos.insert("192.168.1.11".to_string(), "POS_2".to_string());
        let config = Config::default()
            .with_acc_ip_to_pos(ip_to_pos)
            .with_min_dwell_ms(7000) // Production-like: 7s min_dwell
            .with_acc_grouping_strategy(AccGroupingStrategy::FlickerFocusSoft)
            .with_acc_grouping_entry_spread_s(10)
            .with_acc_grouping_other_pos_window_s(30)
            .with_acc_grouping_other_pos_min_s(2)
            .with_acc_grouping_flicker_merge_s(10);
        // soft_dwell defaults to 3000ms (3s)
        let metrics = Arc::new(Metrics::new());
        AccCollector::new(&config, metrics)
    }

    fn set_pos_entry_time(
        collector: &mut AccCollector,
        track_id: TrackId,
        pos: &str,
        entered_at: Instant,
    ) {
        let sessions = collector.pos_sessions.get_mut(&track_id).expect("missing pos session");
        let session = sessions
            .iter_mut()
            .rev()
            .find(|session| session.zone == pos && session.exited_at.is_none())
            .expect("missing open pos session");
        session.entered_at = entered_at;
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

        set_pos_entry_time(
            &mut collector,
            TrackId(100),
            "POS_1",
            Instant::now() - std::time::Duration::from_secs(8),
        );

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

        assert_eq!(collector.pos_for_ip("192.168.1.10"), Some("POS_1"));
        assert_eq!(collector.pos_for_ip("192.168.1.11"), Some("POS_2"));
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

        set_pos_entry_time(
            &mut collector,
            TrackId(100),
            "POS_1",
            Instant::now() - std::time::Duration::from_secs(8),
        );

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

        // Process ACC - should match most recent exit with sufficient dwell
        let result = collector.process_acc("192.168.1.10", &mut jm, None);

        assert_eq!(result, vec![TrackId(100)]);
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
        // Test MAX_TIME_SINCE_EXIT boundary: 2.999s should match
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));

        // Record exit with sufficient dwell
        collector.record_pos_exit(TrackId(100), "POS_1", 8000);

        // Simulate 2.999s since exit (just within MAX_TIME_SINCE_EXIT=3000)
        if let Some(exits) = collector.recent_exits.get_mut("POS_1") {
            if let Some(exit) = exits.first_mut() {
                exit.exited_at = Instant::now() - std::time::Duration::from_millis(2999);
            }
        }

        // Process ACC - should match
        let result = collector.process_acc("192.168.1.10", &mut jm, None);
        assert!(result.contains(&TrackId(100)), "Exit within 2.999s should match ACC");
    }

    #[test]
    fn test_recent_exit_boundary_outside() {
        // Test MAX_TIME_SINCE_EXIT boundary: 3.001s should NOT match
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));

        // Record exit with sufficient dwell
        collector.record_pos_exit(TrackId(100), "POS_1", 8000);

        // Simulate 3.001s since exit (just outside MAX_TIME_SINCE_EXIT=3000)
        if let Some(exits) = collector.recent_exits.get_mut("POS_1") {
            if let Some(exit) = exits.first_mut() {
                exit.exited_at = Instant::now() - std::time::Duration::from_millis(3001);
            }
        }

        // Process ACC - should NOT match
        let result = collector.process_acc("192.168.1.10", &mut jm, None);
        assert!(result.is_empty(), "Exit after 3.001s should NOT match ACC");
    }

    #[test]
    fn test_pos_occupied_blocks_recent_exit_match() {
        // When POS is occupied but no present matches, recent exits can be matched
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));
        jm.new_journey(TrackId(200));

        // Track 100 exits with sufficient dwell
        collector.record_pos_exit(TrackId(100), "POS_1", 8000);

        // Track 200 enters (POS now occupied)
        collector.record_pos_entry(TrackId(200), "POS_1");

        // Process ACC - should match recent exit because present dwell is insufficient
        let result = collector.process_acc("192.168.1.10", &mut jm, None);

        assert_eq!(result, vec![TrackId(100)]);
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

        // Track 100: 8 seconds dwell (sufficient). Tracks 200, 300: just entered.
        set_pos_entry_time(
            &mut collector,
            TrackId(100),
            "POS_1",
            Instant::now() - std::time::Duration::from_secs(8),
        );

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
    fn test_recent_exit_exact_boundary_3000ms() {
        // Test exactly at MAX_TIME_SINCE_EXIT=3000ms boundary
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));
        collector.record_pos_exit(TrackId(100), "POS_1", 8000);

        // Exactly at 3000ms - the condition is <= so this should pass
        if let Some(exits) = collector.recent_exits.get_mut("POS_1") {
            if let Some(exit) = exits.first_mut() {
                exit.exited_at = Instant::now() - std::time::Duration::from_millis(3000);
            }
        }

        let result = collector.process_acc("192.168.1.10", &mut jm, None);
        assert!(result.contains(&TrackId(100)), "Exactly 3000ms should match (uses <=)");
    }

    #[test]
    fn test_recent_exit_not_included_when_group_present() {
        // Track A exits; Track B remains present when ACC arrives.
        // Only present track should be authorized.
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100)); // Track A (will exit before ACC)
        jm.new_journey(TrackId(200)); // Track B (still present when ACC arrives)

        // Both enter POS_1 forming a group
        collector.record_pos_entry(TrackId(100), "POS_1");
        collector.record_pos_entry(TrackId(200), "POS_1");

        // Verify group formed
        assert_eq!(
            collector.pos_groups.get("POS_1").unwrap().all_members().len(),
            2,
            "Both tracks should be in group"
        );

        set_pos_entry_time(
            &mut collector,
            TrackId(100),
            "POS_1",
            Instant::now() - std::time::Duration::from_secs(8),
        );
        set_pos_entry_time(
            &mut collector,
            TrackId(200),
            "POS_1",
            Instant::now() - std::time::Duration::from_secs(8),
        );

        // Track 100 exits (goes to recent_exits)
        collector.record_pos_exit(TrackId(100), "POS_1", 120000); // 2 min dwell

        // Verify: Track 100 is in recent_exits, Track 200 still in group
        assert_eq!(
            collector.pos_groups.get("POS_1").unwrap().all_members(),
            vec![TrackId(200)],
            "Only track 200 should remain in group"
        );
        assert!(
            collector.recent_exits.get("POS_1").unwrap().iter().any(|e| e.track_id == TrackId(100)),
            "Track 100 should be in recent_exits"
        );

        // Process ACC (simulates payment terminal)
        let result = collector.process_acc("192.168.1.10", &mut jm, None);

        assert_eq!(result, vec![TrackId(200)]);

        // Verify journeys updated
        assert!(
            !jm.get(TrackId(100)).unwrap().acc_matched,
            "Track 100 journey should NOT have acc_matched"
        );
        assert!(
            jm.get(TrackId(200)).unwrap().acc_matched,
            "Track 200 journey should have acc_matched"
        );
    }

    #[test]
    fn test_recent_exit_not_matched_if_outside_time_window() {
        // Similar to above but exit is too old - should NOT be included
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));
        jm.new_journey(TrackId(200));

        collector.record_pos_entry(TrackId(100), "POS_1");
        collector.record_pos_entry(TrackId(200), "POS_1");

        set_pos_entry_time(
            &mut collector,
            TrackId(100),
            "POS_1",
            Instant::now() - std::time::Duration::from_secs(8),
        );
        set_pos_entry_time(
            &mut collector,
            TrackId(200),
            "POS_1",
            Instant::now() - std::time::Duration::from_secs(8),
        );

        collector.record_pos_exit(TrackId(100), "POS_1", 8000);

        // Make exit too old (4 seconds ago, outside MAX_TIME_SINCE_EXIT=3000)
        if let Some(exits) = collector.recent_exits.get_mut("POS_1") {
            if let Some(exit) = exits.first_mut() {
                exit.exited_at = Instant::now() - std::time::Duration::from_millis(4000);
            }
        }

        let result = collector.process_acc("192.168.1.10", &mut jm, None);

        // Only Track 200 should be matched (Track 100 exited too long ago)
        assert_eq!(result, vec![TrackId(200)], "Only present track should match");
        assert!(
            !jm.get(TrackId(100)).unwrap().acc_matched,
            "Track 100 should NOT have acc_matched (exit too old)"
        );
        assert!(jm.get(TrackId(200)).unwrap().acc_matched, "Track 200 should have acc_matched");
    }

    #[test]
    fn test_flicker_focus_soft_blocks_on_entry_spread() {
        let mut collector =
            create_test_collector_with_strategy(AccGroupingStrategy::FlickerFocusSoft);
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));
        jm.new_journey(TrackId(200));

        collector.record_pos_entry(TrackId(100), "POS_1");
        collector.record_pos_entry(TrackId(200), "POS_1");

        // Modify entry times in pos_sessions for consistency
        let now = Instant::now();
        let time_100 = now - std::time::Duration::from_secs(20);
        let time_200 = now - std::time::Duration::from_secs(8);
        set_pos_entry_time(&mut collector, TrackId(100), "POS_1", time_100);
        set_pos_entry_time(&mut collector, TrackId(200), "POS_1", time_200);

        let result = collector.process_acc("192.168.1.10", &mut jm, None);
        assert_eq!(result, vec![TrackId(100)]);
    }

    #[test]
    fn test_flicker_focus_soft_merges_entry_time() {
        let mut collector =
            create_test_collector_with_strategy(AccGroupingStrategy::FlickerFocusSoft);
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));
        jm.new_journey(TrackId(200));

        collector.record_pos_entry(TrackId(100), "POS_1");
        collector.record_pos_entry(TrackId(200), "POS_1");

        collector.record_pos_exit(TrackId(100), "POS_1", 2000);

        let now = Instant::now();
        if let Some(sessions) = collector.pos_sessions.get_mut(&TrackId(100)) {
            if let Some(last) = sessions.last_mut() {
                last.entered_at = now - std::time::Duration::from_secs(8);
                last.exited_at = Some(now - std::time::Duration::from_secs(5));
            }
        }
        if let Some(sessions) = collector.pos_sessions.get_mut(&TrackId(200)) {
            if let Some(last) = sessions.last_mut() {
                last.entered_at = now - std::time::Duration::from_secs(8);
            }
        }

        collector.record_pos_entry(TrackId(100), "POS_1");

        let result = collector.process_acc("192.168.1.10", &mut jm, None);
        assert_eq!(result.len(), 2, "Merged entry should qualify both tracks");
        assert!(result.contains(&TrackId(100)));
        assert!(result.contains(&TrackId(200)));
    }

    #[test]
    fn test_flicker_focus_soft_excludes_other_pos_activity() {
        let mut collector =
            create_test_collector_with_strategy(AccGroupingStrategy::FlickerFocusSoft);
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));
        jm.new_journey(TrackId(200));

        collector.record_pos_entry(TrackId(100), "POS_1");
        collector.record_pos_entry(TrackId(200), "POS_1");

        // Modify entry times in pos_sessions for consistency
        let now = Instant::now();
        let time_100 = now - std::time::Duration::from_secs(9);
        let time_200 = now - std::time::Duration::from_secs(8);

        if let Some(sessions) = collector.pos_sessions.get_mut(&TrackId(100)) {
            // Track 100 also visited POS_2 (3 seconds of other_pos activity)
            sessions.push(PosSession {
                zone: "POS_2".to_string(),
                entered_at: now - std::time::Duration::from_secs(6),
                exited_at: Some(now - std::time::Duration::from_secs(3)),
            });
        }
        set_pos_entry_time(&mut collector, TrackId(100), "POS_1", time_100);
        set_pos_entry_time(&mut collector, TrackId(200), "POS_1", time_200);

        let result = collector.process_acc("192.168.1.10", &mut jm, None);
        assert_eq!(result, vec![TrackId(100)]);
    }

    // ============================================================
    // New tests for tightened ACC grouping
    // ============================================================

    #[test]
    fn test_members_overlapping_with_exit_filters_non_overlapping() {
        // Direct unit test for members_overlapping_with_exit.
        // Tests that members who entered after the primary's exit + grace are excluded.
        let mut collector =
            create_test_collector_with_strategy(AccGroupingStrategy::FlickerFocusSoft);

        let now = Instant::now();

        // Set up Track 200 as present in pos_groups with sufficient dwell
        collector.record_pos_entry(TrackId(200), "POS_1");
        let track200_entry = now - std::time::Duration::from_secs(10);
        set_pos_entry_time(&mut collector, TrackId(200), "POS_1", track200_entry);

        // Primary exited at now - 15s, grace is 2s, so overlap deadline is now - 13s
        // Track 200 entered at now - 10s which is AFTER now - 13s -> no overlap
        let exit_info = ExitSessionInfo { exited_at: now - std::time::Duration::from_secs(15) };

        let group = collector.pos_groups.get("POS_1").unwrap();
        let result = collector.members_overlapping_with_exit("POS_1", group, &exit_info, None);

        // Track 200 should NOT be included - no overlap with primary's session
        assert!(
            result.is_empty(),
            "Member who entered after primary exit + grace should be excluded. Got: {:?}",
            result
        );
    }

    #[test]
    fn test_members_overlapping_with_exit_includes_overlapping() {
        // Direct unit test for members_overlapping_with_exit.
        // Tests that members who overlapped with the primary's session ARE included.
        let mut collector =
            create_test_collector_with_strategy(AccGroupingStrategy::FlickerFocusSoft);

        let now = Instant::now();

        // Set up Track 200 as present in pos_groups with sufficient dwell
        collector.record_pos_entry(TrackId(200), "POS_1");
        let track200_entry = now - std::time::Duration::from_secs(10);
        set_pos_entry_time(&mut collector, TrackId(200), "POS_1", track200_entry);

        // Primary exited at now - 5s, Track 200 entered at now - 10s (before exit + grace)
        let exit_info = ExitSessionInfo { exited_at: now - std::time::Duration::from_secs(5) };

        let group = collector.pos_groups.get("POS_1").unwrap();
        let result = collector.members_overlapping_with_exit("POS_1", group, &exit_info, None);

        // Track 200 should be included - overlapped with primary's session
        assert_eq!(
            result,
            vec![TrackId(200)],
            "Member who overlapped with primary should be included"
        );
    }

    #[test]
    fn test_members_overlapping_with_exit_filters_other_pos_activity() {
        // Direct unit test for members_overlapping_with_exit.
        // Tests that members with other_pos activity >= threshold are excluded.
        let mut collector =
            create_test_collector_with_strategy(AccGroupingStrategy::FlickerFocusSoft);

        let now = Instant::now();

        // Set up Track 200 as present in pos_groups with sufficient dwell
        collector.record_pos_entry(TrackId(200), "POS_1");
        let track200_entry = now - std::time::Duration::from_secs(10);
        set_pos_entry_time(&mut collector, TrackId(200), "POS_1", track200_entry);

        // Add other_pos activity for Track 200 (3s at POS_2, >= 2s threshold)
        if let Some(sessions) = collector.pos_sessions.get_mut(&TrackId(200)) {
            sessions.push(PosSession {
                zone: "POS_2".to_string(),
                entered_at: now - std::time::Duration::from_secs(8),
                exited_at: Some(now - std::time::Duration::from_secs(5)),
            });
        }

        let exit_info = ExitSessionInfo { exited_at: now - std::time::Duration::from_secs(5) };

        let group = collector.pos_groups.get("POS_1").unwrap();
        let result = collector.members_overlapping_with_exit("POS_1", group, &exit_info, None);

        assert!(
            result.is_empty(),
            "Member with other_pos activity >= threshold should be excluded"
        );
    }

    #[test]
    fn test_members_overlapping_with_exit_filters_insufficient_dwell() {
        let mut collector =
            create_test_collector_with_strategy(AccGroupingStrategy::FlickerFocusSoft);
        let now = Instant::now();

        // Track 200: 500ms dwell (below soft_dwell threshold)
        collector.record_pos_entry(TrackId(200), "POS_1");
        set_pos_entry_time(
            &mut collector,
            TrackId(200),
            "POS_1",
            now - std::time::Duration::from_millis(500),
        );

        let exit_info = ExitSessionInfo { exited_at: now - std::time::Duration::from_secs(1) };

        let group = collector.pos_groups.get("POS_1").unwrap();
        let result = collector.members_overlapping_with_exit("POS_1", group, &exit_info, None);

        assert!(result.is_empty(), "Member with insufficient dwell should be excluded");
    }

    #[test]
    fn test_members_overlapping_with_exit_uses_accumulated_dwell() {
        let mut collector =
            create_test_collector_with_strategy(AccGroupingStrategy::FlickerFocusSoft);
        let now = Instant::now();

        // Track 200: 1s session dwell (below soft_dwell), but 10s accumulated dwell
        collector.record_pos_entry(TrackId(200), "POS_1");
        set_pos_entry_time(
            &mut collector,
            TrackId(200),
            "POS_1",
            now - std::time::Duration::from_secs(1),
        );

        let exit_info = ExitSessionInfo { exited_at: now - std::time::Duration::from_millis(500) };

        let mut accumulated = HashMap::new();
        accumulated.insert(TrackId(200), 10_000);

        let group = collector.pos_groups.get("POS_1").unwrap();
        let result =
            collector.members_overlapping_with_exit("POS_1", group, &exit_info, Some(&accumulated));

        assert_eq!(result, vec![TrackId(200)], "Accumulated dwell should qualify member");
    }

    #[test]
    fn test_present_dwell_enforces_entry_spread() {
        // PresentDwell strategy should now reject groups where members
        // entered too far apart (violates entry-spread rule).
        let mut collector = create_test_collector_with_strategy(AccGroupingStrategy::PresentDwell);
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));
        jm.new_journey(TrackId(200));

        // Both enter POS_1
        collector.record_pos_entry(TrackId(100), "POS_1");
        collector.record_pos_entry(TrackId(200), "POS_1");

        let now = Instant::now();

        // Track 100 entered 20 seconds ago, Track 200 entered 8 seconds ago
        // Spread is 12 seconds, which exceeds grouping_entry_spread_s (10)
        set_pos_entry_time(
            &mut collector,
            TrackId(100),
            "POS_1",
            now - std::time::Duration::from_secs(20),
        );
        set_pos_entry_time(
            &mut collector,
            TrackId(200),
            "POS_1",
            now - std::time::Duration::from_secs(8),
        );

        // Process ACC - both have sufficient dwell but entry spread is too wide
        let result = collector.process_acc("192.168.1.10", &mut jm, None);

        // With entry-spread enforcement, only the primary should be matched
        // (or empty if the primary check fails the expansion)
        assert_eq!(
            result,
            vec![TrackId(100)],
            "PresentDwell should reject groups with wide entry spread"
        );
    }

    #[test]
    fn test_recent_exit_expansion_with_soft_dwell() {
        // Integration test: validates that recent-exit expansion works when
        // present members have soft_dwell (3s) but not full min_dwell (7s)
        //
        // Config: min_dwell = 7s, soft_dwell = 3s
        // Scenario:
        // - Track A and B both enter POS_1, overlapping in time
        // - Track A has min_dwell (8s), Track B has soft_dwell (4s) only
        // - Track A exits, then ACC arrives
        // - Track B doesn't qualify as present primary (4s < 7s min_dwell)
        // - Track A is selected as recent-exit primary
        // - Track B should be included via overlap expansion (4s >= 3s soft_dwell)
        let mut collector = create_test_collector_for_soft_dwell_tests();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100)); // Track A - will exit before ACC
        jm.new_journey(TrackId(200)); // Track B - still present when ACC arrives

        let now = Instant::now();

        // Both enter POS_1 at similar times (within entry spread)
        collector.record_pos_entry(TrackId(100), "POS_1");
        collector.record_pos_entry(TrackId(200), "POS_1");

        // Track A: entered 9s ago (8s dwell after exit), Track B: entered 4s ago
        // Both within entry_spread_s (10s), Track B at soft_dwell (4s > 3s but < 7s)
        let track_a_entry = now - std::time::Duration::from_secs(9);
        let track_b_entry = now - std::time::Duration::from_secs(4);
        set_pos_entry_time(&mut collector, TrackId(100), "POS_1", track_a_entry);
        set_pos_entry_time(&mut collector, TrackId(200), "POS_1", track_b_entry);

        // Track A exits with sufficient dwell (8s > min_dwell 7s)
        collector.record_pos_exit(TrackId(100), "POS_1", 8000);

        // Make sure Track A's exit session is recorded properly for overlap check
        if let Some(sessions) = collector.pos_sessions.get_mut(&TrackId(100)) {
            if let Some(session) = sessions.iter_mut().find(|s| s.zone == "POS_1") {
                session.entered_at = track_a_entry;
                session.exited_at = Some(now);
            }
        }

        // Process ACC - Track A is primary (recent exit), Track B should expand
        // Track B has 4s dwell >= soft_dwell (3s) and overlapped with Track A
        let result = collector.process_acc("192.168.1.10", &mut jm, None);

        assert!(
            result.contains(&TrackId(100)),
            "Track A (recent exit primary) should be in result"
        );
        assert!(
            result.contains(&TrackId(200)),
            "Track B (present with soft_dwell, overlapping) should be in result via expansion"
        );
        assert_eq!(result.len(), 2, "Both tracks should be matched");
    }

    #[test]
    fn test_recent_exit_expansion_blocked_by_insufficient_soft_dwell() {
        // Config: min_dwell = 7s, soft_dwell = 3s
        // Track A exits with 8s dwell, Track B present with 2s dwell (< soft_dwell)
        let mut collector = create_test_collector_for_soft_dwell_tests();
        let mut jm = JourneyManager::new();
        jm.new_journey(TrackId(100));
        jm.new_journey(TrackId(200));

        let now = Instant::now();
        collector.record_pos_entry(TrackId(100), "POS_1");
        collector.record_pos_entry(TrackId(200), "POS_1");

        let track_a_entry = now - std::time::Duration::from_secs(9);
        let track_b_entry = now - std::time::Duration::from_secs(2);
        set_pos_entry_time(&mut collector, TrackId(100), "POS_1", track_a_entry);
        set_pos_entry_time(&mut collector, TrackId(200), "POS_1", track_b_entry);

        collector.record_pos_exit(TrackId(100), "POS_1", 8000);

        if let Some(sessions) = collector.pos_sessions.get_mut(&TrackId(100)) {
            if let Some(session) = sessions.iter_mut().find(|s| s.zone == "POS_1") {
                session.entered_at = track_a_entry;
                session.exited_at = Some(now);
            }
        }

        let result = collector.process_acc("192.168.1.10", &mut jm, None);

        assert_eq!(result, vec![TrackId(100)], "Track B below soft_dwell should not be expanded");
    }

    #[test]
    fn test_recent_exit_expansion_with_multiple_soft_dwell_members() {
        // Config: min_dwell = 7s, soft_dwell = 3s, entry_spread = 10s
        // Track A exits with 8s, Tracks B (5s) and C (4s) present with soft_dwell
        // B and C entered 1s apart (within entry_spread) -> all three should match
        let mut collector = create_test_collector_for_soft_dwell_tests();
        let mut jm = JourneyManager::new();
        jm.new_journey(TrackId(100));
        jm.new_journey(TrackId(200));
        jm.new_journey(TrackId(300));

        let now = Instant::now();
        collector.record_pos_entry(TrackId(100), "POS_1");
        collector.record_pos_entry(TrackId(200), "POS_1");
        collector.record_pos_entry(TrackId(300), "POS_1");

        let track_a_entry = now - std::time::Duration::from_secs(9);
        let track_b_entry = now - std::time::Duration::from_secs(5);
        let track_c_entry = now - std::time::Duration::from_secs(4);

        set_pos_entry_time(&mut collector, TrackId(100), "POS_1", track_a_entry);
        set_pos_entry_time(&mut collector, TrackId(200), "POS_1", track_b_entry);
        set_pos_entry_time(&mut collector, TrackId(300), "POS_1", track_c_entry);

        collector.record_pos_exit(TrackId(100), "POS_1", 8000);

        if let Some(sessions) = collector.pos_sessions.get_mut(&TrackId(100)) {
            if let Some(session) = sessions.iter_mut().find(|s| s.zone == "POS_1") {
                session.entered_at = track_a_entry;
                session.exited_at = Some(now);
            }
        }

        let result = collector.process_acc("192.168.1.10", &mut jm, None);

        assert!(result.contains(&TrackId(100)), "Track A (primary) should match");
        assert!(result.contains(&TrackId(200)), "Track B should be expanded");
        assert!(result.contains(&TrackId(300)), "Track C should be expanded");
        assert_eq!(result.len(), 3);
    }
}
