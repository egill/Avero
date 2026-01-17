//! Mock CloudPlus TCP Server
//!
//! Simulates CloudPlus gate controller for local testing.
//!
//! Protocol (TypeB):
//! - Frame: [STX][Rand][Command][Address][Door][LenL][LenH][Data][Checksum][ETX]
//! - STX = 0x02, ETX = 0x03
//! - Commands: 0x56 (Heartbeat), 0x2C (Open Door)
//!
//! Behavior:
//! 1. Listens on configurable port (default 8000)
//! 2. Sends periodic heartbeats (like real device)
//! 3. When 0x2C (open door) command received:
//!    - Logs the command
//!    - Calls gateway's /door/simulate endpoint to simulate door state sequence
//!    - Sequence: moving -> open -> (wait) -> closed
//!
//! Usage:
//!   cargo run --bin mock_cloudplus -- --port 8000 --gateway-url http://localhost:9090

use bytes::BytesMut;
use clap::Parser;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

// Protocol constants (matching src/io/cloudplus.rs)
const STX: u8 = 0x02;
const ETX: u8 = 0x03;
const MIN_FRAME_SIZE: usize = 9;

// Commands
const CMD_HEARTBEAT: u8 = 0x56;
const CMD_OPEN_DOOR: u8 = 0x2C;

#[derive(Parser, Debug)]
#[command(name = "mock_cloudplus")]
#[command(about = "Mock CloudPlus gate controller for local simulation")]
struct Args {
    /// TCP port to listen on
    #[arg(short, long, default_value = "8000")]
    port: u16,

    /// Gateway HTTP URL for door state simulation
    #[arg(short, long, default_value = "http://localhost:9090")]
    gateway_url: String,

    /// Delay before door starts moving (ms)
    #[arg(long, default_value = "100")]
    move_delay_ms: u64,

    /// Delay between moving and open (ms)
    #[arg(long, default_value = "150")]
    open_delay_ms: u64,

    /// How long door stays open (ms)
    #[arg(long, default_value = "3000")]
    open_duration_ms: u64,

    /// Heartbeat interval (seconds)
    #[arg(long, default_value = "15")]
    heartbeat_interval_secs: u64,
}

/// Calculate XOR checksum of frame data
fn calculate_checksum(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |acc, &b| acc ^ b)
}

/// Build a server->device frame (length bytes: low, high)
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

/// Build heartbeat frame with simulated device data
fn build_heartbeat(rand: u8) -> Vec<u8> {
    // Simulated heartbeat data (50 bytes like real device)
    let mut data = vec![0u8; 50];
    // Door state at offset 7 (0=closed)
    data[7] = 0x00;
    // OEM code at offsets 19-20
    data[19] = 0x01;
    data[20] = 0x00;
    // Serial number at offsets 21-26
    data[21..27].copy_from_slice(b"SIMDEV");

    build_frame(CMD_HEARTBEAT, 0, 0, rand, &data)
}

/// Parse incoming frame from gateway (device<-server: length bytes swapped)
fn parse_frame(buf: &[u8]) -> Option<(ParsedFrame, usize)> {
    if buf.len() < MIN_FRAME_SIZE {
        return None;
    }

    // Find STX
    let stx_idx = buf.iter().position(|&b| b == STX)?;
    if stx_idx > 0 {
        return Some((ParsedFrame::Skip(stx_idx), stx_idx));
    }

    if buf.len() < 7 {
        return None;
    }

    let rand = buf[1];
    let command = buf[2];
    let address = buf[3];
    let door = buf[4];

    // Server->Device: length bytes are normal (low, high)
    let data_len = (buf[5] as u16) | ((buf[6] as u16) << 8);
    let total_len = 7 + data_len as usize + 2;

    if buf.len() < total_len {
        return None;
    }

    let etx = buf[total_len - 1];
    if etx != ETX {
        return Some((ParsedFrame::Invalid("bad ETX".to_string()), total_len));
    }

    // Validate checksum
    let expected_checksum = calculate_checksum(&buf[..7 + data_len as usize]);
    let actual_checksum = buf[7 + data_len as usize];
    if actual_checksum != expected_checksum {
        return Some((ParsedFrame::Invalid("checksum mismatch".to_string()), total_len));
    }

    Some((ParsedFrame::Valid { _rand: rand, command, _address: address, door }, total_len))
}

#[derive(Debug)]
enum ParsedFrame {
    Skip(usize),
    Invalid(String),
    Valid { _rand: u8, command: u8, _address: u8, door: u8 },
}

/// Simulate door state sequence by calling gateway HTTP endpoints
async fn simulate_door_sequence(
    gateway_url: &str,
    move_delay_ms: u64,
    open_delay_ms: u64,
    open_duration_ms: u64,
) {
    let client = reqwest::Client::new();
    let base_url = format!("{}/door/simulate", gateway_url);

    // Wait before moving
    tokio::time::sleep(Duration::from_millis(move_delay_ms)).await;

    // Moving
    if let Err(e) = client.post(format!("{}?status=moving", base_url)).send().await {
        eprintln!("[MOCK] Failed to send moving state: {}", e);
    } else {
        println!("[MOCK] Door -> MOVING");
    }

    // Wait before open
    tokio::time::sleep(Duration::from_millis(open_delay_ms)).await;

    // Open
    if let Err(e) = client.post(format!("{}?status=open", base_url)).send().await {
        eprintln!("[MOCK] Failed to send open state: {}", e);
    } else {
        println!("[MOCK] Door -> OPEN");
    }

    // Stay open
    tokio::time::sleep(Duration::from_millis(open_duration_ms)).await;

    // Close
    if let Err(e) = client.post(format!("{}?status=closed", base_url)).send().await {
        eprintln!("[MOCK] Failed to send closed state: {}", e);
    } else {
        println!("[MOCK] Door -> CLOSED");
    }
}

/// Handle a single client connection
async fn handle_connection(
    mut socket: TcpStream,
    peer: std::net::SocketAddr,
    gateway_url: String,
    move_delay_ms: u64,
    open_delay_ms: u64,
    open_duration_ms: u64,
    heartbeat_interval: Duration,
) {
    println!("[MOCK] Gateway connected from {}", peer);

    let mut buf = BytesMut::with_capacity(1024);
    let mut temp = [0u8; 512];
    let mut heartbeat_timer = tokio::time::interval(heartbeat_interval);
    let mut rand_counter: u8 = 0;
    let start_time = Instant::now();

    loop {
        tokio::select! {
            // Send periodic heartbeats
            _ = heartbeat_timer.tick() => {
                rand_counter = rand_counter.wrapping_add(1);
                let heartbeat = build_heartbeat(rand_counter);
                if socket.write_all(&heartbeat).await.is_err() {
                    println!("[MOCK] Connection closed (write failed)");
                    break;
                }
                let elapsed = start_time.elapsed().as_secs();
                println!("[MOCK] Sent heartbeat #{} ({}s elapsed)", rand_counter, elapsed);
            }

            // Read incoming data
            result = socket.read(&mut temp) => {
                match result {
                    Ok(0) => {
                        println!("[MOCK] Gateway disconnected");
                        break;
                    }
                    Ok(n) => {
                        buf.extend_from_slice(&temp[..n]);

                        // Parse frames
                        loop {
                            match parse_frame(&buf) {
                                Some((ParsedFrame::Skip(n), _)) => {
                                    buf.advance(n);
                                }
                                Some((ParsedFrame::Invalid(err), consumed)) => {
                                    eprintln!("[MOCK] Invalid frame: {}", err);
                                    buf.advance(consumed);
                                }
                                Some((ParsedFrame::Valid { command, door, .. }, consumed)) => {
                                    buf.advance(consumed);

                                    match command {
                                        CMD_OPEN_DOOR => {
                                            println!("[MOCK] ========================================");
                                            println!("[MOCK] GATE OPEN COMMAND RECEIVED (door={})", door);
                                            println!("[MOCK] ========================================");

                                            // Spawn door sequence simulation
                                            let url = gateway_url.clone();
                                            tokio::spawn(async move {
                                                simulate_door_sequence(
                                                    &url,
                                                    move_delay_ms,
                                                    open_delay_ms,
                                                    open_duration_ms,
                                                ).await;
                                            });
                                        }
                                        CMD_HEARTBEAT => {
                                            // Gateway acknowledged our heartbeat
                                            println!("[MOCK] Heartbeat ACK received");
                                        }
                                        _ => {
                                            println!("[MOCK] Unknown command: 0x{:02X}", command);
                                        }
                                    }
                                }
                                None => break, // Need more data
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("[MOCK] Read error: {}", e);
                        break;
                    }
                }
            }
        }
    }
}

use bytes::Buf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║           Mock CloudPlus Gate Controller                 ║");
    println!("╠══════════════════════════════════════════════════════════╣");
    println!("║ Port:            {:>5}                                   ║", args.port);
    println!("║ Gateway URL:     {:<38} ║", args.gateway_url);
    println!("║ Move delay:      {:>5} ms                                ║", args.move_delay_ms);
    println!("║ Open delay:      {:>5} ms                                ║", args.open_delay_ms);
    println!("║ Open duration:   {:>5} ms                                ║", args.open_duration_ms);
    println!(
        "║ Heartbeat:       {:>5} s                                 ║",
        args.heartbeat_interval_secs
    );
    println!("╚══════════════════════════════════════════════════════════╝");
    println!();
    println!("[MOCK] Waiting for gateway connection...");

    let listener = TcpListener::bind(format!("0.0.0.0:{}", args.port)).await?;

    loop {
        let (socket, peer) = listener.accept().await?;

        let gateway_url = args.gateway_url.clone();
        let move_delay_ms = args.move_delay_ms;
        let open_delay_ms = args.open_delay_ms;
        let open_duration_ms = args.open_duration_ms;
        let heartbeat_interval = Duration::from_secs(args.heartbeat_interval_secs);

        tokio::spawn(async move {
            handle_connection(
                socket,
                peer,
                gateway_url,
                move_delay_ms,
                open_delay_ms,
                open_duration_ms,
                heartbeat_interval,
            )
            .await;
        });
    }
}
