//! Event handlers for the Tracker
//!
//! Each handler processes a specific event type, updating person state,
//! journey state, and triggering side effects (gate commands, etc.)

use super::Tracker;
use crate::domain::journey::{epoch_ms, JourneyEvent, JourneyEventType, JourneyOutcome};
use crate::domain::types::{DoorStatus, GeometryId, ParsedEvent, Person, TrackId};
use crate::infra::metrics::{GATE_STATE_CLOSED, GATE_STATE_MOVING, GATE_STATE_OPEN};
use crate::io::{
    AccDebugPending, AccDebugTrack, AccEventPayload, GateStatePayload, TrackEventPayload,
    ZoneEventPayload,
};
use crate::services::gate_worker::GateCmd;
use crate::services::stitcher::StitchMatch;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info, warn};

/// Xovis GROUP track bit - track IDs with this bit set are group aggregates, not individuals
const XOVIS_GROUP_BIT: i64 = 0x80000000;

/// Check if a track ID represents a Xovis GROUP aggregate (not an individual person)
///
/// Retained for future metrics/logging use. Group tracks now flow through all handlers
/// like regular tracks (can accumulate dwell, be authorized, and trigger gate opens).
#[inline]
#[allow(dead_code)]
fn is_group_track(track_id: TrackId) -> bool {
    track_id.0 & XOVIS_GROUP_BIT != 0
}

impl Tracker {
    /// Handle a new track being created by the sensor
    ///
    /// This may either:
    /// 1. Stitch to an existing pending track (continuing a journey)
    /// 2. Match a recent exit (re-entry detection)
    /// 3. Create a fresh new journey
    pub(crate) fn handle_track_create(&mut self, event: &ParsedEvent) {
        let track_id = event.track_id;
        let ts = epoch_ms();

        // Determine if this looks like a "spawn" (re-detection)
        // If the track appears in a zone (geometry_id present), it might be spawned
        // True spawn detection happens later (no STORE zone, no ENTRY line), but we enable
        // relaxed height matching here for all track creates to help catch re-detections
        let current_zone: Option<Arc<str>> =
            event.geometry_id.map(|gid| self.config.zone_name(gid));
        let spawn_hint = current_zone.as_deref().is_some_and(|z| z.starts_with("POS_"));

        // Fresh store entry: track appears in STORE zone = new customer from store side.
        // Don't stitch these to pending tracks - they're starting a fresh journey.
        let is_fresh_store_entry = current_zone.as_deref() == Some("STORE");

        // Try to find a stitch candidate with spawn-hint context
        // When spawn_hint is true and pending was in POS zone, uses:
        // - 15cm height tolerance (vs 10cm base)
        // - 10s time window if same zone (vs 8s)
        // - 190cm distance if same zone (vs 180cm base)
        // Skip stitch attempt entirely for fresh store entries.
        let stitch_match = if is_fresh_store_entry {
            debug!(track_id = %track_id, "skip_stitch_fresh_store_entry");
            None
        } else {
            self.stitcher.find_match_with_context(
                event.position,
                current_zone.as_deref(),
                spawn_hint,
            )
        };

        if let Some(stitch) = stitch_match {
            let StitchMatch { mut person, time_ms, distance_cm } = stitch;
            self.metrics.record_stitch_matched();
            self.metrics.record_stitch_distance(distance_cm as u64);
            self.metrics.record_stitch_time(time_ms);

            // Stitch found! Transfer state from old track to new track
            let old_track_id = person.track_id;
            person.track_id = track_id;
            person.last_position = event.position;

            let dwell_ms = self.journey_manager.get_dwell(old_track_id);

            debug!(
                new_track_id = %track_id,
                old_track_id = %old_track_id,
                authorized = %person.authorized,
                dwell_ms = %dwell_ms,
                time_ms = %time_ms,
                distance_cm = %distance_cm,
                spawn_hint = %spawn_hint,
                "track_stitched"
            );

            // Publish stitch event to MQTT
            if let Some(ref sender) = self.egress_sender {
                sender.send_track_event(TrackEventPayload {
                    site: None,
                    ts,
                    t: "stitch".to_string(),
                    tid: track_id.0,
                    prev_tid: Some(old_track_id.0),
                    auth: person.authorized,
                    dwell_ms,
                    stitch_dist_cm: Some(distance_cm as u64),
                    stitch_time_ms: Some(time_ms),
                    parent_jid: None,
                });
            }

            self.persons.insert(track_id, person);

            // Stitch in journey manager (handles event recording)
            self.journey_manager.stitch_journey(old_track_id, track_id, time_ms, distance_cm);

            if let Some(journey) = self.journey_manager.get_any(track_id) {
                if journey.authorized {
                    if let Some(p) = self.persons.get_mut(&track_id) {
                        p.authorized = true;
                    }
                }
            }
        } else {
            // New track, no stitch - check for re-entry match
            let height = event.position.map(|p| p[2]);
            let reentry_match = self.reentry_detector.try_match(height);

            debug!(track_id = %track_id, reentry = %reentry_match.is_some(), "track_created");
            let mut person = Person::new(track_id);
            person.last_position = event.position;
            self.persons.insert(track_id, person);

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
                    JourneyEvent::new(JourneyEventType::TrackCreate, ts)
                        .with_extra(&format!("reentry_from={}", reentry.parent_jid)),
                );

                // Publish re-entry event to MQTT
                if let Some(ref sender) = self.egress_sender {
                    sender.send_track_event(TrackEventPayload {
                        site: None,
                        ts,
                        t: "reentry".to_string(),
                        tid: track_id.0,
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
                    .add_event(track_id, JourneyEvent::new(JourneyEventType::TrackCreate, ts));

                // Publish create event to MQTT
                if let Some(ref sender) = self.egress_sender {
                    sender.send_track_event(TrackEventPayload {
                        site: None,
                        ts,
                        t: "create".to_string(),
                        tid: track_id.0,
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
        let ts = epoch_ms();

        if let Some(mut person) = self.persons.remove(&track_id) {
            // Update last position from event if available
            if event.position.is_some() {
                person.last_position = event.position;
            }

            let last_zone: String = person
                .current_zone
                .map(|id| self.config.zone_name(id).to_string())
                .unwrap_or_default();

            let journey_dwell = self.journey_manager.get_dwell(track_id);

            debug!(
                track_id = %track_id,
                authorized = %person.authorized,
                dwell_ms = %journey_dwell,
                last_zone = %last_zone,
                "track_pending_stitch"
            );

            // Publish pending event to MQTT (track deleted, waiting for stitch)
            if let Some(ref sender) = self.egress_sender {
                sender.send_track_event(TrackEventPayload {
                    site: None,
                    ts,
                    t: "pending".to_string(),
                    tid: track_id.0,
                    prev_tid: None,
                    auth: person.authorized,
                    dwell_ms: journey_dwell,
                    stitch_dist_cm: None,
                    stitch_time_ms: None,
                    parent_jid: None,
                });
            }

            // Update journey manager state before going to pending
            if let Some(journey) = self.journey_manager.get_mut(track_id) {
                journey.authorized = person.authorized;
            }
            self.journey_manager.add_event(
                track_id,
                JourneyEvent::new(JourneyEventType::Pending, ts)
                    .with_zone(&last_zone)
                    .with_extra(&format!("auth={},dwell={}", person.authorized, journey_dwell)),
            );

            // Determine journey outcome based on events
            // ReturnedToStore: went back into store (POS zone, STORE, or backward entry cross)
            // Completed: exit inferred (track lost in exit corridor after approach cross)
            // Lost: disappeared near gate/exit area without clear exit signal
            let (outcome, exit_inferred) = self.determine_journey_outcome(track_id);
            if exit_inferred {
                if let Some(journey) = self.journey_manager.get_mut(track_id) {
                    journey.exit_inferred = true;
                }
            }
            self.journey_manager.end_journey(track_id, outcome);

            // Add to stitcher ONLY for genuinely lost tracks (sensor gap).
            // Don't stitch when:
            // - Completed: exited via gate, they're gone
            // - ReturnedToStore: walked into store, can't verify identity of returning track
            // The reentry_detector handles proper re-entry matching by height for store returns.
            if outcome == JourneyOutcome::Lost {
                let last_zone_name = if last_zone.is_empty() { None } else { Some(last_zone) };
                self.stitcher.add_pending(person, event.position, last_zone_name);
            } else {
                debug!(
                    track_id = %track_id,
                    outcome = %outcome.as_str(),
                    "skip_stitch_not_lost"
                );
            }
        }
    }

    /// Handle a person entering a zone
    ///
    /// Special handling for:
    /// - POS zones: start dwell timer
    /// - Gate zone (when authorized): send gate open command
    pub(crate) fn handle_zone_entry(&mut self, event: &ParsedEvent) {
        let track_id = event.track_id;
        let geometry_id = event.geometry_id.unwrap_or(GeometryId(0));
        let zone = self.config.zone_name(geometry_id);
        let ts = epoch_ms();

        debug!(
            track_id = %track_id,
            zone = %zone,
            event_time = %event.event_time,
            "zone_entry"
        );

        // Get or create person
        let person = self.persons.entry(track_id).or_insert_with(|| Person::new(track_id));
        person.current_zone = Some(geometry_id);
        let journey_authorized =
            self.journey_manager.get_any(track_id).map(|j| j.authorized).unwrap_or(false);
        let authorized = person.authorized || journey_authorized;
        let gate_already_opened =
            self.journey_manager.get_any(track_id).and_then(|j| j.gate_cmd_at).is_some();

        // Add to journey manager
        self.journey_manager.add_event(
            track_id,
            JourneyEvent::new(JourneyEventType::ZoneEntry, ts).with_zone(&zone),
        );

        let journey_dwell = self.journey_manager.get_dwell(track_id);

        // Publish zone event to MQTT
        if let Some(ref sender) = self.egress_sender {
            sender.send_zone_event(ZoneEventPayload {
                site: None,
                tid: track_id.0,
                t: "zone_entry".to_string(),
                z: Some(zone.to_string()),
                ts,
                auth: person.authorized,
                dwell_ms: None,
                total_dwell_ms: Some(journey_dwell),
                event_time: Some(event.event_time),
            });
        }

        if self.config.is_pos_zone(geometry_id.0) {
            // Record POS entry in per-zone occupancy state
            self.pos_occupancy.record_entry(&zone, track_id, event.received_at);
            // Update POS occupancy metric
            self.metrics.pos_zone_enter(geometry_id.0);
        } else if geometry_id == self.config.gate_zone() {
            // Gate zone - check authorization and send command or blocked event
            if authorized && !gate_already_opened {
                self.send_gate_open_command(track_id, ts, "tracker", event.received_at);
            } else if !authorized {
                // Emit gate blocked event for TUI visibility
                info!(
                    track_id = %track_id,
                    dwell_ms = %journey_dwell,
                    "gate_entry_not_authorized"
                );
                if let Some(ref sender) = self.egress_sender {
                    sender.send_gate_state(GateStatePayload::new(
                        ts,
                        "blocked",
                        Some(track_id.0),
                        "tracker",
                    ));
                }
            }
        }
    }

    /// Handle a person exiting a zone
    ///
    /// For POS zones, calculates dwell time and checks authorization threshold.
    pub(crate) fn handle_zone_exit(&mut self, event: &ParsedEvent) {
        let track_id = event.track_id;
        let geometry_id = event.geometry_id.unwrap_or(GeometryId(0));
        let zone = self.config.zone_name(geometry_id);
        let ts = epoch_ms();

        debug!(
            track_id = %track_id,
            zone = %zone,
            event_time = %event.event_time,
            "zone_exit"
        );

        let Some(person) = self.persons.get_mut(&track_id) else {
            return;
        };

        // Calculate dwell time if exiting a POS zone (from PosOccupancyState)
        let zone_dwell_ms = if self.config.is_pos_zone(geometry_id.0) {
            // Record POS exit in per-zone occupancy state and get dwell
            let dwell_result = self.pos_occupancy.record_exit(&zone, track_id, Instant::now());
            // Update POS occupancy metric
            self.metrics.pos_zone_exit(geometry_id.0);

            if let Some((session_dwell_ms, _zone_total_dwell_ms)) = dwell_result {
                // Update journey manager with session dwell (adds to journey total)
                self.journey_manager.add_event(
                    track_id,
                    JourneyEvent::new(JourneyEventType::ZoneExit, ts)
                        .with_zone(&zone)
                        .with_extra(&format!("dwell={session_dwell_ms}")),
                );
                // Journey tracks total dwell across ALL zones
                let journey_total = if let Some(journey) = self.journey_manager.get_mut(track_id) {
                    journey.total_dwell_ms += session_dwell_ms;
                    journey.total_dwell_ms
                } else {
                    0
                };

                // Log when dwell threshold met (authorization comes from ACC match)
                if journey_total >= self.config.min_dwell_ms() {
                    debug!(
                        track_id = %track_id,
                        zone = %zone,
                        dwell_ms = %journey_total,
                        "dwell_threshold_met"
                    );
                }
                Some(session_dwell_ms)
            } else {
                None
            }
        } else {
            self.journey_manager.add_event(
                track_id,
                JourneyEvent::new(JourneyEventType::ZoneExit, ts).with_zone(&zone),
            );
            None
        };

        let journey_dwell = self.journey_manager.get_dwell(track_id);

        // Publish zone exit event to MQTT
        if let Some(ref sender) = self.egress_sender {
            sender.send_zone_event(ZoneEventPayload {
                site: None,
                tid: track_id.0,
                t: "zone_exit".to_string(),
                z: Some(zone.to_string()),
                ts,
                auth: person.authorized,
                dwell_ms: zone_dwell_ms,
                total_dwell_ms: Some(journey_dwell),
                event_time: Some(event.event_time),
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
        let geometry_id = event.geometry_id.unwrap_or(GeometryId(0));
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
        let event_type = if self.config.entry_line() == Some(geometry_id.0) {
            JourneyEventType::EntryCross
        } else if geometry_id.0 == self.config.exit_line() {
            JourneyEventType::ExitCross
        } else if self.config.approach_line() == Some(geometry_id.0) {
            JourneyEventType::ApproachCross
        } else {
            JourneyEventType::LineCross
        };

        // Add line cross event to journey manager
        self.journey_manager.add_event(
            track_id,
            JourneyEvent::new(event_type, ts).with_extra(&format!("dir={direction}")),
        );

        // Mark crossed_entry if this is the entry line (forward direction)
        // Backward crossing means person is returning to store
        if self.config.entry_line() == Some(geometry_id.0) {
            if direction == "forward" {
                if let Some(journey) = self.journey_manager.get_mut(track_id) {
                    journey.crossed_entry = true;
                }
            } else if direction == "backward" {
                debug!(
                    track_id = %track_id,
                    "entry_line_backward_returning_to_store"
                );
            }
        }

        let Some(person) = self.persons.remove(&track_id) else {
            return;
        };

        // Journey complete if crossing exit line forward
        if geometry_id.0 == self.config.exit_line() && direction == "forward" {
            // Get journey info for logging
            let (gate_cmd_at, event_count, started_at, journey_dwell) = self
                .journey_manager
                .get(track_id)
                .map(|j| (j.gate_cmd_at, j.events.len(), j.started_at, j.total_dwell_ms))
                .unwrap_or((None, 0, 0, 0));
            let duration_ms =
                if started_at > 0 { epoch_ms().saturating_sub(started_at) } else { 0 };

            info!(
                track_id = %track_id,
                authorized = %person.authorized,
                gate_opened = %gate_cmd_at.is_some(),
                duration_ms = %duration_ms,
                dwell_ms = %journey_dwell,
                events = %event_count,
                "journey_complete"
            );

            // Sync final auth state to journey manager and complete
            if let Some(journey) = self.journey_manager.get_mut(track_id) {
                journey.authorized = person.authorized;

                // Record exit for potential re-entry detection
                let height = person.last_position.map(|p| p[2]);
                self.reentry_detector.record_exit(&journey.jid, &journey.pid, height);

                // Record exit in Prometheus metrics
                self.metrics.record_exit();
            }
            self.journey_manager.end_journey(track_id, JourneyOutcome::Completed);
        } else {
            // Put person back - not complete yet
            self.persons.insert(track_id, person);
        }
    }

    /// Handle door state change from RS485 monitor
    pub(crate) fn handle_door_state_change(&mut self, status: DoorStatus) {
        info!(door_status = %status.as_str(), "door_state_change");

        let state_value = match status {
            DoorStatus::Open => GATE_STATE_OPEN,
            DoorStatus::Moving => GATE_STATE_MOVING,
            DoorStatus::Closed | DoorStatus::Unknown => GATE_STATE_CLOSED,
        };
        self.metrics.set_gate_state(state_value);

        // Publish gate state change to MQTT
        if let Some(ref sender) = self.egress_sender {
            sender.send_gate_state(GateStatePayload::new(
                epoch_ms(),
                status.as_str(),
                self.door_correlator.last_gate_cmd_track_id().map(|t| t.0),
                "rs485",
            ));
        }

        // Correlate door state with recent gate commands
        self.door_correlator.process_door_state(status, &mut self.journey_manager);
    }

    /// Handle an ACC (payment terminal) event
    ///
    /// The ip is the peer IP address of the ACC terminal connection.
    /// It's mapped to a POS zone via the ip_to_pos config.
    /// Uses PosOccupancyState to find candidates with accumulated_dwell >= min_dwell.
    pub(crate) fn handle_acc_event(&mut self, ip: &str, received_at: Instant) {
        let ts = epoch_ms();
        let now = Instant::now();

        // Look up POS zone from IP - early return if unknown
        let Some(pos_zone) = self.acc_collector.pos_for_ip(ip).map(|s| s.to_string()) else {
            self.publish_unmatched_acc_event(ip, None, ts);
            return;
        };

        // Get candidates sorted by: present first (dwell desc), then recent exits (dwell desc)
        let candidates = self.pos_occupancy.get_candidates(&pos_zone, now);
        self.pos_occupancy.prune_expired(&pos_zone, now);

        // Filter by min_dwell_ms
        let min_dwell = self.pos_occupancy.min_dwell_ms();
        let qualified: Vec<(TrackId, u64)> =
            candidates.into_iter().filter(|(_, dwell)| *dwell >= min_dwell).collect();

        if qualified.is_empty() {
            self.metrics.record_acc_event(false);
            self.publish_unmatched_acc_event(ip, Some(&pos_zone), ts);
            return;
        }

        // Primary is first (highest dwell among present, or highest dwell among recent exits)
        let primary = qualified[0].0;
        let authorized_tracks: Vec<TrackId> = qualified.iter().map(|(tid, _)| *tid).collect();

        // Record ACC match on all authorized journeys
        for &track_id in &authorized_tracks {
            if let Some(journey) = self.journey_manager.get_mut_any(track_id) {
                journey.acc_matched = true;
            }
            self.journey_manager.add_event(
                track_id,
                JourneyEvent::new(JourneyEventType::Acc, ts)
                    .with_zone(&pos_zone)
                    .with_extra(&format!("kiosk={ip},count={}", authorized_tracks.len())),
            );
        }

        // Record ACC metric
        self.metrics.record_acc_event(true);

        // Authorize all matched tracks
        for &track_id in &authorized_tracks {
            if let Some(person) = self.persons.get_mut(&track_id) {
                person.authorized = true;
            }
            if let Some(journey) = self.journey_manager.get_mut_any(track_id) {
                journey.authorized = true;
            } else {
                self.metrics.record_acc_no_journey();
                warn!(
                    track_id = %track_id,
                    ip = %ip,
                    pos = %pos_zone,
                    "acc_matched_no_journey"
                );
                if let Some(ref sender) = self.egress_sender {
                    sender.send_acc_event(AccEventPayload {
                        site: None,
                        ts,
                        t: "matched_no_journey".to_string(),
                        ip: ip.to_string(),
                        pos: Some(pos_zone.clone()),
                        tid: Some(track_id.0),
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

        // Check for late ACC (customer already entered gate zone before ACC arrived)
        let gate_zone = self.config.gate_zone();
        let gate_zone_name = self.config.zone_name(gate_zone);
        for &track_id in &authorized_tracks {
            self.check_late_acc_and_open_gate(
                track_id,
                ip,
                &pos_zone,
                &gate_zone_name,
                gate_zone,
                ts,
                received_at,
            );
        }

        info!(
            ip = %ip,
            pos = %pos_zone,
            authorized_count = %authorized_tracks.len(),
            tracks = ?authorized_tracks,
            primary = %primary,
            "acc_authorized"
        );

        // Publish matched ACC event to MQTT
        if let Some(ref sender) = self.egress_sender {
            let dwell_ms = self.journey_manager.get_dwell(primary);
            sender.send_acc_event(AccEventPayload {
                site: None,
                ts,
                t: "matched".to_string(),
                ip: ip.to_string(),
                pos: Some(pos_zone),
                tid: Some(primary.0),
                dwell_ms: Some(dwell_ms),
                gate_zone: None,
                gate_entry_ts: None,
                delta_ms: None,
                gate_cmd_at: None,
                debug_active: None,
                debug_pending: None,
            });
        }
    }

    /// Check if ACC arrived late (after customer entered gate zone) and open gate if needed
    fn check_late_acc_and_open_gate(
        &mut self,
        track_id: TrackId,
        ip: &str,
        pos_zone: &str,
        gate_zone_name: &Arc<str>,
        gate_zone: GeometryId,
        ts: u64,
        received_at: Instant,
    ) {
        // Check for late ACC (customer entered gate zone before ACC)
        if let Some(journey) = self.journey_manager.get_any(track_id) {
            let gate_entry_ts = journey
                .events
                .iter()
                .rev()
                .find(|e| {
                    e.t == JourneyEventType::ZoneEntry && e.z.as_deref() == Some(&**gate_zone_name)
                })
                .map(|e| e.ts);

            if let Some(entry_ts) = gate_entry_ts {
                let delta_ms = ts.saturating_sub(entry_ts);
                if delta_ms > 0 {
                    self.metrics.record_acc_late();
                    info!(
                        track_id = %track_id,
                        ip = %ip,
                        pos = %pos_zone,
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
                            pos: Some(pos_zone.to_string()),
                            tid: Some(track_id.0),
                            dwell_ms: Some(journey.total_dwell_ms),
                            gate_zone: Some(gate_zone_name.to_string()),
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

        // Open gate if customer is currently in gate zone and gate hasn't opened yet
        let in_gate_zone = self
            .persons
            .get(&track_id)
            .and_then(|p| p.current_zone)
            .is_some_and(|z| z == gate_zone);
        let gate_already_opened =
            self.journey_manager.get_any(track_id).and_then(|j| j.gate_cmd_at).is_some();

        if in_gate_zone && !gate_already_opened {
            self.send_gate_open_command(track_id, ts, "acc", received_at);
        }
    }

    /// Publish an unmatched ACC event with debug info
    fn publish_unmatched_acc_event(&self, ip: &str, pos: Option<&str>, ts: u64) {
        let Some(ref sender) = self.egress_sender else {
            return;
        };

        let debug_active: Vec<AccDebugTrack> = self
            .persons
            .iter()
            .map(|(tid, p)| AccDebugTrack {
                tid: tid.0,
                zone: p.current_zone.map(|z| self.config.zone_name(z).to_string()),
                dwell_ms: self.journey_manager.get_dwell(*tid),
                auth: p.authorized,
            })
            .collect();

        let debug_pending: Vec<AccDebugPending> = self
            .stitcher
            .get_pending_info()
            .into_iter()
            .map(|p| AccDebugPending {
                tid: p.track_id.0,
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
            pos: pos.map(|s| s.to_string()),
            tid: None,
            dwell_ms: None,
            gate_zone: None,
            gate_entry_ts: None,
            delta_ms: None,
            gate_cmd_at: None,
            debug_active: if debug_active.is_empty() { None } else { Some(debug_active) },
            debug_pending: if debug_pending.is_empty() { None } else { Some(debug_pending) },
        });
    }

    /// Enqueue gate open command to worker and record E2E latency
    ///
    /// `received_at` is when the triggering event was received (zone entry or ACC).
    /// This allows us to measure the full E2E latency from event reception to command enqueue.
    ///
    /// This method returns immediately after enqueueing - the actual network I/O
    /// is handled by the GateCmdWorker task asynchronously.
    ///
    /// State/metrics are only updated on successful enqueue to avoid recording
    /// commands that were dropped due to channel full.
    fn send_gate_open_command(
        &mut self,
        track_id: TrackId,
        ts: u64,
        src: &str,
        received_at: Instant,
    ) {
        // Enqueue command to worker - never blocks on network I/O
        let cmd = GateCmd { track_id, enqueued_at: Instant::now() };
        match self.gate_cmd_tx.try_send(cmd) {
            Ok(()) => {
                // Record E2E gate latency only on successful enqueue
                let e2e_latency_us = received_at.elapsed().as_micros() as u64;
                self.metrics.record_gate_latency(e2e_latency_us);
                self.metrics.record_gate_command();

                // Update journey state
                if let Some(journey) = self.journey_manager.get_mut_any(track_id) {
                    journey.gate_cmd_at = Some(ts);
                }
                self.journey_manager.add_event(
                    track_id,
                    JourneyEvent::new(JourneyEventType::GateCmd, ts)
                        .with_extra(&format!("e2e_us={e2e_latency_us}")),
                );

                // Emit state for TUI and record for door correlation
                if let Some(ref sender) = self.egress_sender {
                    sender.send_gate_state(GateStatePayload::new(
                        ts,
                        "cmd_enqueued",
                        Some(track_id.0),
                        src,
                    ));
                }
                self.door_correlator.record_gate_cmd(track_id);
            }
            Err(e) => {
                // Command dropped - gate won't open for this customer
                self.metrics.record_gate_cmd_dropped();
                warn!(track_id = %track_id, error = %e, "gate_cmd_enqueue_failed");

                // Emit dropped state for TUI visibility
                if let Some(ref sender) = self.egress_sender {
                    sender.send_gate_state(GateStatePayload::new(
                        ts,
                        "cmd_dropped",
                        Some(track_id.0),
                        src,
                    ));
                }
            }
        }
    }

    /// Determine the journey outcome when a track is deleted
    ///
    /// Priority:
    /// 1. EXIT line crossed → Completed (definitive exit)
    /// 2. Last zone was EXIT → Completed (they're gone)
    /// 3. Backward entry cross → ReturnedToStore (went back)
    /// 4. Only touched STORE/ENTRY → ReturnedToStore (never went deep)
    /// 5. Else → Lost (stitch candidate)
    fn determine_journey_outcome(&self, track_id: TrackId) -> (JourneyOutcome, bool) {
        let Some(journey) = self.journey_manager.get_any(track_id) else {
            return (JourneyOutcome::Lost, false);
        };

        // 1. Crossed EXIT line = definitely gone
        if journey.events.iter().any(|e| e.t == JourneyEventType::ExitCross) {
            debug!(track_id = %track_id, "journey_completed_exit_cross");
            return (JourneyOutcome::Completed, false);
        }

        // 2. Last zone was EXIT = gone
        if journey.events.iter().rev().find_map(|e| e.z.as_deref()) == Some("EXIT") {
            debug!(track_id = %track_id, "journey_completed_exit_zone");
            return (JourneyOutcome::Completed, false);
        }

        // 3. Backward entry cross = returned to store
        let has_backward_entry = journey.events.iter().any(|e| {
            e.t == JourneyEventType::EntryCross
                && e.extra.as_ref().is_some_and(|x| x.contains("dir=backward"))
        });
        if has_backward_entry {
            debug!(track_id = %track_id, "journey_returned_backward_entry");
            return (JourneyOutcome::ReturnedToStore, false);
        }

        // 4. Only touched shallow zones (STORE/ENTRY) = never went deep into checkout
        let went_deep = journey.events.iter().filter_map(|e| e.z.as_deref()).any(|z| {
            z.starts_with("POS_") || z.starts_with("GATE") || z == "APPROACH" || z == "EXIT"
        });

        if !went_deep {
            debug!(track_id = %track_id, "journey_returned_shallow");
            return (JourneyOutcome::ReturnedToStore, false);
        }

        // 5. Went deep but didn't exit = stitch candidate
        debug!(track_id = %track_id, "journey_lost_stitch_candidate");
        (JourneyOutcome::Lost, false)
    }
}
