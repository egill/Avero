//! CloudPlus TypeB TCP/IP protocol implementation
//!
//! Protocol:
//! - Frame: [STX][Rand][Command][Address][Door][LenL][LenH][Data][Checksum][ETX]
//! - Device→Server: Length bytes are swapped (high, low)
//! - Server→Device: Length bytes are normal (low, high)
//! - Checksum: XOR of all bytes before checksum

use bytes::{Buf, BytesMut};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{debug, error, info, warn};

/// Log connection failure (cold path)
#[cold]
fn log_connect_failed(e: &(dyn std::error::Error + Send + Sync)) {
    error!(error = %e, "cloudplus_connect_failed");
}

/// Log read error (cold path)
#[cold]
fn log_read_error(e: &std::io::Error) {
    error!(error = %e, "cloudplus_read_error");
}

/// Log write error (cold path)
#[cold]
fn log_write_error(e: &std::io::Error) {
    error!(error = %e, "cloudplus_write_error");
}

/// Log write timeout (cold path)
#[cold]
fn log_write_timeout() {
    error!("cloudplus_write_timeout");
}

// Protocol constants
const STX: u8 = 0x02;
const ETX: u8 = 0x03;
const MIN_FRAME_SIZE: usize = 9;
const MAX_DATA_LEN: usize = 4096;

// Commands from device
const CMD_HEARTBEAT: u8 = 0x56;
const CMD_REQUEST: u8 = 0x53;

// Commands to device
const CMD_OPEN_DOOR: u8 = 0x2C;
#[allow(dead_code)]
const CMD_DOOR_NORM_OPEN: u8 = 0x2D;
#[allow(dead_code)]
const CMD_CLOSE_DOOR: u8 = 0x2E;
#[allow(dead_code)]
const CMD_TIME_SYNC: u8 = 0x07;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct HeartbeatData {
    pub received_at: Instant,
    pub door_state: u8,
    pub relay_status: u8,
    pub oem_code: u16,
    pub serial_number: [u8; 6],
    pub version: u8,
}

#[derive(Debug)]
pub struct Frame {
    pub rand: u8,
    pub command: u8,
    pub address: u8,
    pub door: u8,
    pub data: Vec<u8>,
    pub valid: bool,
    pub parse_err: Option<String>,
}

impl Frame {
    /// Parse a device→server frame. Length bytes are swapped in that direction.
    pub fn parse(buf: &[u8]) -> Option<(Frame, usize)> {
        if buf.len() < MIN_FRAME_SIZE {
            return None;
        }

        // Find STX
        let stx_idx = buf.iter().position(|&b| b == STX)?;
        if stx_idx > 0 {
            // Skip bytes before STX
            return Some((
                Frame {
                    rand: 0,
                    command: 0,
                    address: 0,
                    door: 0,
                    data: vec![],
                    valid: false,
                    parse_err: Some("skipping to STX".to_string()),
                },
                stx_idx,
            ));
        }

        if buf.len() < 7 {
            return None;
        }

        let rand = buf[1];
        let command = buf[2];
        let address = buf[3];
        let door = buf[4];

        // Device→Server: length bytes are swapped (high, low)
        let data_len = (buf[6] as u16) | ((buf[5] as u16) << 8);
        if data_len as usize > MAX_DATA_LEN {
            return Some((
                Frame {
                    rand,
                    command,
                    address,
                    door,
                    data: vec![],
                    valid: false,
                    parse_err: Some("data length exceeds maximum".to_string()),
                },
                1, // Skip STX and resync
            ));
        }

        let total_len = 7 + data_len as usize + 2;
        if buf.len() < total_len {
            return None; // Need more data
        }

        let data = if data_len > 0 { buf[7..7 + data_len as usize].to_vec() } else { vec![] };

        let checksum = buf[7 + data_len as usize];
        let etx = buf[7 + data_len as usize + 1];

        if etx != ETX {
            return Some((
                Frame {
                    rand,
                    command,
                    address,
                    door,
                    data,
                    valid: false,
                    parse_err: Some("invalid ETX".to_string()),
                },
                total_len,
            ));
        }

        let expected_checksum = calculate_checksum(&buf[..7 + data_len as usize]);
        if checksum != expected_checksum {
            return Some((
                Frame {
                    rand,
                    command,
                    address,
                    door,
                    data,
                    valid: false,
                    parse_err: Some("checksum mismatch".to_string()),
                },
                total_len,
            ));
        }

        Some((
            Frame { rand, command, address, door, data, valid: true, parse_err: None },
            total_len,
        ))
    }
}

fn calculate_checksum(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |acc, &b| acc ^ b)
}

/// Build a server→device frame. Length bytes are normal (low, high).
fn build_frame(command: u8, address: u8, door: u8, data: &[u8]) -> Vec<u8> {
    build_frame_with_rand(command, address, door, 0x00, data)
}

fn build_frame_with_rand(command: u8, address: u8, door: u8, rand: u8, data: &[u8]) -> Vec<u8> {
    let data_len = data.len();
    if data_len > 65535 {
        return vec![];
    }

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

fn parse_heartbeat(data: &[u8]) -> Option<HeartbeatData> {
    if data.len() < 50 {
        return None;
    }

    let mut serial = [0u8; 6];
    if data.len() > 26 {
        serial.copy_from_slice(&data[21..27]);
    }

    Some(HeartbeatData {
        received_at: Instant::now(),
        door_state: if data.len() > 7 { data[7] } else { 0 },
        relay_status: if data.len() > 12 { data[12] } else { 0 },
        oem_code: if data.len() > 20 { (data[19] as u16) | ((data[20] as u16) << 8) } else { 0 },
        serial_number: serial,
        version: if data.len() > 18 { data[18] } else { 0 },
    })
}

#[derive(Debug, Clone)]
pub struct CloudPlusConfig {
    pub addr: String,
    pub dial_timeout: Duration,
    pub read_timeout: Duration,
    pub write_timeout: Duration,
    #[allow(dead_code)]
    pub heartbeat_wait: Duration,
}

impl Default for CloudPlusConfig {
    fn default() -> Self {
        Self {
            addr: "192.168.0.245:8000".to_string(),
            dial_timeout: Duration::from_secs(10),
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(5),
            heartbeat_wait: Duration::from_secs(3),
        }
    }
}

/// Connection state that must be updated atomically to prevent deadlocks.
/// All fields are modified together during connect/disconnect.
#[derive(Default)]
struct CloudPlusState {
    read_half: Option<ReadHalf<TcpStream>>,
    write_half: Option<WriteHalf<TcpStream>>,
    connected: bool,
    request_mode: bool,
}

pub struct CloudPlusClient {
    config: CloudPlusConfig,
    state: Arc<Mutex<CloudPlusState>>,
    outbound_tx: mpsc::Sender<Vec<u8>>,
    outbound_rx: Arc<Mutex<mpsc::Receiver<Vec<u8>>>>,
    internal_tx: mpsc::Sender<Vec<u8>>,
    internal_rx: Arc<Mutex<mpsc::Receiver<Vec<u8>>>>,
    last_heartbeat: Arc<RwLock<Option<HeartbeatData>>>,
    heartbeats_rx: Arc<RwLock<u64>>,
    heartbeats_ack: Arc<RwLock<u64>>,
}

impl CloudPlusClient {
    pub fn new(config: CloudPlusConfig) -> Self {
        let (outbound_tx, outbound_rx) = mpsc::channel(64);
        let (internal_tx, internal_rx) = mpsc::channel(16);

        Self {
            config,
            state: Arc::new(Mutex::new(CloudPlusState::default())),
            outbound_tx,
            outbound_rx: Arc::new(Mutex::new(outbound_rx)),
            internal_tx,
            internal_rx: Arc::new(Mutex::new(internal_rx)),
            last_heartbeat: Arc::new(RwLock::new(None)),
            heartbeats_rx: Arc::new(RwLock::new(0)),
            heartbeats_ack: Arc::new(RwLock::new(0)),
        }
    }

    /// Get the current outbound queue depth (for metrics)
    pub fn outbound_queue_depth(&self) -> usize {
        self.outbound_tx.max_capacity() - self.outbound_tx.capacity()
    }

    /// Get the outbound queue max capacity (for utilization calculation)
    pub fn outbound_max_capacity(&self) -> usize {
        self.outbound_tx.max_capacity()
    }

    pub async fn connect(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!(addr = %self.config.addr, "cloudplus_connecting");

        let stream =
            tokio::time::timeout(self.config.dial_timeout, TcpStream::connect(&self.config.addr))
                .await??;

        // Enable TCP nodelay for low latency
        stream.set_nodelay(true)?;

        info!(addr = %self.config.addr, "cloudplus_connected");

        // Split stream into read and write halves for independent operation
        let (read_half, write_half) = tokio::io::split(stream);

        // Atomically update all connection state
        {
            let mut state = self.state.lock().await;
            state.read_half = Some(read_half);
            state.write_half = Some(write_half);
            state.connected = true;
            state.request_mode = false;
        }

        Ok(())
    }

    pub async fn is_connected(&self) -> bool {
        self.state.lock().await.connected
    }

    pub async fn run(&self, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        loop {
            // Try to connect
            if let Err(e) = self.connect().await {
                log_connect_failed(e.as_ref());
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }

            // Take stream halves out of state for independent operation in loops
            let (read_half, write_half) = {
                let mut state = self.state.lock().await;
                (state.read_half.take(), state.write_half.take())
            };

            let Some(read_half) = read_half else {
                warn!("cloudplus_no_read_half");
                continue;
            };
            let Some(write_half) = write_half else {
                warn!("cloudplus_no_write_half");
                continue;
            };

            // Run read/write loops
            let read_handle = {
                let state = self.state.clone();
                let internal_tx = self.internal_tx.clone();
                let last_heartbeat = self.last_heartbeat.clone();
                let heartbeats_rx = self.heartbeats_rx.clone();
                let heartbeats_ack = self.heartbeats_ack.clone();
                let read_timeout = self.config.read_timeout;

                tokio::spawn(async move {
                    Self::read_loop(
                        read_half,
                        internal_tx,
                        last_heartbeat,
                        state,
                        heartbeats_rx,
                        heartbeats_ack,
                        read_timeout,
                    )
                    .await
                })
            };

            let write_handle = {
                let outbound_rx = self.outbound_rx.clone();
                let internal_rx = self.internal_rx.clone();
                let write_timeout = self.config.write_timeout;

                tokio::spawn(async move {
                    Self::write_loop(write_half, outbound_rx, internal_rx, write_timeout).await
                })
            };

            // Wait for disconnect or shutdown
            tokio::select! {
                _ = read_handle => {
                    warn!("cloudplus_read_loop_exited");
                }
                _ = write_handle => {
                    warn!("cloudplus_write_loop_exited");
                }
                _ = shutdown.changed() => {
                    info!("cloudplus_shutdown");
                    break;
                }
            }

            // Atomically clear connection state
            {
                let mut state = self.state.lock().await;
                state.connected = false;
                state.read_half = None;
                state.write_half = None;
            }

            // Wait before reconnect
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn read_loop(
        mut read_half: ReadHalf<TcpStream>,
        internal_tx: mpsc::Sender<Vec<u8>>,
        last_heartbeat: Arc<RwLock<Option<HeartbeatData>>>,
        state: Arc<Mutex<CloudPlusState>>,
        heartbeats_rx: Arc<RwLock<u64>>,
        heartbeats_ack: Arc<RwLock<u64>>,
        read_timeout: Duration,
    ) {
        let mut buf = [0u8; 4096];
        let mut acc = BytesMut::with_capacity(4096);

        loop {
            let n = match tokio::time::timeout(read_timeout, read_half.read(&mut buf)).await {
                Ok(Ok(0)) => {
                    warn!("cloudplus_connection_closed");
                    return;
                }
                Ok(Ok(n)) => n,
                Ok(Err(e)) => {
                    log_read_error(&e);
                    return;
                }
                Err(_) => continue, // Timeout, try again
            };

            acc.extend_from_slice(&buf[..n]);

            // Parse frames
            loop {
                let Some((frame, consumed)) = Frame::parse(&acc) else {
                    break;
                };

                acc.advance(consumed);

                if !frame.valid {
                    if let Some(ref err) = frame.parse_err {
                        if err != "skipping to STX" {
                            warn!(error = %err, "cloudplus_invalid_frame");
                        }
                    }
                    continue;
                }

                match frame.command {
                    CMD_HEARTBEAT => {
                        if let Some(hb) = parse_heartbeat(&frame.data) {
                            {
                                let mut rx = heartbeats_rx.write().await;
                                *rx += 1;
                                debug!(
                                    count = *rx,
                                    oem_code = hb.oem_code,
                                    door_state = hb.door_state,
                                    relay_status = hb.relay_status,
                                    "cloudplus_heartbeat_received"
                                );
                            }
                            *last_heartbeat.write().await = Some(hb.clone());

                            // Send ack if not in request mode
                            let request_mode = state.lock().await.request_mode;
                            if !request_mode {
                                let hi = ((hb.oem_code >> 8) & 0xFF) as u8;
                                let lo = (hb.oem_code & 0xFF) as u8;
                                let resp = build_frame_with_rand(
                                    CMD_HEARTBEAT,
                                    0,
                                    0,
                                    frame.rand,
                                    &[hi, lo],
                                );

                                if internal_tx.try_send(resp).is_ok() {
                                    let mut ack = heartbeats_ack.write().await;
                                    *ack += 1;
                                    debug!(count = *ack, "cloudplus_heartbeat_ack_sent");
                                }
                            }
                        }
                    }
                    CMD_REQUEST => {
                        // Card/button request - for now just log it
                        info!(
                            address = frame.address,
                            door = frame.door,
                            data_len = frame.data.len(),
                            "cloudplus_request_received"
                        );
                        // TODO: Handle access control requests
                    }
                    _ => {
                        debug!(command = frame.command, "cloudplus_unknown_command");
                    }
                }
            }
        }
    }

    async fn write_loop(
        mut write_half: WriteHalf<TcpStream>,
        outbound_rx: Arc<Mutex<mpsc::Receiver<Vec<u8>>>>,
        internal_rx: Arc<Mutex<mpsc::Receiver<Vec<u8>>>>,
        write_timeout: Duration,
    ) {
        let mut internal = internal_rx.lock().await;
        let mut outbound = outbound_rx.lock().await;

        loop {
            // Check outbound first (commands), then internal (heartbeat acks)
            // This ensures commands get sent even when heartbeats are flowing
            let (msg, source) = tokio::select! {
                biased;

                msg = outbound.recv() => {
                    match msg {
                        Some(m) => (m, "outbound"),
                        None => return,
                    }
                }
                msg = internal.recv() => {
                    match msg {
                        Some(m) => (m, "internal"),
                        None => return,
                    }
                }
            };

            let cmd_byte = if msg.len() > 2 { msg[2] } else { 0 };
            debug!(source = %source, cmd = format!("0x{:02X}", cmd_byte), "write_loop_msg_received");

            let result = tokio::time::timeout(write_timeout, write_half.write_all(&msg)).await;

            match result {
                Ok(Ok(_)) => {
                    let hex: String =
                        msg.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ");
                    debug!(len = msg.len(), hex = %hex, "cloudplus_frame_sent");
                }
                Ok(Err(e)) => {
                    log_write_error(&e);
                    return;
                }
                Err(_) => {
                    log_write_timeout();
                    return;
                }
            }
        }
    }

    /// Send door open command
    ///
    /// Uses try_send to avoid blocking. If queue is full, returns Err("queue full").
    /// A dropped command means the gate won't open for this customer - this is
    /// acceptable if TCP is so backed up, as dropping is better than blocking forever.
    pub fn send_open(&self, door_id: u8) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        let start = Instant::now();
        let door = if door_id > 1 { 1 } else { door_id + 1 };
        let frame = build_frame(CMD_OPEN_DOOR, 0xff, door, &[]);

        info!(door_id = door_id, "cloudplus_sending_open_command");

        match self.outbound_tx.try_send(frame) {
            Ok(()) => {
                let latency_us = start.elapsed().as_micros() as u64;
                info!(door_id = door_id, latency_us = latency_us, "cloudplus_open_command_queued");
                Ok(latency_us)
            }
            Err(mpsc::error::TrySendError::Full(_)) => Err("queue full".into()),
            Err(mpsc::error::TrySendError::Closed(_)) => {
                error!(door_id = door_id, "cloudplus_channel_closed");
                Err("channel closed".into())
            }
        }
    }

    /// Send door close command
    #[allow(dead_code)]
    pub async fn send_close(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !self.is_connected().await {
            return Err("not connected".into());
        }

        let frame = build_frame(CMD_CLOSE_DOOR, 0xff, 1, &[]);
        self.outbound_tx.send(frame).await.map_err(|_| "channel closed")?;

        info!("cloudplus_close_command_sent");
        Ok(())
    }

    /// Send time sync command
    #[allow(dead_code)]
    pub async fn send_time_sync(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !self.is_connected().await {
            return Err("not connected".into());
        }

        // Get current time components using time crate
        let now =
            time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());

        let data = [
            now.second(),
            now.minute(),
            now.hour(),
            now.weekday().number_from_monday(), // 1-7
            now.day(),
            now.month() as u8,
            (now.year() % 100) as u8,
        ];

        let frame = build_frame(CMD_TIME_SYNC, 0xff, 0, &data);
        self.outbound_tx.send(frame).await.map_err(|_| "channel closed")?;

        info!("cloudplus_time_sync_sent");
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn last_heartbeat(&self) -> Option<HeartbeatData> {
        self.last_heartbeat.read().await.clone()
    }

    #[allow(dead_code)]
    pub async fn stats(&self) -> (u64, u64) {
        let rx = *self.heartbeats_rx.read().await;
        let ack = *self.heartbeats_ack.read().await;
        (rx, ack)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_checksum() {
        let data = [STX, 0x00, CMD_OPEN_DOOR, 0xff, 0x01, 0x00, 0x00];
        let cs = calculate_checksum(&data);
        assert_eq!(cs, 0x02 ^ 0x00 ^ 0x2C ^ 0xff ^ 0x01 ^ 0x00 ^ 0x00);
    }

    #[test]
    fn test_build_frame() {
        let frame = build_frame(CMD_OPEN_DOOR, 0xff, 1, &[]);
        assert_eq!(frame.len(), 9);
        assert_eq!(frame[0], STX);
        assert_eq!(frame[2], CMD_OPEN_DOOR);
        assert_eq!(frame[3], 0xff);
        assert_eq!(frame[4], 1);
        assert_eq!(frame[5], 0); // data len low
        assert_eq!(frame[6], 0); // data len high
        assert_eq!(frame[8], ETX);
    }

    #[test]
    fn test_parse_frame_valid() {
        // Build a frame and parse it (simulating device→server with swapped length)
        let mut buf = vec![STX, 0x42, CMD_HEARTBEAT, 0x00, 0x00, 0x00, 0x02]; // len = 2, swapped
        buf.extend_from_slice(&[0xAB, 0xCD]); // data
        let checksum = calculate_checksum(&buf);
        buf.push(checksum);
        buf.push(ETX);

        let result = Frame::parse(&buf);
        assert!(result.is_some());
        let (frame, consumed) = result.unwrap();
        assert!(frame.valid);
        assert_eq!(frame.command, CMD_HEARTBEAT);
        assert_eq!(frame.rand, 0x42);
        assert_eq!(frame.data, vec![0xAB, 0xCD]);
        assert_eq!(consumed, buf.len());
    }

    #[test]
    fn test_parse_frame_invalid_checksum() {
        let mut buf = vec![STX, 0x00, CMD_HEARTBEAT, 0x00, 0x00, 0x00, 0x00];
        buf.push(0xFF); // wrong checksum
        buf.push(ETX);

        let result = Frame::parse(&buf);
        assert!(result.is_some());
        let (frame, _) = result.unwrap();
        assert!(!frame.valid);
        assert_eq!(frame.parse_err, Some("checksum mismatch".to_string()));
    }
}
