//! ACC TCP listener for payment terminal events
//!
//! Listens on port 25803 for connections from ACC terminals.
//! Protocol: "ACC <receipt_id>\n"
//! The peer IP is used to look up the POS zone via ip_to_pos config.

use crate::domain::types::{EventType, ParsedEvent, TrackId};
use crate::infra::metrics::Metrics;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, warn};

/// ACC listener configuration
#[derive(Debug, Clone)]
pub struct AccListenerConfig {
    pub port: u16,
    pub enabled: bool,
}

impl Default for AccListenerConfig {
    fn default() -> Self {
        Self { port: 25803, enabled: true }
    }
}

/// Start the ACC TCP listener
///
/// Listens for connections from ACC terminals and sends events to the tracker.
/// Events are sent via try_send to avoid blocking - drops are counted in metrics.
pub async fn start_acc_listener(
    config: AccListenerConfig,
    event_tx: mpsc::Sender<ParsedEvent>,
    metrics: Arc<Metrics>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !config.enabled {
        info!("acc_listener_disabled");
        return Ok(());
    }

    let addr = format!("0.0.0.0:{}", config.port);
    let listener = TcpListener::bind(&addr).await?;

    info!(port = %config.port, "acc_listener_started");

    loop {
        tokio::select! {
            // Check for shutdown
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    info!("acc_listener_shutdown");
                    return Ok(());
                }
            }
            // Accept new connections
            result = listener.accept() => {
                match result {
                    Ok((socket, addr)) => {
                        let tx = event_tx.clone();
                        let m = metrics.clone();
                        tokio::spawn(async move {
                            handle_acc_connection(socket, addr, tx, m).await;
                        });
                    }
                    Err(e) => {
                        error!(error = %e, "acc_listener_accept_failed");
                    }
                }
            }
        }
    }
}

async fn handle_acc_connection(
    socket: tokio::net::TcpStream,
    addr: SocketAddr,
    event_tx: mpsc::Sender<ParsedEvent>,
    metrics: Arc<Metrics>,
) {
    let peer_ip = addr.ip().to_string();
    debug!(ip = %peer_ip, "acc_connection_accepted");

    let reader = BufReader::new(socket);
    let mut lines = reader.lines();

    // Rate-limit drop warnings to 1 per second
    let mut last_drop_warn = Instant::now() - Duration::from_secs(2);

    // Read lines from the connection
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();

        // Parse "ACC <receipt_id>" format
        if let Some(receipt_id) = line.strip_prefix("ACC ") {
            let receipt_id = receipt_id.trim();

            if receipt_id.is_empty() {
                warn!(line = %line, "acc_missing_receipt_id");
                continue;
            }

            info!(
                receipt_id = %receipt_id,
                peer_ip = %peer_ip,
                "acc_event_received"
            );

            // Create ParsedEvent with AccEvent type
            // The peer IP is used to look up the POS zone via ip_to_pos config
            let event = ParsedEvent {
                event_type: EventType::AccEvent(peer_ip.clone()),
                track_id: TrackId(0), // Not used for ACC events
                geometry_id: None,
                direction: None,
                event_time: 0,
                received_at: Instant::now(),
                position: None,
            };

            // Use try_send to never block the connection handler
            metrics.record_acc_event_received();
            match event_tx.try_send(event) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    metrics.record_acc_event_dropped();
                    // Rate-limit warning to 1 per second
                    if last_drop_warn.elapsed() > Duration::from_secs(1) {
                        warn!(peer_ip = %peer_ip, "acc_event_dropped: channel full");
                        last_drop_warn = Instant::now();
                    }
                }
                Err(TrySendError::Closed(_)) => {
                    warn!(peer_ip = %peer_ip, "acc_event_channel_closed");
                    break;
                }
            }
        } else if !line.is_empty() {
            debug!(peer_ip = %peer_ip, line = %line, "acc_unknown_message");
        }
    }

    debug!(peer_ip = %peer_ip, "acc_connection_closed");
}
