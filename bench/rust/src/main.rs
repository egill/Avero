//! Gate latency benchmark - measures TCP command to door moving
//!
//! Copied from production gateway-poc rs485.rs and cloudplus.rs

use clap::Parser;
use std::io::ErrorKind;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_serial::SerialPortBuilderExt;

// CloudPlus protocol
const STX: u8 = 0x02;
const ETX: u8 = 0x03;
const CMD_OPEN_DOOR: u8 = 0x2C;

// RS485 protocol (from production rs485.rs)
const RS485_START_CMD: u8 = 0x7E;
const RS485_START_RSP: u8 = 0x7F;
const RS485_CMD_QUERY: u8 = 0x10;
const RS485_FRAME_LEN: usize = 18;

// Door status codes (from production rs485.rs)
const DOOR_CLOSED_PROPERLY: u8 = 0x00;
const DOOR_RIGHT_OPEN_PROPERLY: u8 = 0x02; // Resting = closed
const DOOR_IN_MOTION: u8 = 0x03;

#[derive(Debug, Clone, Copy, PartialEq)]
enum DoorStatus {
    Closed,
    Moving,
    Other,
}

fn door_status_from_code(code: u8) -> DoorStatus {
    match code {
        DOOR_CLOSED_PROPERLY | DOOR_RIGHT_OPEN_PROPERLY => DoorStatus::Closed,
        DOOR_IN_MOTION => DoorStatus::Moving,
        _ => DoorStatus::Other,
    }
}

#[derive(Parser)]
#[command(name = "gate-bench")]
struct Args {
    #[arg(long, default_value = "192.168.0.245:8000")]
    gate_addr: String,
    #[arg(long, default_value = "/dev/ttyUSB0")]
    rs485_device: String,
    #[arg(long, default_value = "19200")]
    rs485_baud: u32,
    #[arg(short, long, default_value = "20")]
    trials: u32,
    #[arg(long, default_value = "5")]
    delay: u64,
}

fn build_open_command() -> Vec<u8> {
    // From production cloudplus.rs: address=0xff, door=0x01
    let mut frame = vec![STX, 0x00, CMD_OPEN_DOOR, 0xff, 0x01, 0x00, 0x00];
    let checksum = frame.iter().fold(0u8, |acc, &x| acc ^ x);
    frame.push(checksum);
    frame.push(ETX);
    frame
}

fn build_rs485_query() -> [u8; 8] {
    // From production rs485.rs
    let mut frame = [0u8; 8];
    frame[0] = RS485_START_CMD;
    frame[1] = 0x00;
    frame[2] = 0x01; // machine number
    frame[3] = RS485_CMD_QUERY;
    let sum: u8 = frame[..7].iter().fold(0u8, |acc, &x| acc.wrapping_add(x));
    frame[7] = !sum;
    frame
}

// From production rs485.rs: find_and_parse_frame + parse_response
fn parse_rs485_response(data: &[u8]) -> Option<DoorStatus> {
    for i in 0..data.len() {
        if data[i] == RS485_START_RSP && i + RS485_FRAME_LEN <= data.len() {
            let frame = &data[i..i + RS485_FRAME_LEN];
            // Validate checksum: sum + 1 should be 0
            let sum: u8 = frame.iter().fold(0u8, |acc, &x| acc.wrapping_add(x));
            if sum.wrapping_add(1) == 0 {
                let door_status = frame[4]; // From production: data[4]
                return Some(door_status_from_code(door_status));
            }
        }
    }
    None
}

// From production rs485.rs: the read loop
async fn poll_rs485(port: &mut tokio_serial::SerialStream, query: &[u8]) -> Option<DoorStatus> {
    if port.write_all(query).await.is_err() {
        return None;
    }

    let mut buf = [0u8; 64];
    let mut total = 0;
    let timeout = Instant::now();

    // From production: read until we have 18 bytes or 200ms timeout
    while total < buf.len() && timeout.elapsed() < Duration::from_millis(200) {
        match port.read(&mut buf[total..]).await {
            Ok(n) if n > 0 => {
                total += n;
                if total >= RS485_FRAME_LEN {
                    break;
                }
            }
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }

    if total >= RS485_FRAME_LEN {
        parse_rs485_response(&buf[..total])
    } else {
        None
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    println!("Gate Latency Benchmark (Rust)");
    println!("=============================");
    println!("Gate TCP: {}", args.gate_addr);
    println!("RS485: {} @ {} baud", args.rs485_device, args.rs485_baud);
    println!("Trials: {}", args.trials);
    println!();

    // Open RS485 (from production)
    let mut rs485 = tokio_serial::new(&args.rs485_device, args.rs485_baud)
        .timeout(Duration::from_millis(100))
        .open_native_async()?;
    println!("RS485 port opened");

    let query = build_rs485_query();

    // Test RS485
    print!("Testing RS485... ");
    let mut ok = false;
    for _ in 0..5 {
        if let Some(s) = poll_rs485(&mut rs485, &query).await {
            println!("OK (status: {:?})", s);
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    if !ok {
        println!("FAILED");
        return Err("RS485 not responding".into());
    }

    let open_cmd = build_open_command();
    let mut results: Vec<u64> = vec![];

    // Wait for closed
    println!("Waiting for door to be closed...");
    loop {
        if let Some(DoorStatus::Closed) = poll_rs485(&mut rs485, &query).await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    println!("Door closed. Starting benchmark.\n");

    for trial in 1..=args.trials {
        // Wait for closed
        loop {
            if let Some(DoorStatus::Closed) = poll_rs485(&mut rs485, &query).await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Fresh TCP connection (gate closes idle connections)
        let mut gate = TcpStream::connect(&args.gate_addr).await?;
        gate.set_nodelay(true)?;

        // Send command
        let cmd_time = Instant::now();
        gate.write_all(&open_cmd).await?;
        gate.flush().await?;
        drop(gate);

        print!("Trial {:2}: ", trial);

        // Poll until moving (250ms interval per production config)
        let mut found = false;
        let deadline = Instant::now() + Duration::from_secs(10);

        while Instant::now() < deadline {
            if let Some(DoorStatus::Moving) = poll_rs485(&mut rs485, &query).await {
                let ms = cmd_time.elapsed().as_millis() as u64;
                results.push(ms);
                println!("{} ms", ms);
                found = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        if !found {
            println!("TIMEOUT");
        }

        if trial < args.trials {
            tokio::time::sleep(Duration::from_secs(args.delay)).await;
        }
    }

    // Stats
    println!("\n=============================");
    println!("Results:");
    if !results.is_empty() {
        let sum: u64 = results.iter().sum();
        let avg = sum / results.len() as u64;
        let mut sorted = results.clone();
        sorted.sort();
        let min = sorted[0];
        let max = sorted[sorted.len() - 1];
        let p50 = sorted[sorted.len() / 2];
        let p95 = sorted[(sorted.len() as f64 * 0.95) as usize].min(max);

        println!("  Successful: {}/{}", results.len(), args.trials);
        println!("  Min: {} ms", min);
        println!("  Max: {} ms", max);
        println!("  Avg: {} ms", avg);
        println!("  P50: {} ms", p50);
        println!("  P95: {} ms", p95);
    } else {
        println!("  No successful trials!");
    }

    Ok(())
}
