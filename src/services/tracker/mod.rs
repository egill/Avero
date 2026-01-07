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

use crate::infra::config::Config;
use crate::services::acc_collector::AccCollector;
use crate::services::door_correlator::DoorCorrelator;
use crate::io::egress::Egress;
use crate::io::EgressSender;
use crate::services::gate::GateController;
use crate::domain::journey::Journey;
use crate::services::journey_manager::JourneyManager;
use crate::infra::metrics::Metrics;
use crate::services::reentry_detector::ReentryDetector;
use crate::services::stitcher::Stitcher;
use crate::domain::types::{EventType, ParsedEvent, Person};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};

/// Central event processor for person tracking and journey management
pub struct Tracker {
    /// Active persons by track_id
    pub(crate) persons: HashMap<i64, Person>,
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
    /// Writes completed journeys to file
    pub(crate) egress: Egress,
    /// Application configuration
    pub(crate) config: Config,
    /// Gate control interface
    pub(crate) gate: Arc<GateController>,
    /// Metrics collector
    pub(crate) metrics: Arc<Metrics>,
    /// MQTT egress sender (optional)
    pub(crate) egress_sender: Option<EgressSender>,
}

impl Tracker {
    /// Create a new Tracker with the given configuration and dependencies
    pub fn new(
        config: Config,
        gate: Arc<GateController>,
        metrics: Arc<Metrics>,
        egress_sender: Option<EgressSender>,
    ) -> Self {
        let egress = Egress::new(config.egress_file());
        let acc_collector = AccCollector::new(&config);
        Self {
            persons: HashMap::new(),
            stitcher: Stitcher::with_metrics(metrics.clone()),
            journey_manager: JourneyManager::new(),
            door_correlator: DoorCorrelator::new(),
            reentry_detector: ReentryDetector::new(),
            acc_collector,
            egress,
            config,
            gate,
            metrics,
            egress_sender,
        }
    }

    /// Start the tracker, consuming events from the channel
    pub async fn run(&mut self, mut event_rx: mpsc::Receiver<ParsedEvent>) {
        // Tick interval for journey egress (1 second as per requirements)
        let mut tick_interval = interval(Duration::from_secs(1));

        loop {
            tokio::select! {
                // Process incoming events
                event = event_rx.recv() => {
                    match event {
                        Some(e) => self.process_event(e).await,
                        None => break, // Channel closed
                    }
                }
                // Periodic tick for journey egress
                _ = tick_interval.tick() => {
                    self.tick_and_egress();
                }
            }
        }
    }

    /// Tick journey manager and write ready journeys to egress
    fn tick_and_egress(&mut self) {
        let ready_journeys = self.journey_manager.tick();
        if !ready_journeys.is_empty() {
            // Write to file
            self.egress.write_journeys(&ready_journeys);

            // Publish to MQTT
            if let Some(ref sender) = self.egress_sender {
                for journey in &ready_journeys {
                    sender.send_journey(journey);
                }
            }
        }
    }

    /// Process a single event, dispatching to the appropriate handler
    pub async fn process_event(&mut self, event: ParsedEvent) {
        let process_start = Instant::now();

        match event.event_type {
            EventType::TrackCreate => {
                self.handle_track_create(&event);
            }
            EventType::TrackDelete => {
                self.handle_track_delete(&event);
            }
            EventType::ZoneEntry => {
                self.handle_zone_entry(&event).await;
            }
            EventType::ZoneExit => {
                self.handle_zone_exit(&event);
            }
            EventType::LineCrossForward => {
                self.handle_line_cross(&event, "forward");
            }
            EventType::LineCrossBackward => {
                self.handle_line_cross(&event, "backward");
            }
            EventType::DoorStateChange(status) => {
                self.handle_door_state_change(status);
            }
            EventType::AccEvent(ip) => {
                self.handle_acc_event(&ip, event.received_at).await;
            }
            EventType::Unknown(_) => {}
        }

        // Record processing latency (lock-free)
        let latency_us = process_start.elapsed().as_micros() as u64;
        self.metrics.record_event_processed(latency_us);
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
