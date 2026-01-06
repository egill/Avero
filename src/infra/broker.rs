//! Embedded MQTT broker using rumqttd

use crate::infra::config::Config as AppConfig;
use rumqttd::{Broker, Config, ConnectionSettings, RouterConfig, ServerSettings};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::thread;
use tracing::{info, warn};

/// Start the embedded MQTT broker with configuration
pub fn start_embedded_broker(app_config: &AppConfig) {
    let bind_address = app_config.broker_bind_address().to_string();
    let port = app_config.broker_port();

    let router_config = RouterConfig {
        max_segment_size: 104857600,
        max_segment_count: 10,
        max_connections: 10010,
        max_outgoing_packet_count: 200,
        initialized_filters: None,
        ..Default::default()
    };

    // Parse bind address
    let addr_str = format!("{}:{}", bind_address, port);
    let listen_addr: SocketAddr = match addr_str.parse() {
        Ok(addr) => addr,
        Err(e) => {
            warn!(error = %e, addr = %addr_str, "broker_invalid_bind_address");
            return;
        }
    };

    let mut servers = HashMap::new();
    servers.insert(
        "v4".to_string(),
        ServerSettings {
            name: "v4".to_string(),
            listen: listen_addr,
            tls: None,
            next_connection_delay_ms: 1,
            connections: ConnectionSettings {
                connection_timeout_ms: 5000,
                max_payload_size: 262144,
                max_inflight_count: 200,
                auth: None,
                dynamic_filters: false,
                external_auth: None,
            },
        },
    );

    let config = Config {
        id: 0,
        router: router_config,
        v4: Some(servers),
        v5: None,
        ws: None,
        prometheus: None,
        metrics: None,
        bridge: None,
        console: None,
        cluster: None,
    };

    thread::spawn(move || {
        let mut broker = Broker::new(config);
        match broker.start() {
            Ok(_) => {
                // Note: start() blocks, so this won't be reached
            }
            Err(e) => {
                warn!(error = %e, "broker_start_failed");
            }
        }
    });

    // Give broker time to start
    thread::sleep(std::time::Duration::from_millis(100));
    info!(bind_address = %bind_address, port = %port, "broker_started");
}
