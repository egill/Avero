//! Gateway Simulation Controller
//!
//! Unified TUI for local development and testing.
//! Spawns gateway and mock services based on configuration.
//!
//! Usage:
//!   cargo run --bin simctl                    # Interactive TUI mode
//!   cargo run --bin simctl -- --test all      # Run all test scenarios
//!   cargo run --bin simctl -- --test happy_path,no_payment
//!
//! Features:
//! - Interactive startup menu to configure emulation modes
//! - Spawns gateway binary with appropriate config
//! - Spawns mock CloudPlus TCP server if gate emulation enabled
//! - Event injection (Xovis, ACC, barcode)
//! - State inspector panel showing person states and gate status
//! - Scenario runner with assertions
//! - Automated test mode with pass/fail reporting

use chrono::Utc;
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Gauge},
    Frame, Terminal,
};
use rumqttc::{AsyncClient, Event as MqttEvent, MqttOptions, Packet, QoS};
use serde::Deserialize;
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use std::io;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

// ============================================================================
// Constants
// ============================================================================

const MAX_LOG_ENTRIES: usize = 200;
const MAX_STATE_EVENTS: usize = 50;
const XOVIS_TOPIC: &str = "xovis/sim";

// Zone/Line geometry IDs (matching config)
const ZONE_POS_1: i64 = 1001;
const ZONE_POS_2: i64 = 1002;
const ZONE_POS_3: i64 = 1003;
const ZONE_POS_4: i64 = 1004;
const ZONE_POS_5: i64 = 1005;
const LINE_EXIT: i64 = 1006;
const ZONE_GATE: i64 = 1007;
const LINE_ENTRY: i64 = 1008;
const LINE_APPROACH: i64 = 1009;
const ZONE_STORE: i64 = 1010;

// Spawn positions
const POS_ENTRANCE: [f64; 3] = [2.0, 0.5, 1.70];
const POS_IN_STORE: [f64; 3] = [4.0, 3.0, 1.70];

// Line cross directions
const DIR_FORWARD: &str = "forward";
const DIR_BACKWARD: &str = "backward";

// ============================================================================
// CLI Arguments
// ============================================================================

#[derive(Parser, Debug)]
#[command(name = "simctl")]
#[command(about = "Gateway Simulation Controller - TUI and automated testing")]
struct Args {
    /// Run test scenarios instead of interactive mode
    /// Examples: --test all, --test happy_path, --test happy_path,no_payment
    #[arg(short, long)]
    test: Option<String>,

    /// Verbose output in test mode
    #[arg(short, long)]
    verbose: bool,
}

// ============================================================================
// Scenarios
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq)]
enum ScenarioStep {
    CreateTrack { in_store: bool },
    DeleteTrack,
    ZoneEntry(&'static str),
    ZoneExit(&'static str),
    LineCross { line: &'static str, forward: bool },
    Acc(&'static str), // POS zone name (e.g., "POS_1")
    Wait(u64),         // milliseconds
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)] // Variants reserved for future scenario types
enum ExpectedOutcome {
    GateOpen,
    GateBlocked,
    JourneyAuthorized,
    JourneyUnauthorized,
}

#[derive(Debug, Clone)]
struct Scenario {
    name: &'static str,
    description: &'static str,
    steps: &'static [ScenarioStep],
    expected: ExpectedOutcome,
    timeout_ms: u64,
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "happy_path",
        description: "Customer pays and exits through gate",
        steps: &[
            ScenarioStep::CreateTrack { in_store: false },
            ScenarioStep::ZoneEntry("POS_1"),
            ScenarioStep::Wait(8000), // dwell > 7000ms threshold
            ScenarioStep::Acc("POS_1"),
            ScenarioStep::ZoneExit("POS_1"),
            ScenarioStep::ZoneEntry("GATE_1"),
            ScenarioStep::Wait(500),
            ScenarioStep::ZoneExit("GATE_1"),
            ScenarioStep::LineCross { line: "EXIT_1", forward: true },
            ScenarioStep::DeleteTrack,
        ],
        expected: ExpectedOutcome::GateOpen,
        timeout_ms: 20000,
    },
    Scenario {
        name: "no_payment",
        description: "Customer dwells but doesn't pay - gate blocked",
        steps: &[
            ScenarioStep::CreateTrack { in_store: false },
            ScenarioStep::ZoneEntry("POS_1"),
            ScenarioStep::Wait(8000),
            // No ACC event
            ScenarioStep::ZoneExit("POS_1"),
            ScenarioStep::ZoneEntry("GATE_1"),
            ScenarioStep::Wait(1000),
        ],
        expected: ExpectedOutcome::GateBlocked,
        timeout_ms: 15000,
    },
    Scenario {
        name: "fast_exit",
        description: "Customer doesn't dwell long enough - gate blocked",
        steps: &[
            ScenarioStep::CreateTrack { in_store: false },
            ScenarioStep::ZoneEntry("POS_1"),
            ScenarioStep::Wait(2000), // < 7000ms threshold
            ScenarioStep::Acc("POS_1"),
            ScenarioStep::ZoneExit("POS_1"),
            ScenarioStep::ZoneEntry("GATE_1"),
            ScenarioStep::Wait(1000),
        ],
        expected: ExpectedOutcome::GateBlocked,
        timeout_ms: 10000,
    },
    Scenario {
        name: "gate_zone_exit",
        description: "Customer enters and exits gate zone without crossing exit line",
        steps: &[
            ScenarioStep::CreateTrack { in_store: false },
            ScenarioStep::ZoneEntry("POS_1"),
            ScenarioStep::Wait(8000),
            ScenarioStep::Acc("POS_1"),
            ScenarioStep::ZoneExit("POS_1"),
            ScenarioStep::ZoneEntry("GATE_1"),
            ScenarioStep::Wait(500),
            ScenarioStep::ZoneExit("GATE_1"), // Exit gate without crossing line
            ScenarioStep::Wait(500),
        ],
        expected: ExpectedOutcome::GateOpen, // Gate should still open on entry
        timeout_ms: 15000,
    },
    Scenario {
        name: "multiple_pos_zones",
        description: "Customer visits multiple POS zones before paying at second",
        steps: &[
            ScenarioStep::CreateTrack { in_store: false },
            ScenarioStep::ZoneEntry("POS_1"),
            ScenarioStep::Wait(3000), // Browse at POS_1
            ScenarioStep::ZoneExit("POS_1"),
            ScenarioStep::ZoneEntry("POS_2"),
            ScenarioStep::Wait(7500), // Must dwell >= 7000ms in the zone where ACC happens
            ScenarioStep::Acc("POS_2"),
            ScenarioStep::ZoneExit("POS_2"),
            ScenarioStep::ZoneEntry("GATE_1"),
            ScenarioStep::Wait(500),
        ],
        expected: ExpectedOutcome::GateOpen,
        timeout_ms: 20000,
    },
];

fn get_scenario(name: &str) -> Option<&'static Scenario> {
    SCENARIOS.iter().find(|s| s.name == name)
}

fn get_all_scenario_names() -> Vec<&'static str> {
    SCENARIOS.iter().map(|s| s.name).collect()
}

// ============================================================================
// State from MQTT (State Inspector)
// ============================================================================

#[derive(Debug, Clone, Default, Deserialize)]
struct PersonState {
    track_id: i64,
    #[serde(default)]
    dwell_ms: u64,
    #[serde(default)]
    authorized: bool,
    #[serde(default, rename = "current_zone")]
    _current_zone: Option<String>, // Deserialized but not yet displayed in UI
    #[serde(default)]
    acc_matched: bool,
}

#[derive(Debug, Clone, Default)]
struct GatewayState {
    // Person states from gateway/tracks
    persons: HashMap<i64, PersonState>,

    // Recent events from gateway/events
    recent_events: VecDeque<String>,

    // Gate status
    gate_status: String,
    last_gate_event: Option<String>,
    gate_open_count: u32,
    gate_blocked_count: u32,

    // Door state
    door_status: String,

    // Journey stats
    journeys_completed: u32,
    journeys_authorized: u32,
    journeys_unauthorized: u32,
}

// ============================================================================
// Scenario Runner State
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
enum ScenarioStatus {
    NotStarted,
    Running { step_index: usize, step_started: Instant },
    WaitingForOutcome { started: Instant },
    Passed,
    Failed(String),
}

#[derive(Debug, Clone)]
struct ScenarioRunner {
    scenario: Option<&'static Scenario>,
    status: ScenarioStatus,
    track_id: Option<i64>,
    observed_gate_open: bool,
    observed_gate_blocked: bool,
    observed_journey_auth: bool,
    observed_journey_unauth: bool,
    start_time: Option<Instant>,
}

impl Default for ScenarioRunner {
    fn default() -> Self {
        Self {
            scenario: None,
            status: ScenarioStatus::NotStarted,
            track_id: None,
            observed_gate_open: false,
            observed_gate_blocked: false,
            observed_journey_auth: false,
            observed_journey_unauth: false,
            start_time: None,
        }
    }
}

impl ScenarioRunner {
    fn start(&mut self, scenario: &'static Scenario) {
        self.scenario = Some(scenario);
        self.status = ScenarioStatus::Running { step_index: 0, step_started: Instant::now() };
        self.track_id = None;
        self.observed_gate_open = false;
        self.observed_gate_blocked = false;
        self.observed_journey_auth = false;
        self.observed_journey_unauth = false;
        self.start_time = Some(Instant::now());
    }

    fn check_outcome(&mut self) -> bool {
        let scenario = match self.scenario {
            Some(s) => s,
            None => return false,
        };

        let outcome_met = match scenario.expected {
            ExpectedOutcome::GateOpen => self.observed_gate_open,
            ExpectedOutcome::GateBlocked => self.observed_gate_blocked,
            ExpectedOutcome::JourneyAuthorized => self.observed_journey_auth,
            ExpectedOutcome::JourneyUnauthorized => self.observed_journey_unauth,
        };

        if outcome_met {
            self.status = ScenarioStatus::Passed;
            return true;
        }

        // Check for timeout
        if let Some(start) = self.start_time {
            if start.elapsed().as_millis() as u64 > scenario.timeout_ms {
                self.status = ScenarioStatus::Failed(format!(
                    "Timeout waiting for {:?}",
                    scenario.expected
                ));
                return true;
            }
        }

        // Check for wrong outcome
        match scenario.expected {
            ExpectedOutcome::GateOpen if self.observed_gate_blocked => {
                self.status = ScenarioStatus::Failed("Got GateBlocked, expected GateOpen".to_string());
                return true;
            }
            ExpectedOutcome::GateBlocked if self.observed_gate_open => {
                self.status = ScenarioStatus::Failed("Got GateOpen, expected GateBlocked".to_string());
                return true;
            }
            _ => {}
        }

        false
    }

    fn is_running(&self) -> bool {
        matches!(self.status, ScenarioStatus::Running { .. } | ScenarioStatus::WaitingForOutcome { .. })
    }

    fn progress(&self) -> (usize, usize) {
        let scenario = match self.scenario {
            Some(s) => s,
            None => return (0, 1),
        };
        match self.status {
            ScenarioStatus::Running { step_index, .. } => (step_index, scenario.steps.len()),
            ScenarioStatus::WaitingForOutcome { .. } => (scenario.steps.len(), scenario.steps.len()),
            ScenarioStatus::Passed | ScenarioStatus::Failed(_) => (scenario.steps.len(), scenario.steps.len()),
            ScenarioStatus::NotStarted => (0, scenario.steps.len()),
        }
    }
}

// ============================================================================
// Configuration
// ============================================================================

#[derive(Debug, Clone)]
struct SimConfig {
    emulate_xovis: bool,
    emulate_gate: bool,
    emulate_rs485: bool,
    emulate_acc: bool,
    mqtt_host: String,
    mqtt_port: u16,
    gate_tcp_addr: String,
    rs485_device: String,
    site_id: String,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            emulate_xovis: true,
            emulate_gate: true,
            emulate_rs485: true,
            emulate_acc: true,
            mqtt_host: "localhost".to_string(),
            mqtt_port: 1883,
            gate_tcp_addr: "127.0.0.1:8000".to_string(),
            rs485_device: "/dev/null".to_string(),
            site_id: "sim".to_string(),
        }
    }
}

// ============================================================================
// App State
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq)]
enum AppPhase {
    ConfigMenu,
    Running,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ViewMode {
    Logs,
    State,
    Scenarios,
}

#[derive(Debug, Clone)]
struct LogEntry {
    timestamp: chrono::DateTime<Utc>,
    source: LogSource,
    message: String,
    color: Color,
}

#[derive(Debug, Clone, Copy)]
enum LogSource {
    SimCtl,
    Gateway,
    MockGate,
    Mqtt,
    Scenario,
}

impl LogSource {
    fn prefix(&self) -> &'static str {
        match self {
            LogSource::SimCtl => "[SIM]",
            LogSource::Gateway => "[GW]",
            LogSource::MockGate => "[GATE]",
            LogSource::Mqtt => "[MQTT]",
            LogSource::Scenario => "[TEST]",
        }
    }

    fn color(&self) -> Color {
        match self {
            LogSource::SimCtl => Color::Cyan,
            LogSource::Gateway => Color::White,
            LogSource::MockGate => Color::Yellow,
            LogSource::Mqtt => Color::Green,
            LogSource::Scenario => Color::Magenta,
        }
    }
}

#[derive(Debug, Clone)]
struct SimTrack {
    track_id: i64,
    position: [f64; 3],
    current_zone: Option<String>,
    created_at: Instant,
}

impl SimTrack {
    fn new(track_id: i64, spawn_in_store: bool) -> Self {
        let position = if spawn_in_store { POS_IN_STORE } else { POS_ENTRANCE };
        Self {
            track_id,
            position,
            current_zone: None,
            created_at: Instant::now(),
        }
    }
}

struct AppState {
    phase: AppPhase,
    config: SimConfig,
    menu_selection: usize,
    view_mode: ViewMode,

    // Child processes
    gateway_process: Option<Child>,
    mock_gate_process: Option<Child>,

    // Logs
    logs: VecDeque<LogEntry>,

    // Track management
    tracks: HashMap<i64, SimTrack>,
    next_track_id: i64,
    selected_track_id: Option<i64>,

    // MQTT client state
    mqtt_connected: bool,

    // Gateway state (from MQTT)
    gateway_state: GatewayState,

    // Scenario runner
    scenario_runner: ScenarioRunner,
    scenario_selection: usize,

    // Stats
    events_sent: u64,
    gate_commands: u64,
}

impl AppState {
    fn new() -> Self {
        Self {
            phase: AppPhase::ConfigMenu,
            config: SimConfig::default(),
            menu_selection: 0,
            view_mode: ViewMode::Logs,
            gateway_process: None,
            mock_gate_process: None,
            logs: VecDeque::new(),
            tracks: HashMap::new(),
            next_track_id: 100,
            selected_track_id: None,
            mqtt_connected: false,
            gateway_state: GatewayState::default(),
            scenario_runner: ScenarioRunner::default(),
            scenario_selection: 0,
            events_sent: 0,
            gate_commands: 0,
        }
    }

    fn log(&mut self, source: LogSource, message: String) {
        self.logs.push_back(LogEntry {
            timestamp: Utc::now(),
            source,
            message,
            color: source.color(),
        });
        if self.logs.len() > MAX_LOG_ENTRIES {
            self.logs.pop_front();
        }
    }

    fn create_track(&mut self, in_store: bool) -> i64 {
        let track_id = self.next_track_id;
        self.next_track_id += 1;
        self.tracks.insert(track_id, SimTrack::new(track_id, in_store));
        self.selected_track_id = Some(track_id);
        track_id
    }

    fn delete_track(&mut self, track_id: i64) {
        self.tracks.remove(&track_id);
        if self.selected_track_id == Some(track_id) {
            self.selected_track_id = self.tracks.keys().next().copied();
        }
    }

    fn select_next_track(&mut self) {
        let ids: Vec<i64> = self.tracks.keys().copied().collect();
        if ids.is_empty() {
            return;
        }
        if let Some(current) = self.selected_track_id {
            if let Some(pos) = ids.iter().position(|&id| id == current) {
                self.selected_track_id = Some(ids[(pos + 1) % ids.len()]);
            }
        } else {
            self.selected_track_id = Some(ids[0]);
        }
    }

    fn select_prev_track(&mut self) {
        let ids: Vec<i64> = self.tracks.keys().copied().collect();
        if ids.is_empty() {
            return;
        }
        if let Some(current) = self.selected_track_id {
            if let Some(pos) = ids.iter().position(|&id| id == current) {
                let prev = if pos == 0 { ids.len() - 1 } else { pos - 1 };
                self.selected_track_id = Some(ids[prev]);
            }
        } else {
            self.selected_track_id = Some(ids[0]);
        }
    }

    fn process_mqtt_message(&mut self, topic: &str, payload: &[u8]) {
        let payload_str = match std::str::from_utf8(payload) {
            Ok(s) => s,
            Err(_) => return,
        };

        if topic.contains("gate") {
            // Parse gate events
            if payload_str.contains("cmd_sent") {
                self.gateway_state.gate_open_count += 1;
                self.gateway_state.gate_status = "OPEN CMD".to_string();
                self.gateway_state.last_gate_event = Some("cmd_sent".to_string());
                self.scenario_runner.observed_gate_open = true;
                self.gate_commands += 1;
            } else if payload_str.contains("blocked") {
                self.gateway_state.gate_blocked_count += 1;
                self.gateway_state.gate_status = "BLOCKED".to_string();
                self.gateway_state.last_gate_event = Some("blocked".to_string());
                self.scenario_runner.observed_gate_blocked = true;
            }

            // Parse door state
            if payload_str.contains("\"door\":") {
                if payload_str.contains("\"moving\"") {
                    self.gateway_state.door_status = "MOVING".to_string();
                } else if payload_str.contains("\"open\"") {
                    self.gateway_state.door_status = "OPEN".to_string();
                } else if payload_str.contains("\"closed\"") {
                    self.gateway_state.door_status = "CLOSED".to_string();
                }
            }

            let msg = if payload_str.len() > 80 {
                format!("{}...", &payload_str[..77])
            } else {
                payload_str.to_string()
            };
            self.log(LogSource::Mqtt, format!("← gate: {}", msg));
        } else if topic.contains("journey") {
            self.gateway_state.journeys_completed += 1;

            if payload_str.contains("\"authorized\":true") || payload_str.contains("\"outcome\":\"authorized\"") {
                self.gateway_state.journeys_authorized += 1;
                self.scenario_runner.observed_journey_auth = true;
            } else {
                self.gateway_state.journeys_unauthorized += 1;
                self.scenario_runner.observed_journey_unauth = true;
            }

            self.log(LogSource::Mqtt, "← journey completed".to_string());
        } else if topic.contains("events") {
            // Store recent events
            let summary = if payload_str.len() > 60 {
                format!("{}...", &payload_str[..57])
            } else {
                payload_str.to_string()
            };
            self.gateway_state.recent_events.push_back(summary);
            if self.gateway_state.recent_events.len() > MAX_STATE_EVENTS {
                self.gateway_state.recent_events.pop_front();
            }
        } else if topic.contains("tracks") || topic.contains("persons") {
            // Try to parse person state updates
            if let Ok(person) = serde_json::from_str::<PersonState>(payload_str) {
                self.gateway_state.persons.insert(person.track_id, person);
            }
        }
    }
}

// ============================================================================
// Config Generation
// ============================================================================

fn generate_config_toml(config: &SimConfig) -> String {
    let gate_addr = if config.emulate_gate {
        "127.0.0.1:8000".to_string()
    } else {
        config.gate_tcp_addr.clone()
    };

    let rs485_device = if config.emulate_rs485 {
        "/dev/null".to_string()
    } else {
        config.rs485_device.clone()
    };

    let mqtt_host = if config.emulate_xovis { "localhost" } else { &config.mqtt_host };
    let mqtt_port = if config.emulate_xovis { 1883 } else { config.mqtt_port };

    format!(
        r##"# Auto-generated simulation config
[site]
id = "{site_id}"

[mqtt]
host = "{mqtt_host}"
port = {mqtt_port}
topic = "#"

[broker]
bind_address = "0.0.0.0"
port = 1883

[gate]
mode = "tcp"
tcp_addr = "{gate_addr}"
http_url = ""
timeout_ms = 2000

[rs485]
device = "{rs485_device}"
baud = 19200
poll_interval_ms = 250

[zones]
pos_zones = [1001, 1002, 1003, 1004, 1005]
gate_zone = 1007
exit_line = 1006
entry_line = 1008

[zones.names]
1001 = "POS_1"
1002 = "POS_2"
1003 = "POS_3"
1004 = "POS_4"
1005 = "POS_5"
1006 = "EXIT_1"
1007 = "GATE_1"
1008 = "ENTRY_1"

[authorization]
min_dwell_ms = 7000

[metrics]
prometheus_port = 9091
interval_secs = 5

[acc]
listener_enabled = true
listener_port = 25803
flicker_merge_s = 10
recent_exit_window_ms = 3000

[acc.ip_to_pos]
"127.0.0.1" = "POS_1"

[mqtt_egress]
enabled = true
host = "localhost"
port = 1883
"##,
        site_id = config.site_id,
        mqtt_host = mqtt_host,
        mqtt_port = mqtt_port,
        gate_addr = gate_addr,
        rs485_device = rs485_device,
    )
}

// ============================================================================
// Process Management
// ============================================================================

fn find_binary(name: &str) -> Option<std::path::PathBuf> {
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(dir) = current_exe.parent() {
            let candidate = dir.join(name);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    let candidate = std::path::PathBuf::from("target/release").join(name);
    if candidate.exists() {
        return Some(candidate);
    }

    if let Ok(output) = Command::new("which").arg(name).output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(std::path::PathBuf::from(path));
            }
        }
    }

    None
}

fn spawn_gateway(config_path: &str) -> io::Result<Child> {
    let binary = find_binary("gateway").ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "gateway binary not found")
    })?;

    Command::new(binary)
        .args(["--config", config_path])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
}

fn spawn_mock_gate(gateway_url: &str) -> io::Result<Child> {
    let binary = find_binary("mock_cloudplus").ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "mock_cloudplus binary not found")
    })?;

    Command::new(binary)
        .args(["--port", "8000", "--gateway-url", gateway_url])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
}

// ============================================================================
// MQTT Message Building
// ============================================================================

fn zone_name_to_id(name: &str) -> i64 {
    match name {
        "POS_1" => ZONE_POS_1,
        "POS_2" => ZONE_POS_2,
        "POS_3" => ZONE_POS_3,
        "POS_4" => ZONE_POS_4,
        "POS_5" => ZONE_POS_5,
        "EXIT_1" => LINE_EXIT,
        "GATE_1" => ZONE_GATE,
        "ENTRY_1" => LINE_ENTRY,
        "APPROACH_1" => LINE_APPROACH,
        "STORE_1" => ZONE_STORE,
        _ => 0,
    }
}

fn build_xovis_message(
    event_type: &str,
    track_id: i64,
    geometry_id: Option<i64>,
    position: [f64; 3],
    direction: Option<&str>,
) -> String {
    let now = Utc::now();
    let timestamp = now.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    let mut event_attrs = json!({ "track_id": track_id });
    if let Some(gid) = geometry_id {
        event_attrs["geometry_id"] = json!(gid);
    }
    if let Some(dir) = direction {
        event_attrs["direction"] = json!(dir);
    }

    json!({
        "live_data": {
            "frames": [{
                "time": timestamp,
                "tracked_objects": [{
                    "track_id": track_id,
                    "type": "PERSON",
                    "position": position
                }],
                "events": [{
                    "type": event_type,
                    "attributes": event_attrs
                }]
            }]
        }
    })
    .to_string()
}

// ============================================================================
// Event Injection
// ============================================================================

async fn send_track_create(client: &AsyncClient, state: &mut AppState, track_id: i64) {
    if let Some(track) = state.tracks.get(&track_id) {
        let msg = build_xovis_message("TRACK_CREATE", track_id, None, track.position, None);
        if client.publish(XOVIS_TOPIC, QoS::AtLeastOnce, false, msg).await.is_ok() {
            state.log(LogSource::SimCtl, format!("→ TRACK_CREATE T{}", track_id));
            state.events_sent += 1;
        }
    }
}

async fn send_track_delete(client: &AsyncClient, state: &mut AppState, track_id: i64) {
    if let Some(track) = state.tracks.get(&track_id) {
        let msg = build_xovis_message("TRACK_DELETE", track_id, None, track.position, None);
        if client.publish(XOVIS_TOPIC, QoS::AtLeastOnce, false, msg).await.is_ok() {
            state.log(LogSource::SimCtl, format!("→ TRACK_DELETE T{}", track_id));
            state.events_sent += 1;
        }
    }
}

async fn send_zone_entry(client: &AsyncClient, state: &mut AppState, track_id: i64, zone: &str) {
    if let Some(track) = state.tracks.get_mut(&track_id) {
        let geometry_id = zone_name_to_id(zone);
        let msg = build_xovis_message("ZONE_ENTRY", track_id, Some(geometry_id), track.position, None);
        if client.publish(XOVIS_TOPIC, QoS::AtLeastOnce, false, msg).await.is_ok() {
            track.current_zone = Some(zone.to_string());
            state.log(LogSource::SimCtl, format!("→ ZONE_ENTRY T{} {}", track_id, zone));
            state.events_sent += 1;
        }
    }
}

async fn send_zone_exit(client: &AsyncClient, state: &mut AppState, track_id: i64, zone: &str) {
    if let Some(track) = state.tracks.get_mut(&track_id) {
        let geometry_id = zone_name_to_id(zone);
        let msg = build_xovis_message("ZONE_EXIT", track_id, Some(geometry_id), track.position, None);
        if client.publish(XOVIS_TOPIC, QoS::AtLeastOnce, false, msg).await.is_ok() {
            if track.current_zone.as_deref() == Some(zone) {
                track.current_zone = None;
            }
            state.log(LogSource::SimCtl, format!("→ ZONE_EXIT T{} {}", track_id, zone));
            state.events_sent += 1;
        }
    }
}

async fn send_line_cross(
    client: &AsyncClient,
    state: &mut AppState,
    track_id: i64,
    line: &str,
    direction: &str,
) {
    if let Some(track) = state.tracks.get(&track_id) {
        let geometry_id = zone_name_to_id(line);
        let msg = build_xovis_message("LINE_CROSS", track_id, Some(geometry_id), track.position, Some(direction));
        if client.publish(XOVIS_TOPIC, QoS::AtLeastOnce, false, msg).await.is_ok() {
            let dir_symbol = if direction == DIR_FORWARD { "→" } else { "←" };
            state.log(LogSource::SimCtl, format!("{} LINE_CROSS T{} {} ({})", dir_symbol, track_id, line, direction));
            state.events_sent += 1;
        }
    }
}

async fn send_gate_open_http(state: &mut AppState) {
    let client = reqwest::Client::new();
    match client.post("http://localhost:9091/gate/open").send().await {
        Ok(resp) if resp.status().is_success() => {
            state.log(LogSource::SimCtl, "→ HTTP gate/open command sent".to_string());
            state.gate_commands += 1;
        }
        Ok(resp) => {
            state.log(LogSource::SimCtl, format!("Gate open failed: {}", resp.status()));
        }
        Err(e) => {
            state.log(LogSource::SimCtl, format!("Gate open error: {}", e));
        }
    }
}

/// Send ACC event via HTTP /acc/simulate endpoint
/// This bypasses IP-to-POS mapping and directly specifies the POS zone
async fn send_acc_event(state: &mut AppState, pos_zone: &str) {
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:9091/acc/simulate?pos={}", pos_zone);

    match client.post(&url).send().await {
        Ok(resp) if resp.status().is_success() => {
            state.log(LogSource::SimCtl, format!("→ ACC {} ({})", pos_zone, resp.status()));
            state.events_sent += 1;
        }
        Ok(resp) => {
            state.log(
                LogSource::SimCtl,
                format!("ACC {} failed: {}", pos_zone, resp.status()),
            );
        }
        Err(e) => {
            state.log(
                LogSource::SimCtl,
                format!("ACC {} error: {}", pos_zone, e),
            );
        }
    }
}

// ============================================================================
// Scenario Execution
// ============================================================================

async fn execute_scenario_step(
    client: &AsyncClient,
    state: &mut AppState,
    step: ScenarioStep,
) {
    match step {
        ScenarioStep::CreateTrack { in_store } => {
            let track_id = state.create_track(in_store);
            state.scenario_runner.track_id = Some(track_id);
            state.log(LogSource::Scenario, format!("Creating track T{}", track_id));
            send_track_create(client, state, track_id).await;
        }
        ScenarioStep::DeleteTrack => {
            if let Some(tid) = state.scenario_runner.track_id {
                state.log(LogSource::Scenario, format!("Deleting track T{}", tid));
                send_track_delete(client, state, tid).await;
                state.delete_track(tid);
            }
        }
        ScenarioStep::ZoneEntry(zone) => {
            if let Some(tid) = state.scenario_runner.track_id {
                state.log(LogSource::Scenario, format!("Zone entry: {}", zone));
                send_zone_entry(client, state, tid, zone).await;
            }
        }
        ScenarioStep::ZoneExit(zone) => {
            if let Some(tid) = state.scenario_runner.track_id {
                state.log(LogSource::Scenario, format!("Zone exit: {}", zone));
                send_zone_exit(client, state, tid, zone).await;
            }
        }
        ScenarioStep::LineCross { line, forward } => {
            if let Some(tid) = state.scenario_runner.track_id {
                let dir = if forward { DIR_FORWARD } else { DIR_BACKWARD };
                state.log(LogSource::Scenario, format!("Line cross: {} ({})", line, dir));
                send_line_cross(client, state, tid, line, dir).await;
            }
        }
        ScenarioStep::Acc(pos_zone) => {
            state.log(LogSource::Scenario, format!("Sending ACC event for {}", pos_zone));
            send_acc_event(state, pos_zone).await;
        }
        ScenarioStep::Wait(ms) => {
            state.log(LogSource::Scenario, format!("Waiting {}ms", ms));
            // Wait is handled in the runner loop
        }
    }
}

// ============================================================================
// UI Rendering - Config Menu
// ============================================================================

fn draw_config_menu(f: &mut Frame, state: &AppState) {
    let area = f.area();

    let popup_width = 60;
    let popup_height = 18;
    let popup_area = centered_rect(popup_width, popup_height, area);

    f.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Simulation Controller - Configuration ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    f.render_widget(block, popup_area);

    let inner = Layout::default()
        .direction(Direction::Vertical)
        .margin(2)
        .constraints([Constraint::Min(10), Constraint::Length(3)])
        .split(popup_area);

    let menu_items = [
        ("Xovis Events", state.config.emulate_xovis, "Inject simulated sensor events"),
        ("Gate (CloudPlus)", state.config.emulate_gate, "Use mock TCP gate controller"),
        ("RS485 Door", state.config.emulate_rs485, "Simulate door state via HTTP"),
        ("ACC Terminal", state.config.emulate_acc, "Inject simulated payments"),
    ];

    let items: Vec<ListItem> = menu_items
        .iter()
        .enumerate()
        .map(|(i, (name, enabled, desc))| {
            let selected = i == state.menu_selection;
            let checkbox = if *enabled { "[✓]" } else { "[ ]" };
            let prefix = if selected { "▸ " } else { "  " };

            let style = if selected {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            ListItem::new(vec![
                Line::from(vec![
                    Span::raw(prefix),
                    Span::styled(checkbox, if *enabled { Style::default().fg(Color::Green) } else { Style::default().fg(Color::DarkGray) }),
                    Span::raw(" "),
                    Span::styled(*name, style),
                ]),
                Line::from(vec![
                    Span::raw("     "),
                    Span::styled(*desc, Style::default().fg(Color::DarkGray)),
                ]),
            ])
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner[0]);

    let help = Paragraph::new("↑↓ Navigate  Space=Toggle  Enter=Start  q=Quit")
        .style(Style::default().fg(Color::DarkGray))
        .alignment(ratatui::layout::Alignment::Center);
    f.render_widget(help, inner[1]);
}

// ============================================================================
// UI Rendering - Running
// ============================================================================

fn draw_running(f: &mut Frame, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3),  // Header
            Constraint::Min(10),    // Main content
            Constraint::Length(3),  // Help
        ])
        .split(f.area());

    draw_header(f, chunks[0], state);

    match state.view_mode {
        ViewMode::Logs => draw_logs_view(f, chunks[1], state),
        ViewMode::State => draw_state_view(f, chunks[1], state),
        ViewMode::Scenarios => draw_scenarios_view(f, chunks[1], state),
    }

    draw_help(f, chunks[2], state);
}

fn draw_header(f: &mut Frame, area: Rect, state: &AppState) {
    let mqtt = if state.mqtt_connected {
        Span::styled("MQTT:✓", Style::default().fg(Color::Green))
    } else {
        Span::styled("MQTT:✗", Style::default().fg(Color::Red))
    };

    let view_indicator = match state.view_mode {
        ViewMode::Logs => "[Logs]",
        ViewMode::State => "[State]",
        ViewMode::Scenarios => "[Tests]",
    };

    let scenario_status = if state.scenario_runner.is_running() {
        let (current, total) = state.scenario_runner.progress();
        format!(" | Running: {}/{}", current, total)
    } else {
        String::new()
    };

    let header = Paragraph::new(Line::from(vec![
        mqtt,
        Span::raw("  "),
        Span::styled(format!("Site: {}", state.config.site_id), Style::default().fg(Color::Cyan)),
        Span::raw("  "),
        Span::styled(view_indicator, Style::default().fg(Color::Yellow)),
        Span::raw("  "),
        Span::styled(format!("Sent: {}", state.events_sent), Style::default().fg(Color::White)),
        Span::raw("  "),
        Span::styled(format!("Gate: {} open, {} blocked",
            state.gateway_state.gate_open_count,
            state.gateway_state.gate_blocked_count
        ), Style::default().fg(Color::Yellow)),
        Span::styled(scenario_status, Style::default().fg(Color::Magenta)),
    ]))
    .block(
        Block::default()
            .title(" Gateway Simulation Controller (Tab=switch view, F1-F3) ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );

    f.render_widget(header, area);
}

fn draw_logs_view(f: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(area);

    // Left panel: Tracks + Quick Actions
    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[0]);

    draw_tracks_panel(f, left_chunks[0], state);
    draw_quick_actions(f, left_chunks[1], state);

    // Right panel: Logs
    draw_logs(f, chunks[1], state);
}

fn draw_state_view(f: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    // Left: Gateway state
    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Min(5)])
        .split(chunks[0]);

    // Gate/Door status
    let gate_status = vec![
        Line::from(vec![
            Span::raw("Gate Status: "),
            Span::styled(&state.gateway_state.gate_status,
                if state.gateway_state.gate_status == "OPEN CMD" {
                    Style::default().fg(Color::Green)
                } else if state.gateway_state.gate_status == "BLOCKED" {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default().fg(Color::DarkGray)
                }
            ),
        ]),
        Line::from(vec![
            Span::raw("Door Status: "),
            Span::styled(&state.gateway_state.door_status,
                if state.gateway_state.door_status == "OPEN" {
                    Style::default().fg(Color::Green)
                } else if state.gateway_state.door_status == "MOVING" {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default().fg(Color::DarkGray)
                }
            ),
        ]),
        Line::from(""),
        Line::from(format!("Gate Opens: {}", state.gateway_state.gate_open_count)),
        Line::from(format!("Gate Blocked: {}", state.gateway_state.gate_blocked_count)),
        Line::from(format!("Journeys: {} ({} auth, {} unauth)",
            state.gateway_state.journeys_completed,
            state.gateway_state.journeys_authorized,
            state.gateway_state.journeys_unauthorized,
        )),
    ];

    let gate_widget = Paragraph::new(gate_status).block(
        Block::default()
            .title(" Gate & Journey Stats ")
            .borders(Borders::ALL),
    );
    f.render_widget(gate_widget, left_chunks[0]);

    // Persons from gateway
    let person_items: Vec<ListItem> = state
        .gateway_state
        .persons
        .values()
        .map(|p| {
            let auth_indicator = if p.authorized { "✓" } else { "✗" };
            let acc_indicator = if p.acc_matched { "💳" } else { "" };
            ListItem::new(format!(
                "T{:<4} dwell:{:>5}ms auth:{} {}",
                p.track_id, p.dwell_ms, auth_indicator, acc_indicator
            ))
        })
        .collect();

    let persons_widget = List::new(person_items).block(
        Block::default()
            .title(" Gateway Person States ")
            .borders(Borders::ALL),
    );
    f.render_widget(persons_widget, left_chunks[1]);

    // Right: Recent events
    let event_items: Vec<ListItem> = state
        .gateway_state
        .recent_events
        .iter()
        .rev()
        .take(area.height.saturating_sub(2) as usize)
        .map(|e| ListItem::new(e.as_str()))
        .collect();

    let events_widget = List::new(event_items).block(
        Block::default()
            .title(" Recent Gateway Events ")
            .borders(Borders::ALL),
    );
    f.render_widget(events_widget, chunks[1]);
}

fn draw_scenarios_view(f: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    // Left: Scenario list
    let scenario_items: Vec<ListItem> = SCENARIOS
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let selected = i == state.scenario_selection;
            let prefix = if selected { "▸ " } else { "  " };

            let status_indicator = if state.scenario_runner.scenario.map(|r| r.name) == Some(s.name) {
                match &state.scenario_runner.status {
                    ScenarioStatus::Running { .. } => " [RUNNING]",
                    ScenarioStatus::WaitingForOutcome { .. } => " [WAITING]",
                    ScenarioStatus::Passed => " [PASSED ✓]",
                    ScenarioStatus::Failed(_) => " [FAILED ✗]",
                    ScenarioStatus::NotStarted => "",
                }
            } else {
                ""
            };

            let style = if selected {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            ListItem::new(vec![
                Line::from(vec![
                    Span::raw(prefix),
                    Span::styled(s.name, style),
                    Span::styled(status_indicator,
                        if status_indicator.contains("PASSED") {
                            Style::default().fg(Color::Green)
                        } else if status_indicator.contains("FAILED") {
                            Style::default().fg(Color::Red)
                        } else {
                            Style::default().fg(Color::Yellow)
                        }
                    ),
                ]),
                Line::from(vec![
                    Span::raw("   "),
                    Span::styled(s.description, Style::default().fg(Color::DarkGray)),
                ]),
            ])
        })
        .collect();

    let scenarios_widget = List::new(scenario_items).block(
        Block::default()
            .title(" Test Scenarios (Enter=run, ↑↓=select) ")
            .borders(Borders::ALL),
    );
    f.render_widget(scenarios_widget, chunks[0]);

    // Right: Scenario details and progress
    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(10), Constraint::Min(5)])
        .split(chunks[1]);

    // Selected scenario details
    if let Some(scenario) = SCENARIOS.get(state.scenario_selection) {
        let steps: Vec<Line> = scenario
            .steps
            .iter()
            .enumerate()
            .map(|(i, step)| {
                let (current_step, _) = state.scenario_runner.progress();
                let is_current = state.scenario_runner.scenario.map(|s| s.name) == Some(scenario.name)
                    && matches!(state.scenario_runner.status, ScenarioStatus::Running { .. })
                    && i == current_step;

                let prefix = if is_current { "▶ " } else { "  " };
                let step_str = match step {
                    ScenarioStep::CreateTrack { in_store } =>
                        format!("CreateTrack({})", if *in_store { "in_store" } else { "entrance" }),
                    ScenarioStep::DeleteTrack => "DeleteTrack".to_string(),
                    ScenarioStep::ZoneEntry(z) => format!("ZoneEntry({})", z),
                    ScenarioStep::ZoneExit(z) => format!("ZoneExit({})", z),
                    ScenarioStep::LineCross { line, forward } =>
                        format!("LineCross({}, {})", line, if *forward { "→" } else { "←" }),
                    ScenarioStep::Acc(pos) => format!("ACC {}", pos),
                    ScenarioStep::Wait(ms) => format!("Wait({}ms)", ms),
                };

                let style = if is_current {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default().fg(Color::DarkGray)
                };

                Line::from(Span::styled(format!("{}{}", prefix, step_str), style))
            })
            .collect();

        let expected_str = format!("Expected: {:?}", scenario.expected);
        let mut details = vec![
            Line::from(Span::styled(expected_str, Style::default().fg(Color::Cyan))),
            Line::from(""),
        ];
        details.extend(steps);

        let details_widget = Paragraph::new(details).block(
            Block::default()
                .title(format!(" {} ", scenario.name))
                .borders(Borders::ALL),
        );
        f.render_widget(details_widget, right_chunks[0]);
    }

    // Progress bar and result
    if state.scenario_runner.is_running() {
        let (current, total) = state.scenario_runner.progress();
        let ratio = current as f64 / total as f64;

        let gauge = Gauge::default()
            .block(Block::default().title(" Progress ").borders(Borders::ALL))
            .gauge_style(Style::default().fg(Color::Cyan))
            .ratio(ratio)
            .label(format!("{}/{}", current, total));
        f.render_widget(gauge, right_chunks[1]);
    } else if let ScenarioStatus::Failed(ref msg) = state.scenario_runner.status {
        let result = Paragraph::new(vec![
            Line::from(Span::styled("FAILED", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))),
            Line::from(""),
            Line::from(msg.as_str()),
        ]).block(Block::default().title(" Result ").borders(Borders::ALL));
        f.render_widget(result, right_chunks[1]);
    } else if state.scenario_runner.status == ScenarioStatus::Passed {
        let result = Paragraph::new(vec![
            Line::from(Span::styled("PASSED ✓", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))),
        ]).block(Block::default().title(" Result ").borders(Borders::ALL));
        f.render_widget(result, right_chunks[1]);
    }
}

fn draw_tracks_panel(f: &mut Frame, area: Rect, state: &AppState) {
    let track_items: Vec<ListItem> = state
        .tracks
        .values()
        .map(|track| {
            let selected = state.selected_track_id == Some(track.track_id);
            let prefix = if selected { "▸ " } else { "  " };
            let zone = track.current_zone.as_deref().unwrap_or("-");
            let elapsed = track.created_at.elapsed().as_secs();

            let style = if selected {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            ListItem::new(format!("{}T{:<4} {:>8} {}s", prefix, track.track_id, zone, elapsed))
                .style(style)
        })
        .collect();

    let tracks = List::new(track_items).block(
        Block::default()
            .title(" Tracks (t/T=new d=del) ")
            .borders(Borders::ALL),
    );
    f.render_widget(tracks, area);
}

fn draw_quick_actions(f: &mut Frame, area: Rect, _state: &AppState) {
    let actions = vec![
        Line::from(vec![
            Span::styled("1-5", Style::default().fg(Color::Cyan)),
            Span::raw(" POS "),
            Span::styled("!@#$%", Style::default().fg(Color::Cyan)),
            Span::raw(" exit"),
        ]),
        Line::from(vec![
            Span::styled("s/S", Style::default().fg(Color::Cyan)),
            Span::raw(" Store  "),
            Span::styled("g/G", Style::default().fg(Color::Cyan)),
            Span::raw(" Gate"),
        ]),
        Line::from(vec![
            Span::styled("i/I", Style::default().fg(Color::Cyan)),
            Span::raw(" Entry  "),
            Span::styled("p/P", Style::default().fg(Color::Cyan)),
            Span::raw(" Approach"),
        ]),
        Line::from(vec![
            Span::styled("e/E", Style::default().fg(Color::Cyan)),
            Span::raw(" Exit line"),
        ]),
        Line::from(vec![
            Span::styled("a", Style::default().fg(Color::Cyan)),
            Span::raw(" ACC  "),
            Span::styled("o", Style::default().fg(Color::Yellow)),
            Span::raw(" HTTP gate"),
        ]),
    ];

    let actions_widget = Paragraph::new(actions).block(
        Block::default()
            .title(" Quick Actions ")
            .borders(Borders::ALL),
    );
    f.render_widget(actions_widget, area);
}

fn draw_logs(f: &mut Frame, area: Rect, state: &AppState) {
    let log_items: Vec<ListItem> = state
        .logs
        .iter()
        .rev()
        .take(area.height.saturating_sub(2) as usize)
        .map(|entry| {
            let time = entry.timestamp.format("%H:%M:%S%.3f");
            let prefix = entry.source.prefix();
            ListItem::new(format!("{} {} {}", time, prefix, entry.message))
                .style(Style::default().fg(entry.color))
        })
        .collect();

    let logs = List::new(log_items).block(
        Block::default()
            .title(" Event Log ")
            .borders(Borders::ALL),
    );
    f.render_widget(logs, area);
}

fn draw_help(f: &mut Frame, area: Rect, state: &AppState) {
    let help_text = match state.view_mode {
        ViewMode::Logs => "Tab/F1-F3=view t/T=track 1-5=POS g=gate a=ACC e=exit o=open r=reset q=quit",
        ViewMode::State => "Tab/F1-F3=view | State view shows gateway internal state",
        ViewMode::Scenarios => "Tab/F1-F3=view ↑↓=select Enter=run | Run test scenarios",
    };

    let help = Paragraph::new(help_text)
        .style(Style::default().fg(Color::DarkGray))
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(help, area);
}

fn centered_rect(width: u16, height: u16, r: Rect) -> Rect {
    let x = r.x + (r.width.saturating_sub(width)) / 2;
    let y = r.y + (r.height.saturating_sub(height)) / 2;
    Rect::new(x, y, width.min(r.width), height.min(r.height))
}

// ============================================================================
// Test Mode (Non-Interactive)
// ============================================================================

async fn run_test_mode(test_arg: &str, verbose: bool) -> Result<i32, Box<dyn std::error::Error>> {
    let scenarios_to_run: Vec<&str> = if test_arg == "all" {
        get_all_scenario_names()
    } else {
        test_arg.split(',').map(|s| s.trim()).collect()
    };

    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║         Gateway Simulation - Test Mode                   ║");
    println!("╠══════════════════════════════════════════════════════════╣");
    println!("║ Running {} scenario(s)                                    ║", scenarios_to_run.len());
    println!("╚══════════════════════════════════════════════════════════╝");
    println!();

    // Validate scenarios
    for name in &scenarios_to_run {
        if get_scenario(name).is_none() {
            eprintln!("Unknown scenario: {}", name);
            eprintln!("Available: {}", get_all_scenario_names().join(", "));
            return Ok(1);
        }
    }

    // Setup
    let config = SimConfig::default();
    let config_content = generate_config_toml(&config);
    let config_path = "/tmp/simctl_gateway.toml";
    std::fs::write(config_path, &config_content)?;

    println!("[SETUP] Starting mock CloudPlus server...");
    let mut mock_gate = spawn_mock_gate("http://localhost:9091")?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    println!("[SETUP] Starting gateway...");
    let mut gateway = spawn_gateway(config_path)?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Connect MQTT
    println!("[SETUP] Connecting to MQTT...");
    let mut mqtt_options = MqttOptions::new("simctl-test", "localhost", 1883);
    mqtt_options.set_keep_alive(Duration::from_secs(30));
    let (client, mut eventloop) = AsyncClient::new(mqtt_options, 100);
    client.subscribe("gateway/#", QoS::AtLeastOnce).await?;

    // Shared state for MQTT handler
    let state = Arc::new(Mutex::new(AppState::new()));
    state.lock().await.config = config;
    state.lock().await.phase = AppPhase::Running;

    // Spawn MQTT handler
    let mqtt_state = state.clone();
    tokio::spawn(async move {
        loop {
            match eventloop.poll().await {
                Ok(MqttEvent::Incoming(Packet::ConnAck(_))) => {
                    let mut s = mqtt_state.lock().await;
                    s.mqtt_connected = true;
                }
                Ok(MqttEvent::Incoming(Packet::Publish(publish))) => {
                    let mut s = mqtt_state.lock().await;
                    s.process_mqtt_message(&publish.topic, &publish.payload);
                }
                Ok(_) => {}
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    });

    // Wait for MQTT connection
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Run scenarios
    let mut passed = 0;
    let mut failed = 0;
    let mut results: Vec<(String, bool, String)> = Vec::new();

    for scenario_name in scenarios_to_run {
        let scenario = get_scenario(scenario_name).unwrap();

        println!();
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("Running: {} - {}", scenario.name, scenario.description);
        println!("Expected: {:?}", scenario.expected);
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

        // Reset state for new scenario
        // NOTE: Don't reset next_track_id - we want unique IDs across scenarios
        // so the gateway doesn't see leftover state from previous scenarios
        {
            let mut s = state.lock().await;
            s.scenario_runner = ScenarioRunner::default();
            s.scenario_runner.start(scenario);
            s.tracks.clear();
            // s.next_track_id continues incrementing for unique IDs
        }

        // Execute steps
        for (step_idx, step) in scenario.steps.iter().enumerate() {
            if verbose {
                println!("  Step {}: {:?}", step_idx + 1, step);
            }

            // Handle Wait specially
            if let ScenarioStep::Wait(ms) = step {
                if verbose {
                    println!("    Waiting {}ms...", ms);
                }
                tokio::time::sleep(Duration::from_millis(*ms)).await;

                // Check outcome during wait
                let mut s = state.lock().await;
                if s.scenario_runner.check_outcome() {
                    break;
                }
                continue;
            }

            // Execute step
            {
                let mut s = state.lock().await;
                execute_scenario_step(&client, &mut s, *step).await;
            }

            // Small delay between steps
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Wait for outcome with timeout
        let start = Instant::now();
        let timeout = Duration::from_millis(scenario.timeout_ms);

        loop {
            {
                let mut s = state.lock().await;
                s.scenario_runner.status = ScenarioStatus::WaitingForOutcome { started: Instant::now() };
                if s.scenario_runner.check_outcome() {
                    break;
                }
            }

            if start.elapsed() > timeout {
                let mut s = state.lock().await;
                s.scenario_runner.status = ScenarioStatus::Failed(format!(
                    "Timeout after {}ms waiting for {:?}",
                    scenario.timeout_ms, scenario.expected
                ));
                break;
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Record result
        let s = state.lock().await;
        match &s.scenario_runner.status {
            ScenarioStatus::Passed => {
                println!("  ✓ PASSED");
                passed += 1;
                results.push((scenario.name.to_string(), true, "".to_string()));
            }
            ScenarioStatus::Failed(msg) => {
                println!("  ✗ FAILED: {}", msg);
                failed += 1;
                results.push((scenario.name.to_string(), false, msg.clone()));
            }
            _ => {
                println!("  ? UNKNOWN STATE");
                failed += 1;
                results.push((scenario.name.to_string(), false, "Unknown state".to_string()));
            }
        }

        // Delay between scenarios
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    // Cleanup
    let _ = gateway.kill();
    let _ = mock_gate.kill();

    // Print summary
    println!();
    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║                    TEST RESULTS                          ║");
    println!("╠══════════════════════════════════════════════════════════╣");

    for (name, success, msg) in &results {
        let status = if *success { "✓ PASS" } else { "✗ FAIL" };
        let detail = if msg.is_empty() { "" } else { &format!(": {}", msg) };
        println!("║ {:6} {:20} {:27} ║", status, name, detail);
    }

    println!("╠══════════════════════════════════════════════════════════╣");
    println!("║ Total: {} passed, {} failed                              ║", passed, failed);
    println!("╚══════════════════════════════════════════════════════════╝");

    if failed > 0 {
        Ok(1)
    } else {
        Ok(0)
    }
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Test mode
    if let Some(ref test_arg) = args.test {
        let exit_code = run_test_mode(test_arg, args.verbose).await?;
        std::process::exit(exit_code);
    }

    // Interactive TUI mode
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let state = Arc::new(Mutex::new(AppState::new()));
    let mut mqtt_client: Option<AsyncClient> = None;

    let tick_rate = Duration::from_millis(50);
    let mut last_tick = Instant::now();

    'main: loop {
        // Draw
        {
            let state_guard = state.lock().await;
            terminal.draw(|f| {
                match state_guard.phase {
                    AppPhase::ConfigMenu => draw_config_menu(f, &state_guard),
                    AppPhase::Running => draw_running(f, &state_guard),
                }
            })?;
        }

        // Process scenario steps if running
        if let Some(ref client) = mqtt_client {
            let mut state_guard = state.lock().await;
            if let ScenarioStatus::Running { step_index, step_started } = state_guard.scenario_runner.status {
                if let Some(scenario) = state_guard.scenario_runner.scenario {
                    if step_index < scenario.steps.len() {
                        let step = scenario.steps[step_index];

                        // Handle Wait step
                        if let ScenarioStep::Wait(ms) = step {
                            if step_started.elapsed().as_millis() as u64 >= ms {
                                // Move to next step
                                let next_idx = step_index + 1;
                                if next_idx < scenario.steps.len() {
                                    state_guard.scenario_runner.status = ScenarioStatus::Running {
                                        step_index: next_idx,
                                        step_started: Instant::now(),
                                    };
                                } else {
                                    state_guard.scenario_runner.status = ScenarioStatus::WaitingForOutcome {
                                        started: Instant::now(),
                                    };
                                }
                            }
                        } else {
                            // Execute non-wait step
                            execute_scenario_step(client, &mut state_guard, step).await;

                            let next_idx = step_index + 1;
                            if next_idx < scenario.steps.len() {
                                state_guard.scenario_runner.status = ScenarioStatus::Running {
                                    step_index: next_idx,
                                    step_started: Instant::now(),
                                };
                            } else {
                                state_guard.scenario_runner.status = ScenarioStatus::WaitingForOutcome {
                                    started: Instant::now(),
                                };
                            }
                        }
                    }
                }
            }

            // Check outcome if waiting
            if matches!(state_guard.scenario_runner.status, ScenarioStatus::WaitingForOutcome { .. }) {
                state_guard.scenario_runner.check_outcome();
            }
        }

        // Handle input
        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    let mut state_guard = state.lock().await;

                    match state_guard.phase {
                        AppPhase::ConfigMenu => {
                            match key.code {
                                KeyCode::Char('q') => break 'main,
                                KeyCode::Up => {
                                    if state_guard.menu_selection > 0 {
                                        state_guard.menu_selection -= 1;
                                    }
                                }
                                KeyCode::Down => {
                                    if state_guard.menu_selection < 3 {
                                        state_guard.menu_selection += 1;
                                    }
                                }
                                KeyCode::Char(' ') => {
                                    match state_guard.menu_selection {
                                        0 => state_guard.config.emulate_xovis = !state_guard.config.emulate_xovis,
                                        1 => state_guard.config.emulate_gate = !state_guard.config.emulate_gate,
                                        2 => state_guard.config.emulate_rs485 = !state_guard.config.emulate_rs485,
                                        3 => state_guard.config.emulate_acc = !state_guard.config.emulate_acc,
                                        _ => {}
                                    }
                                }
                                KeyCode::Enter => {
                                    let config_content = generate_config_toml(&state_guard.config);
                                    let config_path = "/tmp/simctl_gateway.toml";
                                    std::fs::write(config_path, &config_content)?;

                                    state_guard.log(LogSource::SimCtl, "Starting services...".to_string());

                                    if state_guard.config.emulate_gate {
                                        state_guard.log(LogSource::SimCtl, "Starting mock CloudPlus server...".to_string());
                                        match spawn_mock_gate("http://localhost:9091") {
                                            Ok(child) => {
                                                state_guard.mock_gate_process = Some(child);
                                                state_guard.log(LogSource::MockGate, "Mock gate started on port 8000".to_string());
                                            }
                                            Err(e) => {
                                                state_guard.log(LogSource::SimCtl, format!("Failed to start mock gate: {}", e));
                                            }
                                        }
                                        tokio::time::sleep(Duration::from_millis(500)).await;
                                    }

                                    state_guard.log(LogSource::SimCtl, "Starting gateway...".to_string());
                                    match spawn_gateway(config_path) {
                                        Ok(child) => {
                                            state_guard.gateway_process = Some(child);
                                            state_guard.log(LogSource::Gateway, "Gateway starting...".to_string());
                                        }
                                        Err(e) => {
                                            state_guard.log(LogSource::SimCtl, format!("Failed to start gateway: {}", e));
                                        }
                                    }

                                    tokio::time::sleep(Duration::from_secs(2)).await;

                                    let mqtt_host = if state_guard.config.emulate_xovis {
                                        "localhost"
                                    } else {
                                        &state_guard.config.mqtt_host
                                    };

                                    let mut mqtt_options = MqttOptions::new(
                                        "simctl",
                                        mqtt_host,
                                        if state_guard.config.emulate_xovis { 1883 } else { state_guard.config.mqtt_port },
                                    );
                                    mqtt_options.set_keep_alive(Duration::from_secs(30));

                                    let (client, mut eventloop) = AsyncClient::new(mqtt_options, 100);
                                    client.subscribe("gateway/#", QoS::AtLeastOnce).await?;

                                    mqtt_client = Some(client);

                                    let mqtt_state = state.clone();
                                    tokio::spawn(async move {
                                        loop {
                                            match eventloop.poll().await {
                                                Ok(MqttEvent::Incoming(Packet::ConnAck(_))) => {
                                                    let mut s = mqtt_state.lock().await;
                                                    s.mqtt_connected = true;
                                                    s.log(LogSource::Mqtt, "Connected to MQTT broker".to_string());
                                                }
                                                Ok(MqttEvent::Incoming(Packet::Publish(publish))) => {
                                                    let mut s = mqtt_state.lock().await;
                                                    s.process_mqtt_message(&publish.topic, &publish.payload);
                                                }
                                                Ok(_) => {}
                                                Err(e) => {
                                                    let mut s = mqtt_state.lock().await;
                                                    s.mqtt_connected = false;
                                                    s.log(LogSource::Mqtt, format!("Error: {}", e));
                                                    tokio::time::sleep(Duration::from_secs(1)).await;
                                                }
                                            }
                                        }
                                    });

                                    state_guard.phase = AppPhase::Running;
                                    state_guard.log(LogSource::SimCtl, "Ready! Use keyboard to inject events.".to_string());
                                }
                                _ => {}
                            }
                        }
                        AppPhase::Running => {
                            // View switching
                            match key.code {
                                KeyCode::Tab => {
                                    state_guard.view_mode = match state_guard.view_mode {
                                        ViewMode::Logs => ViewMode::State,
                                        ViewMode::State => ViewMode::Scenarios,
                                        ViewMode::Scenarios => ViewMode::Logs,
                                    };
                                }
                                KeyCode::F(1) => state_guard.view_mode = ViewMode::Logs,
                                KeyCode::F(2) => state_guard.view_mode = ViewMode::State,
                                KeyCode::F(3) => state_guard.view_mode = ViewMode::Scenarios,
                                KeyCode::Char('q') => break 'main,
                                _ => {}
                            }

                            // Scenario view controls
                            if state_guard.view_mode == ViewMode::Scenarios {
                                match key.code {
                                    KeyCode::Up => {
                                        if state_guard.scenario_selection > 0 {
                                            state_guard.scenario_selection -= 1;
                                        }
                                    }
                                    KeyCode::Down => {
                                        if state_guard.scenario_selection < SCENARIOS.len() - 1 {
                                            state_guard.scenario_selection += 1;
                                        }
                                    }
                                    KeyCode::Enter => {
                                        if !state_guard.scenario_runner.is_running() {
                                            let scenario = &SCENARIOS[state_guard.scenario_selection];
                                            state_guard.scenario_runner.start(scenario);
                                            state_guard.tracks.clear();
                                            state_guard.next_track_id = 100;
                                            state_guard.log(LogSource::Scenario, format!("Starting scenario: {}", scenario.name));
                                        }
                                    }
                                    _ => {}
                                }
                            }

                            // Event injection controls (only in Logs view and when emulating)
                            if state_guard.view_mode == ViewMode::Logs {
                                if let Some(ref client) = mqtt_client {
                                    match key.code {
                                        // Track management
                                        KeyCode::Char('t') if state_guard.config.emulate_xovis => {
                                            let track_id = state_guard.create_track(false);
                                            state_guard.log(LogSource::SimCtl, format!("Created T{} at entrance", track_id));
                                            send_track_create(client, &mut state_guard, track_id).await;
                                        }
                                        KeyCode::Char('T') if state_guard.config.emulate_xovis => {
                                            let track_id = state_guard.create_track(true);
                                            state_guard.log(LogSource::SimCtl, format!("Created T{} in-store", track_id));
                                            send_track_create(client, &mut state_guard, track_id).await;
                                        }
                                        KeyCode::Char('d') if state_guard.config.emulate_xovis => {
                                            if let Some(tid) = state_guard.selected_track_id {
                                                send_track_delete(client, &mut state_guard, tid).await;
                                                state_guard.delete_track(tid);
                                            }
                                        }
                                        KeyCode::Up => state_guard.select_prev_track(),
                                        KeyCode::Down => state_guard.select_next_track(),

                                        // POS zones
                                        KeyCode::Char('1') if state_guard.config.emulate_xovis => {
                                            if let Some(tid) = state_guard.selected_track_id {
                                                send_zone_entry(client, &mut state_guard, tid, "POS_1").await;
                                            }
                                        }
                                        KeyCode::Char('2') if state_guard.config.emulate_xovis => {
                                            if let Some(tid) = state_guard.selected_track_id {
                                                send_zone_entry(client, &mut state_guard, tid, "POS_2").await;
                                            }
                                        }
                                        KeyCode::Char('3') if state_guard.config.emulate_xovis => {
                                            if let Some(tid) = state_guard.selected_track_id {
                                                send_zone_entry(client, &mut state_guard, tid, "POS_3").await;
                                            }
                                        }
                                        KeyCode::Char('4') if state_guard.config.emulate_xovis => {
                                            if let Some(tid) = state_guard.selected_track_id {
                                                send_zone_entry(client, &mut state_guard, tid, "POS_4").await;
                                            }
                                        }
                                        KeyCode::Char('5') if state_guard.config.emulate_xovis => {
                                            if let Some(tid) = state_guard.selected_track_id {
                                                send_zone_entry(client, &mut state_guard, tid, "POS_5").await;
                                            }
                                        }
                                        KeyCode::Char('!') if state_guard.config.emulate_xovis => {
                                            if let Some(tid) = state_guard.selected_track_id {
                                                send_zone_exit(client, &mut state_guard, tid, "POS_1").await;
                                            }
                                        }
                                        KeyCode::Char('@') if state_guard.config.emulate_xovis => {
                                            if let Some(tid) = state_guard.selected_track_id {
                                                send_zone_exit(client, &mut state_guard, tid, "POS_2").await;
                                            }
                                        }
                                        KeyCode::Char('#') if state_guard.config.emulate_xovis => {
                                            if let Some(tid) = state_guard.selected_track_id {
                                                send_zone_exit(client, &mut state_guard, tid, "POS_3").await;
                                            }
                                        }
                                        KeyCode::Char('$') if state_guard.config.emulate_xovis => {
                                            if let Some(tid) = state_guard.selected_track_id {
                                                send_zone_exit(client, &mut state_guard, tid, "POS_4").await;
                                            }
                                        }
                                        KeyCode::Char('%') if state_guard.config.emulate_xovis => {
                                            if let Some(tid) = state_guard.selected_track_id {
                                                send_zone_exit(client, &mut state_guard, tid, "POS_5").await;
                                            }
                                        }

                                        // Store zone
                                        KeyCode::Char('s') if state_guard.config.emulate_xovis => {
                                            if let Some(tid) = state_guard.selected_track_id {
                                                send_zone_entry(client, &mut state_guard, tid, "STORE_1").await;
                                            }
                                        }
                                        KeyCode::Char('S') if state_guard.config.emulate_xovis => {
                                            if let Some(tid) = state_guard.selected_track_id {
                                                send_zone_exit(client, &mut state_guard, tid, "STORE_1").await;
                                            }
                                        }

                                        // Lines
                                        KeyCode::Char('i') if state_guard.config.emulate_xovis => {
                                            if let Some(tid) = state_guard.selected_track_id {
                                                send_line_cross(client, &mut state_guard, tid, "ENTRY_1", DIR_FORWARD).await;
                                            }
                                        }
                                        KeyCode::Char('I') if state_guard.config.emulate_xovis => {
                                            if let Some(tid) = state_guard.selected_track_id {
                                                send_line_cross(client, &mut state_guard, tid, "ENTRY_1", DIR_BACKWARD).await;
                                            }
                                        }
                                        KeyCode::Char('p') if state_guard.config.emulate_xovis => {
                                            if let Some(tid) = state_guard.selected_track_id {
                                                send_line_cross(client, &mut state_guard, tid, "APPROACH_1", DIR_FORWARD).await;
                                            }
                                        }
                                        KeyCode::Char('P') if state_guard.config.emulate_xovis => {
                                            if let Some(tid) = state_guard.selected_track_id {
                                                send_line_cross(client, &mut state_guard, tid, "APPROACH_1", DIR_BACKWARD).await;
                                            }
                                        }
                                        KeyCode::Char('e') if state_guard.config.emulate_xovis => {
                                            if let Some(tid) = state_guard.selected_track_id {
                                                send_line_cross(client, &mut state_guard, tid, "EXIT_1", DIR_FORWARD).await;
                                            }
                                        }
                                        KeyCode::Char('E') if state_guard.config.emulate_xovis => {
                                            if let Some(tid) = state_guard.selected_track_id {
                                                send_line_cross(client, &mut state_guard, tid, "EXIT_1", DIR_BACKWARD).await;
                                            }
                                        }

                                        // Gate zone
                                        KeyCode::Char('g') if state_guard.config.emulate_xovis => {
                                            if let Some(tid) = state_guard.selected_track_id {
                                                send_zone_entry(client, &mut state_guard, tid, "GATE_1").await;
                                            }
                                        }
                                        KeyCode::Char('G') if state_guard.config.emulate_xovis => {
                                            if let Some(tid) = state_guard.selected_track_id {
                                                send_zone_exit(client, &mut state_guard, tid, "GATE_1").await;
                                            }
                                        }

                                        // ACC - send to POS_1 by default (interactive)
                                        KeyCode::Char('a') => {
                                            send_acc_event(&mut state_guard, "POS_1").await;
                                        }

                                        // HTTP gate open
                                        KeyCode::Char('o') => {
                                            send_gate_open_http(&mut state_guard).await;
                                        }

                                        // Reset
                                        KeyCode::Char('r') => {
                                            state_guard.tracks.clear();
                                            state_guard.selected_track_id = None;
                                            state_guard.next_track_id = 100;
                                            state_guard.log(LogSource::SimCtl, "Simulation reset".to_string());
                                        }

                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }
    }

    // Cleanup
    {
        let mut state_guard = state.lock().await;
        if let Some(ref mut child) = state_guard.gateway_process {
            let _ = child.kill();
        }
        if let Some(ref mut child) = state_guard.mock_gate_process {
            let _ = child.kill();
        }
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    println!("Simulation controller stopped.");
    Ok(())
}
