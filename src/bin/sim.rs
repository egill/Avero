//! Gateway Simulation TUI - Event injection and monitoring
//!
//! Interactive terminal for simulating Xovis sensor events and monitoring gateway responses.
//!
//! Keyboard shortcuts:
//! - t: Create new track
//! - d: Delete selected track
//! - 1-5: Enter POS_1 to POS_5
//! - Shift+1-5 (!@#$%): Exit POS zones
//! - g: Enter gate zone
//! - a: Trigger ACC event for current POS
//! - e: Cross exit line
//! - s: Run happy path scenario
//! - r: Reset simulation
//! - Tab: Toggle view
//! - Up/Down: Select track
//! - q: Quit
//!
//! Usage:
//!   cargo run --bin sim -- --config config/sim.toml

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
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame, Terminal,
};
use rumqttc::{AsyncClient, Event as MqttEvent, MqttOptions, Packet, QoS};
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use std::io::{self, Write as IoWrite};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

// ============================================================================
// Constants
// ============================================================================

const MAX_LOG_ENTRIES: usize = 50;
const XOVIS_TOPIC: &str = "xovis/sim";

// Zone geometry IDs (matching config/sim.toml)
const POS_1: i64 = 1001;
const POS_2: i64 = 1002;
const POS_3: i64 = 1003;
const POS_4: i64 = 1004;
const POS_5: i64 = 1005;
const EXIT_LINE: i64 = 1006;
const GATE_ZONE: i64 = 1007;
const ENTRY_LINE: i64 = 1008;

// ============================================================================
// CLI Args
// ============================================================================

#[derive(Parser, Debug)]
#[command(name = "sim")]
#[command(about = "Gateway simulation TUI for local testing")]
struct Args {
    /// Config file path
    #[arg(short, long, default_value = "config/sim.toml")]
    config: String,

    /// MQTT broker host
    #[arg(long, default_value = "localhost")]
    mqtt_host: String,

    /// MQTT broker port
    #[arg(long, default_value = "1883")]
    mqtt_port: u16,

    /// ACC listener port
    #[arg(long, default_value = "25803")]
    acc_port: u16,

    /// Run scenario and exit
    #[arg(long)]
    scenario: Option<String>,
}

// ============================================================================
// Simulated Track
// ============================================================================

#[derive(Debug, Clone)]
struct SimTrack {
    track_id: i64,
    position: [f64; 3],
    current_zone: Option<String>,
    created_at: Instant,
    authorized: bool,
    _dwell_ms: u64, // TODO: implement dwell tracking in simulator
}

impl SimTrack {
    fn new(track_id: i64) -> Self {
        Self {
            track_id,
            position: [2.0, 1.0, 1.70], // Default position near entrance
            current_zone: None,
            created_at: Instant::now(),
            authorized: false,
            _dwell_ms: 0,
        }
    }
}

// ============================================================================
// Event Log Entry
// ============================================================================

#[derive(Debug, Clone)]
enum LogDirection {
    Sent,
    Received,
}

#[derive(Debug, Clone)]
struct LogEntry {
    timestamp: chrono::DateTime<Utc>,
    direction: LogDirection,
    message: String,
    color: Color,
}

// ============================================================================
// Scenario Runner
// ============================================================================

#[derive(Debug, Clone)]
enum ScenarioStep {
    CreateTrack,
    ZoneEntry(String),
    ZoneExit(String),
    LineCross(String),
    AccEvent(String),
    Wait(u64),
    DeleteTrack,
}

#[derive(Debug)]
struct ScenarioRunner {
    name: String,
    steps: Vec<ScenarioStep>,
    current_step: usize,
    track_id: Option<i64>,
    waiting_until: Option<Instant>,
}

impl ScenarioRunner {
    fn happy_path() -> Self {
        Self {
            name: "Happy Path".to_string(),
            steps: vec![
                ScenarioStep::CreateTrack,
                ScenarioStep::Wait(100),
                ScenarioStep::ZoneEntry("POS_1".to_string()),
                ScenarioStep::Wait(8000), // Dwell for 8 seconds
                ScenarioStep::AccEvent("POS_1".to_string()),
                ScenarioStep::Wait(100),
                ScenarioStep::ZoneExit("POS_1".to_string()),
                ScenarioStep::Wait(100),
                ScenarioStep::ZoneEntry("GATE_1".to_string()),
                ScenarioStep::Wait(3500), // Wait for gate to open and close
                ScenarioStep::LineCross("EXIT_1".to_string()),
                ScenarioStep::Wait(100),
                ScenarioStep::DeleteTrack,
            ],
            current_step: 0,
            track_id: None,
            waiting_until: None,
        }
    }

    fn no_payment() -> Self {
        Self {
            name: "No Payment".to_string(),
            steps: vec![
                ScenarioStep::CreateTrack,
                ScenarioStep::Wait(100),
                ScenarioStep::ZoneEntry("POS_1".to_string()),
                ScenarioStep::Wait(8000),
                ScenarioStep::ZoneExit("POS_1".to_string()),
                ScenarioStep::Wait(100),
                ScenarioStep::ZoneEntry("GATE_1".to_string()),
                ScenarioStep::Wait(1000), // Gate should stay closed
                ScenarioStep::DeleteTrack,
            ],
            current_step: 0,
            track_id: None,
            waiting_until: None,
        }
    }

    fn fast_exit() -> Self {
        Self {
            name: "Fast Exit".to_string(),
            steps: vec![
                ScenarioStep::CreateTrack,
                ScenarioStep::Wait(100),
                ScenarioStep::ZoneEntry("POS_1".to_string()),
                ScenarioStep::Wait(2000), // Only 2 seconds (< 7s threshold)
                ScenarioStep::AccEvent("POS_1".to_string()),
                ScenarioStep::Wait(100),
                ScenarioStep::ZoneExit("POS_1".to_string()),
                ScenarioStep::Wait(100),
                ScenarioStep::ZoneEntry("GATE_1".to_string()),
                ScenarioStep::Wait(1000),
                ScenarioStep::DeleteTrack,
            ],
            current_step: 0,
            track_id: None,
            waiting_until: None,
        }
    }

    fn is_complete(&self) -> bool {
        self.current_step >= self.steps.len()
    }

    fn current_step_name(&self) -> String {
        if self.current_step < self.steps.len() {
            match &self.steps[self.current_step] {
                ScenarioStep::CreateTrack => "Create track".to_string(),
                ScenarioStep::ZoneEntry(z) => format!("Enter {}", z),
                ScenarioStep::ZoneExit(z) => format!("Exit {}", z),
                ScenarioStep::LineCross(l) => format!("Cross {}", l),
                ScenarioStep::AccEvent(p) => format!("ACC {}", p),
                ScenarioStep::Wait(ms) => format!("Wait {}ms", ms),
                ScenarioStep::DeleteTrack => "Delete track".to_string(),
            }
        } else {
            "Complete".to_string()
        }
    }
}

// ============================================================================
// App State
// ============================================================================

#[derive(Debug)]
struct AppState {
    // Track management
    tracks: HashMap<i64, SimTrack>,
    next_track_id: i64,
    selected_track_id: Option<i64>,

    // Event log
    log: VecDeque<LogEntry>,

    // Scenario
    scenario: Option<ScenarioRunner>,

    // Connection state
    mqtt_connected: bool,

    // Stats
    events_sent: u64,
    events_received: u64,
    gate_commands_received: u64,
}

impl AppState {
    fn new() -> Self {
        Self {
            tracks: HashMap::new(),
            next_track_id: 100,
            selected_track_id: None,
            log: VecDeque::new(),
            scenario: None,
            mqtt_connected: false,
            events_sent: 0,
            events_received: 0,
            gate_commands_received: 0,
        }
    }

    fn log_sent(&mut self, msg: String) {
        self.log.push_back(LogEntry {
            timestamp: Utc::now(),
            direction: LogDirection::Sent,
            message: msg,
            color: Color::Cyan,
        });
        if self.log.len() > MAX_LOG_ENTRIES {
            self.log.pop_front();
        }
        self.events_sent += 1;
    }

    fn log_received(&mut self, msg: String, color: Color) {
        self.log.push_back(LogEntry {
            timestamp: Utc::now(),
            direction: LogDirection::Received,
            message: msg,
            color,
        });
        if self.log.len() > MAX_LOG_ENTRIES {
            self.log.pop_front();
        }
        self.events_received += 1;
    }

    fn create_track(&mut self) -> i64 {
        let track_id = self.next_track_id;
        self.next_track_id += 1;
        self.tracks.insert(track_id, SimTrack::new(track_id));
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
                let next_pos = (pos + 1) % ids.len();
                self.selected_track_id = Some(ids[next_pos]);
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
                let prev_pos = if pos == 0 { ids.len() - 1 } else { pos - 1 };
                self.selected_track_id = Some(ids[prev_pos]);
            }
        } else {
            self.selected_track_id = Some(ids[0]);
        }
    }
}

// ============================================================================
// MQTT Message Building
// ============================================================================

fn zone_name_to_geometry_id(name: &str) -> i64 {
    match name {
        "POS_1" => POS_1,
        "POS_2" => POS_2,
        "POS_3" => POS_3,
        "POS_4" => POS_4,
        "POS_5" => POS_5,
        "EXIT_1" => EXIT_LINE,
        "GATE_1" => GATE_ZONE,
        "ENTRY_1" => ENTRY_LINE,
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

    let mut event_attrs = json!({
        "track_id": track_id,
    });

    if let Some(gid) = geometry_id {
        event_attrs["geometry_id"] = json!(gid);
    }

    if let Some(dir) = direction {
        event_attrs["direction"] = json!(dir);
    }

    let frame = json!({
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
    });

    frame.to_string()
}

// ============================================================================
// Event Injection
// ============================================================================

async fn send_track_create(client: &AsyncClient, state: &mut AppState, track_id: i64) {
    if let Some(track) = state.tracks.get(&track_id) {
        let msg = build_xovis_message("TRACK_CREATE", track_id, None, track.position, None);
        if client.publish(XOVIS_TOPIC, QoS::AtLeastOnce, false, msg).await.is_ok() {
            state.log_sent(format!("TRACK_CREATE T{}", track_id));
        }
    }
}

async fn send_track_delete(client: &AsyncClient, state: &mut AppState, track_id: i64) {
    if let Some(track) = state.tracks.get(&track_id) {
        let msg = build_xovis_message("TRACK_DELETE", track_id, None, track.position, None);
        if client.publish(XOVIS_TOPIC, QoS::AtLeastOnce, false, msg).await.is_ok() {
            state.log_sent(format!("TRACK_DELETE T{}", track_id));
        }
    }
}

async fn send_zone_entry(client: &AsyncClient, state: &mut AppState, track_id: i64, zone: &str) {
    if let Some(track) = state.tracks.get_mut(&track_id) {
        let geometry_id = zone_name_to_geometry_id(zone);
        let msg = build_xovis_message("ZONE_ENTRY", track_id, Some(geometry_id), track.position, None);
        if client.publish(XOVIS_TOPIC, QoS::AtLeastOnce, false, msg).await.is_ok() {
            track.current_zone = Some(zone.to_string());
            state.log_sent(format!("ZONE_ENTRY T{} {}", track_id, zone));
        }
    }
}

async fn send_zone_exit(client: &AsyncClient, state: &mut AppState, track_id: i64, zone: &str) {
    if let Some(track) = state.tracks.get_mut(&track_id) {
        let geometry_id = zone_name_to_geometry_id(zone);
        let msg = build_xovis_message("ZONE_EXIT", track_id, Some(geometry_id), track.position, None);
        if client.publish(XOVIS_TOPIC, QoS::AtLeastOnce, false, msg).await.is_ok() {
            if track.current_zone.as_deref() == Some(zone) {
                track.current_zone = None;
            }
            state.log_sent(format!("ZONE_EXIT T{} {}", track_id, zone));
        }
    }
}

async fn send_line_cross(client: &AsyncClient, state: &mut AppState, track_id: i64, line: &str) {
    if let Some(track) = state.tracks.get(&track_id) {
        let geometry_id = zone_name_to_geometry_id(line);
        let msg = build_xovis_message("LINE_CROSS", track_id, Some(geometry_id), track.position, Some("forward"));
        if client.publish(XOVIS_TOPIC, QoS::AtLeastOnce, false, msg).await.is_ok() {
            state.log_sent(format!("LINE_CROSS T{} {}", track_id, line));
        }
    }
}

fn send_acc_event(state: &mut AppState, _pos: &str, acc_port: u16) {
    // Send ACC event via TCP
    if let Ok(mut stream) = TcpStream::connect(format!("127.0.0.1:{}", acc_port)) {
        let receipt_id = format!("SIM-{}", Utc::now().timestamp_millis());
        let msg = format!("ACC {}\n", receipt_id);
        if stream.write_all(msg.as_bytes()).is_ok() {
            state.log_sent(format!("ACC {}", receipt_id));
        }
    } else {
        state.log_sent("ACC failed (connection refused)".to_string());
    }
}

// ============================================================================
// Scenario Execution
// ============================================================================

async fn execute_scenario_step(
    client: &AsyncClient,
    state: &mut AppState,
    acc_port: u16,
) {
    // Extract scenario info without holding borrow
    let (step, track_id, scenario_name) = {
        let scenario = match &mut state.scenario {
            Some(s) => s,
            None => return,
        };

        // Check if waiting
        if let Some(until) = scenario.waiting_until {
            if Instant::now() < until {
                return;
            }
            scenario.waiting_until = None;
            scenario.current_step += 1;
        }

        if scenario.is_complete() {
            (None, None, Some(scenario.name.clone()))
        } else {
            let step = scenario.steps[scenario.current_step].clone();
            (Some(step), scenario.track_id, None)
        }
    };

    // Handle completion
    if let Some(name) = scenario_name {
        state.log_sent(format!("Scenario '{}' complete!", name));
        state.scenario = None;
        return;
    }

    let step = match step {
        Some(s) => s,
        None => return,
    };

    match step {
        ScenarioStep::CreateTrack => {
            let track_id = state.create_track();
            if let Some(s) = &mut state.scenario {
                s.track_id = Some(track_id);
            }
            send_track_create(client, state, track_id).await;
            if let Some(s) = &mut state.scenario {
                s.current_step += 1;
            }
        }
        ScenarioStep::ZoneEntry(zone) => {
            if let Some(tid) = track_id {
                send_zone_entry(client, state, tid, &zone).await;
            }
            if let Some(s) = &mut state.scenario {
                s.current_step += 1;
            }
        }
        ScenarioStep::ZoneExit(zone) => {
            if let Some(tid) = track_id {
                send_zone_exit(client, state, tid, &zone).await;
            }
            if let Some(s) = &mut state.scenario {
                s.current_step += 1;
            }
        }
        ScenarioStep::LineCross(line) => {
            if let Some(tid) = track_id {
                send_line_cross(client, state, tid, &line).await;
            }
            if let Some(s) = &mut state.scenario {
                s.current_step += 1;
            }
        }
        ScenarioStep::AccEvent(pos) => {
            send_acc_event(state, &pos, acc_port);
            if let Some(s) = &mut state.scenario {
                s.current_step += 1;
            }
        }
        ScenarioStep::Wait(ms) => {
            if let Some(s) = &mut state.scenario {
                s.waiting_until = Some(Instant::now() + Duration::from_millis(ms));
            }
        }
        ScenarioStep::DeleteTrack => {
            if let Some(tid) = track_id {
                send_track_delete(client, state, tid).await;
                state.delete_track(tid);
            }
            if let Some(s) = &mut state.scenario {
                s.current_step += 1;
            }
        }
    }
}

// ============================================================================
// UI Rendering
// ============================================================================

fn draw_ui(f: &mut Frame, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3),  // Header
            Constraint::Min(10),    // Main content
            Constraint::Length(12), // Log
            Constraint::Length(3),  // Help
        ])
        .split(f.area());

    draw_header(f, chunks[0], state);
    draw_main(f, chunks[1], state);
    draw_log(f, chunks[2], state);
    draw_help(f, chunks[3]);
}

fn draw_header(f: &mut Frame, area: Rect, state: &AppState) {
    let mqtt_status = if state.mqtt_connected {
        Span::styled("MQTT: ✓", Style::default().fg(Color::Green))
    } else {
        Span::styled("MQTT: ✗", Style::default().fg(Color::Red))
    };

    let tracks_count = Span::raw(format!("  Tracks: {}  ", state.tracks.len()));
    let sent = Span::raw(format!("Sent: {}  ", state.events_sent));
    let recv = Span::raw(format!("Recv: {}  ", state.events_received));
    let gate = Span::styled(
        format!("Gate cmds: {}", state.gate_commands_received),
        Style::default().fg(Color::Yellow),
    );

    let header = Paragraph::new(Line::from(vec![mqtt_status, tracks_count, sent, recv, gate]))
        .block(
            Block::default()
                .title(" Gateway Simulation TUI ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        );

    f.render_widget(header, area);
}

fn draw_main(f: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    draw_tracks(f, chunks[0], state);
    draw_scenario(f, chunks[1], state);
}

fn draw_tracks(f: &mut Frame, area: Rect, state: &AppState) {
    let items: Vec<ListItem> = state
        .tracks
        .values()
        .map(|track| {
            let selected = state.selected_track_id == Some(track.track_id);
            let prefix = if selected { "▸ " } else { "  " };
            let zone = track.current_zone.as_deref().unwrap_or("-");
            let auth = if track.authorized { "✓" } else { " " };
            let elapsed = track.created_at.elapsed().as_secs();

            let style = if selected {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            ListItem::new(format!(
                "{}T{:<4} {:>8} [{}] {}s",
                prefix, track.track_id, zone, auth, elapsed
            ))
            .style(style)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .title(" Tracks (↑↓ select, t=new, d=del) ")
            .borders(Borders::ALL),
    );

    f.render_widget(list, area);
}

fn draw_scenario(f: &mut Frame, area: Rect, state: &AppState) {
    let content = if let Some(scenario) = &state.scenario {
        let progress = format!(
            "Step {}/{}: {}",
            scenario.current_step + 1,
            scenario.steps.len(),
            scenario.current_step_name()
        );
        let bar_len = (scenario.current_step * 20) / scenario.steps.len().max(1);
        let bar = format!("[{}{}]", "█".repeat(bar_len), "░".repeat(20 - bar_len));

        vec![
            Line::from(Span::styled(
                format!("Running: {}", scenario.name),
                Style::default().fg(Color::Green),
            )),
            Line::from(""),
            Line::from(progress),
            Line::from(bar),
        ]
    } else {
        vec![
            Line::from("No scenario running"),
            Line::from(""),
            Line::from(Span::styled("s = Happy Path", Style::default().fg(Color::Cyan))),
            Line::from(Span::styled("n = No Payment", Style::default().fg(Color::Cyan))),
            Line::from(Span::styled("f = Fast Exit", Style::default().fg(Color::Cyan))),
        ]
    };

    let para = Paragraph::new(content).block(
        Block::default()
            .title(" Scenarios ")
            .borders(Borders::ALL),
    );

    f.render_widget(para, area);
}

fn draw_log(f: &mut Frame, area: Rect, state: &AppState) {
    let items: Vec<ListItem> = state
        .log
        .iter()
        .rev()
        .take(10)
        .map(|entry| {
            let time = entry.timestamp.format("%H:%M:%S%.3f");
            let arrow = match entry.direction {
                LogDirection::Sent => "→",
                LogDirection::Received => "←",
            };
            ListItem::new(format!("{} {} {}", time, arrow, entry.message))
                .style(Style::default().fg(entry.color))
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .title(" Event Log ")
            .borders(Borders::ALL),
    );

    f.render_widget(list, area);
}

fn draw_help(f: &mut Frame, area: Rect) {
    let help_text = "t=track  d=delete  1-5=POS enter  !@#$%=POS exit  g=gate  a=ACC  e=exit  s/n/f=scenario  q=quit";
    let help = Paragraph::new(help_text)
        .style(Style::default().fg(Color::DarkGray))
        .block(Block::default().borders(Borders::ALL));

    f.render_widget(help, area);
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Setup MQTT client
    let mut mqtt_options = MqttOptions::new("sim-tui", &args.mqtt_host, args.mqtt_port);
    mqtt_options.set_keep_alive(Duration::from_secs(30));

    let (client, mut eventloop) = AsyncClient::new(mqtt_options, 100);

    // Subscribe to gateway topics
    client.subscribe("gateway/#", QoS::AtLeastOnce).await?;

    // Shared state
    let state = Arc::new(Mutex::new(AppState::new()));

    // MQTT event handler
    let mqtt_state = state.clone();
    let mqtt_handle = tokio::spawn(async move {
        loop {
            match eventloop.poll().await {
                Ok(MqttEvent::Incoming(Packet::ConnAck(_))) => {
                    mqtt_state.lock().await.mqtt_connected = true;
                }
                Ok(MqttEvent::Incoming(Packet::Publish(publish))) => {
                    let topic = publish.topic.as_str();
                    if let Ok(payload) = std::str::from_utf8(&publish.payload) {
                        let mut state = mqtt_state.lock().await;

                        // Parse and log based on topic
                        let color = if topic.contains("gate") {
                            if payload.contains("cmd_sent") || payload.contains("open") {
                                state.gate_commands_received += 1;
                            }
                            Color::Yellow
                        } else if topic.contains("acc") {
                            Color::Green
                        } else if topic.contains("journey") {
                            Color::Magenta
                        } else {
                            Color::White
                        };

                        // Truncate long messages
                        let msg = if payload.len() > 80 {
                            format!("{}: {}...", topic, &payload[..77])
                        } else {
                            format!("{}: {}", topic, payload)
                        };
                        state.log_received(msg, color);
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    let mut state = mqtt_state.lock().await;
                    state.mqtt_connected = false;
                    state.log_received(format!("MQTT error: {}", e), Color::Red);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    });

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Main loop
    let tick_rate = Duration::from_millis(50);
    let mut last_tick = Instant::now();

    loop {
        // Draw
        {
            let state = state.lock().await;
            terminal.draw(|f| draw_ui(f, &state))?;
        }

        // Handle input
        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    let mut state = state.lock().await;

                    match key.code {
                        KeyCode::Char('q') => break,

                        // Track management
                        KeyCode::Char('t') => {
                            let track_id = state.create_track();
                            send_track_create(&client, &mut state, track_id).await;
                        }
                        KeyCode::Char('d') => {
                            if let Some(tid) = state.selected_track_id {
                                send_track_delete(&client, &mut state, tid).await;
                                state.delete_track(tid);
                            }
                        }
                        KeyCode::Up => state.select_prev_track(),
                        KeyCode::Down => state.select_next_track(),

                        // Zone entry (1-5)
                        KeyCode::Char('1') => {
                            if let Some(tid) = state.selected_track_id {
                                send_zone_entry(&client, &mut state, tid, "POS_1").await;
                            }
                        }
                        KeyCode::Char('2') => {
                            if let Some(tid) = state.selected_track_id {
                                send_zone_entry(&client, &mut state, tid, "POS_2").await;
                            }
                        }
                        KeyCode::Char('3') => {
                            if let Some(tid) = state.selected_track_id {
                                send_zone_entry(&client, &mut state, tid, "POS_3").await;
                            }
                        }
                        KeyCode::Char('4') => {
                            if let Some(tid) = state.selected_track_id {
                                send_zone_entry(&client, &mut state, tid, "POS_4").await;
                            }
                        }
                        KeyCode::Char('5') => {
                            if let Some(tid) = state.selected_track_id {
                                send_zone_entry(&client, &mut state, tid, "POS_5").await;
                            }
                        }

                        // Zone exit (Shift+1-5 = !@#$%)
                        KeyCode::Char('!') => {
                            if let Some(tid) = state.selected_track_id {
                                send_zone_exit(&client, &mut state, tid, "POS_1").await;
                            }
                        }
                        KeyCode::Char('@') => {
                            if let Some(tid) = state.selected_track_id {
                                send_zone_exit(&client, &mut state, tid, "POS_2").await;
                            }
                        }
                        KeyCode::Char('#') => {
                            if let Some(tid) = state.selected_track_id {
                                send_zone_exit(&client, &mut state, tid, "POS_3").await;
                            }
                        }
                        KeyCode::Char('$') => {
                            if let Some(tid) = state.selected_track_id {
                                send_zone_exit(&client, &mut state, tid, "POS_4").await;
                            }
                        }
                        KeyCode::Char('%') => {
                            if let Some(tid) = state.selected_track_id {
                                send_zone_exit(&client, &mut state, tid, "POS_5").await;
                            }
                        }

                        // Gate zone
                        KeyCode::Char('g') => {
                            if let Some(tid) = state.selected_track_id {
                                send_zone_entry(&client, &mut state, tid, "GATE_1").await;
                            }
                        }

                        // ACC event
                        KeyCode::Char('a') => {
                            send_acc_event(&mut state, "POS_1", args.acc_port);
                        }

                        // Exit line
                        KeyCode::Char('e') => {
                            if let Some(tid) = state.selected_track_id {
                                send_line_cross(&client, &mut state, tid, "EXIT_1").await;
                            }
                        }

                        // Scenarios
                        KeyCode::Char('s') => {
                            state.scenario = Some(ScenarioRunner::happy_path());
                            state.log_sent("Starting Happy Path scenario".to_string());
                        }
                        KeyCode::Char('n') => {
                            state.scenario = Some(ScenarioRunner::no_payment());
                            state.log_sent("Starting No Payment scenario".to_string());
                        }
                        KeyCode::Char('f') => {
                            state.scenario = Some(ScenarioRunner::fast_exit());
                            state.log_sent("Starting Fast Exit scenario".to_string());
                        }

                        // Reset
                        KeyCode::Char('r') => {
                            state.tracks.clear();
                            state.selected_track_id = None;
                            state.scenario = None;
                            state.next_track_id = 100;
                            state.log_sent("Simulation reset".to_string());
                        }

                        _ => {}
                    }
                }
            }
        }

        // Execute scenario steps
        if last_tick.elapsed() >= tick_rate {
            let mut state = state.lock().await;
            execute_scenario_step(&client, &mut state, args.acc_port).await;
            last_tick = Instant::now();
        }
    }

    // Cleanup
    mqtt_handle.abort();
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}
