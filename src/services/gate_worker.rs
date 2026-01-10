//! Gate command worker - processes gate commands off the hot path
//!
//! This worker decouples gate commands from the tracker loop to prevent
//! network I/O from blocking event processing. The tracker enqueues commands
//! via an mpsc channel, and the worker handles actual network operations.

use crate::domain::types::TrackId;
use crate::infra::metrics::Metrics;
use crate::services::gate::{GateCommand, GateController};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{info, warn};

/// A gate command to be processed by the worker
#[derive(Debug)]
pub struct GateCmd {
    /// Track ID that triggered the gate open
    pub track_id: TrackId,
    /// When the command was enqueued (for queue delay measurement)
    pub enqueued_at: Instant,
}

/// Worker that processes gate commands asynchronously
pub struct GateCmdWorker {
    /// Gate controller for sending commands
    gate: Arc<GateController>,
    /// Receiver for gate commands
    cmd_rx: mpsc::Receiver<GateCmd>,
    /// Metrics for recording queue delay and queue depth
    metrics: Arc<Metrics>,
}

impl GateCmdWorker {
    /// Create a new gate command worker
    pub fn new(
        gate: Arc<GateController>,
        cmd_rx: mpsc::Receiver<GateCmd>,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self { gate, cmd_rx, metrics }
    }

    /// Run the worker, processing commands until the channel closes
    pub async fn run(mut self) {
        info!("gate_cmd_worker_started");

        while let Some(cmd) = self.cmd_rx.recv().await {
            // Measure queue delay (time from enqueue to processing start)
            let queue_delay_us = cmd.enqueued_at.elapsed().as_micros() as u64;

            // Send the actual gate command (this may block on network)
            let send_start = Instant::now();
            let send_latency_us = self.gate.send_open_command(cmd.track_id).await;
            let total_send_us = send_start.elapsed().as_micros() as u64;

            // Log with timing breakdown
            info!(
                track_id = %cmd.track_id,
                queue_delay_us = %queue_delay_us,
                send_latency_us = %send_latency_us,
                total_send_us = %total_send_us,
                "gate_cmd_processed"
            );

            // Record queue delay to metrics histogram
            self.metrics.record_gate_queue_delay(queue_delay_us);

            // Warn if queue delay exceeds 1ms - indicates backlog
            if queue_delay_us > 1000 {
                warn!(
                    track_id = %cmd.track_id,
                    queue_delay_us = %queue_delay_us,
                    "gate_cmd_queue_delay_high"
                );
            }
        }

        info!("gate_cmd_worker_stopped");
    }
}

/// Create a gate command channel and worker
///
/// Returns the sender (for tracker) and the worker (to be spawned)
pub fn create_gate_worker(
    gate: Arc<GateController>,
    metrics: Arc<Metrics>,
    buffer_size: usize,
) -> (mpsc::Sender<GateCmd>, GateCmdWorker) {
    let (cmd_tx, cmd_rx) = mpsc::channel(buffer_size);
    let worker = GateCmdWorker::new(gate, cmd_rx, metrics);
    (cmd_tx, worker)
}
