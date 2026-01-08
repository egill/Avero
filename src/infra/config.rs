//! Configuration loading from TOML files
//!
//! Config file is selected via:
//! 1. --config <path> command line argument
//! 2. CONFIG_FILE environment variable
//! 3. Default: config/dev.toml

use anyhow::Context;
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GateMode {
    Http,
    Tcp,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MqttConfig {
    pub host: String,
    pub port: u16,
    pub topic: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GateConfig {
    pub mode: GateMode,
    pub tcp_addr: String,
    pub http_url: String,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rs485Config {
    pub device: String,
    pub baud: u32,
    pub poll_interval_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ZonesConfig {
    pub pos_zones: Vec<i32>,
    pub gate_zone: i32,
    pub exit_line: i32,
    #[serde(default)]
    pub entry_line: Option<i32>,
    #[serde(default)]
    pub approach_line: Option<i32>,
    #[serde(default)]
    pub store_zone: Option<i32>,
    #[serde(default)]
    pub names: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthorizationConfig {
    pub min_dwell_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MetricsConfig {
    pub interval_secs: u64,
    /// Prometheus metrics HTTP port (0 to disable)
    #[serde(default = "default_prometheus_port")]
    pub prometheus_port: u16,
}

fn default_prometheus_port() -> u16 {
    80 // Default to port 80 for Prometheus scraping
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AccConfig {
    /// IP to POS zone name mapping (e.g., "192.168.1.10" = "POS_1")
    #[serde(default)]
    pub ip_to_pos: HashMap<String, String>,
    /// Enable ACC TCP listener
    #[serde(default = "default_acc_listener_enabled")]
    pub listener_enabled: bool,
    /// ACC TCP listener port
    #[serde(default = "default_acc_listener_port")]
    pub listener_port: u16,
}

fn default_acc_listener_enabled() -> bool {
    true
}

fn default_acc_listener_port() -> u16 {
    25803
}

#[derive(Debug, Clone, Deserialize)]
pub struct EgressConfig {
    /// File path for journey egress (JSONL format)
    #[serde(default = "default_egress_file")]
    pub file: String,
}

impl Default for EgressConfig {
    fn default() -> Self {
        Self { file: default_egress_file() }
    }
}

fn default_egress_file() -> String {
    "journeys.jsonl".to_string()
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct MqttEgressConfig {
    /// Enable MQTT egress publishing
    #[serde(default = "default_mqtt_egress_enabled")]
    pub enabled: bool,
    /// Topic for completed journey JSONs (QoS 1)
    #[serde(default = "default_journeys_topic")]
    pub journeys_topic: String,
    /// Topic for live zone events (QoS 0)
    #[serde(default = "default_events_topic")]
    pub events_topic: String,
    /// Topic for periodic metrics snapshots (QoS 0)
    #[serde(default = "default_metrics_topic")]
    pub metrics_topic: String,
    /// Topic for gate state changes (QoS 0)
    #[serde(default = "default_gate_topic")]
    pub gate_topic: String,
    /// Topic for track lifecycle events (QoS 0)
    #[serde(default = "default_tracks_topic")]
    pub tracks_topic: String,
    /// Topic for ACC (payment terminal) events (QoS 0)
    #[serde(default = "default_acc_topic")]
    pub acc_topic: String,
    /// Interval for publishing metrics (seconds)
    #[serde(default = "default_metrics_publish_interval")]
    pub metrics_publish_interval_secs: u64,
}

fn default_mqtt_egress_enabled() -> bool {
    true
}

fn default_journeys_topic() -> String {
    "gateway/journeys".to_string()
}

fn default_events_topic() -> String {
    "gateway/events".to_string()
}

fn default_metrics_topic() -> String {
    "gateway/metrics".to_string()
}

fn default_gate_topic() -> String {
    "gateway/gate".to_string()
}

fn default_tracks_topic() -> String {
    "gateway/tracks".to_string()
}

fn default_acc_topic() -> String {
    "gateway/acc".to_string()
}

fn default_metrics_publish_interval() -> u64 {
    5
}

#[derive(Debug, Clone, Deserialize)]
pub struct BrokerConfig {
    #[serde(default = "default_broker_bind_address")]
    pub bind_address: String,
    #[serde(default = "default_broker_port")]
    pub port: u16,
}

fn default_broker_bind_address() -> String {
    "0.0.0.0".to_string()
}

fn default_broker_port() -> u16 {
    1883
}

impl Default for BrokerConfig {
    fn default() -> Self {
        Self { bind_address: default_broker_bind_address(), port: default_broker_port() }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SiteConfig {
    /// Unique site identifier (e.g., "netto", "grandi")
    #[serde(default = "default_site_id")]
    pub id: String,
}

fn default_site_id() -> String {
    "gateway".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct TomlConfig {
    #[serde(default)]
    pub site: SiteConfig,
    pub mqtt: MqttConfig,
    pub gate: GateConfig,
    pub rs485: Rs485Config,
    pub zones: ZonesConfig,
    pub authorization: AuthorizationConfig,
    pub metrics: MetricsConfig,
    #[serde(default)]
    pub acc: AccConfig,
    #[serde(default)]
    pub egress: EgressConfig,
    #[serde(default)]
    pub broker: BrokerConfig,
    #[serde(default)]
    pub mqtt_egress: MqttEgressConfig,
}

/// Main configuration struct used throughout the application
#[derive(Debug, Clone)]
pub struct Config {
    site_id: String,
    mqtt_host: String,
    mqtt_port: u16,
    mqtt_topic: String,
    mqtt_username: Option<String>,
    mqtt_password: Option<String>,
    gate_mode: GateMode,
    gate_url: String,
    gate_tcp_addr: String,
    gate_timeout_ms: u64,
    rs485_device: String,
    rs485_baud: u32,
    rs485_poll_interval_ms: u64,
    pos_zones: Vec<i32>,
    gate_zone: i32,
    exit_line: i32,
    entry_line: Option<i32>,
    approach_line: Option<i32>,
    _store_zone: Option<i32>,
    zone_names: HashMap<i32, String>,
    min_dwell_ms: u64,
    metrics_interval_secs: u64,
    prometheus_port: u16,
    config_file: String,
    acc_ip_to_pos: HashMap<String, String>,
    acc_listener_enabled: bool,
    acc_listener_port: u16,
    egress_file: String,
    broker_bind_address: String,
    broker_port: u16,
    // MQTT Egress config
    mqtt_egress_enabled: bool,
    mqtt_egress_journeys_topic: String,
    mqtt_egress_events_topic: String,
    mqtt_egress_metrics_topic: String,
    mqtt_egress_gate_topic: String,
    mqtt_egress_tracks_topic: String,
    mqtt_egress_acc_topic: String,
    mqtt_egress_metrics_interval_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            site_id: "gateway".to_string(),
            mqtt_host: "localhost".to_string(),
            mqtt_port: 1883,
            mqtt_topic: "#".to_string(),
            mqtt_username: None,
            mqtt_password: None,
            gate_mode: GateMode::Tcp,
            gate_url: "http://admin:88888888@192.168.0.245/cdor.cgi?door=0&open=1".to_string(),
            gate_tcp_addr: "192.168.0.245:8000".to_string(),
            gate_timeout_ms: 2000,
            rs485_device: "/dev/ttyAMA4".to_string(),
            rs485_baud: 19200,
            rs485_poll_interval_ms: 250,
            pos_zones: vec![1001, 1002, 1003, 1004, 1005],
            gate_zone: 1007,
            exit_line: 1006,
            entry_line: None,
            approach_line: None,
            _store_zone: None,
            zone_names: Self::default_zone_names(),
            min_dwell_ms: 7000,
            metrics_interval_secs: 10,
            prometheus_port: 80,
            config_file: "default".to_string(),
            acc_ip_to_pos: HashMap::new(),
            acc_listener_enabled: true,
            acc_listener_port: 25803,
            egress_file: "journeys.jsonl".to_string(),
            broker_bind_address: "0.0.0.0".to_string(),
            broker_port: 1883,
            mqtt_egress_enabled: true,
            mqtt_egress_journeys_topic: "gateway/journeys".to_string(),
            mqtt_egress_events_topic: "gateway/events".to_string(),
            mqtt_egress_metrics_topic: "gateway/metrics".to_string(),
            mqtt_egress_gate_topic: "gateway/gate".to_string(),
            mqtt_egress_tracks_topic: "gateway/tracks".to_string(),
            mqtt_egress_acc_topic: "gateway/acc".to_string(),
            mqtt_egress_metrics_interval_secs: 5,
        }
    }
}

impl Config {
    fn default_zone_names() -> HashMap<i32, String> {
        let mut names = HashMap::new();
        names.insert(1001, "POS_1".to_string());
        names.insert(1002, "POS_2".to_string());
        names.insert(1003, "POS_3".to_string());
        names.insert(1004, "POS_4".to_string());
        names.insert(1005, "POS_5".to_string());
        names.insert(1006, "EXIT_1".to_string());
        names.insert(1007, "GATE_1".to_string());
        names
    }

    /// Determine config file path from args or environment
    pub fn resolve_config_path(args: &[String]) -> String {
        // Check for --config argument
        for (i, arg) in args.iter().enumerate() {
            if arg == "--config" {
                if let Some(path) = args.get(i + 1) {
                    return path.clone();
                }
            }
            if let Some(path) = arg.strip_prefix("--config=") {
                return path.to_string();
            }
        }

        // Check CONFIG_FILE environment variable
        if let Ok(path) = env::var("CONFIG_FILE") {
            return path;
        }

        // Default to dev.toml
        "config/dev.toml".to_string()
    }

    /// Load configuration from a TOML file
    pub fn from_file<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file {}", path.display()))?;

        let toml_config: TomlConfig = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file {}", path.display()))?;

        // Convert zone names from string keys to i32 keys
        let mut zone_names = HashMap::new();
        for (key, value) in toml_config.zones.names {
            if let Ok(id) = key.parse::<i32>() {
                zone_names.insert(id, value);
            }
        }

        Ok(Self {
            site_id: toml_config.site.id,
            mqtt_host: toml_config.mqtt.host,
            mqtt_port: toml_config.mqtt.port,
            mqtt_topic: toml_config.mqtt.topic,
            mqtt_username: toml_config.mqtt.username,
            mqtt_password: toml_config.mqtt.password,
            gate_mode: toml_config.gate.mode,
            gate_url: toml_config.gate.http_url,
            gate_tcp_addr: toml_config.gate.tcp_addr,
            gate_timeout_ms: toml_config.gate.timeout_ms,
            rs485_device: toml_config.rs485.device,
            rs485_baud: toml_config.rs485.baud,
            rs485_poll_interval_ms: toml_config.rs485.poll_interval_ms,
            pos_zones: toml_config.zones.pos_zones,
            gate_zone: toml_config.zones.gate_zone,
            exit_line: toml_config.zones.exit_line,
            entry_line: toml_config.zones.entry_line,
            approach_line: toml_config.zones.approach_line,
            _store_zone: toml_config.zones.store_zone,
            zone_names,
            min_dwell_ms: toml_config.authorization.min_dwell_ms,
            metrics_interval_secs: toml_config.metrics.interval_secs,
            prometheus_port: toml_config.metrics.prometheus_port,
            config_file: path.display().to_string(),
            acc_ip_to_pos: toml_config.acc.ip_to_pos,
            acc_listener_enabled: toml_config.acc.listener_enabled,
            acc_listener_port: toml_config.acc.listener_port,
            egress_file: toml_config.egress.file,
            broker_bind_address: toml_config.broker.bind_address,
            broker_port: toml_config.broker.port,
            mqtt_egress_enabled: toml_config.mqtt_egress.enabled,
            mqtt_egress_journeys_topic: toml_config.mqtt_egress.journeys_topic,
            mqtt_egress_events_topic: toml_config.mqtt_egress.events_topic,
            mqtt_egress_metrics_topic: toml_config.mqtt_egress.metrics_topic,
            mqtt_egress_gate_topic: toml_config.mqtt_egress.gate_topic,
            mqtt_egress_tracks_topic: toml_config.mqtt_egress.tracks_topic,
            mqtt_egress_acc_topic: toml_config.mqtt_egress.acc_topic,
            mqtt_egress_metrics_interval_secs: toml_config
                .mqtt_egress
                .metrics_publish_interval_secs,
        })
    }

    /// Load configuration - tries TOML file first, falls back to defaults
    pub fn load(args: &[String]) -> Self {
        let config_path = Self::resolve_config_path(args);

        match Self::from_file(&config_path) {
            Ok(config) => config,
            Err(e) => {
                eprintln!("Warning: {}. Using defaults.", e);
                Self::default()
            }
        }
    }

    /// Check if a geometry_id is a POS zone
    pub fn is_pos_zone(&self, geometry_id: i32) -> bool {
        self.pos_zones.contains(&geometry_id)
    }

    /// Get zone name from geometry_id
    pub fn zone_name(&self, geometry_id: i32) -> String {
        self.zone_names
            .get(&geometry_id)
            .cloned()
            .unwrap_or_else(|| format!("ZONE_{}", geometry_id))
    }

    // Getters for all config fields
    pub fn site_id(&self) -> &str {
        &self.site_id
    }

    pub fn mqtt_host(&self) -> &str {
        &self.mqtt_host
    }

    pub fn mqtt_port(&self) -> u16 {
        self.mqtt_port
    }

    pub fn mqtt_topic(&self) -> &str {
        &self.mqtt_topic
    }

    pub fn mqtt_username(&self) -> Option<&str> {
        self.mqtt_username.as_deref()
    }

    pub fn mqtt_password(&self) -> Option<&str> {
        self.mqtt_password.as_deref()
    }

    pub fn gate_mode(&self) -> &GateMode {
        &self.gate_mode
    }

    pub fn gate_url(&self) -> &str {
        &self.gate_url
    }

    pub fn gate_tcp_addr(&self) -> &str {
        &self.gate_tcp_addr
    }

    pub fn gate_timeout_ms(&self) -> u64 {
        self.gate_timeout_ms
    }

    pub fn rs485_device(&self) -> &str {
        &self.rs485_device
    }

    pub fn rs485_baud(&self) -> u32 {
        self.rs485_baud
    }

    pub fn rs485_poll_interval_ms(&self) -> u64 {
        self.rs485_poll_interval_ms
    }

    pub fn pos_zones(&self) -> &[i32] {
        &self.pos_zones
    }

    pub fn gate_zone(&self) -> i32 {
        self.gate_zone
    }

    pub fn exit_line(&self) -> i32 {
        self.exit_line
    }

    pub fn entry_line(&self) -> Option<i32> {
        self.entry_line
    }

    pub fn approach_line(&self) -> Option<i32> {
        self.approach_line
    }

    #[allow(dead_code)]
    pub fn store_zone(&self) -> Option<i32> {
        self._store_zone
    }

    pub fn min_dwell_ms(&self) -> u64 {
        self.min_dwell_ms
    }

    pub fn metrics_interval_secs(&self) -> u64 {
        self.metrics_interval_secs
    }

    pub fn prometheus_port(&self) -> u16 {
        self.prometheus_port
    }

    pub fn config_file(&self) -> &str {
        &self.config_file
    }

    pub fn acc_ip_to_pos(&self) -> &HashMap<String, String> {
        &self.acc_ip_to_pos
    }

    pub fn acc_listener_enabled(&self) -> bool {
        self.acc_listener_enabled
    }

    pub fn acc_listener_port(&self) -> u16 {
        self.acc_listener_port
    }

    pub fn egress_file(&self) -> &str {
        &self.egress_file
    }

    pub fn broker_bind_address(&self) -> &str {
        &self.broker_bind_address
    }

    pub fn broker_port(&self) -> u16 {
        self.broker_port
    }

    // MQTT Egress getters
    pub fn mqtt_egress_enabled(&self) -> bool {
        self.mqtt_egress_enabled
    }

    pub fn mqtt_egress_journeys_topic(&self) -> &str {
        &self.mqtt_egress_journeys_topic
    }

    pub fn mqtt_egress_events_topic(&self) -> &str {
        &self.mqtt_egress_events_topic
    }

    pub fn mqtt_egress_metrics_topic(&self) -> &str {
        &self.mqtt_egress_metrics_topic
    }

    pub fn mqtt_egress_gate_topic(&self) -> &str {
        &self.mqtt_egress_gate_topic
    }

    pub fn mqtt_egress_tracks_topic(&self) -> &str {
        &self.mqtt_egress_tracks_topic
    }

    pub fn mqtt_egress_acc_topic(&self) -> &str {
        &self.mqtt_egress_acc_topic
    }

    pub fn mqtt_egress_metrics_interval_secs(&self) -> u64 {
        self.mqtt_egress_metrics_interval_secs
    }

    /// Builder method for tests to set min_dwell_ms
    #[cfg(test)]
    pub fn with_min_dwell_ms(mut self, ms: u64) -> Self {
        self.min_dwell_ms = ms;
        self
    }

    /// Builder method for tests to set acc_ip_to_pos mapping
    #[cfg(test)]
    pub fn with_acc_ip_to_pos(mut self, ip_to_pos: HashMap<String, String>) -> Self {
        self.acc_ip_to_pos = ip_to_pos;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.mqtt_host(), "localhost");
        assert_eq!(config.mqtt_port(), 1883);
        assert_eq!(config.mqtt_topic(), "#");
        assert_eq!(config.min_dwell_ms(), 7000);
        assert_eq!(config.metrics_interval_secs(), 10);
        assert_eq!(config.pos_zones(), &[1001, 1002, 1003, 1004, 1005]);
        assert_eq!(config.gate_zone(), 1007);
    }

    #[test]
    fn test_is_pos_zone() {
        let config = Config::default();
        assert!(config.is_pos_zone(1001));
        assert!(config.is_pos_zone(1005));
        assert!(!config.is_pos_zone(1007));
        assert!(!config.is_pos_zone(1006));
    }

    #[test]
    fn test_zone_name() {
        let config = Config::default();
        assert_eq!(config.zone_name(1001), "POS_1");
        assert_eq!(config.zone_name(1007), "GATE_1");
        assert_eq!(config.zone_name(1006), "EXIT_1");
        assert_eq!(config.zone_name(9999), "ZONE_9999");
    }

    #[test]
    fn test_resolve_config_path_default() {
        let args: Vec<String> = vec!["gateway-poc".to_string()];
        assert_eq!(Config::resolve_config_path(&args), "config/dev.toml");
    }

    #[test]
    fn test_resolve_config_path_from_arg() {
        let args: Vec<String> = vec![
            "gateway-poc".to_string(),
            "--config".to_string(),
            "config/netto.toml".to_string(),
        ];
        assert_eq!(Config::resolve_config_path(&args), "config/netto.toml");
    }

    #[test]
    fn test_resolve_config_path_from_arg_equals() {
        let args: Vec<String> =
            vec!["gateway-poc".to_string(), "--config=config/grandi.toml".to_string()];
        assert_eq!(Config::resolve_config_path(&args), "config/grandi.toml");
    }

    #[test]
    fn test_egress_file_default() {
        // Verify that EgressConfig::default() returns proper default, not empty string
        let egress = EgressConfig::default();
        assert_eq!(egress.file, "journeys.jsonl");
        assert!(!egress.file.is_empty());

        // Verify that Config::default() also has proper egress file
        let config = Config::default();
        assert_eq!(config.egress_file(), "journeys.jsonl");
    }
}
