//! Configuration loading from TOML files
//!
//! Config file is selected via:
//! 1. --config <path> command line argument
//! 2. CONFIG_FILE environment variable
//! 3. Default: config/dev.toml

use crate::domain::types::GeometryId;
use anyhow::Context;
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use std::sync::Arc;

// ============================================================================
// Default value constants
// ============================================================================

const DEFAULT_PROMETHEUS_PORT: u16 = 80;
const DEFAULT_ACC_LISTENER_PORT: u16 = 25803;
const DEFAULT_BROKER_PORT: u16 = 1883;
const DEFAULT_METRICS_PUBLISH_INTERVAL: u64 = 5;
const DEFAULT_POS_EXIT_GRACE_MS: u64 = 5000;

// ============================================================================
// TOML config structs
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
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
    /// Dwell zones auto-authorize after min_dwell_ms (no ACC needed)
    #[serde(default)]
    pub dwell_zones: Vec<i32>,
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

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct AuthorizationConfig {
    pub min_dwell_ms: Option<u64>,
}

/// POS tracking configuration (new canonical location for dwell settings)
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PosTrackingConfig {
    /// Grace window for re-entry (ms) - if track re-enters within this window
    /// after exit, the session is reopened rather than creating a new one
    pub exit_grace_ms: u64,
    /// Minimum dwell time for ACC qualification (ms)
    pub min_dwell_ms: Option<u64>,
}

impl Default for PosTrackingConfig {
    fn default() -> Self {
        Self { exit_grace_ms: DEFAULT_POS_EXIT_GRACE_MS, min_dwell_ms: None }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct MetricsConfig {
    pub interval_secs: u64,
    #[serde(default = "Defaults::prometheus_port")]
    pub prometheus_port: u16,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AccConfig {
    #[serde(default)]
    pub ip_to_pos: HashMap<String, String>,
    #[serde(default = "Defaults::acc_listener_enabled")]
    pub listener_enabled: bool,
    #[serde(default = "Defaults::acc_listener_port")]
    pub listener_port: u16,
    #[serde(default = "Defaults::acc_flicker_merge_s")]
    pub flicker_merge_s: u64,
    #[serde(default = "Defaults::acc_recent_exit_window_ms")]
    pub recent_exit_window_ms: u64,
}

impl Default for AccConfig {
    fn default() -> Self {
        Self {
            ip_to_pos: HashMap::new(),
            listener_enabled: true,
            listener_port: DEFAULT_ACC_LISTENER_PORT,
            flicker_merge_s: Defaults::acc_flicker_merge_s(),
            recent_exit_window_ms: Defaults::acc_recent_exit_window_ms(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct EgressConfig {
    #[serde(default = "Defaults::egress_file")]
    pub file: String,
}

impl Default for EgressConfig {
    fn default() -> Self {
        Self { file: "journeys.jsonl".to_string() }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct MqttEgressConfig {
    #[serde(default = "Defaults::mqtt_egress_enabled")]
    pub enabled: bool,
    pub host: Option<String>,
    pub port: Option<u16>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default = "Defaults::journeys_topic")]
    pub journeys_topic: String,
    #[serde(default = "Defaults::events_topic")]
    pub events_topic: String,
    #[serde(default = "Defaults::metrics_topic")]
    pub metrics_topic: String,
    #[serde(default = "Defaults::gate_topic")]
    pub gate_topic: String,
    #[serde(default = "Defaults::tracks_topic")]
    pub tracks_topic: String,
    #[serde(default = "Defaults::acc_topic")]
    pub acc_topic: String,
    #[serde(default = "Defaults::positions_topic")]
    pub positions_topic: String,
    #[serde(default = "Defaults::metrics_publish_interval")]
    pub metrics_publish_interval_secs: u64,
}

impl Default for MqttEgressConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            host: None,
            port: None,
            username: None,
            password: None,
            journeys_topic: "gateway/journeys".to_string(),
            events_topic: "gateway/events".to_string(),
            metrics_topic: "gateway/metrics".to_string(),
            gate_topic: "gateway/gate".to_string(),
            tracks_topic: "gateway/tracks".to_string(),
            acc_topic: "gateway/acc".to_string(),
            positions_topic: "gateway/positions".to_string(),
            metrics_publish_interval_secs: DEFAULT_METRICS_PUBLISH_INTERVAL,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct BrokerConfig {
    #[serde(default = "Defaults::broker_bind_address")]
    pub bind_address: String,
    #[serde(default = "Defaults::broker_port")]
    pub port: u16,
}

impl Default for BrokerConfig {
    fn default() -> Self {
        Self { bind_address: "0.0.0.0".to_string(), port: DEFAULT_BROKER_PORT }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SiteConfig {
    #[serde(default = "Defaults::site_id")]
    pub id: String,
}

impl Default for SiteConfig {
    fn default() -> Self {
        Self { id: "gateway".to_string() }
    }
}

/// Analysis logging configuration (for offline position data analysis)
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AnalysisLogConfig {
    /// Enable analysis logging (default: false)
    pub enabled: bool,
    /// Directory to write log files (default: "logs")
    pub dir: String,
    /// Rotation strategy: "daily" or "size:100" for 100MB (default: "daily")
    pub rotation: String,
}

impl Default for AnalysisLogConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            dir: "logs".to_string(),
            rotation: "daily".to_string(),
        }
    }
}

/// Serde default value functions (must be free functions for serde)
struct Defaults;

impl Defaults {
    fn prometheus_port() -> u16 {
        DEFAULT_PROMETHEUS_PORT
    }
    fn acc_listener_enabled() -> bool {
        true
    }
    fn acc_listener_port() -> u16 {
        DEFAULT_ACC_LISTENER_PORT
    }
    fn acc_flicker_merge_s() -> u64 {
        10
    }
    fn acc_recent_exit_window_ms() -> u64 {
        3000
    }
    fn egress_file() -> String {
        "journeys.jsonl".to_string()
    }
    fn mqtt_egress_enabled() -> bool {
        true
    }
    fn journeys_topic() -> String {
        "gateway/journeys".to_string()
    }
    fn events_topic() -> String {
        "gateway/events".to_string()
    }
    fn metrics_topic() -> String {
        "gateway/metrics".to_string()
    }
    fn gate_topic() -> String {
        "gateway/gate".to_string()
    }
    fn tracks_topic() -> String {
        "gateway/tracks".to_string()
    }
    fn acc_topic() -> String {
        "gateway/acc".to_string()
    }
    fn positions_topic() -> String {
        "gateway/positions".to_string()
    }
    fn metrics_publish_interval() -> u64 {
        DEFAULT_METRICS_PUBLISH_INTERVAL
    }
    fn broker_bind_address() -> String {
        "0.0.0.0".to_string()
    }
    fn broker_port() -> u16 {
        DEFAULT_BROKER_PORT
    }
    fn site_id() -> String {
        "gateway".to_string()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TomlConfig {
    #[serde(default)]
    pub site: SiteConfig,
    pub mqtt: MqttConfig,
    pub gate: GateConfig,
    pub rs485: Rs485Config,
    pub zones: ZonesConfig,
    #[serde(default)]
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
    #[serde(default)]
    pub pos_tracking: PosTrackingConfig,
    #[serde(default)]
    pub analysis_log: AnalysisLogConfig,
}

// ============================================================================
// Main Config struct
// ============================================================================

/// Main configuration struct used throughout the application
#[derive(Debug, Clone)]
pub struct Config {
    // Site
    site_id: String,
    config_file: String,

    // MQTT ingress
    mqtt_host: String,
    mqtt_port: u16,
    mqtt_topic: String,
    mqtt_username: Option<String>,
    mqtt_password: Option<String>,

    // Gate control
    gate_mode: GateMode,
    gate_url: String,
    gate_tcp_addr: String,
    gate_timeout_ms: u64,

    // RS485 door sensor
    rs485_device: String,
    rs485_baud: u32,
    rs485_poll_interval_ms: u64,

    // Zone definitions
    pos_zones: Vec<i32>,
    dwell_zones: Vec<i32>,
    gate_zone: i32,
    exit_line: i32,
    entry_line: Option<i32>,
    approach_line: Option<i32>,
    store_zone: Option<i32>,
    zone_names: HashMap<i32, Arc<str>>,

    // Authorization / POS tracking
    min_dwell_ms: u64,
    pos_exit_grace_ms: u64,

    // Metrics
    metrics_interval_secs: u64,
    prometheus_port: u16,

    // ACC payment terminal
    acc_ip_to_pos: HashMap<String, String>,
    acc_listener_enabled: bool,
    acc_listener_port: u16,
    acc_flicker_merge_s: u64,
    acc_recent_exit_window_ms: u64,

    // Egress
    egress_file: String,

    // Embedded broker
    broker_bind_address: String,
    broker_port: u16,

    // MQTT egress
    mqtt_egress_enabled: bool,
    mqtt_egress_host: Option<String>,
    mqtt_egress_port: Option<u16>,
    mqtt_egress_username: Option<String>,
    mqtt_egress_password: Option<String>,
    mqtt_egress_journeys_topic: String,
    mqtt_egress_events_topic: String,
    mqtt_egress_metrics_topic: String,
    mqtt_egress_gate_topic: String,
    mqtt_egress_tracks_topic: String,
    mqtt_egress_acc_topic: String,
    mqtt_egress_positions_topic: String,
    mqtt_egress_metrics_interval_secs: u64,

    // Analysis logging
    analysis_log_enabled: bool,
    analysis_log_dir: String,
    analysis_log_rotation: String,
}

/// Macro to generate simple getter methods
macro_rules! config_getters {
    // &str getters (return reference to String field)
    (str: $($name:ident),* $(,)?) => {
        $(
            #[inline]
            pub fn $name(&self) -> &str {
                &self.$name
            }
        )*
    };
    // Copy type getters (return by value)
    (copy: $($name:ident -> $ty:ty),* $(,)?) => {
        $(
            #[inline]
            pub fn $name(&self) -> $ty {
                self.$name
            }
        )*
    };
    // Option<i32> getters (return by value since Copy)
    (opt_i32: $($name:ident),* $(,)?) => {
        $(
            #[inline]
            pub fn $name(&self) -> Option<i32> {
                self.$name
            }
        )*
    };
}

impl Default for Config {
    fn default() -> Self {
        let mqtt_egress = MqttEgressConfig::default();
        Self {
            site_id: "gateway".to_string(),
            config_file: "default".to_string(),
            mqtt_host: "localhost".to_string(),
            mqtt_port: DEFAULT_BROKER_PORT,
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
            dwell_zones: vec![],
            gate_zone: 1007,
            exit_line: 1006,
            entry_line: None,
            approach_line: None,
            store_zone: None,
            zone_names: Self::default_zone_names(),
            min_dwell_ms: 7000,
            pos_exit_grace_ms: 5000,
            metrics_interval_secs: 10,
            prometheus_port: DEFAULT_PROMETHEUS_PORT,
            acc_ip_to_pos: HashMap::new(),
            acc_listener_enabled: true,
            acc_listener_port: DEFAULT_ACC_LISTENER_PORT,
            acc_flicker_merge_s: Defaults::acc_flicker_merge_s(),
            acc_recent_exit_window_ms: Defaults::acc_recent_exit_window_ms(),
            egress_file: "journeys.jsonl".to_string(),
            broker_bind_address: "0.0.0.0".to_string(),
            broker_port: DEFAULT_BROKER_PORT,
            mqtt_egress_enabled: mqtt_egress.enabled,
            mqtt_egress_host: mqtt_egress.host,
            mqtt_egress_port: mqtt_egress.port,
            mqtt_egress_username: mqtt_egress.username,
            mqtt_egress_password: mqtt_egress.password,
            mqtt_egress_journeys_topic: mqtt_egress.journeys_topic,
            mqtt_egress_events_topic: mqtt_egress.events_topic,
            mqtt_egress_metrics_topic: mqtt_egress.metrics_topic,
            mqtt_egress_gate_topic: mqtt_egress.gate_topic,
            mqtt_egress_tracks_topic: mqtt_egress.tracks_topic,
            mqtt_egress_acc_topic: mqtt_egress.acc_topic,
            mqtt_egress_positions_topic: mqtt_egress.positions_topic,
            mqtt_egress_metrics_interval_secs: mqtt_egress.metrics_publish_interval_secs,
            analysis_log_enabled: false,
            analysis_log_dir: "logs".to_string(),
            analysis_log_rotation: "daily".to_string(),
        }
    }
}

impl Config {
    fn default_zone_names() -> HashMap<i32, Arc<str>> {
        let mut names = HashMap::new();
        names.insert(1001, Arc::from("POS_1"));
        names.insert(1002, Arc::from("POS_2"));
        names.insert(1003, Arc::from("POS_3"));
        names.insert(1004, Arc::from("POS_4"));
        names.insert(1005, Arc::from("POS_5"));
        names.insert(1006, Arc::from("EXIT_1"));
        names.insert(1007, Arc::from("GATE_1"));
        names
    }

    /// Determine config file path from args or environment
    /// Used by tests; prefer clap-parsed arguments for main
    #[allow(dead_code)]
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

    /// Load configuration from a TOML file.
    ///
    /// Parses a TOML configuration file and returns a `Config` instance.
    /// Returns an error if the file cannot be read or parsed.
    ///
    /// # Errors
    ///
    /// Returns `anyhow::Error` if:
    /// - The file does not exist or cannot be read
    /// - The TOML content is invalid or missing required sections
    ///
    /// # Example
    ///
    /// ```no_run
    /// use gateway::infra::Config;
    ///
    /// let config = Config::from_file("config/dev.toml").expect("Failed to load config");
    /// assert_eq!(config.mqtt_port(), 1883);
    /// ```
    pub fn from_file<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file {}", path.display()))?;

        let toml_config: TomlConfig = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file {}", path.display()))?;

        // Convert zone names from string keys to i32 keys with Arc<str> values
        let mut zone_names = HashMap::new();
        for (key, value) in toml_config.zones.names {
            if let Ok(id) = key.parse::<i32>() {
                zone_names.insert(id, Arc::from(value));
            }
        }

        // Resolve min_dwell_ms: prefer [pos_tracking].min_dwell_ms, fall back to [authorization]
        let min_dwell_ms = match (
            toml_config.pos_tracking.min_dwell_ms,
            toml_config.authorization.min_dwell_ms,
        ) {
            (Some(pos_val), Some(auth_val)) if pos_val != auth_val => {
                anyhow::bail!(
                    "Config conflict: [pos_tracking].min_dwell_ms ({}) differs from \
                     [authorization].min_dwell_ms ({}). Please use only [pos_tracking].min_dwell_ms.",
                    pos_val,
                    auth_val
                );
            }
            (Some(pos_val), _) => pos_val,
            (None, Some(auth_val)) => {
                eprintln!(
                    "Warning: [authorization].min_dwell_ms is deprecated. \
                     Please move to [pos_tracking].min_dwell_ms"
                );
                auth_val
            }
            (None, None) => 7000, // Default min dwell time
        };

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
            dwell_zones: toml_config.zones.dwell_zones,
            gate_zone: toml_config.zones.gate_zone,
            exit_line: toml_config.zones.exit_line,
            entry_line: toml_config.zones.entry_line,
            approach_line: toml_config.zones.approach_line,
            store_zone: toml_config.zones.store_zone,
            zone_names,
            min_dwell_ms,
            pos_exit_grace_ms: toml_config.pos_tracking.exit_grace_ms,
            metrics_interval_secs: toml_config.metrics.interval_secs,
            prometheus_port: toml_config.metrics.prometheus_port,
            config_file: path.display().to_string(),
            acc_ip_to_pos: toml_config.acc.ip_to_pos,
            acc_listener_enabled: toml_config.acc.listener_enabled,
            acc_listener_port: toml_config.acc.listener_port,
            acc_flicker_merge_s: toml_config.acc.flicker_merge_s,
            acc_recent_exit_window_ms: toml_config.acc.recent_exit_window_ms,
            egress_file: toml_config.egress.file,
            broker_bind_address: toml_config.broker.bind_address,
            broker_port: toml_config.broker.port,
            mqtt_egress_enabled: toml_config.mqtt_egress.enabled,
            mqtt_egress_host: toml_config.mqtt_egress.host,
            mqtt_egress_port: toml_config.mqtt_egress.port,
            mqtt_egress_username: toml_config.mqtt_egress.username,
            mqtt_egress_password: toml_config.mqtt_egress.password,
            mqtt_egress_journeys_topic: toml_config.mqtt_egress.journeys_topic,
            mqtt_egress_events_topic: toml_config.mqtt_egress.events_topic,
            mqtt_egress_metrics_topic: toml_config.mqtt_egress.metrics_topic,
            mqtt_egress_gate_topic: toml_config.mqtt_egress.gate_topic,
            mqtt_egress_tracks_topic: toml_config.mqtt_egress.tracks_topic,
            mqtt_egress_acc_topic: toml_config.mqtt_egress.acc_topic,
            mqtt_egress_positions_topic: toml_config.mqtt_egress.positions_topic,
            mqtt_egress_metrics_interval_secs: toml_config
                .mqtt_egress
                .metrics_publish_interval_secs,
            analysis_log_enabled: toml_config.analysis_log.enabled,
            analysis_log_dir: toml_config.analysis_log.dir,
            analysis_log_rotation: toml_config.analysis_log.rotation,
        })
    }

    /// Load configuration from a specific path - tries TOML file first, falls back to defaults
    pub fn load_from_path(config_path: &str) -> Self {
        match Self::from_file(config_path) {
            Ok(config) => config,
            Err(e) => {
                eprintln!("Warning: {}. Using defaults.", e);
                Self::default()
            }
        }
    }

    /// Load configuration - tries TOML file first, falls back to defaults
    /// Used by tests; prefer load_from_path with clap-parsed arguments for main
    #[allow(dead_code)]
    pub fn load(args: &[String]) -> Self {
        let config_path = Self::resolve_config_path(args);
        Self::load_from_path(&config_path)
    }

    /// Check if a geometry_id is a POS zone.
    ///
    /// Returns true if the geometry ID is in the list of POS zones
    /// configured for dwell time tracking.
    ///
    /// # Example
    ///
    /// ```
    /// use gateway::infra::Config;
    ///
    /// let config = Config::default();
    /// assert!(config.is_pos_zone(1001));  // Default POS zone
    /// assert!(!config.is_pos_zone(9999)); // Not a POS zone
    /// ```
    pub fn is_pos_zone(&self, geometry_id: i32) -> bool {
        self.pos_zones.contains(&geometry_id)
    }

    /// Get zone name from geometry_id.
    ///
    /// Returns the configured name for the zone (as Arc<str> for cheap cloning),
    /// or creates a new Arc for unknown zones with "ZONE_{id}" format.
    ///
    /// # Example
    ///
    /// ```
    /// use gateway::infra::Config;
    /// use gateway::domain::types::GeometryId;
    ///
    /// let config = Config::default();
    /// assert_eq!(&*config.zone_name(GeometryId(1001)), "POS_1");
    /// assert_eq!(&*config.zone_name(GeometryId(9999)), "ZONE_9999");
    /// ```
    pub fn zone_name(&self, geometry_id: GeometryId) -> Arc<str> {
        self.zone_names
            .get(&geometry_id.0)
            .cloned()
            .unwrap_or_else(|| Arc::from(format!("ZONE_{}", geometry_id.0)))
    }

    // ========================================================================
    // Getters (generated via macro for simple cases)
    // ========================================================================

    config_getters!(str:
        site_id,
        config_file,
        mqtt_host,
        mqtt_topic,
        gate_url,
        gate_tcp_addr,
        rs485_device,
        egress_file,
        broker_bind_address,
        mqtt_egress_journeys_topic,
        mqtt_egress_events_topic,
        mqtt_egress_metrics_topic,
        mqtt_egress_gate_topic,
        mqtt_egress_tracks_topic,
        mqtt_egress_acc_topic,
        mqtt_egress_positions_topic,
        analysis_log_dir,
        analysis_log_rotation,
    );

    config_getters!(copy:
        mqtt_port -> u16,
        gate_timeout_ms -> u64,
        rs485_baud -> u32,
        rs485_poll_interval_ms -> u64,
        exit_line -> i32,
        min_dwell_ms -> u64,
        pos_exit_grace_ms -> u64,
        metrics_interval_secs -> u64,
        prometheus_port -> u16,
        acc_listener_enabled -> bool,
        acc_listener_port -> u16,
        acc_flicker_merge_s -> u64,
        acc_recent_exit_window_ms -> u64,
        broker_port -> u16,
        mqtt_egress_enabled -> bool,
        mqtt_egress_metrics_interval_secs -> u64,
        analysis_log_enabled -> bool,
    );

    config_getters!(opt_i32: entry_line, approach_line);

    #[allow(dead_code)]
    #[inline]
    pub fn store_zone(&self) -> Option<i32> {
        self.store_zone
    }

    #[inline]
    pub fn mqtt_username(&self) -> Option<&str> {
        self.mqtt_username.as_deref()
    }

    #[inline]
    pub fn mqtt_password(&self) -> Option<&str> {
        self.mqtt_password.as_deref()
    }

    #[inline]
    pub fn gate_mode(&self) -> GateMode {
        self.gate_mode
    }

    #[inline]
    pub fn pos_zones(&self) -> &[i32] {
        &self.pos_zones
    }

    #[inline]
    pub fn dwell_zones(&self) -> &[i32] {
        &self.dwell_zones
    }

    /// Check if a geometry_id is a dwell zone (auto-authorizes on dwell threshold).
    pub fn is_dwell_zone(&self, geometry_id: i32) -> bool {
        self.dwell_zones.contains(&geometry_id)
    }

    #[inline]
    pub fn gate_zone(&self) -> GeometryId {
        GeometryId(self.gate_zone)
    }

    #[inline]
    pub fn acc_ip_to_pos(&self) -> &HashMap<String, String> {
        &self.acc_ip_to_pos
    }

    /// Get MQTT egress host, falling back to main mqtt host if not set
    #[inline]
    pub fn mqtt_egress_host(&self) -> &str {
        self.mqtt_egress_host.as_deref().unwrap_or(&self.mqtt_host)
    }

    /// Get MQTT egress port, falling back to main mqtt port if not set
    #[inline]
    pub fn mqtt_egress_port(&self) -> u16 {
        self.mqtt_egress_port.unwrap_or(self.mqtt_port)
    }

    /// Get MQTT egress username, falling back to main mqtt username if not set
    #[inline]
    pub fn mqtt_egress_username(&self) -> Option<&str> {
        self.mqtt_egress_username
            .as_deref()
            .or(self.mqtt_username.as_deref())
    }

    /// Get MQTT egress password, falling back to main mqtt password if not set
    #[inline]
    pub fn mqtt_egress_password(&self) -> Option<&str> {
        self.mqtt_egress_password
            .as_deref()
            .or(self.mqtt_password.as_deref())
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

    /// Builder method for tests to set approach_line
    #[cfg(test)]
    pub fn with_approach_line(mut self, line_id: i32) -> Self {
        self.approach_line = Some(line_id);
        self.zone_names.insert(line_id, Arc::from("APPROACH_1"));
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
        assert_eq!(config.pos_exit_grace_ms(), 5000);
        assert_eq!(config.metrics_interval_secs(), 10);
        assert_eq!(config.pos_zones(), &[1001, 1002, 1003, 1004, 1005]);
        assert_eq!(config.gate_zone(), GeometryId(1007));
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
        assert_eq!(&*config.zone_name(GeometryId(1001)), "POS_1");
        assert_eq!(&*config.zone_name(GeometryId(1007)), "GATE_1");
        assert_eq!(&*config.zone_name(GeometryId(1006)), "EXIT_1");
        assert_eq!(&*config.zone_name(GeometryId(9999)), "ZONE_9999");
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

    #[test]
    fn test_pos_tracking_config_defaults() {
        let pos_tracking = PosTrackingConfig::default();
        assert_eq!(pos_tracking.exit_grace_ms, 5000);
        assert!(pos_tracking.min_dwell_ms.is_none());
    }
}
