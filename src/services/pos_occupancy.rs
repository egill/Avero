//! Per-zone POS occupancy state machine
//!
//! Tracks customer presence and dwell time at each POS zone independently.
//! This is the single source of truth for POS zone occupancy - ACC matching
//! queries this state to find candidates.
//!
//! Key behaviors:
//! - Entry creates/reopens a session, exit accumulates dwell
//! - Re-entry within grace window reopens the session (preserves dwell)
//! - Dwell only accumulates on exit, not on re-entry
//! - get_candidates() returns present tracks first, then recent exits

use crate::domain::types::TrackId;
use std::collections::HashMap;
use std::time::Instant;

/// State for a single track at a single POS zone
#[derive(Debug, Clone)]
pub struct PosState {
    /// Whether the track is currently present in this zone
    pub is_present: bool,
    /// When the track entered (or re-entered) this zone
    pub entry_time: Instant,
    /// When the track exited (None if still present)
    pub exit_time: Option<Instant>,
    /// Total accumulated dwell time in milliseconds
    pub accumulated_dwell_ms: u64,
}

impl PosState {
    fn new(entry_time: Instant) -> Self {
        Self { is_present: true, entry_time, exit_time: None, accumulated_dwell_ms: 0 }
    }
}

/// Per-zone POS occupancy tracker
///
/// Outer key is zone name (e.g. "POS_1"), inner key is track_id
pub struct PosOccupancyState {
    /// zones[zone_name][track_id] = PosState
    zones: HashMap<String, HashMap<i64, PosState>>,
    /// Grace window for re-entry (ms) - if track re-enters within this window
    /// after exit, the session is reopened rather than creating a new one
    exit_grace_ms: u64,
    /// Minimum dwell time for ACC qualification (ms)
    min_dwell_ms: u64,
}

impl PosOccupancyState {
    pub fn new(exit_grace_ms: u64, min_dwell_ms: u64) -> Self {
        Self { zones: HashMap::new(), exit_grace_ms, min_dwell_ms }
    }

    /// Record a track entering a POS zone
    ///
    /// If the track exited recently (within grace window), reopens the session.
    /// Otherwise creates a new session.
    pub fn record_entry(&mut self, zone: &str, track_id: TrackId, now: Instant) {
        let zone_tracks = self.zones.entry(zone.to_string()).or_default();

        if let Some(state) = zone_tracks.get_mut(&track_id.0) {
            if state.is_present {
                // Already present, no-op
                return;
            }

            // Check if we're within grace window
            if let Some(exit_time) = state.exit_time {
                let elapsed_ms = now.duration_since(exit_time).as_millis() as u64;
                if elapsed_ms <= self.exit_grace_ms {
                    // Reopen session - keep accumulated dwell, clear exit, mark present
                    state.is_present = true;
                    state.entry_time = now;
                    state.exit_time = None;
                    return;
                }
            }

            // Outside grace window - reset to new session
            *state = PosState::new(now);
        } else {
            // New track at this zone
            zone_tracks.insert(track_id.0, PosState::new(now));
        }
    }

    /// Record a track exiting a POS zone
    ///
    /// Accumulates dwell time from entry_time to now and marks as not present.
    /// Returns (session_dwell_ms, total_accumulated_dwell_ms) or None if no-op.
    pub fn record_exit(
        &mut self,
        zone: &str,
        track_id: TrackId,
        now: Instant,
    ) -> Option<(u64, u64)> {
        let zone_tracks = self.zones.get_mut(zone)?;
        let state = zone_tracks.get_mut(&track_id.0)?;

        if !state.is_present {
            // Already exited, no-op
            return None;
        }

        // Calculate dwell for this session and accumulate
        let session_dwell_ms = now.duration_since(state.entry_time).as_millis() as u64;
        state.accumulated_dwell_ms += session_dwell_ms;
        state.is_present = false;
        state.exit_time = Some(now);

        Some((session_dwell_ms, state.accumulated_dwell_ms))
    }

    /// Get candidate tracks for ACC matching at a specific zone
    ///
    /// Returns (track_id, dwell_ms) pairs sorted by:
    /// 1. Present tracks first (sorted by dwell descending)
    /// 2. Recent exits second (sorted by dwell descending)
    ///
    /// Only returns tracks that have exited within the grace window or are present.
    /// Does NOT filter by min_dwell_ms - caller should filter if needed.
    pub fn get_candidates(&self, zone: &str, now: Instant) -> Vec<(TrackId, u64)> {
        let Some(zone_tracks) = self.zones.get(zone) else {
            return Vec::new();
        };

        let mut present: Vec<(TrackId, u64)> = Vec::new();
        let mut recent_exits: Vec<(TrackId, u64)> = Vec::new();

        for (&track_id, state) in zone_tracks {
            if state.is_present {
                // For present tracks, calculate current dwell (accumulated + current session)
                let current_session_ms = now.duration_since(state.entry_time).as_millis() as u64;
                let total_dwell = state.accumulated_dwell_ms + current_session_ms;
                present.push((TrackId(track_id), total_dwell));
            } else if let Some(exit_time) = state.exit_time {
                // Check if exit is within grace window
                let elapsed_ms = now.duration_since(exit_time).as_millis() as u64;
                if elapsed_ms <= self.exit_grace_ms {
                    recent_exits.push((TrackId(track_id), state.accumulated_dwell_ms));
                }
            }
        }

        // Sort both by dwell descending
        present.sort_by(|a, b| b.1.cmp(&a.1));
        recent_exits.sort_by(|a, b| b.1.cmp(&a.1));

        // Concatenate: present first, then recent exits
        present.extend(recent_exits);
        present
    }

    /// Remove expired entries from a specific zone
    ///
    /// Removes entries where exit_time + grace < now
    pub fn prune_expired(&mut self, zone: &str, now: Instant) {
        let Some(zone_tracks) = self.zones.get_mut(zone) else {
            return;
        };

        zone_tracks.retain(|_, state| {
            if state.is_present {
                return true;
            }
            // Keep exits within grace window; remove expired or invalid states
            let Some(exit_time) = state.exit_time else {
                return false;
            };
            let elapsed_ms = now.duration_since(exit_time).as_millis() as u64;
            elapsed_ms <= self.exit_grace_ms
        });
    }

    /// Get the configured exit grace window in milliseconds
    #[inline]
    pub fn exit_grace_ms(&self) -> u64 {
        self.exit_grace_ms
    }

    /// Get the configured minimum dwell time in milliseconds
    #[inline]
    pub fn min_dwell_ms(&self) -> u64 {
        self.min_dwell_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_state() -> PosOccupancyState {
        PosOccupancyState::new(5000, 7000)
    }

    #[test]
    fn test_entry_creates_new_state() {
        let mut state = create_state();
        let now = Instant::now();

        state.record_entry("POS_1", TrackId(100), now);

        let zone_tracks = state.zones.get("POS_1").unwrap();
        let pos_state = zone_tracks.get(&100).unwrap();
        assert!(pos_state.is_present);
        assert_eq!(pos_state.accumulated_dwell_ms, 0);
        assert!(pos_state.exit_time.is_none());
    }

    #[test]
    fn test_exit_sets_not_present_and_accumulates() {
        let mut state = create_state();
        let now = Instant::now();
        let later = now + std::time::Duration::from_millis(3000);

        state.record_entry("POS_1", TrackId(100), now);
        state.record_exit("POS_1", TrackId(100), later);

        let zone_tracks = state.zones.get("POS_1").unwrap();
        let pos_state = zone_tracks.get(&100).unwrap();
        assert!(!pos_state.is_present);
        assert_eq!(pos_state.accumulated_dwell_ms, 3000);
        assert!(pos_state.exit_time.is_some());
    }

    #[test]
    fn test_reentry_within_grace_reopens() {
        let mut state = create_state();
        let now = Instant::now();
        let exit_time = now + std::time::Duration::from_millis(3000);
        let reentry_time = exit_time + std::time::Duration::from_millis(4000); // within 5000ms grace

        state.record_entry("POS_1", TrackId(100), now);
        state.record_exit("POS_1", TrackId(100), exit_time);
        state.record_entry("POS_1", TrackId(100), reentry_time);

        let zone_tracks = state.zones.get("POS_1").unwrap();
        let pos_state = zone_tracks.get(&100).unwrap();
        assert!(pos_state.is_present);
        assert!(pos_state.exit_time.is_none());
        // Accumulated dwell preserved from first session
        assert_eq!(pos_state.accumulated_dwell_ms, 3000);
    }

    #[test]
    fn test_reentry_after_grace_creates_new_session() {
        let mut state = create_state();
        let now = Instant::now();
        let exit_time = now + std::time::Duration::from_millis(3000);
        let reentry_time = exit_time + std::time::Duration::from_millis(6000); // beyond 5000ms grace

        state.record_entry("POS_1", TrackId(100), now);
        state.record_exit("POS_1", TrackId(100), exit_time);
        state.record_entry("POS_1", TrackId(100), reentry_time);

        let zone_tracks = state.zones.get("POS_1").unwrap();
        let pos_state = zone_tracks.get(&100).unwrap();
        assert!(pos_state.is_present);
        // New session resets accumulated dwell
        assert_eq!(pos_state.accumulated_dwell_ms, 0);
    }

    #[test]
    fn test_get_candidates_present_sorted_by_dwell_desc() {
        let mut state = create_state();
        let now = Instant::now();

        // Track 100 enters first (longer dwell)
        state.record_entry("POS_1", TrackId(100), now);
        // Track 200 enters later (shorter dwell)
        let later = now + std::time::Duration::from_millis(2000);
        state.record_entry("POS_1", TrackId(200), later);

        // Query at a time where track 100 has 5000ms, track 200 has 3000ms
        let query_time = now + std::time::Duration::from_millis(5000);
        let candidates = state.get_candidates("POS_1", query_time);

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].0, TrackId(100)); // 5000ms dwell
        assert_eq!(candidates[0].1, 5000);
        assert_eq!(candidates[1].0, TrackId(200)); // 3000ms dwell
        assert_eq!(candidates[1].1, 3000);
    }

    #[test]
    fn test_get_candidates_recent_exits_after_present() {
        let mut state = create_state();
        let now = Instant::now();

        // Track 100 enters and exits with 8000ms dwell
        state.record_entry("POS_1", TrackId(100), now);
        let exit_time = now + std::time::Duration::from_millis(8000);
        state.record_exit("POS_1", TrackId(100), exit_time);

        // Track 200 enters and stays present with 3000ms dwell
        let entry_200 = now + std::time::Duration::from_millis(5000);
        state.record_entry("POS_1", TrackId(200), entry_200);

        // Query 1000ms after track 100 exited (within grace)
        let query_time = exit_time + std::time::Duration::from_millis(1000);
        let candidates = state.get_candidates("POS_1", query_time);

        // Present track should come first even though exit has more dwell
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].0, TrackId(200)); // present, 4000ms dwell
        assert_eq!(candidates[1].0, TrackId(100)); // recent exit, 8000ms dwell
    }

    #[test]
    fn test_get_candidates_excludes_expired_exits() {
        let mut state = create_state();
        let now = Instant::now();

        // Track 100 enters and exits
        state.record_entry("POS_1", TrackId(100), now);
        let exit_time = now + std::time::Duration::from_millis(8000);
        state.record_exit("POS_1", TrackId(100), exit_time);

        // Query 6000ms after exit (beyond 5000ms grace)
        let query_time = exit_time + std::time::Duration::from_millis(6000);
        let candidates = state.get_candidates("POS_1", query_time);

        assert!(candidates.is_empty());
    }

    #[test]
    fn test_prune_expired_removes_only_expired() {
        let mut state = create_state();
        let now = Instant::now();

        // Track 100: exits and will be expired
        state.record_entry("POS_1", TrackId(100), now);
        let exit_100 = now + std::time::Duration::from_millis(3000);
        state.record_exit("POS_1", TrackId(100), exit_100);

        // Track 200: still present
        state.record_entry("POS_1", TrackId(200), now);

        // Track 300: exits but within grace
        state.record_entry("POS_1", TrackId(300), now);
        let exit_300 = now + std::time::Duration::from_millis(8000);
        state.record_exit("POS_1", TrackId(300), exit_300);

        // Prune at a time where:
        // - Track 100 exited 7000ms ago (beyond 5000ms grace) -> should be removed
        // - Track 200 is present -> should be kept
        // - Track 300 exited 2000ms ago (within grace) -> should be kept
        let prune_time = exit_300 + std::time::Duration::from_millis(2000);
        state.prune_expired("POS_1", prune_time);

        let zone_tracks = state.zones.get("POS_1").unwrap();
        assert!(!zone_tracks.contains_key(&100)); // removed
        assert!(zone_tracks.contains_key(&200)); // kept
        assert!(zone_tracks.contains_key(&300)); // kept
    }

    #[test]
    fn test_grace_boundary_4999_pass() {
        let mut state = create_state(); // 5000ms grace
        let now = Instant::now();

        state.record_entry("POS_1", TrackId(100), now);
        let exit_time = now + std::time::Duration::from_millis(3000);
        state.record_exit("POS_1", TrackId(100), exit_time);

        // Query at exactly 4999ms after exit (just within grace)
        let query_time = exit_time + std::time::Duration::from_millis(4999);
        let candidates = state.get_candidates("POS_1", query_time);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].0, TrackId(100));
    }

    #[test]
    fn test_grace_boundary_5001_fail() {
        let mut state = create_state(); // 5000ms grace
        let now = Instant::now();

        state.record_entry("POS_1", TrackId(100), now);
        let exit_time = now + std::time::Duration::from_millis(3000);
        state.record_exit("POS_1", TrackId(100), exit_time);

        // Query at exactly 5001ms after exit (just beyond grace)
        let query_time = exit_time + std::time::Duration::from_millis(5001);
        let candidates = state.get_candidates("POS_1", query_time);

        assert!(candidates.is_empty());
    }

    #[test]
    fn test_min_dwell_boundary_6999_fail() {
        let mut state = create_state(); // 7000ms min_dwell
        let now = Instant::now();

        state.record_entry("POS_1", TrackId(100), now);
        // Exit with 6999ms dwell (just below min)
        let exit_time = now + std::time::Duration::from_millis(6999);
        state.record_exit("POS_1", TrackId(100), exit_time);

        let candidates = state.get_candidates("POS_1", exit_time);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].1, 6999);
        // Note: get_candidates returns all, caller filters by min_dwell_ms
        assert!(candidates[0].1 < state.min_dwell_ms());
    }

    #[test]
    fn test_min_dwell_boundary_7001_pass() {
        let mut state = create_state(); // 7000ms min_dwell
        let now = Instant::now();

        state.record_entry("POS_1", TrackId(100), now);
        // Exit with 7001ms dwell (just above min)
        let exit_time = now + std::time::Duration::from_millis(7001);
        state.record_exit("POS_1", TrackId(100), exit_time);

        let candidates = state.get_candidates("POS_1", exit_time);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].1, 7001);
        assert!(candidates[0].1 >= state.min_dwell_ms());
    }

    #[test]
    fn test_multiple_zones_isolated() {
        let mut state = create_state();
        let now = Instant::now();

        state.record_entry("POS_1", TrackId(100), now);
        state.record_entry("POS_2", TrackId(200), now);

        let candidates_pos1 = state.get_candidates("POS_1", now);
        let candidates_pos2 = state.get_candidates("POS_2", now);

        assert_eq!(candidates_pos1.len(), 1);
        assert_eq!(candidates_pos1[0].0, TrackId(100));

        assert_eq!(candidates_pos2.len(), 1);
        assert_eq!(candidates_pos2[0].0, TrackId(200));
    }

    #[test]
    fn test_same_track_multiple_zones() {
        let mut state = create_state();
        let now = Instant::now();

        // Track 100 is in both POS_1 and POS_2
        state.record_entry("POS_1", TrackId(100), now);
        let later = now + std::time::Duration::from_millis(5000);
        state.record_entry("POS_2", TrackId(100), later);

        let query_time = now + std::time::Duration::from_millis(8000);

        let candidates_pos1 = state.get_candidates("POS_1", query_time);
        let candidates_pos2 = state.get_candidates("POS_2", query_time);

        // POS_1: 8000ms dwell
        assert_eq!(candidates_pos1[0].1, 8000);
        // POS_2: 3000ms dwell
        assert_eq!(candidates_pos2[0].1, 3000);
    }

    #[test]
    fn test_accumulated_dwell_across_sessions() {
        let mut state = create_state();
        let now = Instant::now();

        // First session: 3000ms
        state.record_entry("POS_1", TrackId(100), now);
        let exit1 = now + std::time::Duration::from_millis(3000);
        state.record_exit("POS_1", TrackId(100), exit1);

        // Re-entry within grace: 2000ms after exit
        let reentry = exit1 + std::time::Duration::from_millis(2000);
        state.record_entry("POS_1", TrackId(100), reentry);

        // Second session dwell: 4000ms
        let exit2 = reentry + std::time::Duration::from_millis(4000);
        state.record_exit("POS_1", TrackId(100), exit2);

        let zone_tracks = state.zones.get("POS_1").unwrap();
        let pos_state = zone_tracks.get(&100).unwrap();
        // Should have accumulated both sessions: 3000 + 4000 = 7000
        assert_eq!(pos_state.accumulated_dwell_ms, 7000);
    }
}
