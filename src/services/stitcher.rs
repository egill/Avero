//! Track stitching for identity continuity across sensor gaps
//!
//! Enhanced features:
//! - Extended grace time for POS zones (people linger at checkout)
//! - POS zone memory: matching preference for tracks lost in same zone

use crate::domain::types::{Person, TrackId};
use crate::infra::metrics::Metrics;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info};

/// Stitch criteria
const MAX_TIME_MS: u64 = 4500; // 4.5 seconds base grace time
const MAX_TIME_POS_ZONE_MS: u64 = 8000; // 8 seconds for tracks lost in POS zones
const MAX_TIME_SPAWN_HINT_MS: u64 = 10000; // 10 seconds for spawn-hint POS matches
const MAX_DISTANCE_CM: f64 = 180.0; // 180cm
const MAX_DISTANCE_SAME_ZONE_CM: f64 = 300.0; // 300cm if same zone context
const MAX_DISTANCE_SPAWN_HINT_CM: f64 = 190.0; // 190cm for spawn-hint matches
const MAX_HEIGHT_DIFF_CM: f64 = 10.0; // ±10cm
const MAX_HEIGHT_DIFF_POS_CM: f64 = 15.0; // ±15cm for POS zones (people bend)

/// Result of a successful stitch match
#[derive(Debug)]
pub struct StitchMatch {
    pub person: Person,
    pub time_ms: u64,
    pub distance_cm: u32,
}

/// A track pending potential stitching
#[derive(Debug, Clone)]
struct PendingTrack {
    person: Person,
    deleted_at: Instant,
    position: Option<[f64; 3]>,
    last_zone: Option<String>,
}

/// Debug info about a pending track (for ACC debugging)
#[derive(Debug, Clone)]
pub struct PendingTrackInfo {
    pub track_id: TrackId,
    pub last_zone: Option<String>,
    pub dwell_ms: u64,
    pub authorized: bool,
    pub pending_ms: u64,
}

/// Manages track identity stitching
pub struct Stitcher {
    pending: Vec<PendingTrack>,
    metrics: Option<Arc<Metrics>>,
}

impl Default for Stitcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Stitcher {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self { pending: Vec::new(), metrics: None }
    }

    /// Create a stitcher with metrics recording
    pub fn with_metrics(metrics: Arc<Metrics>) -> Self {
        Self { pending: Vec::new(), metrics: Some(metrics) }
    }

    /// Add a deleted track as pending for potential stitching
    pub fn add_pending(
        &mut self,
        person: Person,
        position: Option<[f64; 3]>,
        last_zone: Option<String>,
    ) {
        debug!(
            track_id = %person.track_id,
            authorized = %person.authorized,
            last_zone = ?last_zone,
            "pending_stitch_added"
        );

        self.pending.push(PendingTrack { person, deleted_at: Instant::now(), position, last_zone });
    }

    /// Try to find and remove a stitch candidate for a new track at given position
    /// Returns StitchMatch with Person and metrics (time_ms, distance_cm)
    ///
    /// Enhanced matching:
    /// - Extended grace time for tracks lost in POS zones (8s vs 4.5s)
    /// - Extended distance for tracks with same zone context (300cm vs 180cm)
    pub fn find_match(&mut self, new_position: Option<[f64; 3]>) -> Option<StitchMatch> {
        self.find_match_with_context(new_position, None, false)
    }

    /// Try to find a stitch candidate with optional zone context for matching
    /// If current_zone matches the pending track's last_zone, use extended distance
    pub fn find_match_with_zone(
        &mut self,
        new_position: Option<[f64; 3]>,
        current_zone: Option<&str>,
    ) -> Option<StitchMatch> {
        self.find_match_with_context(new_position, current_zone, false)
    }

    /// Try to find a stitch candidate with spawn-hint context
    ///
    /// spawn_hint: true if the new track appears to be a re-detection (no STORE entry,
    /// no ENTRY line crossing, spawned in POS zone). This boosts matching thresholds:
    /// - Time: 10s (vs 8s for POS zones)
    /// - Distance: 190cm for same-zone matches
    /// - Height: ±15cm for POS zones (people bend at checkout)
    pub fn find_match_with_context(
        &mut self,
        new_position: Option<[f64; 3]>,
        current_zone: Option<&str>,
        spawn_hint: bool,
    ) -> Option<StitchMatch> {
        // First, clean up expired entries
        self.cleanup_expired();

        let new_pos = new_position?;
        let now = Instant::now();

        let mut best_match: Option<(usize, f64, bool)> = None; // (idx, distance, same_zone)

        for (i, pending) in self.pending.iter().enumerate() {
            let is_pos_zone = pending.last_zone.as_ref().is_some_and(|z| z.starts_with("POS_"));
            let same_zone = current_zone.is_some() && pending.last_zone.as_deref() == current_zone;

            // Time check - use extended time for POS zones, even more for spawn-hint
            let age_ms = now.duration_since(pending.deleted_at).as_millis() as u64;
            let max_time = if spawn_hint && is_pos_zone && same_zone {
                MAX_TIME_SPAWN_HINT_MS // 10s for spawn-hint same-zone POS
            } else if is_pos_zone {
                MAX_TIME_POS_ZONE_MS // 8s for POS zones
            } else {
                MAX_TIME_MS // 4.5s base
            };
            if age_ms > max_time {
                continue;
            }

            let old_pos = match pending.position {
                Some(pos) => pos,
                None => continue, // Can't match without position
            };

            // Height check - relaxed for POS zones (people bend at checkout)
            let height_diff_cm = (new_pos[2] - old_pos[2]).abs() * 100.0;
            let max_height = if is_pos_zone {
                MAX_HEIGHT_DIFF_POS_CM // 15cm for POS zones
            } else {
                MAX_HEIGHT_DIFF_CM // 10cm base
            };
            if height_diff_cm > max_height {
                debug!(
                    old_track_id = %pending.person.track_id,
                    height_diff_cm = %height_diff_cm as u32,
                    max_height_cm = %max_height as u32,
                    spawn_hint = %spawn_hint,
                    "stitch_rejected_height"
                );
                continue;
            }

            // Distance check (x, y in meters)
            // Use extended distance for same zone context
            let dx = new_pos[0] - old_pos[0];
            let dy = new_pos[1] - old_pos[1];
            let distance_cm = (dx * dx + dy * dy).sqrt() * 100.0;

            // Distance thresholds:
            // - Same zone with spawn hint: 190cm (re-detection, slightly relaxed)
            // - Same zone without spawn hint: 300cm (existing behavior)
            // - Different zone: 180cm (base)
            let max_distance = if same_zone && spawn_hint {
                MAX_DISTANCE_SPAWN_HINT_CM // 190cm for spawn-hint same-zone
            } else if same_zone {
                MAX_DISTANCE_SAME_ZONE_CM // 300cm for same-zone
            } else {
                MAX_DISTANCE_CM // 180cm base
            };

            if distance_cm > max_distance {
                debug!(
                    old_track_id = %pending.person.track_id,
                    distance_cm = %distance_cm as u32,
                    max_distance_cm = %max_distance as u32,
                    same_zone = %same_zone,
                    spawn_hint = %spawn_hint,
                    "stitch_rejected_distance"
                );
                continue;
            }

            // Track best match: prefer same-zone matches, then closest distance
            let dominated = match &best_match {
                None => false,
                // Current best is same-zone, this candidate isn't - best wins
                Some((_, _, true)) if !same_zone => true,
                Some((_, best_dist, best_same)) => {
                    // Current candidate loses if:
                    // - Best has same_zone advantage and candidate doesn't
                    // - Equal same_zone status but best is closer or equal distance
                    let best_has_zone_advantage = *best_same && !same_zone;
                    let best_is_closer = same_zone == *best_same && *best_dist <= distance_cm;
                    best_has_zone_advantage || best_is_closer
                }
            };
            if !dominated {
                best_match = Some((i, distance_cm, same_zone));
            }
        }

        best_match.map(|(idx, distance_cm, same_zone)| {
            let pending = self.pending.swap_remove(idx);
            let time_ms = now.duration_since(pending.deleted_at).as_millis() as u64;
            info!(
                old_track_id = %pending.person.track_id,
                distance_cm = %distance_cm as u32,
                time_ms = %time_ms,
                same_zone = %same_zone,
                spawn_hint = %spawn_hint,
                last_zone = ?pending.last_zone,
                "stitch_match_found"
            );
            StitchMatch { person: pending.person, time_ms, distance_cm: distance_cm as u32 }
        })
    }

    /// Remove expired pending tracks
    /// Uses extended time for tracks that were in POS zones
    fn cleanup_expired(&mut self) {
        let now = Instant::now();
        let before = self.pending.len();
        let metrics = self.metrics.clone();

        self.pending.retain(|p| {
            let age_ms = now.duration_since(p.deleted_at).as_millis() as u64;
            let max_time = if p.last_zone.as_ref().is_some_and(|z| z.starts_with("POS_")) {
                MAX_TIME_POS_ZONE_MS
            } else {
                MAX_TIME_MS
            };
            if age_ms > max_time {
                info!(
                    track_id = %p.person.track_id,
                    authorized = %p.person.authorized,
                    last_zone = ?p.last_zone,
                    age_ms = %age_ms,
                    "stitch_expired_lost"
                );
                // Record metric for truly lost track
                if let Some(ref m) = metrics {
                    m.record_stitch_expired();
                }
                false
            } else {
                true
            }
        });

        let expired = before - self.pending.len();
        if expired > 0 {
            debug!(expired = %expired, remaining = %self.pending.len(), "stitch_cleanup");
        }
    }

    /// Number of tracks pending stitch
    #[allow(dead_code)]
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Get debug info about all pending tracks (for ACC debugging)
    pub fn get_pending_info(&self) -> Vec<PendingTrackInfo> {
        let now = Instant::now();
        self.pending
            .iter()
            .map(|p| PendingTrackInfo {
                track_id: p.person.track_id,
                last_zone: p.last_zone.clone(),
                dwell_ms: 0, // Dwell now tracked in PosOccupancyState, not Person
                authorized: p.person.authorized,
                pending_ms: now.duration_since(p.deleted_at).as_millis() as u64,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::{Person, TrackId};

    #[test]
    fn test_stitch_within_criteria() {
        let mut stitcher = Stitcher::new();

        let mut person = Person::new(TrackId(100));
        person.authorized = true;

        // Add pending at position [1.0, 1.0, 1.7]
        stitcher.add_pending(person, Some([1.0, 1.0, 1.70]), Some("POS_1".to_string()));

        // New track at [1.5, 1.0, 1.72] - within 180cm and ±10cm height (50cm away)
        let result = stitcher.find_match(Some([1.5, 1.0, 1.72]));

        assert!(result.is_some());
        let stitch = result.unwrap();
        assert_eq!(stitch.person.track_id, TrackId(100));
        assert!(stitch.person.authorized);
        assert_eq!(stitch.distance_cm, 50);
        assert!(stitch.time_ms < 100); // Should be near-instant in test
    }

    #[test]
    fn test_stitch_too_far() {
        let mut stitcher = Stitcher::new();

        let person = Person::new(TrackId(100));
        stitcher.add_pending(person, Some([1.0, 1.0, 1.70]), None);

        // New track at [4.0, 1.0, 1.70] - 300cm away, too far
        let result = stitcher.find_match(Some([4.0, 1.0, 1.70]));

        assert!(result.is_none());
        assert_eq!(stitcher.pending_count(), 1); // Still pending
    }

    #[test]
    fn test_stitch_height_mismatch() {
        let mut stitcher = Stitcher::new();

        let person = Person::new(TrackId(100));
        stitcher.add_pending(person, Some([1.0, 1.0, 1.70]), None);

        // New track same location but 20cm taller
        let result = stitcher.find_match(Some([1.0, 1.0, 1.90]));

        assert!(result.is_none());
    }

    #[test]
    fn test_no_position_no_match() {
        let mut stitcher = Stitcher::new();

        let person = Person::new(TrackId(100));
        stitcher.add_pending(person, Some([1.0, 1.0, 1.70]), None);

        // New track without position
        let result = stitcher.find_match(None);

        assert!(result.is_none());
    }

    #[test]
    fn test_pending_without_position() {
        let mut stitcher = Stitcher::new();

        // Pending track without position (rare but possible)
        let person = Person::new(TrackId(100));
        stitcher.add_pending(person, None, None);

        // New track with position - can't match pending without position
        let result = stitcher.find_match(Some([1.0, 1.0, 1.70]));

        assert!(result.is_none());
        assert_eq!(stitcher.pending_count(), 1); // Still pending
    }

    #[test]
    fn test_best_match_selected() {
        let mut stitcher = Stitcher::new();

        // Add two pending tracks
        let mut person1 = Person::new(TrackId(100));
        person1.authorized = false;
        stitcher.add_pending(person1, Some([1.0, 1.0, 1.70]), None);

        let mut person2 = Person::new(TrackId(200));
        person2.authorized = true;
        stitcher.add_pending(person2, Some([1.2, 1.0, 1.70]), None); // Closer

        // New track - should match closer one (person2, 10cm away vs 30cm)
        let result = stitcher.find_match(Some([1.3, 1.0, 1.70]));

        assert!(result.is_some());
        let stitch = result.unwrap();
        assert_eq!(stitch.person.track_id, TrackId(200)); // Closer match
        assert!(stitch.person.authorized);
        assert_eq!(stitch.distance_cm, 10); // 10cm from person2
        assert_eq!(stitcher.pending_count(), 1); // person1 still pending
    }

    #[test]
    fn test_absolutely_no_stitch() {
        let mut stitcher = Stitcher::new();

        let mut person = Person::new(TrackId(100));
        person.authorized = true;

        // Pending at one corner of the store
        stitcher.add_pending(person, Some([0.0, 0.0, 1.50]), Some("POS_2".to_string()));

        // New track at opposite corner, completely different height
        // Distance: 10m away (1000cm >> 180cm limit)
        // Height: 50cm different (>> 10cm limit)
        let result = stitcher.find_match(Some([10.0, 10.0, 2.00]));

        assert!(result.is_none());
        // Pending track should still be there
        assert_eq!(stitcher.pending_count(), 1);
    }

    #[test]
    fn test_get_pending_info() {
        let mut stitcher = Stitcher::new();

        let mut person = Person::new(TrackId(100));
        person.authorized = true;
        stitcher.add_pending(person, Some([1.0, 1.0, 1.70]), Some("POS_1".to_string()));

        let info = stitcher.get_pending_info();
        assert_eq!(info.len(), 1);
        assert_eq!(info[0].track_id, TrackId(100));
        assert_eq!(info[0].last_zone, Some("POS_1".to_string()));
        assert_eq!(info[0].dwell_ms, 0); // Dwell now tracked in PosOccupancyState
        assert!(info[0].authorized);
    }

    // ============================================================
    // Spawn-hint stitching tests
    // ============================================================

    #[test]
    fn test_spawn_hint_relaxed_height_in_pos_zone() {
        // POS zones allow ±15cm height difference (vs 10cm base)
        // People bend at checkout
        let mut stitcher = Stitcher::new();

        let mut person = Person::new(TrackId(100));
        person.authorized = true;
        // Pending at POS zone
        stitcher.add_pending(person, Some([1.0, 1.0, 1.70]), Some("POS_1".to_string()));

        // New track same location but 12cm height difference
        // Would fail with 10cm threshold, passes with 15cm
        let result = stitcher.find_match_with_context(Some([1.0, 1.0, 1.82]), Some("POS_1"), true);

        assert!(result.is_some(), "12cm height diff should match in POS zone");
        assert_eq!(result.unwrap().person.track_id, TrackId(100));
    }

    #[test]
    fn test_spawn_hint_height_still_rejected_if_too_far() {
        // Even in POS zone, >15cm height should be rejected
        let mut stitcher = Stitcher::new();

        let person = Person::new(TrackId(100));
        stitcher.add_pending(person, Some([1.0, 1.0, 1.70]), Some("POS_1".to_string()));

        // 18cm height difference - too much even for POS zone
        let result = stitcher.find_match_with_context(Some([1.0, 1.0, 1.88]), Some("POS_1"), true);

        assert!(result.is_none(), "18cm height diff should still be rejected");
    }

    #[test]
    fn test_spawn_hint_uses_190cm_distance_for_same_zone() {
        // Spawn hint with same zone: 190cm distance allowed
        let mut stitcher = Stitcher::new();

        let mut person = Person::new(TrackId(100));
        person.authorized = true;
        stitcher.add_pending(person, Some([1.0, 1.0, 1.70]), Some("POS_1".to_string()));

        // 185cm away - would pass normal 180cm but let's verify spawn-hint works
        let result = stitcher.find_match_with_context(Some([2.85, 1.0, 1.70]), Some("POS_1"), true);

        assert!(result.is_some(), "185cm should match with spawn_hint same zone");
        assert_eq!(result.unwrap().distance_cm, 185);
    }

    #[test]
    fn test_spawn_hint_rejects_beyond_190cm() {
        // Spawn hint limits distance to 190cm for same zone
        let mut stitcher = Stitcher::new();

        let person = Person::new(TrackId(100));
        stitcher.add_pending(person, Some([1.0, 1.0, 1.70]), Some("POS_1".to_string()));

        // 195cm away - beyond 190cm limit for spawn-hint
        let result = stitcher.find_match_with_context(Some([2.95, 1.0, 1.70]), Some("POS_1"), true);

        assert!(result.is_none(), "195cm should NOT match with spawn_hint (limit 190cm)");
    }

    #[test]
    fn test_spawn_hint_without_same_zone_uses_base_distance() {
        // spawn_hint but different zones: uses base 180cm
        let mut stitcher = Stitcher::new();

        let person = Person::new(TrackId(100));
        stitcher.add_pending(person, Some([1.0, 1.0, 1.70]), Some("POS_1".to_string()));

        // 185cm away, spawn_hint but different zone - should fail (>180cm)
        let result = stitcher.find_match_with_context(Some([2.85, 1.0, 1.70]), Some("POS_2"), true);

        assert!(result.is_none(), "185cm should NOT match for different zones (limit 180cm)");
    }

    #[test]
    fn test_base_height_check_for_non_pos_zones() {
        // Non-POS zones use base 10cm height threshold
        let mut stitcher = Stitcher::new();

        let person = Person::new(TrackId(100));
        stitcher.add_pending(person, Some([1.0, 1.0, 1.70]), Some("STORE".to_string()));

        // 12cm height diff - should fail for non-POS zone
        let result = stitcher.find_match_with_context(Some([1.0, 1.0, 1.82]), Some("STORE"), false);

        assert!(result.is_none(), "12cm should NOT match in STORE zone (limit 10cm)");
    }

    #[test]
    fn test_same_zone_without_spawn_hint_uses_300cm() {
        // Same zone without spawn hint: uses existing 300cm distance
        let mut stitcher = Stitcher::new();

        let mut person = Person::new(TrackId(100));
        person.authorized = true;
        stitcher.add_pending(person, Some([1.0, 1.0, 1.70]), Some("POS_1".to_string()));

        // 250cm away, same zone, no spawn hint - should pass (300cm limit)
        let result = stitcher.find_match_with_context(Some([3.5, 1.0, 1.70]), Some("POS_1"), false);

        assert!(result.is_some(), "250cm should match for same zone without spawn_hint");
        assert_eq!(result.unwrap().distance_cm, 250);
    }
}
