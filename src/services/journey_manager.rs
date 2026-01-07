//! Journey manager for tracking and persisting customer journeys

use crate::domain::journey::{epoch_ms, Journey, JourneyEvent, JourneyOutcome};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// Delay before emitting a journey (allows for stitching)
const EGRESS_DELAY: Duration = Duration::from_secs(10);

/// A journey pending egress
struct PendingEgress {
    journey: Journey,
    eligible_at: Instant,
}

/// Manages active journeys and handles stitching/egress
pub struct JourneyManager {
    /// Active journeys by current track_id
    active: HashMap<i64, Journey>,
    /// Journeys waiting for egress (10s delay)
    pending_egress: Vec<PendingEgress>,
    /// Mapping of track_id to person_id for stitch lookups
    pid_by_track: HashMap<i64, String>,
}

impl JourneyManager {
    pub fn new() -> Self {
        Self {
            active: HashMap::new(),
            pending_egress: Vec::new(),
            pid_by_track: HashMap::new(),
        }
    }

    /// Create a new journey for a track
    pub fn new_journey(&mut self, track_id: i64) -> &Journey {
        let journey = Journey::new(track_id);
        let pid = journey.pid.clone();

        debug!(
            track_id = %track_id,
            jid = %journey.jid,
            pid = %pid,
            "journey_created"
        );

        self.pid_by_track.insert(track_id, pid);
        self.active.insert(track_id, journey);
        self.active.get(&track_id).unwrap()
    }

    /// Create a new journey with parent reference (for re-entry)
    pub fn new_journey_with_parent(&mut self, track_id: i64, parent_jid: &str, parent_pid: &str) -> &Journey {
        let journey = Journey::new_with_parent(track_id, parent_jid, parent_pid);
        let pid = journey.pid.clone();

        info!(
            track_id = %track_id,
            jid = %journey.jid,
            pid = %pid,
            parent_jid = %parent_jid,
            "journey_created_reentry"
        );

        self.pid_by_track.insert(track_id, pid);
        self.active.insert(track_id, journey);
        self.active.get(&track_id).unwrap()
    }

    /// Stitch a journey from old track to new track
    /// Returns true if stitch was successful
    pub fn stitch_journey(&mut self, old_track_id: i64, new_track_id: i64, time_ms: u64, distance_cm: u32) -> bool {
        // First try to find in pending_egress
        let pending_idx = self.pending_egress.iter()
            .position(|p| p.journey.current_track_id() == old_track_id);

        let journey = if let Some(idx) = pending_idx {
            // Remove from pending egress
            let pending = self.pending_egress.remove(idx);
            Some(pending.journey)
        } else {
            // Try active journeys
            self.active.remove(&old_track_id)
        };

        if let Some(mut journey) = journey {
            let old_pid = journey.pid.clone();
            let old_jid = journey.jid.clone();

            // Add stitch event
            journey.add_event(
                JourneyEvent::new("stitch", epoch_ms())
                    .with_extra(&format!("from={old_track_id},time_ms={time_ms},dist_cm={distance_cm}"))
            );

            // Add new track ID to history
            journey.add_track_id(new_track_id);

            // Reset outcome to in progress (was abandoned on delete)
            journey.outcome = JourneyOutcome::InProgress;
            journey.ended_at = None;

            info!(
                old_track_id = %old_track_id,
                new_track_id = %new_track_id,
                jid = %old_jid,
                pid = %old_pid,
                time_ms = %time_ms,
                distance_cm = %distance_cm,
                "journey_stitched"
            );

            // Update pid mapping
            self.pid_by_track.remove(&old_track_id);
            self.pid_by_track.insert(new_track_id, old_pid);

            // Re-activate the journey
            self.active.insert(new_track_id, journey);
            true
        } else {
            debug!(
                old_track_id = %old_track_id,
                new_track_id = %new_track_id,
                "stitch_failed_no_journey"
            );
            false
        }
    }

    /// Add an event to a journey
    pub fn add_event(&mut self, track_id: i64, event: JourneyEvent) {
        if let Some(journey) = self.active.get_mut(&track_id) {
            journey.add_event(event);
        }
    }

    /// Get mutable reference to journey for a track
    pub fn get_mut(&mut self, track_id: i64) -> Option<&mut Journey> {
        self.active.get_mut(&track_id)
    }

    pub fn get_mut_any(&mut self, track_id: i64) -> Option<&mut Journey> {
        if self.active.contains_key(&track_id) {
            return self.active.get_mut(&track_id);
        }
        self.pending_egress
            .iter_mut()
            .find(|p| p.journey.current_track_id() == track_id)
            .map(|p| &mut p.journey)
    }

    /// Get immutable reference to journey for a track
    pub fn get(&self, track_id: i64) -> Option<&Journey> {
        self.active.get(&track_id)
    }

    pub fn get_any(&self, track_id: i64) -> Option<&Journey> {
        if self.active.contains_key(&track_id) {
            return self.active.get(&track_id);
        }
        self.pending_egress
            .iter()
            .find(|p| p.journey.current_track_id() == track_id)
            .map(|p| &p.journey)
    }

    /// End a journey and move to pending egress
    pub fn end_journey(&mut self, track_id: i64, outcome: JourneyOutcome) {
        if let Some(mut journey) = self.active.remove(&track_id) {
            journey.complete(outcome.clone());

            info!(
                track_id = %track_id,
                jid = %journey.jid,
                outcome = %outcome.as_str(),
                crossed_entry = %journey.crossed_entry,
                "journey_ended"
            );

            // Add to pending egress with 10s delay
            self.pending_egress.push(PendingEgress {
                journey,
                eligible_at: Instant::now() + EGRESS_DELAY,
            });
        }
    }

    /// Check for journeys ready to emit
    /// Returns journeys that have passed the 10s delay and crossed entry
    pub fn tick(&mut self) -> Vec<Journey> {
        let now = Instant::now();
        let mut ready = Vec::new();
        let mut remaining = Vec::new();

        for pending in self.pending_egress.drain(..) {
            if now >= pending.eligible_at {
                // Remove from pid_by_track
                for tid in &pending.journey.tids {
                    self.pid_by_track.remove(tid);
                }

                if pending.journey.crossed_entry {
                    info!(
                        jid = %pending.journey.jid,
                        pid = %pending.journey.pid,
                        tids = ?pending.journey.tids,
                        outcome = %pending.journey.outcome.as_str(),
                        "journey_ready_for_egress"
                    );
                    ready.push(pending.journey);
                } else {
                    debug!(
                        jid = %pending.journey.jid,
                        tids = ?pending.journey.tids,
                        "journey_discarded_no_entry"
                    );
                }
            } else {
                remaining.push(pending);
            }
        }

        self.pending_egress = remaining;
        ready
    }

    /// Number of active journeys
    #[allow(dead_code)]
    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// Number of pending egress journeys
    #[allow(dead_code)]
    pub fn pending_count(&self) -> usize {
        self.pending_egress.len()
    }

    /// Check if a track has an active journey
    pub fn has_journey(&self, track_id: i64) -> bool {
        self.active.contains_key(&track_id)
    }
}

impl Default for JourneyManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_journey() {
        let mut manager = JourneyManager::new();

        let journey = manager.new_journey(100);

        assert_eq!(journey.tids, vec![100]);
        assert!(manager.has_journey(100));
        assert_eq!(manager.active_count(), 1);
    }

    #[test]
    fn test_add_event() {
        let mut manager = JourneyManager::new();
        manager.new_journey(100);

        manager.add_event(100, JourneyEvent::new("zone_entry", 1000).with_zone("POS_1"));

        let journey = manager.get(100).unwrap();
        assert_eq!(journey.events.len(), 1);
        assert_eq!(journey.events[0].t, "zone_entry");
    }

    #[test]
    fn test_end_journey() {
        let mut manager = JourneyManager::new();
        manager.new_journey(100);

        manager.end_journey(100, JourneyOutcome::Completed);

        assert!(!manager.has_journey(100));
        assert_eq!(manager.active_count(), 0);
        assert_eq!(manager.pending_count(), 1);
    }

    #[test]
    fn test_stitch_from_active() {
        let mut manager = JourneyManager::new();
        manager.new_journey(100);

        // Modify journey state
        if let Some(j) = manager.get_mut(100) {
            j.authorized = true;
            j.total_dwell_ms = 5000;
        }

        // Stitch to new track
        let result = manager.stitch_journey(100, 200, 500, 42);

        assert!(result);
        assert!(!manager.has_journey(100));
        assert!(manager.has_journey(200));

        let journey = manager.get(200).unwrap();
        assert_eq!(journey.tids, vec![100, 200]);
        assert!(journey.authorized);
        assert_eq!(journey.total_dwell_ms, 5000);
        assert_eq!(journey.events.len(), 1);
        assert_eq!(journey.events[0].t, "stitch");
    }

    #[test]
    fn test_stitch_from_pending() {
        let mut manager = JourneyManager::new();
        manager.new_journey(100);

        // End journey (moves to pending)
        manager.end_journey(100, JourneyOutcome::Abandoned);
        assert_eq!(manager.pending_count(), 1);

        // Stitch from pending
        let result = manager.stitch_journey(100, 200, 800, 50);

        assert!(result);
        assert!(manager.has_journey(200));
        assert_eq!(manager.pending_count(), 0);

        let journey = manager.get(200).unwrap();
        assert_eq!(journey.outcome, JourneyOutcome::InProgress);
    }

    #[test]
    fn test_stitch_fails_no_journey() {
        let mut manager = JourneyManager::new();

        let result = manager.stitch_journey(100, 200, 500, 42);

        assert!(!result);
        assert!(!manager.has_journey(200));
    }

    #[test]
    fn test_tick_filters_no_entry() {
        let mut manager = JourneyManager::new();
        manager.new_journey(100);

        // End journey without crossing entry
        manager.end_journey(100, JourneyOutcome::Abandoned);

        // Manually set eligible_at to past
        if let Some(pending) = manager.pending_egress.first_mut() {
            pending.eligible_at = Instant::now() - Duration::from_secs(1);
        }

        // Tick should discard (no crossed_entry)
        let ready = manager.tick();

        assert!(ready.is_empty());
        assert_eq!(manager.pending_count(), 0);
    }

    #[test]
    fn test_tick_emits_with_entry() {
        let mut manager = JourneyManager::new();
        manager.new_journey(100);

        // Mark as crossed entry
        if let Some(j) = manager.get_mut(100) {
            j.crossed_entry = true;
        }

        manager.end_journey(100, JourneyOutcome::Completed);

        // Manually set eligible_at to past
        if let Some(pending) = manager.pending_egress.first_mut() {
            pending.eligible_at = Instant::now() - Duration::from_secs(1);
        }

        let ready = manager.tick();

        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].tids, vec![100]);
        assert!(ready[0].crossed_entry);
    }

    #[test]
    fn test_tick_respects_delay() {
        let mut manager = JourneyManager::new();
        manager.new_journey(100);

        if let Some(j) = manager.get_mut(100) {
            j.crossed_entry = true;
        }

        manager.end_journey(100, JourneyOutcome::Completed);

        // Don't modify eligible_at - should still be in future
        let ready = manager.tick();

        assert!(ready.is_empty());
        assert_eq!(manager.pending_count(), 1);
    }

    #[test]
    fn test_journey_state_preserved_on_stitch() {
        let mut manager = JourneyManager::new();
        manager.new_journey(100);

        // Set various state
        if let Some(j) = manager.get_mut(100) {
            j.authorized = true;
            j.total_dwell_ms = 7500;
            j.acc_matched = true;
            j.crossed_entry = true;
            j.gate_cmd_at = Some(1234567890);
        }

        manager.stitch_journey(100, 200, 500, 42);

        let journey = manager.get(200).unwrap();
        assert!(journey.authorized);
        assert_eq!(journey.total_dwell_ms, 7500);
        assert!(journey.acc_matched);
        assert!(journey.crossed_entry);
        assert_eq!(journey.gate_cmd_at, Some(1234567890));
    }
}
