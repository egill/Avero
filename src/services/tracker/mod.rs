//! Person state tracking and event orchestration
//!
//! The Tracker is the central event processor that coordinates:
//! - Person state management (tracking individuals in the store)
//! - Journey lifecycle (creation, stitching, completion, egress)
//! - Gate control (sending open commands when authorized)
//! - Door correlation (matching gate commands to door opens)

mod handlers;
#[cfg(test)]
mod tests;

use crate::domain::journey::Journey;
use crate::domain::types::{DoorStatus, EventType, ParsedEvent, Person, TrackId};
use crate::infra::config::Config;
use crate::infra::metrics::Metrics;
use crate::io::EgressSender;
use crate::services::acc_collector::AccCollector;
use crate::services::door_correlator::DoorCorrelator;
use crate::services::gate_worker::GateCmd;
use crate::services::journey_manager::JourneyManager;
use crate::services::reentry_detector::ReentryDetector;
use crate::services::stitcher::Stitcher;
use rustc_hash::FxHashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, watch};
use tokio::time::{interval, Duration};
use tracing::warn;

/// Central event processor for person tracking and journey management
pub struct Tracker {
    /// Active persons by track_id
    pub(crate) persons: FxHashMap<TrackId, Person>,
    /// Handles track identity stitching across sensor gaps
    pub(crate) stitcher: Stitcher,
    /// Manages journey lifecycle and persistence
    pub(crate) journey_manager: JourneyManager,
    /// Correlates gate commands with door state changes
    pub(crate) door_correlator: DoorCorrelator,
    /// Detects re-entry patterns
    pub(crate) reentry_detector: ReentryDetector,
    /// Correlates ACC (payment) events with journeys
    pub(crate) acc_collector: AccCollector,
    /// Application configuration
    pub(crate) config: Config,
    /// Gate command sender (commands processed by GateCmdWorker)
    pub(crate) gate_cmd_tx: mpsc::Sender<GateCmd>,
    /// Journey egress sender (journeys processed by EgressWriter)
    pub(crate) journey_tx: mpsc::Sender<Journey>,
    /// Metrics collector
    pub(crate) metrics: Arc<Metrics>,
    /// MQTT egress sender (optional)
    pub(crate) egress_sender: Option<EgressSender>,
    /// Watch receiver for door state (RS485 monitor publishes here)
    pub(crate) door_rx: watch::Receiver<DoorStatus>,
    /// Last processed door status (to detect changes)
    pub(crate) last_door_status: DoorStatus,
}

impl Tracker {
    /// Create a new Tracker with the given configuration and dependencies
    ///
    /// The `gate_cmd_tx` channel sends gate commands to a `GateCmdWorker` task,
    /// which handles network I/O asynchronously without blocking the tracker.
    ///
    /// The `journey_tx` channel sends completed journeys to an `EgressWriter` task,
    /// which handles file I/O asynchronously without blocking the tracker.
    ///
    /// The `door_rx` watch receiver provides lossless door state updates from RS485.
    pub fn new(
        config: Config,
        gate_cmd_tx: mpsc::Sender<GateCmd>,
        journey_tx: mpsc::Sender<Journey>,
        metrics: Arc<Metrics>,
        egress_sender: Option<EgressSender>,
        door_rx: watch::Receiver<DoorStatus>,
    ) -> Self {
        let acc_collector = AccCollector::new(&config, metrics.clone());
        Self {
            persons: FxHashMap::default(),
            stitcher: Stitcher::with_metrics(metrics.clone()),
            journey_manager: JourneyManager::new(),
            door_correlator: DoorCorrelator::new(),
            reentry_detector: ReentryDetector::new(),
            acc_collector,
            config,
            gate_cmd_tx,
            journey_tx,
            metrics,
            egress_sender,
            door_rx,
            last_door_status: DoorStatus::Unknown,
        }
    }

    /// Start the tracker, consuming events from the channel
    pub async fn run(&mut self, mut event_rx: mpsc::Receiver<ParsedEvent>) {
        // Tick interval for journey egress (1 second as per requirements)
        let mut tick_interval = interval(Duration::from_secs(1));

        loop {
            tokio::select! {
                // Process incoming events (MQTT, ACC)
                event = event_rx.recv() => {
                    match event {
                        Some(e) => self.process_event(e),
                        None => break, // Channel closed
                    }
                }
                // Watch for door state changes (RS485 via watch channel)
                result = self.door_rx.changed() => {
                    if result.is_ok() {
                        let status = *self.door_rx.borrow();
                        if status != self.last_door_status {
                            self.handle_door_state_change(status);
                            self.last_door_status = status;
                        }
                    }
                }
                // Periodic tick for journey egress
                _ = tick_interval.tick() => {
                    self.tick_and_egress();
                }
            }
        }
    }

    /// Tick journey manager and send ready journeys to egress worker
    fn tick_and_egress(&mut self) {
        let ready_journeys = self.journey_manager.tick();
        for journey in ready_journeys {
            // Publish to MQTT (if enabled)
            if let Some(ref sender) = self.egress_sender {
                sender.send_journey(&journey);
            }

            // Send to egress writer via channel (non-blocking)
            self.metrics.record_journey_egress_received();
            if let Err(e) = self.journey_tx.try_send(journey) {
                self.metrics.record_journey_egress_dropped();
                warn!(error = %e, "journey_egress_queue_full");
            }
        }
    }

    /// Process a single event, dispatching to the appropriate handler
    ///
    /// All handlers are synchronous - gate commands are enqueued to a worker task.
    pub fn process_event(&mut self, event: ParsedEvent) {
        let process_start = Instant::now();

        match event.event_type {
            EventType::TrackCreate => self.handle_track_create(&event),
            EventType::TrackDelete => self.handle_track_delete(&event),
            EventType::ZoneEntry => self.handle_zone_entry(&event),
            EventType::ZoneExit => self.handle_zone_exit(&event),
            EventType::LineCrossForward => self.handle_line_cross(&event, "forward"),
            EventType::LineCrossBackward => self.handle_line_cross(&event, "backward"),
            EventType::AccEvent(ip) => self.handle_acc_event(&ip, event.received_at),
            // Door state comes via watch channel, not event channel
            EventType::DoorStateChange(_) | EventType::Unknown(_) => {}
        }

        let latency_us = process_start.elapsed().as_micros() as u64;
        self.metrics.record_event_processed(latency_us);

        // Update track counts for Prometheus/MQTT metrics (non-blocking atomic stores)
        self.metrics.set_active_tracks(self.active_tracks());
        self.metrics.set_authorized_tracks(self.authorized_tracks());
    }

    /// Get current active track count
    #[allow(dead_code)]
    pub fn active_tracks(&self) -> usize {
        self.persons.len()
    }

    /// Get count of authorized tracks
    #[allow(dead_code)]
    pub fn authorized_tracks(&self) -> usize {
        self.persons.values().filter(|p| p.authorized).count()
    }

    /// Tick the journey manager and return journeys ready for egress
    #[allow(dead_code)]
    pub fn tick_journeys(&mut self) -> Vec<Journey> {
        self.journey_manager.tick()
    }
}
