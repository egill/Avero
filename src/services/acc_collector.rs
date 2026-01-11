//! ACC (payment) event collection and correlation with journeys
//!
//! Simplified single-buyer matching: when ACC arrives, authorize all present
//! tracks at the POS zone that have accumulated_dwell >= min_dwell_ms.
//! The track with longest dwell is considered the "primary" for logging.

use crate::domain::journey::{epoch_ms, JourneyEvent, JourneyEventType};
use crate::domain::types::TrackId;
use crate::infra::config::Config;
use crate::infra::metrics::Metrics;
use crate::services::journey_manager::JourneyManager;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info};

/// Retain POS sessions for a short window to evaluate focus
const POS_SESSION_RETENTION_S: u64 = 120;
/// Limit stored sessions per track to avoid unbounded growth
const MAX_POS_SESSIONS_PER_TRACK: usize = 20;

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

/// Collects ACC events and correlates them with journeys
pub struct AccCollector {
    /// IP to POS name mapping
    ip_to_pos: HashMap<String, String>,
    min_dwell_for_acc: u64,
    flicker_merge_s: u64,
    recent_exit_window_ms: u64,
    /// Recent POS sessions per track
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
            flicker_merge_s: config.acc_flicker_merge_s(),
            recent_exit_window_ms: config.acc_recent_exit_window_ms(),
            pos_sessions: HashMap::new(),
            recent_exits: HashMap::new(),
            last_pos_exit: HashMap::new(),
            metrics,
        }
    }

    /// Record that a track entered a POS zone
    pub fn record_pos_entry(&mut self, track_id: TrackId, pos_zone: &str) {
        self.record_pos_session_entry(track_id, pos_zone);
        let present = self.tracks_present_at_zone(pos_zone);
        debug!(
            track_id = %track_id,
            pos = %pos_zone,
            present_count = %present.len(),
            present = ?present,
            "acc_pos_entry"
        );
    }

    /// Record that a track exited a POS zone
    pub fn record_pos_exit(&mut self, track_id: TrackId, pos_zone: &str, dwell_ms: u64) {
        self.record_pos_session_exit(track_id, pos_zone);
        let present = self.tracks_present_at_zone(pos_zone);
        debug!(
            track_id = %track_id,
            pos = %pos_zone,
            dwell_ms = %dwell_ms,
            remaining = %present.len(),
            "acc_pos_exit"
        );

        // Track when this POS became empty for ACC timing diagnostics
        if present.is_empty() {
            self.last_pos_exit.insert(pos_zone.to_string(), Instant::now());
        }

        self.recent_exits.entry(pos_zone.to_string()).or_default().push(RecentExit {
            track_id,
            exited_at: Instant::now(),
            dwell_ms,
        });
    }

    /// Process an ACC event by IP address and try to match it to a journey
    /// Returns (primary_track, all_authorized_tracks)
    /// accumulated_dwells: map of track_id -> total accumulated dwell time from Person state
    pub fn process_acc(
        &mut self,
        ip: &str,
        journey_manager: &mut JourneyManager,
        accumulated_dwells: Option<&HashMap<TrackId, u64>>,
    ) -> (Option<TrackId>, Vec<TrackId>) {
        let Some(pos) = self.ip_to_pos.get(ip).cloned() else {
            return (None, vec![]);
        };
        self.process_acc_by_pos(&pos, Some(ip), journey_manager, accumulated_dwells)
    }

    /// Process an ACC event by POS zone directly
    /// Returns (primary_track, all_authorized_tracks)
    /// - primary_track: track with longest accumulated dwell (for logging)
    /// - all_authorized_tracks: all tracks at POS with accumulated_dwell >= min_dwell_ms
    pub fn process_acc_by_pos(
        &mut self,
        pos: &str,
        kiosk_id: Option<&str>,
        journey_manager: &mut JourneyManager,
        accumulated_dwells: Option<&HashMap<TrackId, u64>>,
    ) -> (Option<TrackId>, Vec<TrackId>) {
        let ts = epoch_ms();
        let kiosk_str = kiosk_id.unwrap_or(pos);

        info!(kiosk = %kiosk_str, pos = %pos, "acc_event_received");

        self.cleanup_old_exits();

        let now = Instant::now();
        let empty_map = HashMap::new();
        let dwells = accumulated_dwells.unwrap_or(&empty_map);

        // Find all tracks currently present at this POS zone
        let present_tracks = self.tracks_present_at_zone(pos);

        // Filter to those with accumulated_dwell >= min_dwell_ms
        let mut qualified: Vec<(TrackId, u64)> = present_tracks
            .iter()
            .filter_map(|&track_id| {
                let acc_dwell = dwells.get(&track_id).copied().unwrap_or(0);
                if acc_dwell >= self.min_dwell_for_acc {
                    Some((track_id, acc_dwell))
                } else {
                    None
                }
            })
            .collect();

        // Sort by dwell descending - longest dwell is primary
        qualified.sort_by(|a, b| b.1.cmp(&a.1));

        let (primary, authorized) = if !qualified.is_empty() {
            let primary = qualified[0].0;
            let all: Vec<TrackId> = qualified.iter().map(|(tid, _)| *tid).collect();
            debug!(
                pos = %pos,
                present = ?present_tracks,
                qualified = ?all,
                primary = %primary,
                "acc_matched_present"
            );
            (Some(primary), all)
        } else {
            // No present tracks qualify - try recent exits
            debug!(
                pos = %pos,
                present = ?present_tracks,
                "acc_no_present_qualified_checking_recent_exits"
            );

            // Log empty POS timing for diagnostics
            if present_tracks.is_empty() {
                let empty_since_ms = self
                    .last_pos_exit
                    .get(pos)
                    .map(|t| t.elapsed().as_millis() as u64)
                    .unwrap_or(0);
                info!(pos = %pos, empty_since_ms = %empty_since_ms, "acc_arrived_pos_empty");
                self.metrics.record_acc_empty_pos_time(empty_since_ms);
            }

            self.find_recent_exit_match(pos, now)
        };

        let Some(primary) = primary else {
            debug!(pos = %pos, "acc_no_match");
            return (None, vec![]);
        };

        info!(
            pos = %pos,
            primary = %primary,
            authorized_count = %authorized.len(),
            authorized = ?authorized,
            "acc_authorized"
        );

        // Record ACC match on all authorized journeys
        for &track_id in &authorized {
            if let Some(journey) = journey_manager.get_mut_any(track_id) {
                journey.acc_matched = true;
            }
            journey_manager.add_event(
                track_id,
                JourneyEvent::new(JourneyEventType::Acc, ts)
                    .with_zone(pos)
                    .with_extra(&format!("kiosk={kiosk_str},count={}", authorized.len())),
            );
        }

        (Some(primary), authorized)
    }

    /// Find a matching recent exit (fallback when no present tracks qualify)
    fn find_recent_exit_match(
        &mut self,
        pos: &str,
        now: Instant,
    ) -> (Option<TrackId>, Vec<TrackId>) {
        let min_dwell = self.min_dwell_for_acc;
        let recent_exit_window = self.recent_exit_window_ms;

        // Find best candidate from recent exits (newest first)
        let mut idx = None;
        if let Some(exits) = self.recent_exits.get(pos) {
            for (candidate_idx, exit) in exits.iter().enumerate().rev() {
                if exit.dwell_ms < min_dwell {
                    continue;
                }
                if now.duration_since(exit.exited_at).as_millis() as u64 > recent_exit_window {
                    continue;
                }
                idx = Some(candidate_idx);
                break;
            }
        }

        // Remove and return the selected candidate
        if let Some(idx) = idx {
            if let Some(exits) = self.recent_exits.get_mut(pos) {
                let exit = exits.swap_remove(idx);
                let time_since = now.duration_since(exit.exited_at).as_millis() as u64;

                info!(
                    track_id = %exit.track_id,
                    pos = %pos,
                    dwell_ms = %exit.dwell_ms,
                    time_since_exit_ms = %time_since,
                    "acc_matched_recent_exit"
                );

                return (Some(exit.track_id), vec![exit.track_id]);
            }
        }

        (None, vec![])
    }

    /// Get all tracks currently present at a zone (have open session)
    fn tracks_present_at_zone(&self, pos_zone: &str) -> Vec<TrackId> {
        self.pos_sessions
            .iter()
            .filter_map(|(&track_id, sessions)| {
                let has_open = sessions
                    .iter()
                    .any(|s| s.zone == pos_zone && s.exited_at.is_none());
                if has_open {
                    Some(track_id)
                } else {
                    None
                }
            })
            .collect()
    }

    fn record_pos_session_entry(&mut self, track_id: TrackId, pos_zone: &str) -> Instant {
        let now = Instant::now();
        let sessions = self.pos_sessions.entry(track_id).or_default();

        // Check if we can reuse or merge with existing session
        let merge_result = if let Some(last) = sessions.last_mut() {
            if last.zone != pos_zone {
                None
            } else if last.exited_at.is_none() {
                // Already in this zone with open session - reuse it
                Some(last.entered_at)
            } else if let Some(exited_at) = last.exited_at {
                let gap_s = now.duration_since(exited_at).as_secs();
                if gap_s <= self.flicker_merge_s {
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

        // Cap session count by removing oldest
        if sessions.len() > MAX_POS_SESSIONS_PER_TRACK {
            let excess = sessions.len() - MAX_POS_SESSIONS_PER_TRACK;
            sessions.drain(0..excess);
        }
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

    #[test]
    fn test_acc_match_present_single() {
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));
        collector.record_pos_entry(TrackId(100), "POS_1");

        // With accumulated dwell >= min_dwell, should match
        let mut accumulated = HashMap::new();
        accumulated.insert(TrackId(100), 8000);

        let (primary, authorized) =
            collector.process_acc("192.168.1.10", &mut jm, Some(&accumulated));

        assert_eq!(primary, Some(TrackId(100)));
        assert_eq!(authorized, vec![TrackId(100)]);
        assert!(jm.get(TrackId(100)).unwrap().acc_matched);
    }

    #[test]
    fn test_acc_match_present_multiple() {
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));
        jm.new_journey(TrackId(200));

        collector.record_pos_entry(TrackId(100), "POS_1");
        collector.record_pos_entry(TrackId(200), "POS_1");

        // Both have accumulated dwell >= min_dwell
        let mut accumulated = HashMap::new();
        accumulated.insert(TrackId(100), 10000); // longer dwell - should be primary
        accumulated.insert(TrackId(200), 8000);

        let (primary, authorized) =
            collector.process_acc("192.168.1.10", &mut jm, Some(&accumulated));

        assert_eq!(primary, Some(TrackId(100))); // longest dwell is primary
        assert_eq!(authorized.len(), 2);
        assert!(authorized.contains(&TrackId(100)));
        assert!(authorized.contains(&TrackId(200)));
        assert!(jm.get(TrackId(100)).unwrap().acc_matched);
        assert!(jm.get(TrackId(200)).unwrap().acc_matched);
    }

    #[test]
    fn test_acc_match_present_partial_qualification() {
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));
        jm.new_journey(TrackId(200));

        collector.record_pos_entry(TrackId(100), "POS_1");
        collector.record_pos_entry(TrackId(200), "POS_1");

        // Only Track 100 has sufficient dwell
        let mut accumulated = HashMap::new();
        accumulated.insert(TrackId(100), 8000);
        accumulated.insert(TrackId(200), 3000); // insufficient

        let (primary, authorized) =
            collector.process_acc("192.168.1.10", &mut jm, Some(&accumulated));

        assert_eq!(primary, Some(TrackId(100)));
        assert_eq!(authorized, vec![TrackId(100)]);
        assert!(jm.get(TrackId(100)).unwrap().acc_matched);
        assert!(!jm.get(TrackId(200)).unwrap().acc_matched);
    }

    #[test]
    fn test_acc_match_recent_exit() {
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));

        // Record exit with sufficient dwell
        collector.record_pos_exit(TrackId(100), "POS_1", 8000);

        // Process ACC within time window
        let (primary, authorized) = collector.process_acc("192.168.1.10", &mut jm, None);

        assert_eq!(primary, Some(TrackId(100)));
        assert!(authorized.contains(&TrackId(100)));
    }

    #[test]
    fn test_acc_no_match_insufficient_dwell() {
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));
        collector.record_pos_entry(TrackId(100), "POS_1");

        // No accumulated dwell provided
        let (primary, authorized) = collector.process_acc("192.168.1.10", &mut jm, None);

        assert_eq!(primary, None);
        assert!(authorized.is_empty());
    }

    #[test]
    fn test_acc_no_match_unknown_ip() {
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        let (primary, authorized) = collector.process_acc("192.168.1.99", &mut jm, None);

        assert_eq!(primary, None);
        assert!(authorized.is_empty());
    }

    #[test]
    fn test_pos_for_ip() {
        let collector = create_test_collector();

        assert_eq!(collector.pos_for_ip("192.168.1.10"), Some("POS_1"));
        assert_eq!(collector.pos_for_ip("192.168.1.11"), Some("POS_2"));
        assert_eq!(collector.pos_for_ip("192.168.1.99"), None);
    }

    #[test]
    fn test_recent_exit_boundary_within() {
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));
        collector.record_pos_exit(TrackId(100), "POS_1", 8000);

        // Simulate 2.999s since exit (just within recent_exit_window=3000)
        if let Some(exits) = collector.recent_exits.get_mut("POS_1") {
            if let Some(exit) = exits.first_mut() {
                exit.exited_at = Instant::now() - std::time::Duration::from_millis(2999);
            }
        }

        let (primary, _) = collector.process_acc("192.168.1.10", &mut jm, None);
        assert_eq!(primary, Some(TrackId(100)));
    }

    #[test]
    fn test_recent_exit_boundary_outside() {
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));
        collector.record_pos_exit(TrackId(100), "POS_1", 8000);

        // Simulate 3.001s since exit (just outside recent_exit_window=3000)
        if let Some(exits) = collector.recent_exits.get_mut("POS_1") {
            if let Some(exit) = exits.first_mut() {
                exit.exited_at = Instant::now() - std::time::Duration::from_millis(3001);
            }
        }

        let (primary, _) = collector.process_acc("192.168.1.10", &mut jm, None);
        assert_eq!(primary, None);
    }

    #[test]
    fn test_newest_exit_selected() {
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
            exits[0].exited_at = Instant::now() - std::time::Duration::from_millis(1400);
            exits[1].exited_at = Instant::now() - std::time::Duration::from_millis(1000);
            exits[2].exited_at = Instant::now() - std::time::Duration::from_millis(500);
        }

        let (primary, _) = collector.process_acc("192.168.1.10", &mut jm, None);
        assert_eq!(primary, Some(TrackId(300))); // newest exit
    }

    #[test]
    fn test_present_takes_priority_over_recent_exit() {
        let mut collector = create_test_collector();
        let mut jm = JourneyManager::new();

        jm.new_journey(TrackId(100));
        jm.new_journey(TrackId(200));

        // Track 100 exits with sufficient dwell
        collector.record_pos_exit(TrackId(100), "POS_1", 8000);

        // Track 200 enters and has sufficient accumulated dwell
        collector.record_pos_entry(TrackId(200), "POS_1");

        let mut accumulated = HashMap::new();
        accumulated.insert(TrackId(200), 8000);

        let (primary, authorized) =
            collector.process_acc("192.168.1.10", &mut jm, Some(&accumulated));

        // Present track should be matched, not recent exit
        assert_eq!(primary, Some(TrackId(200)));
        assert_eq!(authorized, vec![TrackId(200)]);
        assert!(!jm.get(TrackId(100)).unwrap().acc_matched);
        assert!(jm.get(TrackId(200)).unwrap().acc_matched);
    }

    #[test]
    fn test_tracks_present_at_zone() {
        let mut collector = create_test_collector();

        collector.record_pos_entry(TrackId(100), "POS_1");
        collector.record_pos_entry(TrackId(200), "POS_1");
        collector.record_pos_entry(TrackId(300), "POS_2");

        let present_pos1 = collector.tracks_present_at_zone("POS_1");
        assert_eq!(present_pos1.len(), 2);
        assert!(present_pos1.contains(&TrackId(100)));
        assert!(present_pos1.contains(&TrackId(200)));

        let present_pos2 = collector.tracks_present_at_zone("POS_2");
        assert_eq!(present_pos2, vec![TrackId(300)]);

        // Track exits POS_1
        collector.record_pos_exit(TrackId(100), "POS_1", 5000);
        let present_after_exit = collector.tracks_present_at_zone("POS_1");
        assert_eq!(present_after_exit, vec![TrackId(200)]);
    }

    #[test]
    fn test_flicker_merge() {
        let mut collector = create_test_collector();

        // Track enters POS_1
        collector.record_pos_entry(TrackId(100), "POS_1");
        let initial_entry = collector.pos_sessions.get(&TrackId(100)).unwrap()[0].entered_at;

        // Track exits
        collector.record_pos_exit(TrackId(100), "POS_1", 5000);

        // Track re-enters within flicker_merge window - should merge
        collector.record_pos_entry(TrackId(100), "POS_1");

        let sessions = collector.pos_sessions.get(&TrackId(100)).unwrap();
        assert_eq!(sessions.len(), 1); // Should be single merged session
        assert_eq!(sessions[0].entered_at, initial_entry); // Original entry time preserved
        assert!(sessions[0].exited_at.is_none()); // Session reopened
    }
}
