//! Gateway Analysis - Diagnostic logging tool for gateway-poc
//!
//! Captures MQTT, ACC, and RS485 traffic to JSONL files for offline analysis.
//! Designed to run alongside or instead of gateway-poc during diagnostic sessions.
//!
//! Usage:
//!   gateway-analysis --config config/dev.toml
//!   gateway-analysis --log-dir /var/log/gateway-analysis --site-id netto
//!
//! Log files are written to:
//!   <log-dir>/mqtt/<topic>-YYYYMMDD.jsonl
//!   <log-dir>/acc/acc-YYYYMMDD.jsonl
//!   <log-dir>/rs485/rs485-YYYYMMDD.jsonl
//!   <log-dir>/summary/summary-YYYYMMDD.jsonl

use clap::Parser;
use parking_lot::Mutex;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, warn};
use tracing_subscriber::fmt::time::UtcTime;
use tracing_subscriber::EnvFilter;

use gateway_poc::infra::Config;
use gateway_poc::io::analysis_logger::AnalysisLogger;

/// Gateway Analysis - Diagnostic logging for gateway-poc
#[derive(Parser, Debug)]
#[command(name = "gateway-analysis", version, about, long_about = None)]
struct Args {
    /// Path to TOML configuration file
    ///
    /// Uses the same format as gateway-poc config. Only mqtt, acc, and rs485
    /// sections are used for connection settings.
    #[arg(short, long, default_value = "config/dev.toml")]
    config: String,

    /// Directory for JSONL log output
    ///
    /// Subdirectories are created automatically:
    /// - mqtt/  - MQTT messages per topic
    /// - acc/   - ACC payment terminal events
    /// - rs485/ - Door state changes
    /// - summary/ - Periodic statistics
    #[arg(short = 'd', long, default_value = "logs")]
    log_dir: String,

    /// MQTT topics to subscribe to (comma-separated)
    ///
    /// Use # for all topics under a prefix, e.g., "gateway/#"
    #[arg(short, long, default_value = "gateway/#")]
    topics: String,

    /// Log rotation strategy: "daily" or "size:<MB>"
    ///
    /// - daily: Rotate at midnight UTC (default)
    /// - size:100: Rotate when file exceeds 100 MB
    #[arg(short, long, default_value = "daily")]
    rotation: String,

    /// Site identifier for log records
    ///
    /// Included in each JSONL record for filtering in multi-site analysis.
    /// Defaults to the site.id from config file.
    #[arg(short, long)]
    site_id: Option<String>,
}

/// Analysis-specific configuration derived from CLI args and config file
#[derive(Debug, Clone)]
struct AnalysisConfig {
    /// Base gateway config (MQTT, ACC, RS485 settings)
    gateway: Config,
    /// Directory for log output
    log_dir: String,
    /// MQTT topics to subscribe to
    topics: Vec<String>,
    /// Rotation strategy (for future use)
    #[allow(dead_code)]
    rotation: RotationStrategy,
    /// Site identifier
    site_id: String,
}

/// Log file rotation strategy (for future use)
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
enum RotationStrategy {
    /// Rotate at midnight UTC
    Daily,
    /// Rotate when file exceeds size in bytes
    Size(u64),
}

impl RotationStrategy {
    fn parse(s: &str) -> Self {
        if s == "daily" {
            return Self::Daily;
        }

        // Try parsing "size:<MB>" format
        if let Some(mb) = s.strip_prefix("size:").and_then(|s| s.parse::<u64>().ok()) {
            return Self::Size(mb * 1024 * 1024);
        }

        warn!(rotation = %s, "invalid_rotation_strategy_defaulting_to_daily");
        Self::Daily
    }
}

impl AnalysisConfig {
    /// Create analysis config from CLI args
    fn from_args(args: Args) -> Self {
        let gateway = Config::load_from_path(&args.config);
        let site_id = args.site_id.unwrap_or_else(|| gateway.site_id().to_string());
        let topics = args.topics.split(',').map(|s| s.trim().to_string()).collect();
        let rotation = RotationStrategy::parse(&args.rotation);

        Self { gateway, log_dir: args.log_dir, topics, rotation, site_id }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_timer(UtcTime::rfc_3339())
        .with_target(false)
        .init();

    info!(version = env!("CARGO_PKG_VERSION"), "gateway_analysis_starting");

    // Parse CLI arguments
    let args = Args::parse();

    // Build analysis config
    let config = AnalysisConfig::from_args(args);

    info!(
        config_file = %config.gateway.config_file(),
        log_dir = %config.log_dir,
        topics = ?config.topics,
        rotation = ?config.rotation,
        site_id = %config.site_id,
        mqtt_host = %config.gateway.mqtt_host(),
        mqtt_port = %config.gateway.mqtt_port(),
        acc_port = %config.gateway.acc_listener_port(),
        rs485_device = %config.gateway.rs485_device(),
        "analysis_config_loaded"
    );

    // Create shared analysis logger
    let logger = Arc::new(Mutex::new(AnalysisLogger::new(&config.log_dir, &config.site_id)));

    // Start MQTT client task
    let mqtt_handle = tokio::spawn(run_mqtt_logger(config.clone(), Arc::clone(&logger)));

    info!(topics = ?config.topics, "mqtt_logging_started");

    // Wait for shutdown signal
    tokio::signal::ctrl_c().await?;
    info!("shutdown_signal_received");

    // Abort MQTT task
    mqtt_handle.abort();

    // Flush remaining logs
    logger.lock().flush_all();

    info!("gateway_analysis_shutdown");

    Ok(())
}

/// Run the MQTT logger - subscribes to topics and logs all messages
async fn run_mqtt_logger(config: AnalysisConfig, logger: Arc<Mutex<AnalysisLogger>>) {
    loop {
        if let Err(e) = mqtt_logger_loop(&config, &logger).await {
            error!(error = %e, "mqtt_logger_error");
        }
        warn!("mqtt_reconnecting");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// MQTT logger event loop
async fn mqtt_logger_loop(
    config: &AnalysisConfig,
    logger: &Arc<Mutex<AnalysisLogger>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut mqttoptions = MqttOptions::new(
        "gateway-analysis",
        config.gateway.mqtt_host(),
        config.gateway.mqtt_port(),
    );
    mqttoptions.set_keep_alive(Duration::from_secs(30));

    // Set credentials if configured
    if let (Some(username), Some(password)) =
        (config.gateway.mqtt_username(), config.gateway.mqtt_password())
    {
        mqttoptions.set_credentials(username, password);
    }

    let (client, mut eventloop) = AsyncClient::new(mqttoptions, 100);

    // Subscribe to all configured topics
    for topic in &config.topics {
        client.subscribe(topic.as_str(), QoS::AtMostOnce).await?;
        info!(topic = %topic, "mqtt_subscribed");
    }

    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::Publish(publish))) => {
                let topic = &publish.topic;
                let payload = std::str::from_utf8(&publish.payload);

                match payload {
                    Ok(payload_str) => {
                        // Try to parse as JSON for the fields
                        let parsed: Option<serde_json::Value> =
                            serde_json::from_str(payload_str).ok();

                        logger.lock().log_mqtt(topic, payload_str, parsed.as_ref());
                        debug!(topic = %topic, bytes = payload_str.len(), "mqtt_message_logged");
                    }
                    Err(e) => {
                        // Log raw bytes as hex if not valid UTF-8
                        let hex_payload = hex::encode(&publish.payload);
                        warn!(topic = %topic, error = %e, "invalid_utf8_payload");
                        logger.lock().log_mqtt(topic, &format!("HEX:{}", hex_payload), None);
                    }
                }
            }
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                info!(
                    host = %config.gateway.mqtt_host(),
                    port = %config.gateway.mqtt_port(),
                    "mqtt_connected"
                );
            }
            Ok(_) => {}
            Err(e) => {
                return Err(Box::new(e));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rotation_strategy_parse_daily() {
        assert!(matches!(RotationStrategy::parse("daily"), RotationStrategy::Daily));
    }

    #[test]
    fn test_rotation_strategy_parse_size() {
        let RotationStrategy::Size(bytes) = RotationStrategy::parse("size:100") else {
            panic!("Expected Size variant");
        };
        assert_eq!(bytes, 100 * 1024 * 1024);
    }

    #[test]
    fn test_rotation_strategy_parse_invalid() {
        // Invalid should default to Daily
        assert!(matches!(RotationStrategy::parse("invalid"), RotationStrategy::Daily));
    }
}
