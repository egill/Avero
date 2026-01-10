//! Gate control via HTTP or TCP (CloudPlus) commands

use crate::domain::types::TrackId;
use crate::infra::config::{Config, GateMode};
use crate::infra::metrics::Metrics;
use crate::io::cloudplus::{CloudPlusClient, CloudPlusConfig};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

/// Trait for gate control operations - enables mock implementations for testing
#[async_trait]
pub trait GateCommand: Send + Sync {
    /// Send gate open command, returns latency in microseconds
    async fn send_open_command(&self, track_id: TrackId) -> u64;
}

/// Log TCP client not initialized error (cold path)
#[cold]
fn log_tcp_client_not_initialized(track_id: TrackId) {
    error!(track_id = %track_id, "gate_tcp_client_not_initialized");
}

/// Log TCP command error (cold path)
#[cold]
fn log_tcp_command_error(
    track_id: TrackId,
    latency_us: u64,
    e: &(dyn std::error::Error + Send + Sync),
) {
    error!(
        track_id = %track_id,
        latency_us = %latency_us,
        error = %e,
        mode = "tcp",
        "gate_open_command_error"
    );
}

/// Log HTTP client not initialized error (cold path)
#[cold]
fn log_http_client_not_initialized(track_id: TrackId) {
    error!(track_id = %track_id, "gate_http_client_not_initialized");
}

/// Log HTTP command error (cold path)
#[cold]
fn log_http_command_error(track_id: TrackId, latency_us: u64, e: &reqwest::Error) {
    error!(
        track_id = %track_id,
        latency_us = %latency_us,
        error = %e,
        mode = "http",
        "gate_open_command_error"
    );
}

pub struct GateController {
    mode: GateMode,
    // HTTP mode
    url: String,
    username: Option<String>,
    password: Option<String>,
    http_client: Option<reqwest::Client>,
    // TCP mode
    tcp_client: Option<Arc<CloudPlusClient>>,
    // Metrics for tracking dropped commands
    metrics: Option<Arc<Metrics>>,
}

impl GateController {
    pub fn new(config: Config, metrics: Option<Arc<Metrics>>) -> Self {
        // Parse credentials from URL if present (e.g., http://user:pass@host/path)
        let (url, username, password) = Self::parse_url_with_auth(config.gate_url());
        let timeout = Duration::from_millis(config.gate_timeout_ms());

        let tcp_client = if *config.gate_mode() == GateMode::Tcp {
            let tcp_config =
                CloudPlusConfig { addr: config.gate_tcp_addr().to_string(), ..Default::default() };
            Some(Arc::new(CloudPlusClient::new(tcp_config)))
        } else {
            None
        };

        // Create HTTP client once for reuse (connection pooling)
        let http_client = if *config.gate_mode() == GateMode::Http {
            reqwest::Client::builder().timeout(timeout).http1_only().build().ok()
        } else {
            None
        };

        Self {
            mode: config.gate_mode().clone(),
            url,
            username,
            password,
            http_client,
            tcp_client,
            metrics,
        }
    }

    /// Get the TCP client for running in main
    pub fn tcp_client(&self) -> Option<Arc<CloudPlusClient>> {
        self.tcp_client.clone()
    }

    /// Get the CloudPlus outbound queue depth (for metrics)
    pub fn cloudplus_queue_depth(&self) -> usize {
        self.tcp_client.as_ref().map(|c| c.outbound_queue_depth()).unwrap_or(0)
    }

    /// Parse URL and extract basic auth credentials if present
    fn parse_url_with_auth(url: &str) -> (String, Option<String>, Option<String>) {
        // Try to parse http://user:pass@host/path format
        if let Some(rest) = url.strip_prefix("http://") {
            if let Some(at_pos) = rest.find('@') {
                let auth_part = &rest[..at_pos];
                let host_part = &rest[at_pos + 1..];

                if let Some(colon_pos) = auth_part.find(':') {
                    let username = auth_part[..colon_pos].to_string();
                    let password = auth_part[colon_pos + 1..].to_string();
                    let clean_url = format!("http://{}", host_part);
                    return (clean_url, Some(username), Some(password));
                }
            }
        }
        (url.to_string(), None, None)
    }

    fn send_open_tcp(&self, track_id: TrackId, start: Instant) -> u64 {
        let Some(ref client) = self.tcp_client else {
            log_tcp_client_not_initialized(track_id);
            return start.elapsed().as_micros() as u64;
        };

        match client.send_open(0) {
            Ok(queue_latency_us) => {
                let total_latency_us = start.elapsed().as_micros() as u64;
                info!(
                    track_id = %track_id,
                    latency_us = %total_latency_us,
                    queue_latency_us = %queue_latency_us,
                    mode = "tcp",
                    "gate_open_command"
                );
                total_latency_us
            }
            Err(e) => {
                let latency_us = start.elapsed().as_micros() as u64;
                // Check if this is a queue full error
                let err_str = e.to_string();
                if err_str.contains("queue full") {
                    // Gate command dropped - customer will have to wait
                    warn!(
                        track_id = %track_id,
                        latency_us = %latency_us,
                        "gate_command_dropped_queue_full"
                    );
                    if let Some(ref metrics) = self.metrics {
                        metrics.record_gate_cmd_dropped();
                    }
                } else {
                    log_tcp_command_error(track_id, latency_us, e.as_ref());
                }
                latency_us
            }
        }
    }

    async fn send_open_http(&self, track_id: TrackId, start: Instant) -> u64 {
        let Some(ref client) = self.http_client else {
            log_http_client_not_initialized(track_id);
            return start.elapsed().as_micros() as u64;
        };

        let mut request =
            client.get(&self.url).header("Accept", "*/*").header("User-Agent", "curl/7.88.1");

        if let (Some(username), Some(password)) = (&self.username, &self.password) {
            let credentials = format!("{}:{}", username, password);
            let encoded = STANDARD.encode(credentials.as_bytes());
            let auth_header = format!("Basic {}", encoded);
            request = request.header("Authorization", auth_header);
        }

        match request.send().await {
            Ok(response) => {
                let latency_us = start.elapsed().as_micros() as u64;
                let status = response.status();

                info!(
                    track_id = %track_id,
                    latency_us = %latency_us,
                    status = %status.as_u16(),
                    mode = "http",
                    "gate_open_command"
                );

                latency_us
            }
            Err(e) => {
                let latency_us = start.elapsed().as_micros() as u64;
                log_http_command_error(track_id, latency_us, &e);
                latency_us
            }
        }
    }
}

#[async_trait]
impl GateCommand for GateController {
    async fn send_open_command(&self, track_id: TrackId) -> u64 {
        let start = Instant::now();
        match self.mode {
            GateMode::Tcp => self.send_open_tcp(track_id, start),
            GateMode::Http => self.send_open_http(track_id, start).await,
        }
    }
}

/// Mock gate controller for testing
#[cfg(test)]
pub struct MockGateController;

#[cfg(test)]
#[async_trait]
impl GateCommand for MockGateController {
    async fn send_open_command(&self, track_id: TrackId) -> u64 {
        info!(
            track_id = %track_id,
            latency_us = 0,
            mock = true,
            "gate_open_command"
        );
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::TrackId;

    #[test]
    fn test_parse_url_with_auth() {
        let (url, user, pass) = GateController::parse_url_with_auth(
            "http://admin:88888888@192.168.0.245/cdor.cgi?door=0&open=1",
        );
        assert_eq!(url, "http://192.168.0.245/cdor.cgi?door=0&open=1");
        assert_eq!(user, Some("admin".to_string()));
        assert_eq!(pass, Some("88888888".to_string()));
    }

    #[test]
    fn test_parse_url_without_auth() {
        let (url, user, pass) =
            GateController::parse_url_with_auth("http://192.168.0.245/cdor.cgi?door=0&open=1");
        assert_eq!(url, "http://192.168.0.245/cdor.cgi?door=0&open=1");
        assert_eq!(user, None);
        assert_eq!(pass, None);
    }

    #[tokio::test]
    async fn test_mock_gate_command() {
        let gate = MockGateController;

        let latency_us = gate.send_open_command(TrackId(100)).await;
        // Mock should return very fast
        assert!(latency_us < 10_000); // Less than 10ms
    }

    #[tokio::test]
    async fn test_multiple_gate_commands() {
        let gate = MockGateController;

        for track_id in 1..=5 {
            let latency_us = gate.send_open_command(TrackId(track_id)).await;
            assert!(latency_us < 10_000);
        }
    }
}
