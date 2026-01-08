//! Event handlers for the Tracker
//!
//! Each handler processes a specific event type, updating person state,
//! journey state, and triggering side effects (gate commands, etc.)

use super::Tracker;
use crate::domain::journey::{epoch_ms, JourneyEvent, JourneyOutcome};
use crate::domain::types::{DoorStatus, ParsedEvent, Person, TrackId};
use crate::infra::metrics::{GATE_STATE_CLOSED, GATE_STATE_MOVING, GATE_STATE_OPEN};
use crate::io::{
    AccDebugPending, AccDebugTrack, AccEventPayload, GateStatePayload, TrackEventPayload,
    ZoneEventPayload,
};
use crate::services::stitcher::StitchMatch;
use std::time::Instant;
use tracing::{debug, info, warn};

/// Xovis GROUP track bit - track IDs with this bit set are group aggregates, not individuals
const XOVIS_GROUP_BIT: i64 = 0x80000000;

impl Tracker {
    /// Handle a new track being created by the sensor
    ///
    /// This may either:
    /// 1. Stitch to an existing pending track (continuing a journey)
    /// 2. Match a recent exit (re-entry detection)
    /// 3. Create a fresh new journey
    pub(crate) fn handle_track_create(&mut self, event: &ParsedEvent) {
        let track_id = event.track_id;

        // Skip Xovis GROUP tracks - they represent aggregates, not individual people
        if track_id & XOVIS_GROUP_BIT != 0 {
            debug!(track_id = %track_id, "skipping_group_track");
            return;
        }

        let ts = epoch_ms();

        // Try to find a stitch candidate
        if let Some(stitch) = self.stitcher.find_match(event.position) {
            let StitchMatch {
                mut person,
                time_ms,
                distance_cm,
            } = stitch;
            self.metrics.record_stitch_matched();
            self.metrics.record_stitch_distance(distance_cm as u64);
            self.metrics.record_stitch_time(time_ms);

            // Stitch found! Transfer state from old track to new track
            let old_track_id = person.track_id;
            person.track_id = TrackId(track_id);
            person.last_position = event.position;

            info!(
                new_track_id = %track_id,
                old_track_id = %old_track_id,
                authorized = %person.authorized,
                dwell_ms = %person.accumulated_dwell_ms,
                time_ms = %time_ms,
                distance_cm = %distance_cm,
                "track_stitched"
            );

            // Publish stitch event to MQTT
            if let Some(ref sender) = self.egress_sender {
                sender.send_track_event(TrackEventPayload {
                    site: None,
                    ts,
                    t: "stitch".to_string(),
                    tid: track_id,
                    prev_tid: Some(old_track_id.0),
                    auth: person.authorized,
                    dwell_ms: person.accumulated_dwell_ms,
                    stitch_dist_cm: Some(distance_cm as u64),
                    stitch_time_ms: Some(time_ms),
                    parent_jid: None,
                });
            }

            self.persons.insert(TrackId(track_id), person);

            // Stitch in journey manager (handles event recording)
            self.journey_manager
                .stitch_journey(old_track_id, track_id, time_ms, distance_cm);

            if let Some(journey) = self.journey_manager.get_any(track_id) {
                if journey.authorized {
                    if let Some(p) = self.persons.get_mut(&TrackId(track_id)) {
                        p.authorized = true;
                    }
                }
            }
        } else {
            // New track, no stitch - check for re-entry match
            let height = event.position.map(|p| p[2]);
            let reentry_match = self.reentry_detector.try_match(height);

            debug!(track_id = %track_id, reentry = %reentry_match.is_some(), "track_created");
            let mut person = Person::new(TrackId(track_id));
            person.last_position = event.position;
            self.persons.insert(TrackId(track_id), person);

            // Create journey in journey manager (with parent if re-entry)
            if let Some(reentry) = reentry_match {
                // Re-entry detected - create journey with parent reference
                self.journey_manager.new_journey_with_parent(
                    track_id,
                    &reentry.parent_jid,
                    &reentry.parent_pid,
                );
                self.journey_manager.add_event(
                    track_id,
                    JourneyEvent::new("track_create", ts)
                        .with_extra(&format!("reentry_from={}", reentry.parent_jid)),
                );

                // Publish re-entry event to MQTT
                if let Some(ref sender) = self.egress_sender {
                    sender.send_track_event(TrackEventPayload {
                        site: None,
                        ts,
                        t: "reentry".to_string(),
                        tid: track_id,
                        prev_tid: None,
                        auth: false,
                        dwell_ms: 0,
                        stitch_dist_cm: None,
                        stitch_time_ms: None,
                        parent_jid: Some(reentry.parent_jid.clone()),
                    });
                }
            } else {
                self.journey_manager.new_journey(track_id);
                self.journey_manager
                    .add_event(track_id, JourneyEvent::new("track_create", ts));

                // Publish create event to MQTT
                if let Some(ref sender) = self.egress_sender {
                    sender.send_track_event(TrackEventPayload {
                        site: None,
                        ts,
                        t: "create".to_string(),
                        tid: track_id,
                        prev_tid: None,
                        auth: false,
                        dwell_ms: 0,
                        stitch_dist_cm: None,
                        stitch_time_ms: None,
                        parent_jid: None,
                    });
                }
            }
        }
    }

    /// Handle a track being deleted by the sensor
    ///
    /// The track goes to the stitch pending pool for potential reconnection.
    /// If no stitch occurs within the time window, the journey will be finalized.
    pub(crate) fn handle_track_delete(&mut self, event: &ParsedEvent) {
        let track_id = event.track_id;

        // Skip Xovis GROUP tracks
        if track_id & XOVIS_GROUP_BIT != 0 {
            return;
        }

        let ts = epoch_ms();

        if let Some(mut person) = self.persons.remove(&TrackId(track_id)) {
            // Update last position from event if available
            if event.position.is_some() {
                person.last_position = event.position;
            }

            let last_zone = person.current_zone.map(|id| self.config.zone_name(id)).unwrap_or_default();

            info!(
                track_id = %track_id,
                authorized = %person.authorized,
                dwell_ms = %person.accumulated_dwell_ms,
                last_zone = %last_zone,
                "track_pending_stitch"
            );

            // Publish pending event to MQTT (track deleted, waiting for stitch)
            if let Some(ref sender) = self.egress_sender {
                sender.send_track_event(TrackEventPayload {
                    site: None,
                    ts,
                    t: "pending".to_string(),
                    tid: track_id,
                    prev_tid: None,
                    auth: person.authorized,
                    dwell_ms: person.accumulated_dwell_ms,
                    stitch_dist_cm: None,
                    stitch_time_ms: None,
                    parent_jid: None,
                });
            }

            // Update journey manager state before going to pending
            if let Some(journey) = self.journey_manager.get_mut(track_id) {
                journey.authorized = person.authorized;
                journey.total_dwell_ms = person.accumulated_dwell_ms;
            }
            self.journey_manager.add_event(
                track_id,
                JourneyEvent::new("pending", ts)
                    .with_zone(&last_zone)
                    .with_extra(&format!("auth={},dwell={}", person.authorized, person.accumulated_dwell_ms)),
            );
            self.journey_manager
                .end_journey(track_id, JourneyOutcome::Abandoned);

            // Add to stitcher for potential re-connection (with zone context)
            let last_zone_name = if last_zone.is_empty() {
                None
            } else {
                Some(last_zone.clone())
            };
            self.stitcher
                .add_pending(person, event.position, last_zone_name);
        }
    }

    /// Handle a person entering a zone
    ///
    /// Special handling for:
    /// - POS zones: start dwell timer
    /// - Gate zone (when authorized): send gate open command
    pub(crate) async fn handle_zone_entry(&mut self, event: &ParsedEvent) {
        let track_id = event.track_id;

        // Skip Xovis GROUP tracks
        if track_id & XOVIS_GROUP_BIT != 0 {
            return;
        }

        let geometry_id = event.geometry_id.unwrap_or(0);
        let zone = self.config.zone_name(geometry_id);
        let ts = epoch_ms();

        debug!(
            track_id = %track_id,
            zone = %zone,
            event_time = %event.event_time,
            "zone_entry"
        );

        // Get or create person
        let person = self
            .persons
            .entry(track_id)
            .or_insert_with(|| Person::new(track_id));
        person.current_zone = Some(geometry_id);
        let journey_authorized =
            self.journey_manager.get_any(track_id).map(|j| j.authorized).unwrap_or(false);
        let authorized = person.authorized || journey_authorized;
        let gate_already_opened =
            self.journey_manager.get_any(track_id).and_then(|j| j.gate_cmd_at).is_some();

        // Add to journey manager
        self.journey_manager.add_event(track_id, JourneyEvent::new("zone_entry", ts).with_zone(&zone));

        // Publish zone event to MQTT
        if let Some(ref sender) = self.egress_sender {
            sender.send_zone_event(ZoneEventPayload {
                site: None,
                tid: track_id,
                t: "zone_entry".to_string(),
                z: Some(zone.clone()),
                ts,
                auth: person.authorized,
                dwell_ms: None,
                total_dwell_ms: Some(person.accumulated_dwell_ms),
            });
        }

        if self.config.is_pos_zone(geometry_id) {
            person.zone_entered_at = Some(event.received_at);
            // Record POS entry for ACC matching
            self.acc_collector.record_pos_entry(track_id, &zone);
            // Update POS occupancy metric
            self.metrics.pos_zone_enter(geometry_id);
        } else if geometry_id == self.config.gate_zone() {
            // Gate zone - check authorization and send command or blocked event
            if authorized && !gate_already_opened {
                self.send_gate_open_command(track_id, ts, "tracker", event.received_at)
                    .await;
            } else if !authorized {
                // Emit gate blocked event for TUI visibility
                info!(
                    track_id = %track_id,
                    dwell_ms = %person.accumulated_dwell_ms,
                    "gate_entry_not_authorized"
                );
                if let Some(ref sender) = self.egress_sender {
                    sender.send_gate_state(GateStatePayload {
                        site: None,
                        ts,
                        state: "blocked".to_string(),
                        tid: Some(track_id),
                        src: "tracker".to_string(),
                    });
                }
            }
        }
    }

    /// Handle a person exiting a zone
    ///
    /// For POS zones, calculates dwell time and checks authorization threshold.
    pub(crate) fn handle_zone_exit(&mut self, event: &ParsedEvent) {
        let track_id = event.track_id;

        // Skip Xovis GROUP tracks
        if track_id & XOVIS_GROUP_BIT != 0 {
            return;
        }

        let geometry_id = event.geometry_id.unwrap_or(0);
        let zone = self.config.zone_name(geometry_id);
        let ts = epoch_ms();

        debug!(
            track_id = %track_id,
            zone = %zone,
            event_time = %event.event_time,
            "zone_exit"
        );

        let Some(person) = self.persons.get_mut(&TrackId(track_id)) else {
            return;
        };

        // Calculate dwell time if exiting a POS zone
        let zone_dwell_ms = if self.config.is_pos_zone(geometry_id) {
            if let Some(entered_at) = person.zone_entered_at.take() {
                let dwell_ms = entered_at.elapsed().as_millis() as u64;
                person.accumulated_dwell_ms += dwell_ms;

                // Record POS exit for ACC matching
                self.acc_collector
                    .record_pos_exit(track_id, &zone, dwell_ms);
                // Update POS occupancy metric
                self.metrics.pos_zone_exit(geometry_id);

                // Update journey manager
                self.journey_manager.add_event(
                    track_id,
                    JourneyEvent::new("zone_exit", ts)
                        .with_zone(&zone)
                        .with_extra(&format!("dwell={dwell_ms}")),
                );
                if let Some(journey) = self.journey_manager.get_mut(track_id) {
                    journey.total_dwell_ms = person.accumulated_dwell_ms;
                }

                // Log when dwell threshold met (authorization comes from ACC match)
                if person.accumulated_dwell_ms >= self.config.min_dwell_ms() {
                    debug!(
                        track_id = %track_id,
                        zone = %zone,
                        dwell_ms = %person.accumulated_dwell_ms,
                        "dwell_threshold_met"
                    );
                }
                Some(dwell_ms)
            } else {
                None
            }
        } else {
            self.journey_manager
                .add_event(track_id, JourneyEvent::new("zone_exit", ts).with_zone(&zone));
            None
        };

        // Publish zone exit event to MQTT
        if let Some(ref sender) = self.egress_sender {
            sender.send_zone_event(ZoneEventPayload {
                site: None,
                tid: track_id,
                t: "zone_exit".to_string(),
                z: Some(zone.clone()),
                ts,
                auth: person.authorized,
                dwell_ms: zone_dwell_ms,
                total_dwell_ms: Some(person.accumulated_dwell_ms),
            });
        }

        person.current_zone = None;
    }

    /// Handle a person crossing a line
    ///
    /// Special handling for:
    /// - Entry line: marks journey as having crossed entry
    /// - Exit line (forward): completes the journey
    pub(crate) fn handle_line_cross(&mut self, event: &ParsedEvent, direction: &str) {
        let track_id = event.track_id;

        // Skip Xovis GROUP tracks
        if track_id & XOVIS_GROUP_BIT != 0 {
            return;
        }

        let geometry_id = event.geometry_id.unwrap_or(0);
        let line = self.config.zone_name(geometry_id);
        let ts = epoch_ms();

        debug!(
            track_id = %track_id,
            line = %line,
            direction = %direction,
            event_time = %event.event_time,
            "line_cross"
        );

        // Determine event type based on line
        let event_type = if self.config.entry_line() == Some(geometry_id) {
            "entry_cross"
        } else if geometry_id == self.config.exit_line() {
            "exit_cross"
        } else if self.config.approach_line() == Some(geometry_id) {
            "approach_cross"
        } else {
            "line_cross"
        };

        // Add line cross event to journey manager
        self.journey_manager.add_event(
            track_id,
            JourneyEvent::new(event_type, ts).with_extra(&format!("dir={direction}")),
        );

        // Mark crossed_entry if this is the entry line
        if self.config.entry_line() == Some(geometry_id) && direction == "forward" {
            if let Some(journey) = self.journey_manager.get_mut(track_id) {
                journey.crossed_entry = true;
            }
        }

        let Some(person) = self.persons.remove(&TrackId(track_id)) else {
            return;
        };

        // Journey complete if crossing exit line forward
        if geometry_id == self.config.exit_line() && direction == "forward" {
            // Get journey info for logging
            let (gate_cmd_at, event_count, started_at) = self
                .journey_manager
                .get(track_id)
                .map(|j| (j.gate_cmd_at, j.events.len(), j.started_at))
                .unwrap_or((None, 0, 0));
            let duration_ms = if started_at > 0 { epoch_ms().saturating_sub(started_at) } else { 0 };

            info!(
                track_id = %track_id,
                authorized = %person.authorized,
                gate_opened = %gate_cmd_at.is_some(),
                duration_ms = %duration_ms,
                dwell_ms = %person.accumulated_dwell_ms,
                events = %event_count,
                "journey_complete"
            );

            // Sync final state to journey manager and complete
            if let Some(journey) = self.journey_manager.get_mut(track_id) {
                journey.authorized = person.authorized;
                journey.total_dwell_ms = person.accumulated_dwell_ms;

                // Record exit for potential re-entry detection
                let height = person.last_position.map(|p| p[2]);
                self.reentry_detector
                    .record_exit(&journey.jid, &journey.pid, height);

                // Record exit in Prometheus metrics
                self.metrics.record_exit();
            }
            self.journey_manager
                .end_journey(track_id, JourneyOutcome::Completed);
        } else {
            // Put person back - not complete yet
            self.persons.insert(TrackId(track_id), person);
        }
    }

    /// Handle door state change from RS485 monitor
    ///
    /// Correlates door open events with recent gate commands.
    pub(crate) fn handle_door_state_change(&mut self, status: DoorStatus) {
        info!(door_status = %status.as_str(), "door_state_change");

        // Update Prometheus gate state metric
        let state_value = match status {
            DoorStatus::Open => GATE_STATE_OPEN,
            DoorStatus::Closed => GATE_STATE_CLOSED,
            DoorStatus::Moving => GATE_STATE_MOVING,
            DoorStatus::Unknown => GATE_STATE_CLOSED, // Default to closed for unknown
        };
        self.metrics.set_gate_state(state_value);

        // Publish gate state change to MQTT
        if let Some(ref sender) = self.egress_sender {
            let state = match status {
                DoorStatus::Open => "open",
                DoorStatus::Closed => "closed",
                DoorStatus::Moving => "moving",
                DoorStatus::Unknown => "unknown",
            };
            sender.send_gate_state(GateStatePayload {
                site: None,
                ts: epoch_ms(),
                state: state.to_string(),
                tid: self.door_correlator.last_gate_cmd_track_id(),
                src: "rs485".to_string(),
            });
        }

        // Correlate door state with recent gate commands
        self.door_correlator
            .process_door_state(status, &mut self.journey_manager);
    }

    /// Handle an ACC (payment terminal) event
    ///
    /// The ip is the peer IP address of the ACC terminal connection.
    /// It's mapped to a POS zone via the ip_to_pos config.
    /// If a group is detected (co-presence), all group members are authorized.
    pub(crate) async fn handle_acc_event(&mut self, ip: &str, received_at: Instant) {
        let ts = epoch_ms();

        // Look up POS zone from IP
        let pos = self.acc_collector.pos_for_ip(ip).cloned();

        // Build accumulated dwell map from persons for ACC matching
        // This ensures ACC uses total journey dwell, not just current POS session dwell
        let accumulated_dwells: std::collections::HashMap<i64, u64> =
            self.persons.iter().map(|(tid, p)| (tid.0, p.accumulated_dwell_ms)).collect();

        // Try to match ACC to journeys using IP → POS lookup
        // Returns all group members if any member qualifies (sufficient dwell)
        let matched_tracks = self.acc_collector.process_acc(
            ip,
            &mut self.journey_manager,
            Some(&accumulated_dwells),
        );

        // Record ACC metric
        self.metrics.record_acc_event(!matched_tracks.is_empty());

        // Authorize all matched group members
        for &track_id in &matched_tracks {
            if let Some(person) = self.persons.get_mut(&TrackId(track_id)) {
                person.authorized = true;
            }
            if let Some(journey) = self.journey_manager.get_mut_any(track_id) {
                journey.authorized = true;
            } else {
                self.metrics.record_acc_no_journey();
                warn!(
                    track_id = %track_id,
                    ip = %ip,
                    pos = ?pos,
                    "acc_matched_no_journey"
                );
                if let Some(ref sender) = self.egress_sender {
                    sender.send_acc_event(AccEventPayload {
                        site: None,
                        ts,
                        t: "matched_no_journey".to_string(),
                        ip: ip.to_string(),
                        pos: pos.clone(),
                        tid: Some(track_id),
                        dwell_ms: None,
                        gate_zone: None,
                        gate_entry_ts: None,
                        delta_ms: None,
                        gate_cmd_at: None,
                        debug_active: None,
                        debug_pending: None,
                    });
                }
            }
        }

        let gate_zone = self.config.gate_zone();
        let gate_zone_name = self.config.zone_name(gate_zone);
        for &track_id in &matched_tracks {
            if let Some(journey) = self.journey_manager.get_any(track_id) {
                let gate_entry_ts = journey
                    .events
                    .iter()
                    .rev()
                    .find(|e| {
                        e.t == "zone_entry" && e.z.as_deref() == Some(gate_zone_name.as_str())
                    })
                    .map(|e| e.ts);
                if let Some(entry_ts) = gate_entry_ts {
                    let delta_ms = ts.saturating_sub(entry_ts);
                    if delta_ms > 0 {
                        self.metrics.record_acc_late();
                        info!(
                            track_id = %track_id,
                            ip = %ip,
                            pos = ?pos,
                            gate_zone = %gate_zone_name,
                            gate_entry_ts = %entry_ts,
                            acc_ts = %ts,
                            delta_ms = %delta_ms,
                            gate_cmd_at = ?journey.gate_cmd_at,
                            "late_acc_after_gate_entry"
                        );
                        if let Some(ref sender) = self.egress_sender {
                            sender.send_acc_event(AccEventPayload {
                                site: None,
                                ts,
                                t: "late_after_gate".to_string(),
                                ip: ip.to_string(),
                                pos: pos.clone(),
                                tid: Some(track_id),
                                dwell_ms: self
                                    .persons
                                    .get(&TrackId(track_id))
                                    .map(|p| p.accumulated_dwell_ms),
                                gate_zone: Some(gate_zone_name.clone()),
                                gate_entry_ts: Some(entry_ts),
                                delta_ms: Some(delta_ms),
                                gate_cmd_at: journey.gate_cmd_at,
                                debug_active: None,
                                debug_pending: None,
                            });
                        }
                    }
                }
            }

            let in_gate_zone = self
                .persons
                .get(&TrackId(track_id))
                .and_then(|p| p.current_zone)
                .is_some_and(|z| z == gate_zone);
            let gate_already_opened =
                self.journey_manager.get_any(track_id).and_then(|j| j.gate_cmd_at).is_some();
            if in_gate_zone && !gate_already_opened {
                self.send_gate_open_command(track_id, ts, "acc", received_at)
                    .await;
            }
        }

        if !matched_tracks.is_empty() {
            info!(
                ip = %ip,
                pos = ?pos,
                group_size = %matched_tracks.len(),
                tracks = ?matched_tracks,
                "acc_group_authorized"
            );
        }

        // Publish ACC event to MQTT
        if let Some(ref sender) = self.egress_sender {
            if !matched_tracks.is_empty() {
                // Matched - send event for primary track (first in group)
                let primary_track = matched_tracks[0];
                let dwell_ms = self
                    .persons
                    .get(&primary_track)
                    .map(|p| p.accumulated_dwell_ms);
                sender.send_acc_event(AccEventPayload {
                    site: None,
                    ts,
                    t: "matched".to_string(),
                    ip: ip.to_string(),
                    pos: pos.clone(),
                    tid: Some(primary_track),
                    dwell_ms,
                    gate_zone: None,
                    gate_entry_ts: None,
                    delta_ms: None,
                    gate_cmd_at: None,
                    debug_active: None,
                    debug_pending: None,
                });
            } else {
                // Unmatched (unknown IP or no one at POS) - include debug info
                let debug_active: Vec<AccDebugTrack> = self
                    .persons
                    .iter()
                    .map(|(tid, p)| AccDebugTrack {
                        tid: tid.0,
                        zone: p.current_zone.map(|z| self.config.zone_name(z)),
                        dwell_ms: p.accumulated_dwell_ms,
                        auth: p.authorized,
                    })
                    .collect();

                let debug_pending: Vec<AccDebugPending> = self
                    .stitcher
                    .get_pending_info()
                    .into_iter()
                    .map(|p| AccDebugPending {
                        tid: p.track_id,
                        last_zone: p.last_zone,
                        dwell_ms: p.dwell_ms,
                        auth: p.authorized,
                        pending_ms: p.pending_ms,
                    })
                    .collect();

                info!(
                    ip = %ip,
                    pos = ?pos,
                    active_tracks = %debug_active.len(),
                    pending_tracks = %debug_pending.len(),
                    "acc_unmatched"
                );

                sender.send_acc_event(AccEventPayload {
                    site: None,
                    ts,
                    t: "unmatched".to_string(),
                    ip: ip.to_string(),
                    pos: pos.clone(),
                    tid: None,
                    dwell_ms: None,
                    gate_zone: None,
                    gate_entry_ts: None,
                    delta_ms: None,
                    gate_cmd_at: None,
                    debug_active: if debug_active.is_empty() { None } else { Some(debug_active) },
                    debug_pending: if debug_pending.is_empty() { None } else { Some(debug_pending) },
                    debug_pending: if debug_pending.is_empty() {
                        None
                    } else {
                        Some(debug_pending)
                    },
                });
            }
        }
    }

    /// Send gate open command and record E2E latency
    ///
    /// `received_at` is when the triggering event was received (zone entry or ACC).
    /// This allows us to measure the full E2E latency from event reception to gate command.
    async fn send_gate_open_command(
        &mut self,
        track_id: i64,
        ts: u64,
        src: &str,
        received_at: Instant,
    ) {
        let cmd_latency_us = self.gate.send_open_command(track_id).await;
        self.metrics.record_gate_command();

        // Record E2E gate latency (from event received to command queued)
        let e2e_latency_us = received_at.elapsed().as_micros() as u64;
        self.metrics.record_gate_latency(e2e_latency_us);

        if let Some(journey) = self.journey_manager.get_mut_any(track_id) {
            journey.gate_cmd_at = Some(ts);
        }
        self.journey_manager.add_event(
            track_id,
            JourneyEvent::new("gate_cmd", ts)
                .with_extra(&format!("cmd_us={cmd_latency_us},e2e_us={e2e_latency_us}")),
        );

        if let Some(ref sender) = self.egress_sender {
            sender.send_gate_state(GateStatePayload {
                site: None,
                ts,
                state: "cmd_sent".to_string(),
                tid: Some(track_id),
                src: src.to_string(),
            });
        }

        self.door_correlator.record_gate_cmd(track_id);
    }
}
