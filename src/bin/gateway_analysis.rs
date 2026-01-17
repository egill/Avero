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
use std::collections::HashMap;
use std::io::ErrorKind;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio_serial::SerialPortBuilderExt;
use tracing::{debug, error, info, warn};
use tracing_subscriber::fmt::time::UtcTime;
use tracing_subscriber::EnvFilter;

use gateway::infra::Config;
use gateway::io::analysis_logger::{AnalysisLogger, RotationStrategy};

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

    /// Split ACC logs per kiosk IP
    ///
    /// When enabled, creates separate log files per kiosk:
    /// logs/acc/<kiosk_ip>-YYYYMMDD.jsonl
    /// Otherwise, all ACC events go to logs/acc/acc-YYYYMMDD.jsonl
    #[arg(long, default_value = "false")]
    acc_split_per_kiosk: bool,
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
    /// Rotation strategy
    rotation: RotationStrategy,
    /// Site identifier
    site_id: String,
    /// Split ACC logs per kiosk IP
    acc_split_per_kiosk: bool,
}

/// Parse rotation strategy from CLI string
fn parse_rotation(s: &str) -> RotationStrategy {
    if s == "daily" {
        return RotationStrategy::Daily;
    }

    // Try parsing "size:<MB>" format
    if let Some(mb) = s.strip_prefix("size:").and_then(|n| n.parse::<u64>().ok()) {
        return RotationStrategy::Size(mb * 1024 * 1024);
    }

    warn!(rotation = %s, "invalid_rotation_strategy_defaulting_to_daily");
    RotationStrategy::Daily
}

impl AnalysisConfig {
    /// Create analysis config from CLI args
    fn from_args(args: Args) -> Self {
        let gateway = Config::load_from_path(&args.config);
        let site_id = args.site_id.unwrap_or_else(|| gateway.site_id().to_string());
        let topics = args.topics.split(',').map(|s| s.trim().to_string()).collect();
        let rotation = parse_rotation(&args.rotation);

        Self {
            gateway,
            log_dir: args.log_dir,
            topics,
            rotation,
            site_id,
            acc_split_per_kiosk: args.acc_split_per_kiosk,
        }
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
        acc_split_per_kiosk = %config.acc_split_per_kiosk,
        rs485_device = %config.gateway.rs485_device(),
        "analysis_config_loaded"
    );

    // Create shared analysis logger with configured rotation strategy
    let logger = Arc::new(Mutex::new(AnalysisLogger::with_rotation(
        &config.log_dir,
        &config.site_id,
        config.rotation,
    )));

    // Start MQTT client task
    let mqtt_handle = tokio::spawn(run_mqtt_logger(config.clone(), Arc::clone(&logger)));
    info!(topics = ?config.topics, "mqtt_logging_started");

    // Start ACC listener task
    let acc_handle = tokio::spawn(run_acc_logger(config.clone(), Arc::clone(&logger)));
    info!(port = %config.gateway.acc_listener_port(), "acc_logging_started");

    // Start RS485 logger task
    let rs485_handle = tokio::spawn(run_rs485_logger(config.clone(), Arc::clone(&logger)));
    info!(device = %config.gateway.rs485_device(), "rs485_logging_started");

    // Wait for shutdown signal
    tokio::signal::ctrl_c().await?;
    info!("shutdown_signal_received");

    // Abort tasks
    mqtt_handle.abort();
    acc_handle.abort();
    rs485_handle.abort();

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

/// Run the ACC logger - listens on ACC port and logs all events
async fn run_acc_logger(config: AnalysisConfig, logger: Arc<Mutex<AnalysisLogger>>) {
    loop {
        if let Err(e) = acc_logger_loop(&config, &logger).await {
            error!(error = %e, "acc_logger_error");
        }
        warn!("acc_listener_restarting");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// ACC logger event loop
async fn acc_logger_loop(
    config: &AnalysisConfig,
    logger: &Arc<Mutex<AnalysisLogger>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr = format!("0.0.0.0:{}", config.gateway.acc_listener_port());
    let listener = TcpListener::bind(&addr).await?;

    info!(port = %config.gateway.acc_listener_port(), "acc_listener_bound");

    loop {
        match listener.accept().await {
            Ok((socket, addr)) => {
                let peer_ip = addr.ip().to_string();
                let logger_clone = logger.clone();
                let ip_to_pos = config.gateway.acc_ip_to_pos().clone();
                let split_per_kiosk = config.acc_split_per_kiosk;

                tokio::spawn(async move {
                    handle_acc_connection(
                        socket,
                        peer_ip,
                        logger_clone,
                        ip_to_pos,
                        split_per_kiosk,
                    )
                    .await;
                });
            }
            Err(e) => {
                error!(error = %e, "acc_accept_failed");
            }
        }
    }
}

/// Handle a single ACC connection - read lines and log them
async fn handle_acc_connection(
    socket: tokio::net::TcpStream,
    peer_ip: String,
    logger: Arc<Mutex<AnalysisLogger>>,
    ip_to_pos: HashMap<String, String>,
    split_per_kiosk: bool,
) {
    debug!(ip = %peer_ip, "acc_connection_accepted");

    let reader = BufReader::new(socket);
    let mut lines = reader.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let raw_line = line.trim();
        if raw_line.is_empty() {
            continue;
        }

        // Parse "ACC <receipt_id>" format
        let receipt_id = raw_line.strip_prefix("ACC ").map(|s| s.trim());

        // Look up POS zone from ip_to_pos mapping
        let pos_zone = ip_to_pos.get(&peer_ip).map(|s| s.as_str());

        info!(
            kiosk_ip = %peer_ip,
            raw_line = %raw_line,
            receipt_id = ?receipt_id,
            pos_zone = ?pos_zone,
            "acc_event_received"
        );

        logger.lock().log_acc(&peer_ip, raw_line, receipt_id, pos_zone, split_per_kiosk);
    }

    debug!(ip = %peer_ip, "acc_connection_closed");
}

// RS485 protocol constants (from rs485.rs)
const RS485_START_BYTE_COMMAND: u8 = 0x7E;
const RS485_START_BYTE_RESPONSE: u8 = 0x7F;
const RS485_CMD_QUERY: u8 = 0x10;
const RS485_COMMAND_FRAME_LEN: usize = 8;
const RS485_RESPONSE_FRAME_LEN: usize = 18;
const RS485_MAX_READ_ATTEMPTS: usize = 10;

// Door status codes
const DOOR_CLOSED_PROPERLY: u8 = 0x00;
const DOOR_LEFT_OPEN_PROPERLY: u8 = 0x01;
const DOOR_RIGHT_OPEN_PROPERLY: u8 = 0x02;
const DOOR_IN_MOTION: u8 = 0x03;
const DOOR_FIRE_SIGNAL_OPENING: u8 = 0x04;

/// Run the RS485 logger - polls door state and logs frames
async fn run_rs485_logger(config: AnalysisConfig, logger: Arc<Mutex<AnalysisLogger>>) {
    loop {
        if let Err(e) = rs485_logger_loop(&config, &logger).await {
            error!(error = %e, "rs485_logger_error");
        }
        warn!("rs485_reconnecting");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// RS485 logger event loop
async fn rs485_logger_loop(
    config: &AnalysisConfig,
    logger: &Arc<Mutex<AnalysisLogger>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let device = config.gateway.rs485_device();
    let baud = config.gateway.rs485_baud();
    let poll_interval = Duration::from_millis(config.gateway.rs485_poll_interval_ms());

    let mut port = tokio_serial::new(device, baud)
        .timeout(Duration::from_millis(100))
        .open_native_async()
        .map_err(|e| format!("Failed to open {}: {}", device, e))?;

    info!(device = %device, baud = %baud, "rs485_port_opened");

    let mut read_buffer: Vec<u8> = Vec::with_capacity(64);
    let mut poll_timer = tokio::time::interval(poll_interval);

    loop {
        poll_timer.tick().await;

        // Build and send query command
        let cmd = build_rs485_query_command(1); // machine_number = 1
        if let Err(e) = port.write_all(&cmd).await {
            warn!(error = %e, "rs485_write_error");
            continue;
        }

        // Read response frame
        match read_rs485_frame(&mut port, &mut read_buffer).await {
            Some(frame) => {
                let raw_frame = hex::encode(&frame);
                let (door_status, checksum_ok) = parse_rs485_frame(&frame);

                logger.lock().log_rs485(&raw_frame, door_status, checksum_ok);
            }
            None => {
                // No valid frame received - log as failed read
                if !read_buffer.is_empty() {
                    let raw_frame = hex::encode(&read_buffer);
                    logger.lock().log_rs485(&raw_frame, None, false);
                    read_buffer.clear();
                }
            }
        }
    }
}

/// Build RS485 query command frame (8 bytes)
fn build_rs485_query_command(machine_number: u8) -> [u8; RS485_COMMAND_FRAME_LEN] {
    let mut frame = [0u8; RS485_COMMAND_FRAME_LEN];
    frame[0] = RS485_START_BYTE_COMMAND;
    frame[1] = 0x00; // Undefined
    frame[2] = machine_number;
    frame[3] = RS485_CMD_QUERY;
    frame[4] = 0x00; // Data0
    frame[5] = 0x00; // Data1
    frame[6] = 0x00; // Data2

    // Checksum: sum all bytes, bitwise NOT
    let sum: u8 = frame[..7].iter().fold(0u8, |acc, &x| acc.wrapping_add(x));
    frame[7] = !sum;

    frame
}

/// Read a complete RS485 response frame from the serial port
async fn read_rs485_frame(
    port: &mut tokio_serial::SerialStream,
    read_buffer: &mut Vec<u8>,
) -> Option<Vec<u8>> {
    // Synchronize buffer to start byte
    synchronize_rs485_buffer(read_buffer);

    // Check if we already have a complete frame
    if read_buffer.len() >= RS485_RESPONSE_FRAME_LEN {
        return extract_rs485_frame(read_buffer);
    }

    // Read until we have enough data
    let mut temp_buf = [0u8; 64];
    let mut attempts = 0;

    while read_buffer.len() < RS485_RESPONSE_FRAME_LEN {
        attempts += 1;
        if attempts > RS485_MAX_READ_ATTEMPTS {
            return None;
        }

        match tokio::time::timeout(Duration::from_millis(50), port.read(&mut temp_buf)).await {
            Ok(Ok(n)) if n > 0 => {
                read_buffer.extend_from_slice(&temp_buf[..n]);
                synchronize_rs485_buffer(read_buffer);
            }
            Ok(Ok(_)) => {}                                     // Zero bytes read
            Ok(Err(e)) if e.kind() == ErrorKind::TimedOut => {} // Serial timeout
            Ok(Err(_)) => return None,
            Err(_) => {} // Tokio timeout
        }
    }

    extract_rs485_frame(read_buffer)
}

/// Synchronize read buffer to start with START_BYTE_RESPONSE (0x7F)
fn synchronize_rs485_buffer(buffer: &mut Vec<u8>) {
    if buffer.first() == Some(&RS485_START_BYTE_RESPONSE) {
        return;
    }

    if let Some(start_idx) = buffer.iter().position(|&b| b == RS485_START_BYTE_RESPONSE) {
        buffer.drain(..start_idx);
    } else {
        buffer.clear();
    }
}

/// Extract a frame from the buffer
fn extract_rs485_frame(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    if buffer.len() < RS485_RESPONSE_FRAME_LEN {
        return None;
    }

    Some(buffer.drain(..RS485_RESPONSE_FRAME_LEN).collect())
}

/// Parse RS485 response frame and extract door status
fn parse_rs485_frame(frame: &[u8]) -> (Option<&'static str>, bool) {
    if frame.len() != RS485_RESPONSE_FRAME_LEN {
        return (None, false);
    }

    if frame[0] != RS485_START_BYTE_RESPONSE {
        return (None, false);
    }

    // Validate checksum: sum all bytes (including checksum), add 1, should be 0
    let sum: u8 = frame.iter().fold(0u8, |acc, &x| acc.wrapping_add(x));
    let checksum_ok = sum.wrapping_add(1) == 0;

    if !checksum_ok {
        return (None, false);
    }

    // Parse door status from byte 4
    // Note: "right open" is the resting/closed position for this door type
    let door_status = match frame[4] {
        DOOR_CLOSED_PROPERLY | DOOR_RIGHT_OPEN_PROPERLY => "closed",
        DOOR_LEFT_OPEN_PROPERLY | DOOR_FIRE_SIGNAL_OPENING => "open",
        DOOR_IN_MOTION => "moving",
        _ => "unknown",
    };

    (Some(door_status), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rotation_strategy_parse_daily() {
        assert!(matches!(parse_rotation("daily"), RotationStrategy::Daily));
    }

    #[test]
    fn test_rotation_strategy_parse_size() {
        let RotationStrategy::Size(bytes) = parse_rotation("size:100") else {
            panic!("Expected Size variant");
        };
        assert_eq!(bytes, 100 * 1024 * 1024);
    }

    #[test]
    fn test_rotation_strategy_parse_invalid() {
        // Invalid should default to Daily
        assert!(matches!(parse_rotation("invalid"), RotationStrategy::Daily));
    }
}
