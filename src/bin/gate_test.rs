//! Gate Behavior Investigation Tool
//!
//! Investigates how the gate responds to multiple open commands at various intervals.
//! Records all state transitions and timing to predict gate behavior patterns.
//!
//! Usage:
//!   cargo run --bin gate_test -- --tcp 192.168.0.245:8000 --rs485 /dev/ttyAMA4
//!
//! Tests performed:
//!   1. Single open - baseline timing (cmd → moving → open → closed)
//!   2. Double open with varying intervals (0.5s, 1s, 2s, 5s)
//!   3. Triple open
//!   4. Continuous opens (every 2s while door stays open)

use clap::Parser;
use std::io::ErrorKind;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_serial::SerialPortBuilderExt;

// CloudPlus protocol constants
const STX: u8 = 0x02;
const ETX: u8 = 0x03;
const CMD_OPEN_DOOR: u8 = 0x2C;

// RS485 protocol constants
const RS485_START_CMD: u8 = 0x7E;
const RS485_START_RSP: u8 = 0x7F;
const RS485_CMD_QUERY: u8 = 0x10;

#[derive(Debug, Clone, Copy, PartialEq)]
enum DoorStatus {
    Closed,
    Open,
    Moving,
    Unknown,
}

impl DoorStatus {
    fn from_code(code: u8) -> Self {
        match code {
            0x00 => DoorStatus::Closed,
            0x01 | 0x02 => DoorStatus::Open,
            0x03 | 0x04 => DoorStatus::Moving,
            _ => DoorStatus::Unknown,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            DoorStatus::Closed => "CLOSED",
            DoorStatus::Open => "OPEN",
            DoorStatus::Moving => "MOVING",
            DoorStatus::Unknown => "UNKNOWN",
        }
    }
}

#[derive(Debug, Clone)]
struct StateEvent {
    time_ms: u64,
    event: String,
    _status: DoorStatus,
}

#[derive(Parser, Debug)]
#[command(name = "gate_test", about = "Gate behavior investigation tool")]
struct Args {
    /// CloudPlus TCP address (e.g., 192.168.0.245:8000)
    #[arg(long, default_value = "192.168.0.245:8000")]
    tcp: String,

    /// RS485 device path
    #[arg(long, default_value = "/dev/ttyAMA4")]
    rs485: String,

    /// RS485 baud rate
    #[arg(long, default_value = "19200")]
    baud: u32,

    /// Skip waiting between tests (faster but less reliable)
    #[arg(long)]
    fast: bool,

    /// Only run monitor mode (no commands)
    #[arg(long)]
    monitor_only: bool,
}

struct GateInvestigator {
    tcp_stream: Option<TcpStream>,
    rs485_port: Option<tokio_serial::SerialStream>,
    last_status: DoorStatus,
    start_time: Instant,
    events: Vec<StateEvent>,
    open_cmd_count: u32,
}

impl GateInvestigator {
    fn new() -> Self {
        Self {
            tcp_stream: None,
            rs485_port: None,
            last_status: DoorStatus::Unknown,
            start_time: Instant::now(),
            events: Vec::new(),
            open_cmd_count: 0,
        }
    }

    async fn connect_tcp(&mut self, addr: &str) -> Result<(), Box<dyn std::error::Error>> {
        println!("Connecting to CloudPlus at {}...", addr);
        let stream = TcpStream::connect(addr).await?;
        stream.set_nodelay(true)?;
        println!("CloudPlus connected ✓");
        self.tcp_stream = Some(stream);
        Ok(())
    }

    fn connect_rs485(&mut self, device: &str, baud: u32) -> Result<(), Box<dyn std::error::Error>> {
        println!("Opening RS485 at {} @ {}baud...", device, baud);
        let port = tokio_serial::new(device, baud)
            .timeout(Duration::from_millis(100))
            .open_native_async()?;
        println!("RS485 opened ✓");
        self.rs485_port = Some(port);
        Ok(())
    }

    fn elapsed_ms(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }

    fn reset_timer(&mut self) {
        self.start_time = Instant::now();
        self.events.clear();
        self.open_cmd_count = 0;
    }

    async fn send_open(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.open_cmd_count += 1;
        let cmd_num = self.open_cmd_count;
        let t = self.elapsed_ms();
        let status = self.last_status;

        // Build CloudPlus open command
        let rand = 0x00u8;
        let address = 0x00u8;
        let door = 0x00u8;

        let mut frame = vec![STX, rand, CMD_OPEN_DOOR, address, door, 0x00, 0x00];
        let checksum = frame.iter().fold(0u8, |acc, &x| acc ^ x);
        frame.push(checksum);
        frame.push(ETX);

        println!("[{:>6}ms] >>> OPEN_CMD_{}", t, cmd_num);

        self.events.push(StateEvent {
            time_ms: t,
            event: format!("OPEN_CMD_{}", cmd_num),
            _status: status,
        });

        let Some(ref mut stream) = self.tcp_stream else {
            return Err("TCP not connected".into());
        };
        stream.write_all(&frame).await?;
        stream.flush().await?;

        Ok(())
    }

    async fn poll_door_status(&mut self) -> Result<Option<DoorStatus>, Box<dyn std::error::Error>> {
        let Some(ref mut port) = self.rs485_port else {
            return Err("RS485 not connected".into());
        };

        // Build query command
        let mut cmd = [0u8; 8];
        cmd[0] = RS485_START_CMD;
        cmd[1] = 0x00;
        cmd[2] = 0x01; // machine number
        cmd[3] = RS485_CMD_QUERY;
        let sum: u8 = cmd[..7].iter().fold(0u8, |acc, &x| acc.wrapping_add(x));
        cmd[7] = !sum;

        port.write_all(&cmd).await?;

        // Read response
        let mut buf = [0u8; 64];
        match tokio::time::timeout(Duration::from_millis(200), port.read(&mut buf)).await {
            Ok(Ok(n)) if n >= 18 => {
                if let Some(start) = buf[..n].iter().position(|&b| b == RS485_START_RSP) {
                    if start + 18 <= n {
                        let status_byte = buf[start + 10];
                        return Ok(Some(DoorStatus::from_code(status_byte)));
                    }
                }
            }
            Ok(Err(e)) if e.kind() != ErrorKind::TimedOut => {
                return Err(e.into());
            }
            _ => {}
        }

        Ok(None)
    }

    fn record_status_change(&mut self, new_status: DoorStatus) -> bool {
        if new_status != self.last_status {
            let t = self.elapsed_ms();
            println!("[{:>6}ms] DOOR: {} -> {}", t, self.last_status.as_str(), new_status.as_str());
            self.events.push(StateEvent {
                time_ms: t,
                event: new_status.as_str().to_string(),
                _status: new_status,
            });
            self.last_status = new_status;
            true
        } else {
            false
        }
    }

    async fn wait_for_closed(
        &mut self,
        timeout: Duration,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(status) = self.poll_door_status().await? {
                self.record_status_change(status);
                if status == DoorStatus::Closed {
                    return Ok(true);
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok(false)
    }

    async fn monitor_until_closed(
        &mut self,
        timeout: Duration,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(status) = self.poll_door_status().await? {
                self.record_status_change(status);
                if status == DoorStatus::Closed && self.last_status == DoorStatus::Closed {
                    // Give a bit more time to confirm it stays closed
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    if let Some(s) = self.poll_door_status().await? {
                        if s == DoorStatus::Closed {
                            return Ok(());
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok(())
    }

    fn analyze_events(&self) -> TestResult {
        let mut result = TestResult::default();

        // Find timing points
        let first_cmd = self.events.iter().find(|e| e.event.starts_with("OPEN_CMD"));
        let first_moving = self.events.iter().find(|e| e.event == "MOVING");
        let first_open = self.events.iter().find(|e| e.event == "OPEN");
        let last_closed = self.events.iter().rev().find(|e| e.event == "CLOSED");

        if let Some(cmd) = first_cmd {
            result.first_cmd_at = cmd.time_ms;
        }
        if let (Some(cmd), Some(moving)) = (first_cmd, first_moving) {
            result.cmd_to_moving_ms = Some(moving.time_ms - cmd.time_ms);
        }
        if let (Some(cmd), Some(open)) = (first_cmd, first_open) {
            result.cmd_to_open_ms = Some(open.time_ms - cmd.time_ms);
        }
        if let (Some(open), Some(closed)) = (first_open, last_closed) {
            if closed.time_ms > open.time_ms {
                result.open_duration_ms = Some(closed.time_ms - open.time_ms);
            }
        }
        result.total_open_cmds = self.open_cmd_count;

        result
    }

    fn print_timeline(&self) {
        println!("\n  Timeline:");
        for event in &self.events {
            println!("    [{:>6}ms] {}", event.time_ms, event.event);
        }
    }

    async fn run_test(
        &mut self,
        name: &str,
        open_intervals_ms: &[u64],
    ) -> Result<TestResult, Box<dyn std::error::Error>> {
        println!("\n{}", "=".repeat(60));
        println!("TEST: {}", name);
        println!(
            "  Open commands: {} with intervals {:?}ms",
            open_intervals_ms.len() + 1,
            open_intervals_ms
        );
        println!("{}", "=".repeat(60));

        // Ensure door is closed
        println!("\n  Waiting for door to be CLOSED...");
        if !self.wait_for_closed(Duration::from_secs(30)).await? {
            println!("  ⚠ Timeout waiting for door to close");
            return Err("Door not closed".into());
        }

        // Reset timer for this test
        self.reset_timer();
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Send first open
        self.send_open().await?;

        // Send subsequent opens at specified intervals
        for &interval_ms in open_intervals_ms {
            tokio::time::sleep(Duration::from_millis(interval_ms)).await;

            // Poll status while waiting
            if let Some(status) = self.poll_door_status().await? {
                self.record_status_change(status);
            }

            self.send_open().await?;
        }

        // Monitor until door closes
        self.monitor_until_closed(Duration::from_secs(60)).await?;

        self.print_timeline();

        let result = self.analyze_events();
        println!("\n  Results:");
        println!("    Commands sent: {}", result.total_open_cmds);
        if let Some(ms) = result.cmd_to_moving_ms {
            println!("    Cmd → Moving: {}ms", ms);
        }
        if let Some(ms) = result.cmd_to_open_ms {
            println!("    Cmd → Open: {}ms", ms);
        }
        if let Some(ms) = result.open_duration_ms {
            println!("    Open duration: {}ms ({:.1}s)", ms, ms as f64 / 1000.0);
        }

        Ok(result)
    }
}

#[derive(Debug, Default)]
struct TestResult {
    first_cmd_at: u64,
    cmd_to_moving_ms: Option<u64>,
    cmd_to_open_ms: Option<u64>,
    open_duration_ms: Option<u64>,
    total_open_cmds: u32,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    println!("\n╔══════════════════════════════════════════════════════════╗");
    println!("║         GATE BEHAVIOR INVESTIGATION TOOL                 ║");
    println!("╚══════════════════════════════════════════════════════════╝");
    println!("\nConfiguration:");
    println!("  CloudPlus TCP: {}", args.tcp);
    println!("  RS485 device:  {} @ {}baud", args.rs485, args.baud);
    println!();

    let mut investigator = GateInvestigator::new();

    // Connect to RS485
    investigator.connect_rs485(&args.rs485, args.baud)?;

    if args.monitor_only {
        println!("\n=== MONITOR MODE (no commands) ===\n");
        loop {
            if let Some(status) = investigator.poll_door_status().await? {
                investigator.record_status_change(status);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    // Connect to CloudPlus
    investigator.connect_tcp(&args.tcp).await?;

    let mut results: Vec<(String, TestResult)> = Vec::new();

    // Test 1: Baseline - single open command
    println!("\n\n▶ PHASE 1: Baseline (single open command)");
    let r = investigator.run_test("Single Open (baseline)", &[]).await?;
    results.push(("Single Open".to_string(), r));

    if !args.fast {
        println!("\n  ⏳ Waiting 5s before next test...");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    // Test 2: Double opens with different intervals
    println!("\n\n▶ PHASE 2: Double opens at various intervals");

    for interval in [500, 1000, 2000, 3000] {
        let name = format!("Double Open ({}ms interval)", interval);
        let r = investigator.run_test(&name, &[interval]).await?;
        results.push((name, r));

        if !args.fast {
            println!("\n  ⏳ Waiting 5s before next test...");
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    // Test 3: Triple opens
    println!("\n\n▶ PHASE 3: Triple opens");
    let r = investigator.run_test("Triple Open (1s intervals)", &[1000, 1000]).await?;
    results.push(("Triple Open 1s".to_string(), r));

    if !args.fast {
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    // Test 4: Quad opens
    println!("\n\n▶ PHASE 4: Quad opens");
    let r = investigator.run_test("Quad Open (1s intervals)", &[1000, 1000, 1000]).await?;
    results.push(("Quad Open 1s".to_string(), r));

    if !args.fast {
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    // Test 5: Rapid fire
    println!("\n\n▶ PHASE 5: Rapid fire (5 opens, 500ms apart)");
    let r = investigator.run_test("Rapid Fire x5", &[500, 500, 500, 500]).await?;
    results.push(("Rapid x5".to_string(), r));

    // Print summary
    println!("\n\n╔══════════════════════════════════════════════════════════╗");
    println!("║                      SUMMARY                              ║");
    println!("╚══════════════════════════════════════════════════════════╝\n");
    println!("{:<30} {:>6} {:>10} {:>12}", "Test", "Cmds", "Cmd→Open", "Open Duration");
    println!("{}", "-".repeat(60));

    let baseline_open_duration = results.first().and_then(|(_, r)| r.open_duration_ms).unwrap_or(0);

    for (name, r) in &results {
        let cmd_open = r.cmd_to_open_ms.map(|ms| format!("{}ms", ms)).unwrap_or("-".to_string());
        let open_dur = r
            .open_duration_ms
            .map(|ms| {
                let delta = ms as i64 - baseline_open_duration as i64;
                let sign = if delta >= 0 { "+" } else { "" };
                format!("{:.1}s ({}{}ms)", ms as f64 / 1000.0, sign, delta)
            })
            .unwrap_or("-".to_string());
        let open_dur = r
            .open_duration_ms
            .map(|ms| {
                let delta = ms as i64 - baseline_open_duration as i64;
                let sign = if delta >= 0 { "+" } else { "" };
                format!("{:.1}s ({}{}ms)", ms as f64 / 1000.0, sign, delta)
            })
            .unwrap_or("-".to_string());

        println!("{:<30} {:>6} {:>10} {:>12}", name, r.total_open_cmds, cmd_open, open_dur);
    }

    println!("\n\n╔══════════════════════════════════════════════════════════╗");
    println!("║                    ANALYSIS                               ║");
    println!("╚══════════════════════════════════════════════════════════╝\n");

    // Analyze patterns
    let durations: Vec<u64> = results.iter().filter_map(|(_, r)| r.open_duration_ms).collect();

    if durations.len() >= 2 {
        let baseline = durations[0];
        let extended: Vec<_> = durations[1..].iter().filter(|&&d| d > baseline + 500).collect();

        if extended.is_empty() {
            println!("FINDING: Additional OPEN commands do NOT extend the open duration.");
            println!("         The gate ignores subsequent opens while already processing.");
        } else {
            let avg_extension =
                extended.iter().map(|&&d| d - baseline).sum::<u64>() / extended.len() as u64;
            println!("FINDING: Additional OPEN commands DO extend the open duration.");
            println!("         Average extension: ~{}ms per additional command", avg_extension);
        }
    }

    println!("\n\nInvestigation complete.");
    Ok(())
}
