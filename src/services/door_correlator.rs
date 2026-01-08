//! Gate/Door correlation for journey management
//!
//! Correlates gate commands sent by the tracker with actual door state
//! changes from the RS485 monitor.

use crate::domain::journey::{epoch_ms, JourneyEvent};
use crate::domain::types::DoorStatus;
use crate::services::journey_manager::JourneyManager;
use std::time::Instant;
use tracing::{debug, info};

/// Maximum time window to correlate gate_open with gate_cmd (5 seconds)
const MAX_GATE_CORRELATION_MS: u64 = 5000;

/// Tracks recent gate commands for correlation
#[derive(Debug, Clone)]
struct PendingGateCmd {
    track_id: i64,
    sent_at: Instant,
    _sent_at_ms: u64,    // epoch ms
    door_was_open: bool, // door state when command was sent
}

/// Correlates door state changes with journey gate commands
pub struct DoorCorrelator {
    /// Previous door status for detecting transitions
    last_status: DoorStatus,
    /// Pending gate commands waiting for correlation
    pending_cmds: Vec<PendingGateCmd>,
    /// Track ID of current gate flow (preserved across open/moving/closed cycle)
    current_flow_track_id: Option<i64>,
}

impl DoorCorrelator {
    pub fn new() -> Self {
        Self {
            last_status: DoorStatus::Unknown,
            pending_cmds: Vec::new(),
            current_flow_track_id: None,
        }
    }

    /// Record that a gate command was sent for a track
    pub fn record_gate_cmd(&mut self, track_id: i64) {
        let now = Instant::now();
        let now_ms = epoch_ms();

        // Record door state at time of command (per-command, not global)
        let door_was_open = self.last_status == DoorStatus::Open;

        debug!(
            track_id = %track_id,
            door_status = %self.last_status.as_str(),
            door_was_open = %door_was_open,
            "gate_cmd_recorded"
        );

        self.pending_cmds.push(PendingGateCmd {
            track_id,
            sent_at: now,
            _sent_at_ms: now_ms,
            door_was_open,
        });
    }

    /// Process a door state change and correlate with pending gate commands
    /// Returns Some(track_id) if correlated with a journey
    pub fn process_door_state(
        &mut self,
        status: DoorStatus,
        journey_manager: &mut JourneyManager,
    ) -> Option<i64> {
        let prev_status = self.last_status;
        self.last_status = status;

        // Clean up old pending commands
        self.cleanup_old_cmds();

        // Clear current flow when door closes (cycle complete)
        if status == DoorStatus::Closed {
            self.current_flow_track_id = None;
        }

        // Only correlate on transition TO open
        if status != DoorStatus::Open || prev_status == DoorStatus::Open {
            debug!(
                status = %status.as_str(),
                prev_status = %prev_status.as_str(),
                "door_state_no_correlation"
            );
            return self.current_flow_track_id; // Return current flow even if no new correlation
        }

        let now = Instant::now();
        let now_ms = epoch_ms();

        // Find the most recent (newest) gate command within window
        // Iterate from end to find the newest valid command
        let cmd_idx = self
            .pending_cmds
            .iter()
            .enumerate()
            .rev()
            .find(|(_, cmd)| {
                let elapsed_ms = now.duration_since(cmd.sent_at).as_millis() as u64;
                elapsed_ms <= MAX_GATE_CORRELATION_MS
            })
            .map(|(idx, _)| idx);

        if let Some(idx) = cmd_idx {
            let cmd = self.pending_cmds.swap_remove(idx);
            let delta_ms = now.duration_since(cmd.sent_at).as_millis() as u64;
            let track_id = cmd.track_id;

            // Set current flow track_id (preserved across moving/open/closed)
            self.current_flow_track_id = Some(track_id);

            info!(
                track_id = %track_id,
                delta_ms = %delta_ms,
                door_was_open = %cmd.door_was_open,
                "gate_open_correlated"
            );

            // Update journey with per-command door state
            if let Some(journey) = journey_manager.get_mut(track_id) {
                journey.gate_opened_at = Some(now_ms);
                journey.gate_was_open = cmd.door_was_open;
            }

            // Add event to journey
            journey_manager.add_event(
                track_id,
                JourneyEvent::new("gate_open", now_ms).with_extra(&format!("delta_ms={delta_ms}")),
            );

            return Some(track_id);
        }

        debug!(pending_cmds = %self.pending_cmds.len(), "gate_open_no_cmd_found");
        None
    }

    /// Clean up gate commands older than correlation window
    fn cleanup_old_cmds(&mut self) {
        let now = Instant::now();
        self.pending_cmds.retain(|cmd| {
            let elapsed_ms = now.duration_since(cmd.sent_at).as_millis() as u64;
            elapsed_ms <= MAX_GATE_CORRELATION_MS * 2 // Keep a bit longer for safety
        });
    }

    /// Get the current door status
    #[allow(dead_code)]
    pub fn current_status(&self) -> DoorStatus {
        self.last_status
    }

    /// Get the track ID of the current gate flow (preserved across door cycle)
    /// Falls back to most recent pending command if no flow active
    pub fn last_gate_cmd_track_id(&self) -> Option<i64> {
        self.current_flow_track_id.or_else(|| self.pending_cmds.last().map(|cmd| cmd.track_id))
    }

    /// Get the current flow track ID (only set after correlation, cleared on close)
    #[allow(dead_code)]
    pub fn current_flow_track_id(&self) -> Option<i64> {
        self.current_flow_track_id
    }
}

impl Default for DoorCorrelator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::journey_manager::JourneyManager;

    #[test]
    fn test_gate_cmd_recorded() {
        let mut correlator = DoorCorrelator::new();

        correlator.record_gate_cmd(100);

        assert_eq!(correlator.pending_cmds.len(), 1);
        assert_eq!(correlator.pending_cmds[0].track_id, 100);
    }

    #[test]
    fn test_door_open_correlates() {
        let mut correlator = DoorCorrelator::new();
        let mut jm = JourneyManager::new();
        jm.new_journey(100);

        // Record gate command
        correlator.record_gate_cmd(100);

        // Door opens
        let result = correlator.process_door_state(DoorStatus::Open, &mut jm);

        assert_eq!(result, Some(100));
        let journey = jm.get(100).unwrap();
        assert!(journey.gate_opened_at.is_some());
        assert!(!journey.gate_was_open);
    }

    #[test]
    fn test_door_was_already_open() {
        let mut correlator = DoorCorrelator::new();
        let mut jm = JourneyManager::new();
        jm.new_journey(100);

        // Door is already open when command sent
        correlator.last_status = DoorStatus::Open;
        correlator.record_gate_cmd(100);
        assert!(correlator.pending_cmds[0].door_was_open);

        // Door "opens" again (state confirmation)
        correlator.last_status = DoorStatus::Moving; // simulate intermediate state
        let result = correlator.process_door_state(DoorStatus::Open, &mut jm);

        assert_eq!(result, Some(100));
        let journey = jm.get(100).unwrap();
        assert!(journey.gate_was_open);
    }

    #[test]
    fn test_no_correlation_without_cmd() {
        let mut correlator = DoorCorrelator::new();
        let mut jm = JourneyManager::new();

        // Door opens without any gate command
        let result = correlator.process_door_state(DoorStatus::Open, &mut jm);

        assert_eq!(result, None);
    }

    #[test]
    fn test_no_correlation_door_closed() {
        let mut correlator = DoorCorrelator::new();
        let mut jm = JourneyManager::new();
        jm.new_journey(100);

        correlator.record_gate_cmd(100);

        // Door closes (not opens)
        let result = correlator.process_door_state(DoorStatus::Closed, &mut jm);

        assert_eq!(result, None);
    }

    #[test]
    fn test_no_correlation_already_open() {
        let mut correlator = DoorCorrelator::new();
        let mut jm = JourneyManager::new();
        jm.new_journey(100);

        // Door already open
        correlator.last_status = DoorStatus::Open;
        correlator.record_gate_cmd(100);

        // Door is still open (no transition)
        let result = correlator.process_door_state(DoorStatus::Open, &mut jm);

        assert_eq!(result, None);
    }

    #[test]
    fn test_cleanup_old_cmds() {
        let mut correlator = DoorCorrelator::new();
        let mut jm = JourneyManager::new();

        // Add a command with artificially old timestamp
        correlator.pending_cmds.push(PendingGateCmd {
            track_id: 100,
            sent_at: Instant::now() - std::time::Duration::from_secs(15),
            _sent_at_ms: 0,
            door_was_open: false,
        });

        // Process door state (triggers cleanup)
        correlator.process_door_state(DoorStatus::Closed, &mut jm);

        // Old command should be cleaned up
        assert!(correlator.pending_cmds.is_empty());
    }

    #[test]
    fn test_moving_to_open_transition() {
        let mut correlator = DoorCorrelator::new();
        let mut jm = JourneyManager::new();
        jm.new_journey(100);

        correlator.record_gate_cmd(100);

        // Door goes through moving state first
        correlator.process_door_state(DoorStatus::Moving, &mut jm);
        assert_eq!(correlator.pending_cmds.len(), 1); // Still pending

        // Then opens
        let result = correlator.process_door_state(DoorStatus::Open, &mut jm);
        assert_eq!(result, Some(100));
    }

    #[test]
    fn test_newest_command_selected() {
        let mut correlator = DoorCorrelator::new();
        let mut jm = JourneyManager::new();
        jm.new_journey(100);
        jm.new_journey(200);

        // Record two gate commands - 100 first, then 200
        correlator.record_gate_cmd(100);
        correlator.record_gate_cmd(200);

        assert_eq!(correlator.pending_cmds.len(), 2);

        // Door opens - should match the NEWEST command (200), not the oldest (100)
        let result = correlator.process_door_state(DoorStatus::Open, &mut jm);

        assert_eq!(result, Some(200)); // Newest, not oldest!
        assert_eq!(correlator.pending_cmds.len(), 1); // 100 still pending
        assert_eq!(correlator.pending_cmds[0].track_id, 100);
    }

    #[test]
    fn test_per_command_door_was_open() {
        let mut correlator = DoorCorrelator::new();
        let mut jm = JourneyManager::new();
        jm.new_journey(100);
        jm.new_journey(200);

        // First command: door is closed
        correlator.last_status = DoorStatus::Closed;
        correlator.record_gate_cmd(100);
        assert!(!correlator.pending_cmds[0].door_was_open);

        // Second command: door is open
        correlator.last_status = DoorStatus::Open;
        correlator.record_gate_cmd(200);
        assert!(correlator.pending_cmds[1].door_was_open);

        // Door transitions from Open -> Moving -> Open
        correlator.last_status = DoorStatus::Moving;
        let result = correlator.process_door_state(DoorStatus::Open, &mut jm);

        // Should match track 200 (newest) and use ITS door_was_open (true)
        assert_eq!(result, Some(200));
        let journey = jm.get(200).unwrap();
        assert!(journey.gate_was_open); // From track 200's command
    }
}
