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
use tracing::info;
use tracing_subscriber::fmt::time::UtcTime;
use tracing_subscriber::EnvFilter;

use gateway_poc::infra::Config;

/// Default log directory for analysis output
const DEFAULT_LOG_DIR: &str = "logs";

/// Default rotation strategy
const DEFAULT_ROTATION: &str = "daily";

/// Default MQTT topics to subscribe to
const DEFAULT_TOPICS: &str = "gateway/#";

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
    #[arg(short = 'd', long, default_value = DEFAULT_LOG_DIR)]
    log_dir: String,

    /// MQTT topics to subscribe to (comma-separated)
    ///
    /// Use # for all topics under a prefix, e.g., "gateway/#"
    #[arg(short, long, default_value = DEFAULT_TOPICS)]
    topics: String,

    /// Log rotation strategy: "daily" or "size:<MB>"
    ///
    /// - daily: Rotate at midnight UTC (default)
    /// - size:100: Rotate when file exceeds 100 MB
    #[arg(short, long, default_value = DEFAULT_ROTATION)]
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
pub struct AnalysisConfig {
    /// Base gateway config (MQTT, ACC, RS485 settings)
    pub gateway: Config,
    /// Directory for log output
    pub log_dir: String,
    /// MQTT topics to subscribe to
    pub topics: Vec<String>,
    /// Rotation strategy
    pub rotation: RotationStrategy,
    /// Site identifier
    pub site_id: String,
}

/// Log file rotation strategy
#[derive(Debug, Clone, Copy)]
pub enum RotationStrategy {
    /// Rotate at midnight UTC
    Daily,
    /// Rotate when file exceeds size in bytes
    Size(u64),
}

impl RotationStrategy {
    fn parse(s: &str) -> Self {
        if s == "daily" {
            return RotationStrategy::Daily;
        }
        if let Some(size_str) = s.strip_prefix("size:") {
            if let Ok(mb) = size_str.parse::<u64>() {
                return RotationStrategy::Size(mb * 1024 * 1024);
            }
        }
        // Default to daily on parse error
        tracing::warn!(rotation = %s, "Invalid rotation strategy, defaulting to daily");
        RotationStrategy::Daily
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

    info!(
        version = env!("CARGO_PKG_VERSION"),
        "gateway_analysis_starting"
    );

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

    // Placeholder for future stories - the logging infrastructure will be added in GA-002+
    info!("Gateway analysis ready. Logging infrastructure will be added in GA-002.");

    // Keep running until Ctrl+C
    tokio::signal::ctrl_c().await?;
    info!("gateway_analysis_shutdown");

    Ok(())
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
