//! MQTT publisher for egress events
//!
//! Publishes gateway events to MQTT topics for downstream consumers:
//! - gateway/journeys - Completed journey JSONs (QoS 1)
//! - gateway/events - Live zone events (QoS 0)
//! - gateway/metrics - Periodic metrics snapshots (QoS 0)
//! - gateway/gate - Gate state changes (QoS 0)
//! - gateway/tracks - Track lifecycle events (QoS 0)

use crate::infra::config::Config;
use crate::io::egress_channel::EgressMessage;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, warn};

/// MQTT publisher actor
///
/// Receives messages from the egress channel and publishes to MQTT topics.
pub struct MqttPublisher {
    client: AsyncClient,
    rx: mpsc::Receiver<EgressMessage>,
    journeys_topic: String,
    events_topic: String,
    metrics_topic: String,
    gate_topic: String,
    tracks_topic: String,
    acc_topic: String,
}

impl MqttPublisher {
    /// Create a new MQTT publisher
    ///
    /// Connects to the broker at the configured MQTT host/port.
    pub fn new(config: &Config, rx: mpsc::Receiver<EgressMessage>) -> Self {
        let client_id = format!("gateway-egress-{}", std::process::id());
        let mut mqttoptions = MqttOptions::new(client_id, config.mqtt_host(), config.mqtt_port());
        mqttoptions.set_keep_alive(Duration::from_secs(30));
        mqttoptions.set_clean_session(true);

        // Set credentials if configured
        if let (Some(username), Some(password)) = (config.mqtt_username(), config.mqtt_password()) {
            mqttoptions.set_credentials(username, password);
        }

        let (client, eventloop) = AsyncClient::new(mqttoptions, 100);

        // Spawn the eventloop handler
        tokio::spawn(async move {
            let mut eventloop = eventloop;
            loop {
                match eventloop.poll().await {
                    Ok(Event::Incoming(Packet::ConnAck(_))) => {
                        info!("mqtt_egress_connected");
                    }
                    Ok(Event::Incoming(Packet::PubAck(_))) => {
                        // QoS 1 acknowledgement received
                        debug!("mqtt_egress_puback");
                    }
                    Ok(_) => {}
                    Err(e) => {
                        warn!(error = %e, "mqtt_egress_error");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        });

        Self {
            client,
            rx,
            journeys_topic: config.mqtt_egress_journeys_topic().to_string(),
            events_topic: config.mqtt_egress_events_topic().to_string(),
            metrics_topic: config.mqtt_egress_metrics_topic().to_string(),
            gate_topic: config.mqtt_egress_gate_topic().to_string(),
            tracks_topic: config.mqtt_egress_tracks_topic().to_string(),
            acc_topic: config.mqtt_egress_acc_topic().to_string(),
        }
    }

    /// Run the publisher loop
    ///
    /// Processes messages from the channel and publishes to MQTT.
    /// Runs until shutdown signal is received.
    pub async fn run(mut self, mut shutdown: watch::Receiver<bool>) {
        info!(
            journeys = %self.journeys_topic,
            events = %self.events_topic,
            metrics = %self.metrics_topic,
            gate = %self.gate_topic,
            acc = %self.acc_topic,
            "mqtt_egress_started"
        );

        loop {
            tokio::select! {
                // Check for shutdown
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!("mqtt_egress_shutdown");
                        // Drain remaining messages
                        while let Ok(msg) = self.rx.try_recv() {
                            self.publish_message(msg).await;
                        }
                        return;
                    }
                }
                // Process messages
                Some(msg) = self.rx.recv() => {
                    self.publish_message(msg).await;
                }
            }
        }
    }

    async fn publish_message(&self, msg: EgressMessage) {
        match msg {
            EgressMessage::Journey(payload) => {
                // Use QoS 1 for journeys (at-least-once delivery)
                if let Err(e) = self
                    .client
                    .publish(&self.journeys_topic, QoS::AtLeastOnce, false, payload.json.as_bytes())
                    .await
                {
                    error!(error = %e, "mqtt_egress_journey_failed");
                }
            }
            EgressMessage::ZoneEvent(payload) => {
                // Use QoS 0 for live events (fire-and-forget)
                if let Ok(json) = serde_json::to_string(&payload) {
                    if let Err(e) = self
                        .client
                        .publish(&self.events_topic, QoS::AtMostOnce, false, json.as_bytes())
                        .await
                    {
                        debug!(error = %e, "mqtt_egress_event_failed");
                    }
                }
            }
            EgressMessage::Metrics(payload) => {
                // Use QoS 0 for metrics
                if let Ok(json) = serde_json::to_string(&payload) {
                    if let Err(e) = self
                        .client
                        .publish(&self.metrics_topic, QoS::AtMostOnce, false, json.as_bytes())
                        .await
                    {
                        debug!(error = %e, "mqtt_egress_metrics_failed");
                    }
                }
            }
            EgressMessage::GateState(payload) => {
                // Use QoS 0 for gate state
                if let Ok(json) = serde_json::to_string(&payload) {
                    if let Err(e) = self
                        .client
                        .publish(&self.gate_topic, QoS::AtMostOnce, false, json.as_bytes())
                        .await
                    {
                        debug!(error = %e, "mqtt_egress_gate_failed");
                    }
                }
            }
            EgressMessage::TrackEvent(payload) => {
                // Use QoS 0 for track events
                if let Ok(json) = serde_json::to_string(&payload) {
                    if let Err(e) = self
                        .client
                        .publish(&self.tracks_topic, QoS::AtMostOnce, false, json.as_bytes())
                        .await
                    {
                        debug!(error = %e, "mqtt_egress_track_failed");
                    }
                }
            }
            EgressMessage::AccEvent(payload) => {
                // Use QoS 0 for ACC events
                if let Ok(json) = serde_json::to_string(&payload) {
                    if let Err(e) = self
                        .client
                        .publish(&self.acc_topic, QoS::AtMostOnce, false, json.as_bytes())
                        .await
                    {
                        debug!(error = %e, "mqtt_egress_acc_failed");
                    }
                }
            }
        }
    }
}
