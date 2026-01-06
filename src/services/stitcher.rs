//! Track stitching for identity continuity across sensor gaps

use crate::domain::types::Person;
use std::time::Instant;
use tracing::{info, debug};

/// Stitch criteria
const MAX_TIME_MS: u64 = 4500;       // 4.5 seconds
const MAX_DISTANCE_CM: f64 = 180.0;  // 180cm
const MAX_HEIGHT_DIFF_CM: f64 = 10.0; // ±10cm

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
}

/// Manages track identity stitching
pub struct Stitcher {
    pending: Vec<PendingTrack>,
}

impl Stitcher {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }

    /// Add a deleted track as pending for potential stitching
    pub fn add_pending(&mut self, person: Person, position: Option<[f64; 3]>) {
        debug!(
            track_id = %person.track_id,
            authorized = %person.authorized,
            dwell_ms = %person.accumulated_dwell_ms,
            "pending_stitch_added"
        );

        self.pending.push(PendingTrack {
            person,
            deleted_at: Instant::now(),
            position,
        });
    }

    /// Try to find and remove a stitch candidate for a new track at given position
    /// Returns StitchMatch with Person and metrics (time_ms, distance_cm)
    pub fn find_match(&mut self, new_position: Option<[f64; 3]>) -> Option<StitchMatch> {
        // First, clean up expired entries
        self.cleanup_expired();

        let new_pos = new_position?;
        let now = Instant::now();

        let mut best_match: Option<(usize, f64)> = None;

        for (i, pending) in self.pending.iter().enumerate() {
            // Time check
            let age_ms = now.duration_since(pending.deleted_at).as_millis() as u64;
            if age_ms > MAX_TIME_MS {
                continue;
            }

            let old_pos = pending.position?;

            // Height check (position[2] is height in meters)
            let height_diff_cm = (new_pos[2] - old_pos[2]).abs() * 100.0;
            if height_diff_cm > MAX_HEIGHT_DIFF_CM {
                continue;
            }

            // Distance check (x, y in meters)
            let dx = new_pos[0] - old_pos[0];
            let dy = new_pos[1] - old_pos[1];
            let distance_cm = (dx * dx + dy * dy).sqrt() * 100.0;
            if distance_cm > MAX_DISTANCE_CM {
                continue;
            }

            // Track best match (closest)
            match &best_match {
                None => best_match = Some((i, distance_cm)),
                Some((_, best_dist)) if distance_cm < *best_dist => {
                    best_match = Some((i, distance_cm));
                }
                _ => {}
            }
        }

        best_match.map(|(idx, distance_cm)| {
            let pending = self.pending.remove(idx);
            let time_ms = now.duration_since(pending.deleted_at).as_millis() as u64;
            info!(
                old_track_id = %pending.person.track_id,
                distance_cm = %distance_cm as u32,
                time_ms = %time_ms,
                "stitch_match_found"
            );
            StitchMatch {
                person: pending.person,
                time_ms,
                distance_cm: distance_cm as u32,
            }
        })
    }

    /// Remove expired pending tracks
    fn cleanup_expired(&mut self) {
        let now = Instant::now();
        let before = self.pending.len();

        self.pending.retain(|p| {
            let age_ms = now.duration_since(p.deleted_at).as_millis() as u64;
            if age_ms > MAX_TIME_MS {
                info!(
                    track_id = %p.person.track_id,
                    authorized = %p.person.authorized,
                    dwell_ms = %p.person.accumulated_dwell_ms,
                    "stitch_expired_lost"
                );
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::Person;

    #[test]
    fn test_stitch_within_criteria() {
        let mut stitcher = Stitcher::new();

        let mut person = Person::new(100);
        person.authorized = true;
        person.accumulated_dwell_ms = 5000;

        // Add pending at position [1.0, 1.0, 1.7]
        stitcher.add_pending(person, Some([1.0, 1.0, 1.70]));

        // New track at [1.5, 1.0, 1.72] - within 180cm and ±10cm height (50cm away)
        let result = stitcher.find_match(Some([1.5, 1.0, 1.72]));

        assert!(result.is_some());
        let stitch = result.unwrap();
        assert_eq!(stitch.person.track_id, 100);
        assert!(stitch.person.authorized);
        assert_eq!(stitch.person.accumulated_dwell_ms, 5000);
        assert_eq!(stitch.distance_cm, 50);
        assert!(stitch.time_ms < 100); // Should be near-instant in test
    }

    #[test]
    fn test_stitch_too_far() {
        let mut stitcher = Stitcher::new();

        let person = Person::new(100);
        stitcher.add_pending(person, Some([1.0, 1.0, 1.70]));

        // New track at [4.0, 1.0, 1.70] - 300cm away, too far
        let result = stitcher.find_match(Some([4.0, 1.0, 1.70]));

        assert!(result.is_none());
        assert_eq!(stitcher.pending_count(), 1); // Still pending
    }

    #[test]
    fn test_stitch_height_mismatch() {
        let mut stitcher = Stitcher::new();

        let person = Person::new(100);
        stitcher.add_pending(person, Some([1.0, 1.0, 1.70]));

        // New track same location but 20cm taller
        let result = stitcher.find_match(Some([1.0, 1.0, 1.90]));

        assert!(result.is_none());
    }

    #[test]
    fn test_no_position_no_match() {
        let mut stitcher = Stitcher::new();

        let person = Person::new(100);
        stitcher.add_pending(person, Some([1.0, 1.0, 1.70]));

        // New track without position
        let result = stitcher.find_match(None);

        assert!(result.is_none());
    }

    #[test]
    fn test_pending_without_position() {
        let mut stitcher = Stitcher::new();

        // Pending track without position (rare but possible)
        let person = Person::new(100);
        stitcher.add_pending(person, None);

        // New track with position - can't match pending without position
        let result = stitcher.find_match(Some([1.0, 1.0, 1.70]));

        assert!(result.is_none());
        assert_eq!(stitcher.pending_count(), 1); // Still pending
    }

    #[test]
    fn test_best_match_selected() {
        let mut stitcher = Stitcher::new();

        // Add two pending tracks
        let mut person1 = Person::new(100);
        person1.authorized = false;
        stitcher.add_pending(person1, Some([1.0, 1.0, 1.70]));

        let mut person2 = Person::new(200);
        person2.authorized = true;
        stitcher.add_pending(person2, Some([1.2, 1.0, 1.70])); // Closer

        // New track - should match closer one (person2, 10cm away vs 30cm)
        let result = stitcher.find_match(Some([1.3, 1.0, 1.70]));

        assert!(result.is_some());
        let stitch = result.unwrap();
        assert_eq!(stitch.person.track_id, 200); // Closer match
        assert!(stitch.person.authorized);
        assert_eq!(stitch.distance_cm, 10); // 10cm from person2
        assert_eq!(stitcher.pending_count(), 1); // person1 still pending
    }

    #[test]
    fn test_absolutely_no_stitch() {
        let mut stitcher = Stitcher::new();

        let mut person = Person::new(100);
        person.authorized = true;
        person.accumulated_dwell_ms = 99999;

        // Pending at one corner of the store
        stitcher.add_pending(person, Some([0.0, 0.0, 1.50]));

        // New track at opposite corner, completely different height
        // Distance: 10m away (1000cm >> 180cm limit)
        // Height: 50cm different (>> 10cm limit)
        let result = stitcher.find_match(Some([10.0, 10.0, 2.00]));

        assert!(result.is_none());
        // Pending track should still be there
        assert_eq!(stitcher.pending_count(), 1);
    }
}
