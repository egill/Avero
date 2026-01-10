//! Gate Behavior Investigation Tool
//!
//! Investigates how the gate responds to open commands.
//! Properly handles CloudPlus heartbeats to maintain connection.

use clap::Parser;
use std::io::ErrorKind;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_serial::SerialPortBuilderExt;

// CloudPlus protocol constants
const STX: u8 = 0x02;
const ETX: u8 = 0x03;
const CMD_OPEN_DOOR: u8 = 0x2C;
const CMD_HEARTBEAT: u8 = 0x56;

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
            0x01 => DoorStatus::Open,
            0x02 => DoorStatus::Closed, // Right open = resting = closed
            0x03 => DoorStatus::Moving,
            0x04 => DoorStatus::Open,
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

#[derive(Parser, Debug)]
#[command(name = "gate_test", about = "Gate behavior investigation tool")]
struct Args {
    #[arg(long, default_value = "192.168.0.245:8000")]
    tcp: String,

    #[arg(long, default_value = "/dev/ttyAMA4")]
    rs485: String,

    #[arg(long, default_value = "19200")]
    baud: u32,

    #[arg(long)]
    fast: bool,

    #[arg(long)]
    monitor_only: bool,
}

fn calculate_checksum(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |acc, &x| acc ^ x)
}

fn build_frame(command: u8, address: u8, door: u8, rand: u8, data: &[u8]) -> Vec<u8> {
    let data_len = data.len();
    let mut frame = vec![0u8; 7 + data_len + 2];
    frame[0] = STX;
    frame[1] = rand;
    frame[2] = command;
    frame[3] = address;
    frame[4] = door;
    frame[5] = (data_len & 0xFF) as u8;
    frame[6] = ((data_len >> 8) & 0xFF) as u8;

    if data_len > 0 {
        frame[7..7 + data_len].copy_from_slice(data);
    }

    frame[7 + data_len] = calculate_checksum(&frame[..7 + data_len]);
    frame[7 + data_len + 1] = ETX;
    frame
}

fn build_open_command() -> Vec<u8> {
    build_frame(CMD_OPEN_DOOR, 0xff, 0x01, 0x00, &[])
}

fn build_heartbeat_ack(rand: u8, oem_hi: u8, oem_lo: u8) -> Vec<u8> {
    build_frame(CMD_HEARTBEAT, 0, 0, rand, &[oem_hi, oem_lo])
}

/// Parse a CloudPlus frame from buffer, returns (command, rand, data_consumed)
fn parse_frame(buf: &[u8]) -> Option<(u8, u8, Vec<u8>, usize)> {
    if buf.len() < 9 {
        return None;
    }

    // Find STX
    let start = buf.iter().position(|&b| b == STX)?;
    if start + 9 > buf.len() {
        return None;
    }

    let rand = buf[start + 1];
    let command = buf[start + 2];
    let data_len = buf[start + 5] as usize | ((buf[start + 6] as usize) << 8);

    let frame_len = 7 + data_len + 2;
    if start + frame_len > buf.len() {
        return None;
    }

    // Check ETX
    if buf[start + frame_len - 1] != ETX {
        return None;
    }

    let data = if data_len > 0 {
        buf[start + 7..start + 7 + data_len].to_vec()
    } else {
        vec![]
    };

    Some((command, rand, data, start + frame_len))
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

    // Open RS485
    println!("Opening RS485 at {} @ {}baud...", args.rs485, args.baud);
    let rs485_port = tokio_serial::new(&args.rs485, args.baud)
        .timeout(Duration::from_millis(100))
        .open_native_async()?;
    println!("RS485 opened ✓");

    let rs485 = Arc::new(tokio::sync::Mutex::new(rs485_port));

    if args.monitor_only {
        println!("\n=== MONITOR MODE (no commands) ===\n");
        let mut last_status = DoorStatus::Unknown;
        loop {
            if let Some(status) = poll_door_status(&rs485).await? {
                if status != last_status {
                    println!("[{}] DOOR: {} -> {}", chrono_time(), last_status.as_str(), status.as_str());
                    last_status = status;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    // Connect TCP
    println!("Connecting to CloudPlus at {}...", args.tcp);
    let stream = TcpStream::connect(&args.tcp).await?;
    stream.set_nodelay(true)?;
    println!("CloudPlus connected ✓");

    let (read_half, write_half) = stream.into_split();
    let read_half = Arc::new(tokio::sync::Mutex::new(read_half));
    let write_half = Arc::new(tokio::sync::Mutex::new(write_half));

    // Channel for sending frames
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(16);

    // Spawn TCP handler (reads heartbeats, sends ACKs and queued commands)
    let write_half_clone = write_half.clone();
    let read_half_clone = read_half.clone();
    let tcp_handle = tokio::spawn(async move {
        let mut read_buf = vec![0u8; 256];
        let mut buf = Vec::with_capacity(512);

        loop {
            tokio::select! {
                // Check for outbound commands
                cmd = rx.recv() => {
                    match cmd {
                        Some(frame) => {
                            let mut w = write_half_clone.lock().await;
                            if let Err(e) = w.write_all(&frame).await {
                                eprintln!("TCP write error: {}", e);
                                break;
                            }
                        }
                        None => break,
                    }
                }

                // Read from TCP with timeout
                result = async {
                    let mut r = read_half_clone.lock().await;
                    tokio::time::timeout(Duration::from_millis(100), r.read(&mut read_buf)).await
                } => {
                    match result {
                        Ok(Ok(0)) => {
                            eprintln!("TCP connection closed");
                            break;
                        }
                        Ok(Ok(n)) => {
                            buf.extend_from_slice(&read_buf[..n]);

                            // Parse frames
                            while let Some((cmd, rand, data, consumed)) = parse_frame(&buf) {
                                buf.drain(..consumed);

                                if cmd == CMD_HEARTBEAT && data.len() >= 2 {
                                    // Send ACK
                                    let ack = build_heartbeat_ack(rand, data[0], data[1]);
                                    let mut w = write_half_clone.lock().await;
                                    let _ = w.write_all(&ack).await;
                                }
                            }
                        }
                        Ok(Err(e)) if e.kind() == ErrorKind::TimedOut => {}
                        Ok(Err(e)) => {
                            eprintln!("TCP read error: {}", e);
                            break;
                        }
                        Err(_) => {} // Timeout
                    }
                }
            }
        }
    });

    // Wait for door to be closed
    println!("\nWaiting for door to be CLOSED...");
    let mut last_status = DoorStatus::Unknown;
    loop {
        if let Some(status) = poll_door_status(&rs485).await? {
            if status != last_status {
                println!("  DOOR: {} -> {}", last_status.as_str(), status.as_str());
                last_status = status;
            }
            if status == DoorStatus::Closed {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    println!("\n============================================================");
    println!("TEST: Single Open Command - Measuring Latency");
    println!("============================================================\n");

    // Send open command and measure
    let start = Instant::now();
    let cmd = build_open_command();
    tx.send(cmd).await?;
    let send_time = start.elapsed();
    println!("[{:>6}us] OPEN_CMD sent (queue time: {}us)",
             start.elapsed().as_micros(), send_time.as_micros());

    // Monitor door state changes
    let mut saw_moving = false;
    let mut saw_open = false;
    let mut moving_time: Option<Duration> = None;
    let mut open_time: Option<Duration> = None;
    let mut closed_time: Option<Duration> = None;

    let timeout = Duration::from_secs(30);
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        if let Some(status) = poll_door_status(&rs485).await? {
            if status != last_status {
                let elapsed = start.elapsed();
                println!("[{:>6}ms] DOOR: {} -> {}",
                         elapsed.as_millis(), last_status.as_str(), status.as_str());

                match status {
                    DoorStatus::Moving if !saw_moving => {
                        saw_moving = true;
                        moving_time = Some(elapsed);
                    }
                    DoorStatus::Open if !saw_open => {
                        saw_open = true;
                        open_time = Some(elapsed);
                    }
                    DoorStatus::Closed if saw_open => {
                        closed_time = Some(elapsed);
                        break; // Done - completed full cycle
                    }
                    _ => {}
                }
                last_status = status;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Print results
    println!("\n============================================================");
    println!("RESULTS");
    println!("============================================================");
    println!("  Queue latency:    {}us", send_time.as_micros());
    if let Some(t) = moving_time {
        println!("  Cmd → MOVING:     {}ms", t.as_millis());
    }
    if let Some(t) = open_time {
        println!("  Cmd → OPEN:       {}ms", t.as_millis());
    }
    if let (Some(open), Some(closed)) = (open_time, closed_time) {
        println!("  Open duration:    {}ms", (closed - open).as_millis());
    }
    if let Some(t) = closed_time {
        println!("  Full cycle:       {}ms", t.as_millis());
    }

    // Cleanup
    drop(tx);
    let _ = tokio::time::timeout(Duration::from_secs(1), tcp_handle).await;

    Ok(())
}

fn chrono_time() -> String {
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:03}", now.as_secs() % 100000, now.subsec_millis())
}

async fn poll_door_status(
    rs485: &Arc<tokio::sync::Mutex<tokio_serial::SerialStream>>,
) -> Result<Option<DoorStatus>, Box<dyn std::error::Error>> {
    let mut port = rs485.lock().await;

    // Build query command
    let mut cmd = [0u8; 8];
    cmd[0] = RS485_START_CMD;
    cmd[1] = 0x00;
    cmd[2] = 0x01;
    cmd[3] = RS485_CMD_QUERY;
    let sum: u8 = cmd[..7].iter().fold(0u8, |acc, &x| acc.wrapping_add(x));
    cmd[7] = !sum;

    port.write_all(&cmd).await?;

    let mut buf = [0u8; 64];
    match tokio::time::timeout(Duration::from_millis(100), port.read(&mut buf)).await {
        Ok(Ok(n)) if n >= 18 => {
            if let Some(start) = buf[..n].iter().position(|&b| b == RS485_START_RSP) {
                if start + 18 <= n {
                    let status_byte = buf[start + 4];
                    return Ok(Some(DoorStatus::from_code(status_byte)));
                }
            }
        }
        _ => {}
    }

    Ok(None)
}
