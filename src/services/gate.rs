//! Gate control via HTTP or TCP (CloudPlus) commands

use crate::io::cloudplus::{CloudPlusClient, CloudPlusConfig};
use crate::infra::config::{Config, GateMode};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{error, info};

pub struct GateController {
    mode: GateMode,
    // HTTP mode
    url: String,
    username: Option<String>,
    password: Option<String>,
    http_client: Option<reqwest::Client>,
    // TCP mode
    tcp_client: Option<Arc<CloudPlusClient>>,
    #[cfg(test)]
    mock_enabled: bool,
}

impl GateController {
    pub fn new(config: Config) -> Self {
        // Parse credentials from URL if present (e.g., http://user:pass@host/path)
        let (url, username, password) = Self::parse_url_with_auth(config.gate_url());
        let timeout = Duration::from_millis(config.gate_timeout_ms());

        let tcp_client = if *config.gate_mode() == GateMode::Tcp {
            let tcp_config = CloudPlusConfig {
                addr: config.gate_tcp_addr().to_string(),
                ..Default::default()
            };
            Some(Arc::new(CloudPlusClient::new(tcp_config)))
        } else {
            None
        };

        // Create HTTP client once for reuse (connection pooling)
        let http_client = if *config.gate_mode() == GateMode::Http {
            reqwest::Client::builder()
                .timeout(timeout)
                .http1_only()
                .build()
                .ok()
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
            #[cfg(test)]
            mock_enabled: true,
        }
    }

    /// Get the TCP client for running in main
    pub fn tcp_client(&self) -> Option<Arc<CloudPlusClient>> {
        self.tcp_client.clone()
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

    /// Send gate open command
    /// Returns latency in microseconds
    pub async fn send_open_command(&self, track_id: i64) -> u64 {
        let start = Instant::now();

        #[cfg(test)]
        if self.mock_enabled {
            let latency_us = start.elapsed().as_micros() as u64;
            info!(
                track_id = %track_id,
                latency_us = %latency_us,
                mock = true,
                "gate_open_command"
            );
            return latency_us;
        }

        match self.mode {
            GateMode::Tcp => self.send_open_tcp(track_id, start).await,
            GateMode::Http => self.send_open_http(track_id, start).await,
        }
    }

    async fn send_open_tcp(&self, track_id: i64, start: Instant) -> u64 {
        let Some(ref client) = self.tcp_client else {
            error!(track_id = %track_id, "gate_tcp_client_not_initialized");
            return start.elapsed().as_micros() as u64;
        };

        match client.send_open(0).await {
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
                error!(
                    track_id = %track_id,
                    latency_us = %latency_us,
                    error = %e,
                    mode = "tcp",
                    "gate_open_command_error"
                );
                latency_us
            }
        }
    }

    async fn send_open_http(&self, track_id: i64, start: Instant) -> u64 {
        let Some(ref client) = self.http_client else {
            error!(track_id = %track_id, "gate_http_client_not_initialized");
            return start.elapsed().as_micros() as u64;
        };

        let mut request = client
            .get(&self.url)
            .header("Accept", "*/*")
            .header("User-Agent", "curl/7.88.1");

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
                error!(
                    track_id = %track_id,
                    latency_us = %latency_us,
                    error = %e,
                    mode = "http",
                    "gate_open_command_error"
                );
                latency_us
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::config::Config;

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
        let config = Config::default();
        let gate = GateController::new(config);

        let latency_us = gate.send_open_command(100).await;
        // Mock should return very fast
        assert!(latency_us < 10_000); // Less than 10ms
    }

    #[tokio::test]
    async fn test_multiple_gate_commands() {
        let config = Config::default();
        let gate = GateController::new(config);

        for track_id in 1..=5 {
            let latency_us = gate.send_open_command(track_id).await;
            assert!(latency_us < 10_000);
        }
    }
}
