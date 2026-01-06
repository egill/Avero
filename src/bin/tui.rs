//! Gateway TUI - Live monitoring dashboard for the checkout gateway
//!
//! Subscribes to MQTT topics and displays:
//! - POS occupancy (who's in each zone, dwell times, authorization)
//! - Track lifecycle (creates, stitches, pending, lost)
//! - Gate activity (commands, door state)
//! - System metrics (events/sec, latency, tracks)

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
    widgets::{Block, Borders, List, ListItem, Paragraph, Gauge, Row, Table},
    Frame, Terminal,
};
use rumqttc::{AsyncClient, Event as MqttEvent, MqttOptions, Packet, QoS};
use serde::Deserialize;
use std::collections::{HashMap, VecDeque};
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Maximum events to keep in history
const MAX_TRACK_EVENTS: usize = 15;
const MAX_GATE_EVENTS: usize = 10;
const MAX_ACC_EVENTS: usize = 10;

/// Zone event from gateway/events topic
#[derive(Debug, Clone, Deserialize)]
struct ZoneEvent {
    tid: i32,
    t: String,
    z: Option<String>,
    #[allow(dead_code)]
    ts: u64,
    auth: bool,
    dwell_ms: Option<u64>,
    total_dwell_ms: Option<u64>,
}

/// Track event from gateway/tracks topic
#[derive(Debug, Clone, Deserialize)]
struct TrackEvent {
    #[allow(dead_code)]
    ts: u64,
    t: String,  // create, stitch, pending, reentry, lost
    tid: i32,
    prev_tid: Option<i32>,
    auth: bool,
    dwell_ms: u64,
    stitch_dist_cm: Option<u64>,
    stitch_time_ms: Option<u64>,
    parent_jid: Option<String>,
}

/// Metrics from gateway/metrics topic
#[derive(Debug, Clone, Deserialize, Default)]
struct Metrics {
    #[allow(dead_code)]
    ts: u64,
    events_total: u64,
    events_per_sec: f64,
    avg_latency_us: u64,
    max_latency_us: u64,
    active_tracks: usize,
    authorized_tracks: usize,
    gate_cmds: u64,
}

/// Gate state from gateway/gate topic
#[derive(Debug, Clone, Deserialize)]
struct GateState {
    ts: u64,
    state: String,
    tid: Option<i32>,
    src: String,
}

/// Gate flow timeline - tracks timing of a single gate passage
#[derive(Debug, Clone, Default)]
struct GateFlow {
    tid: Option<i32>,
    // Gate zone entry
    gate_entry_event_ts: Option<u64>,   // Camera event timestamp
    gate_entry_received: Option<Instant>, // When we received it
    // Command sent
    cmd_sent_ts: Option<u64>,
    cmd_sent_received: Option<Instant>,
    // Door movement
    moving_ts: Option<u64>,
    moving_received: Option<Instant>,
    // Door open
    open_ts: Option<u64>,
    open_received: Option<Instant>,
    // Exit
    exit_ts: Option<u64>,
    exit_received: Option<Instant>,
}

/// Journey from gateway/journeys topic
#[derive(Debug, Clone, Deserialize)]
struct Journey {
    #[allow(dead_code)]
    jid: String,
    #[allow(dead_code)]
    pid: String,
    auth: bool,
    #[allow(dead_code)]
    dwell: u64,
    #[allow(dead_code)]
    out: String,
}

/// ACC event from gateway/acc topic
#[derive(Debug, Clone, Deserialize)]
struct AccEvent {
    #[allow(dead_code)]
    ts: u64,
    t: String,  // matched, unmatched
    ip: String,
    pos: Option<String>,
    tid: Option<i32>,
    #[allow(dead_code)]
    dwell_ms: Option<u64>,
    gate_zone: Option<String>,
    gate_entry_ts: Option<u64>,
    delta_ms: Option<u64>,
    gate_cmd_at: Option<u64>,
}

/// POS zone occupancy state
#[derive(Debug, Clone, Default)]
struct PosZone {
    name: String,
    occupant_tid: Option<i32>,
    occupant_auth: bool,
    occupant_dwell_ms: u64,
    entered_at: Option<Instant>,
}

/// Dashboard state shared between MQTT handler and UI
#[derive(Debug, Default)]
struct DashboardState {
    // POS Occupancy - map of zone name to occupancy state
    pos_zones: HashMap<String, PosZone>,

    // Track lifecycle events
    track_events: VecDeque<TrackEvent>,

    // Gate activity
    gate_events: VecDeque<GateState>,
    last_gate_state: String,
    gate_cmd_count: u64,
    current_gate_flow: GateFlow,
    last_gate_flow: Option<GateFlow>,

    // ACC activity
    acc_events: VecDeque<AccEvent>,
    acc_matched: u64,
    acc_unmatched: u64,
    acc_late: u64,
    acc_no_journey: u64,

    // Metrics
    metrics: Metrics,

    // Journeys completed
    journeys_completed: u64,
    journeys_authorized: u64,
    last_journey: Option<Journey>,

    // Connection status
    connected: bool,
    last_message: Option<Instant>,
}

impl DashboardState {
    fn new() -> Self {
        let mut state = Self::default();
        // Initialize POS zones
        for i in 1..=5 {
            let name = format!("POS_{}", i);
            state.pos_zones.insert(name.clone(), PosZone {
                name,
                ..Default::default()
            });
        }
        state
    }

    fn handle_zone_event(&mut self, event: ZoneEvent) {
        let now = Instant::now();

        if let Some(zone_name) = &event.z {
            // Track POS zones
            if zone_name.starts_with("POS_") {
                if let Some(zone) = self.pos_zones.get_mut(zone_name) {
                    match event.t.as_str() {
                        "zone_entry" => {
                            zone.occupant_tid = Some(event.tid);
                            zone.occupant_auth = event.auth;
                            zone.occupant_dwell_ms = event.total_dwell_ms.unwrap_or(0);
                            zone.entered_at = Some(now);
                        }
                        "zone_exit" => {
                            if zone.occupant_tid == Some(event.tid) {
                                zone.occupant_tid = None;
                                zone.occupant_auth = false;
                                zone.occupant_dwell_ms = 0;
                                zone.entered_at = None;
                            }
                        }
                        _ => {}
                    }
                }
            }

            // Track GATE zone entry for timing
            if zone_name == "GATE_1" && event.t == "zone_entry" {
                // Save previous flow if it had activity
                if self.current_gate_flow.gate_entry_received.is_some() {
                    self.last_gate_flow = Some(self.current_gate_flow.clone());
                }
                // Start new flow
                self.current_gate_flow = GateFlow {
                    tid: Some(event.tid),
                    gate_entry_event_ts: Some(event.ts),
                    gate_entry_received: Some(now),
                    ..Default::default()
                };
            }

            // Track EXIT line crossing
            if zone_name == "EXIT_1" && event.t == "line_cross" {
                if self.current_gate_flow.tid == Some(event.tid) {
                    self.current_gate_flow.exit_ts = Some(event.ts);
                    self.current_gate_flow.exit_received = Some(now);
                }
            }
        }
        self.last_message = Some(now);
    }

    fn handle_track_event(&mut self, event: TrackEvent) {
        // Clean up POS zones when tracks are lost or stitched
        match event.t.as_str() {
            "lost" => {
                // Remove track from any POS zone it occupied
                for zone in self.pos_zones.values_mut() {
                    if zone.occupant_tid == Some(event.tid) {
                        zone.occupant_tid = None;
                        zone.occupant_auth = false;
                        zone.occupant_dwell_ms = 0;
                        zone.entered_at = None;
                    }
                }
            }
            "stitch" => {
                // Transfer occupancy from old track to new track
                if let Some(prev_tid) = event.prev_tid {
                    for zone in self.pos_zones.values_mut() {
                        if zone.occupant_tid == Some(prev_tid) {
                            zone.occupant_tid = Some(event.tid);
                            zone.occupant_auth = event.auth;
                            // Keep the dwell time accumulating
                        }
                    }
                }
            }
            "pending" => {
                // Track is pending stitch - clear from zones (it's gone temporarily)
                for zone in self.pos_zones.values_mut() {
                    if zone.occupant_tid == Some(event.tid) {
                        zone.occupant_tid = None;
                        zone.occupant_auth = false;
                        zone.occupant_dwell_ms = 0;
                        zone.entered_at = None;
                    }
                }
            }
            _ => {}
        }

        self.track_events.push_front(event);
        if self.track_events.len() > MAX_TRACK_EVENTS {
            self.track_events.pop_back();
        }
        self.last_message = Some(Instant::now());
    }

    fn handle_gate_event(&mut self, event: GateState) {
        let now = Instant::now();
        self.last_gate_state = event.state.clone();

        // Track timing in current gate flow
        match event.state.as_str() {
            "cmd_sent" => {
                self.gate_cmd_count += 1;
                self.current_gate_flow.cmd_sent_ts = Some(event.ts);
                self.current_gate_flow.cmd_sent_received = Some(now);
                // Associate track if not set
                if self.current_gate_flow.tid.is_none() {
                    self.current_gate_flow.tid = event.tid;
                }
            }
            "moving" => {
                self.current_gate_flow.moving_ts = Some(event.ts);
                self.current_gate_flow.moving_received = Some(now);
            }
            "open" => {
                self.current_gate_flow.open_ts = Some(event.ts);
                self.current_gate_flow.open_received = Some(now);
            }
            "closed" => {
                // Gate cycle complete - save flow if we had a cmd
                if self.current_gate_flow.cmd_sent_received.is_some() ||
                   self.current_gate_flow.gate_entry_received.is_some() {
                    self.last_gate_flow = Some(self.current_gate_flow.clone());
                    self.current_gate_flow = GateFlow::default();
                }
            }
            _ => {}
        }

        self.gate_events.push_front(event);
        if self.gate_events.len() > MAX_GATE_EVENTS {
            self.gate_events.pop_back();
        }

        self.last_message = Some(now);
    }

    fn update_metrics(&mut self, metrics: Metrics) {
        self.metrics = metrics;
        self.last_message = Some(Instant::now());
    }

    fn add_journey(&mut self, journey: Journey) {
        self.journeys_completed += 1;
        if journey.auth {
            self.journeys_authorized += 1;
        }
        self.last_journey = Some(journey);
        self.last_message = Some(Instant::now());
    }

    fn handle_acc_event(&mut self, event: AccEvent) {
        match event.t.as_str() {
            "matched" => self.acc_matched += 1,
            "unmatched" => self.acc_unmatched += 1,
            "late_after_gate" => self.acc_late += 1,
            "matched_no_journey" => self.acc_no_journey += 1,
            _ => {}
        }

        self.acc_events.push_front(event);
        if self.acc_events.len() > MAX_ACC_EVENTS {
            self.acc_events.pop_back();
        }

        self.last_message = Some(Instant::now());
    }
}

type SharedState = Arc<Mutex<DashboardState>>;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let broker_host: String = args.get(1).cloned().unwrap_or_else(|| "localhost".to_string());
    let broker_port: u16 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1883);
    let mqtt_user: Option<String> = args.get(3).cloned();
    let mqtt_pass: Option<String> = args.get(4).cloned();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let state = Arc::new(Mutex::new(DashboardState::new()));

    let mqtt_state = state.clone();
    let mqtt_handle = tokio::spawn(async move {
        run_mqtt_subscriber(&broker_host, broker_port, mqtt_user, mqtt_pass, mqtt_state).await;
    });

    let result = run_ui(&mut terminal, state).await;

    mqtt_handle.abort();
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

async fn run_mqtt_subscriber(host: &str, port: u16, username: Option<String>, password: Option<String>, state: SharedState) {
    let client_id = format!("gateway-tui-{}", std::process::id());
    let mut mqttoptions = MqttOptions::new(client_id, host, port);
    mqttoptions.set_keep_alive(Duration::from_secs(30));
    mqttoptions.set_clean_session(true);

    // Set credentials if provided
    if let (Some(user), Some(pass)) = (username, password) {
        mqttoptions.set_credentials(user, pass);
    }

    let (client, mut eventloop) = AsyncClient::new(mqttoptions, 100);
    let _ = client.subscribe("gateway/#", QoS::AtMostOnce).await;

    loop {
        match eventloop.poll().await {
            Ok(MqttEvent::Incoming(Packet::ConnAck(_))) => {
                let mut s = state.lock().await;
                s.connected = true;
            }
            Ok(MqttEvent::Incoming(Packet::Publish(publish))) => {
                let topic = publish.topic.as_str();
                let payload = std::str::from_utf8(&publish.payload).unwrap_or("");

                let mut s = state.lock().await;

                match topic {
                    "gateway/events" => {
                        if let Ok(event) = serde_json::from_str::<ZoneEvent>(payload) {
                            s.handle_zone_event(event);
                        }
                    }
                    "gateway/tracks" => {
                        if let Ok(event) = serde_json::from_str::<TrackEvent>(payload) {
                            s.handle_track_event(event);
                        }
                    }
                    "gateway/gate" => {
                        if let Ok(event) = serde_json::from_str::<GateState>(payload) {
                            s.handle_gate_event(event);
                        }
                    }
                    "gateway/metrics" => {
                        if let Ok(metrics) = serde_json::from_str::<Metrics>(payload) {
                            s.update_metrics(metrics);
                        }
                    }
                    "gateway/journeys" => {
                        if let Ok(journey) = serde_json::from_str::<Journey>(payload) {
                            s.add_journey(journey);
                        }
                    }
                    "gateway/acc" => {
                        if let Ok(event) = serde_json::from_str::<AccEvent>(payload) {
                            s.handle_acc_event(event);
                        }
                    }
                    _ => {}
                }
            }
            Ok(_) => {}
            Err(_) => {
                let mut s = state.lock().await;
                s.connected = false;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

async fn run_ui(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: SharedState,
) -> Result<(), Box<dyn std::error::Error>> {
    let tick_rate = Duration::from_millis(100);
    let mut last_tick = Instant::now();

    loop {
        let s = state.lock().await;
        terminal.draw(|f| draw_ui(f, &s))?;
        drop(s);

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        _ => {}
                    }
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
        }
    }
}

fn draw_ui(f: &mut Frame, state: &DashboardState) {
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),   // Header
            Constraint::Length(9),   // POS Occupancy
            Constraint::Min(0),      // Bottom panels
        ])
        .split(f.area());

    draw_header(f, main_chunks[0], state);
    draw_pos_panel(f, main_chunks[1], state);

    // Bottom: 4 columns - Track Events, ACC, Gate, Metrics
    let bottom_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30),  // Track events
            Constraint::Percentage(25),  // ACC
            Constraint::Percentage(22),  // Gate
            Constraint::Percentage(23),  // Metrics
        ])
        .split(main_chunks[2]);

    draw_track_panel(f, bottom_chunks[0], state);
    draw_acc_panel(f, bottom_chunks[1], state);
    draw_gate_panel(f, bottom_chunks[2], state);
    draw_metrics_panel(f, bottom_chunks[3], state);
}

fn draw_header(f: &mut Frame, area: Rect, state: &DashboardState) {
    let status_color = if state.connected { Color::Green } else { Color::Red };
    let status_text = if state.connected { "CONNECTED" } else { "DISCONNECTED" };

    let last_msg = state.last_message
        .map(|t| format!("{}s ago", t.elapsed().as_secs()))
        .unwrap_or_else(|| "never".to_string());

    let header = Paragraph::new(Line::from(vec![
        Span::styled("Gateway TUI ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw("| "),
        Span::styled(status_text, Style::default().fg(status_color)),
        Span::raw(" | Last: "),
        Span::raw(last_msg),
        Span::raw(" | Journeys: "),
        Span::styled(
            format!("{}/{}", state.journeys_authorized, state.journeys_completed),
            Style::default().fg(Color::Yellow)
        ),
        Span::raw(" | Press 'q' to quit"),
    ]))
    .block(Block::default().borders(Borders::ALL));

    f.render_widget(header, area);
}

fn draw_pos_panel(f: &mut Frame, area: Rect, state: &DashboardState) {
    let mut rows: Vec<Row> = Vec::new();

    // Sort zones by name
    let mut zones: Vec<_> = state.pos_zones.values().collect();
    zones.sort_by(|a, b| a.name.cmp(&b.name));

    for zone in zones {
        let (status, tid_str, dwell_str, auth_str) = if let Some(tid) = zone.occupant_tid {
            let live_dwell = zone.entered_at
                .map(|t| zone.occupant_dwell_ms + t.elapsed().as_millis() as u64)
                .unwrap_or(zone.occupant_dwell_ms);

            let status_icon = if zone.occupant_auth { "●" } else { "○" };
            let status_color = if zone.occupant_auth { Color::Green } else { Color::Yellow };

            (
                Span::styled(status_icon, Style::default().fg(status_color)),
                format!("T{}", tid),
                format!("{:.1}s", live_dwell as f64 / 1000.0),
                if zone.occupant_auth { "AUTH" } else { "..." },
            )
        } else {
            (
                Span::styled("·", Style::default().fg(Color::DarkGray)),
                "-".to_string(),
                "-".to_string(),
                "-",
            )
        };

        rows.push(Row::new(vec![
            status.to_string(),
            zone.name.clone(),
            tid_str,
            dwell_str,
            auth_str.to_string(),
        ]));
    }

    let table = Table::new(
        rows,
        [
            Constraint::Length(3),   // Status
            Constraint::Length(8),   // Zone
            Constraint::Length(6),   // TID
            Constraint::Length(8),   // Dwell
            Constraint::Length(6),   // Auth
        ],
    )
    .header(
        Row::new(vec!["", "Zone", "Track", "Dwell", "Auth"])
            .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
    )
    .block(Block::default()
        .title(" POS Occupancy ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue)));

    f.render_widget(table, area);
}

fn draw_track_panel(f: &mut Frame, area: Rect, state: &DashboardState) {
    let items: Vec<ListItem> = state.track_events.iter().map(|e| {
        let (icon, color) = match e.t.as_str() {
            "create" => ("+", Color::Green),
            "stitch" => ("↔", Color::Cyan),
            "pending" => ("?", Color::Yellow),
            "reentry" => ("↩", Color::Magenta),
            "lost" => ("✗", Color::Red),
            _ => ("·", Color::White),
        };

        let auth_icon = if e.auth { "A" } else { " " };

        let extra = match e.t.as_str() {
            "stitch" => {
                let prev = e.prev_tid.map(|t| format!("←T{}", t)).unwrap_or_default();
                let dist = e.stitch_dist_cm.map(|d| format!(" {}cm", d)).unwrap_or_default();
                let time = e.stitch_time_ms.map(|t| format!(" {}ms", t)).unwrap_or_default();
                format!("{}{}{}", prev, dist, time)
            }
            "reentry" => {
                e.parent_jid.as_ref().map(|j| format!("←{}", &j[..8])).unwrap_or_default()
            }
            _ => String::new(),
        };

        ListItem::new(Line::from(vec![
            Span::styled(icon, Style::default().fg(color)),
            Span::raw(format!(" T{:<4} ", e.tid)),
            Span::styled(auth_icon, Style::default().fg(if e.auth { Color::Green } else { Color::DarkGray })),
            Span::raw(format!(" {:.1}s ", e.dwell_ms as f64 / 1000.0)),
            Span::styled(extra, Style::default().fg(Color::DarkGray)),
        ]))
    }).collect();

    let list = List::new(items)
        .block(Block::default()
            .title(" Track Lifecycle ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)));

    f.render_widget(list, area);
}

fn draw_acc_panel(f: &mut Frame, area: Rect, state: &DashboardState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),   // Stats
            Constraint::Min(0),      // Events
        ])
        .split(area);

    // ACC stats
    let match_rate = if state.acc_matched + state.acc_unmatched > 0 {
        (state.acc_matched as f64 / (state.acc_matched + state.acc_unmatched) as f64) * 100.0
    } else {
        0.0
    };

    let stats = Paragraph::new(Line::from(vec![
        Span::styled(format!("{}", state.acc_matched), Style::default().fg(Color::Green)),
        Span::raw("/"),
        Span::styled(format!("{}", state.acc_unmatched), Style::default().fg(Color::Red)),
        Span::raw(format!(" ({:.0}%) ", match_rate)),
        Span::styled(format!("L{}", state.acc_late), Style::default().fg(Color::Yellow)),
        Span::raw(" "),
        Span::styled(format!("NJ{}", state.acc_no_journey), Style::default().fg(Color::Magenta)),
    ]))
    .block(Block::default()
        .title(" ACC Match/Miss ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow)));

    f.render_widget(stats, chunks[0]);

    // ACC events
    let events: Vec<ListItem> = state.acc_events.iter().map(|e| {
        let (icon, color) = match e.t.as_str() {
            "matched" => ("✓", Color::Green),
            "unmatched" => ("✗", Color::Red),
            "late_after_gate" => ("!", Color::Yellow),
            "matched_no_journey" => ("?", Color::Magenta),
            _ => ("?", Color::White),
        };

        let pos = e.pos.as_ref().map(|p| p.as_str()).unwrap_or("-");
        let tid = e.tid.map(|t| format!("T{}", t)).unwrap_or_else(|| "-".to_string());

        ListItem::new(Line::from(vec![
            Span::styled(icon, Style::default().fg(color)),
            Span::raw(format!(" {} ", pos)),
            Span::styled(tid, Style::default().fg(if e.tid.is_some() { Color::Cyan } else { Color::DarkGray })),
            Span::styled(
                match e.t.as_str() {
                    "late_after_gate" => e
                        .delta_ms
                        .map(|d| format!(" Δ{}ms", d))
                        .unwrap_or_else(|| " Δ-".to_string()),
                    "matched_no_journey" => " NOJ".to_string(),
                    _ => "".to_string(),
                },
                Style::default().fg(Color::DarkGray),
            ),
        ]))
    }).collect();

    let events_list = List::new(events)
        .block(Block::default()
            .title(" Events ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow)));

    f.render_widget(events_list, chunks[1]);
}

fn draw_gate_panel(f: &mut Frame, area: Rect, state: &DashboardState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(9),   // Gate timing
            Constraint::Min(0),      // Gate events
        ])
        .split(area);

    // Show current or last gate flow timing
    let flow = if state.current_gate_flow.gate_entry_received.is_some() ||
                  state.current_gate_flow.cmd_sent_received.is_some() {
        &state.current_gate_flow
    } else if let Some(ref last) = state.last_gate_flow {
        last
    } else {
        &state.current_gate_flow
    };

    let tid_str = flow.tid.map(|t| format!("T{}", t)).unwrap_or_else(|| "-".to_string());

    // Calculate timing deltas
    let entry_to_cmd = match (flow.gate_entry_received, flow.cmd_sent_received) {
        (Some(e), Some(c)) => format!("{:>5}ms", c.duration_since(e).as_millis()),
        _ => "    -".to_string(),
    };
    let cmd_to_move = match (flow.cmd_sent_received, flow.moving_received) {
        (Some(c), Some(m)) => format!("{:>5}ms", m.duration_since(c).as_millis()),
        _ => "    -".to_string(),
    };
    let move_to_open = match (flow.moving_received, flow.open_received) {
        (Some(m), Some(o)) => format!("{:>5}ms", o.duration_since(m).as_millis()),
        _ => "    -".to_string(),
    };
    let entry_to_exit = match (flow.gate_entry_received, flow.exit_received) {
        (Some(e), Some(x)) => format!("{:>5}ms", x.duration_since(e).as_millis()),
        _ => "    -".to_string(),
    };

    // Status indicators
    let entry_status = if flow.gate_entry_received.is_some() { "●" } else { "○" };
    let cmd_status = if flow.cmd_sent_received.is_some() { "●" } else { "○" };
    let move_status = if flow.moving_received.is_some() { "●" } else { "○" };
    let open_status = if flow.open_received.is_some() { "●" } else { "○" };
    let exit_status = if flow.exit_received.is_some() { "●" } else { "○" };

    let timing_text = vec![
        Line::from(vec![
            Span::styled(format!("{:<8}", tid_str), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::raw(format!(" Cmds:{}", state.gate_cmd_count)),
        ]),
        Line::from(vec![
            Span::styled(entry_status, Style::default().fg(if flow.gate_entry_received.is_some() { Color::Green } else { Color::DarkGray })),
            Span::raw(" entry    "),
        ]),
        Line::from(vec![
            Span::styled(cmd_status, Style::default().fg(if flow.cmd_sent_received.is_some() { Color::Cyan } else { Color::DarkGray })),
            Span::raw(format!(" cmd      {}", entry_to_cmd)),
        ]),
        Line::from(vec![
            Span::styled(move_status, Style::default().fg(if flow.moving_received.is_some() { Color::Yellow } else { Color::DarkGray })),
            Span::raw(format!(" moving   {}", cmd_to_move)),
        ]),
        Line::from(vec![
            Span::styled(open_status, Style::default().fg(if flow.open_received.is_some() { Color::Green } else { Color::DarkGray })),
            Span::raw(format!(" open     {}", move_to_open)),
        ]),
        Line::from(vec![
            Span::styled(exit_status, Style::default().fg(if flow.exit_received.is_some() { Color::Magenta } else { Color::DarkGray })),
            Span::raw(format!(" exit     {}", entry_to_exit)),
        ]),
    ];

    let gate_status = Paragraph::new(timing_text)
        .block(Block::default()
            .title(format!(" Gate: {} ", state.last_gate_state.to_uppercase()))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Magenta)));

    f.render_widget(gate_status, chunks[0]);

    let events: Vec<ListItem> = state.gate_events.iter().map(|e| {
        let color = match e.state.as_str() {
            "open" => Color::Green,
            "closed" => Color::Red,
            "moving" => Color::Yellow,
            "cmd_sent" => Color::Cyan,
            _ => Color::White,
        };
        let tid = e.tid.map(|t| format!("T{}", t)).unwrap_or_else(|| "-".to_string());
        let src = format!(" {}", e.src);

        ListItem::new(Line::from(vec![
            Span::styled(format!("{:<8}", e.state), Style::default().fg(color)),
            Span::raw(format!("{:<5}{}", tid, src)),
        ]))
    }).collect();

    let events_list = List::new(events)
        .block(Block::default()
            .title(" Events ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Magenta)));

    f.render_widget(events_list, chunks[1]);
}

fn draw_metrics_panel(f: &mut Frame, area: Rect, state: &DashboardState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),   // Events/sec
            Constraint::Length(3),   // Latency
            Constraint::Min(0),      // Stats
        ])
        .split(area);

    // Events per second gauge
    let eps_ratio = (state.metrics.events_per_sec / 100.0).min(1.0);
    let eps_gauge = Gauge::default()
        .block(Block::default().title(" Events/sec ").borders(Borders::ALL))
        .gauge_style(Style::default().fg(Color::Cyan))
        .ratio(eps_ratio)
        .label(format!("{:.1}/s", state.metrics.events_per_sec));
    f.render_widget(eps_gauge, chunks[0]);

    // Latency gauge
    let latency_ratio = (state.metrics.avg_latency_us as f64 / 1000.0).min(1.0);
    let latency_color = if state.metrics.avg_latency_us < 200 {
        Color::Green
    } else if state.metrics.avg_latency_us < 500 {
        Color::Yellow
    } else {
        Color::Red
    };
    let latency_gauge = Gauge::default()
        .block(Block::default().title(" Latency ").borders(Borders::ALL))
        .gauge_style(Style::default().fg(latency_color))
        .ratio(latency_ratio)
        .label(format!("{}us", state.metrics.avg_latency_us));
    f.render_widget(latency_gauge, chunks[1]);

    // Stats
    let stats = Paragraph::new(vec![
        Line::from(format!("Events:    {}", state.metrics.events_total)),
        Line::from(format!("Tracks:    {}", state.metrics.active_tracks)),
        Line::from(format!("Auth:      {}", state.metrics.authorized_tracks)),
        Line::from(format!("Max lat:   {}us", state.metrics.max_latency_us)),
    ])
    .block(Block::default()
        .title(" Statistics ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green)));

    f.render_widget(stats, chunks[2]);
}
