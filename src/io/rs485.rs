//! RS485 door status monitoring
//!
//! Protocol:
//! - Baud: 19200, 8N1
//! - Command frame: 8 bytes, starts with 0x7E
//! - Response frame: 18 bytes, starts with 0x7F
//! - Checksum: sum all bytes, bitwise NOT

use crate::domain::types::{DoorStatus, EventType, ParsedEvent, TrackId};
use crate::infra::config::Config;
use std::io::ErrorKind;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::time::interval;
use tokio_serial::SerialPortBuilderExt;
use tracing::{error, info, warn};

// Protocol constants
const START_BYTE_COMMAND: u8 = 0x7E;
const START_BYTE_RESPONSE: u8 = 0x7F;
const CMD_QUERY: u8 = 0x10;
const COMMAND_FRAME_LEN: usize = 8;
const RESPONSE_FRAME_LEN: usize = 18;

// Door status codes
const DOOR_CLOSED_PROPERLY: u8 = 0x00;
const DOOR_LEFT_OPEN_PROPERLY: u8 = 0x01;
const DOOR_RIGHT_OPEN_PROPERLY: u8 = 0x02;
const DOOR_IN_MOTION: u8 = 0x03;
const DOOR_FIRE_SIGNAL_OPENING: u8 = 0x04;

/// Maximum read attempts before giving up (prevents infinite loop)
const MAX_READ_ATTEMPTS: usize = 50;

pub struct Rs485Monitor {
    device: String,
    baud: u32,
    machine_number: u8,
    poll_interval: Duration,
    last_status: DoorStatus,
    last_poll_time: Option<Instant>,
    event_tx: Option<mpsc::Sender<ParsedEvent>>,
    /// Persistent read buffer that accumulates bytes across reads.
    /// RS485 responses can arrive in chunks (e.g., 16 bytes + 2 bytes),
    /// so we need to keep partial data for the next read.
    read_buffer: Vec<u8>,
}

impl Rs485Monitor {
    pub fn new(config: &Config) -> Self {
        Self {
            device: config.rs485_device().to_string(),
            baud: config.rs485_baud(),
            machine_number: 1, // Default machine number
            poll_interval: Duration::from_millis(config.rs485_poll_interval_ms()),
            last_status: DoorStatus::Unknown,
            last_poll_time: None,
            event_tx: None,
            read_buffer: Vec::with_capacity(64),
        }
    }

    /// Set the event sender for door state changes
    pub fn with_event_tx(mut self, tx: mpsc::Sender<ParsedEvent>) -> Self {
        self.event_tx = Some(tx);
        self
    }

    /// Build query command frame (8 bytes)
    fn build_query_command(&self) -> [u8; COMMAND_FRAME_LEN] {
        let mut frame = [0u8; COMMAND_FRAME_LEN];
        frame[0] = START_BYTE_COMMAND;
        frame[1] = 0x00; // Undefined
        frame[2] = self.machine_number;
        frame[3] = CMD_QUERY;
        frame[4] = 0x00; // Data0
        frame[5] = 0x00; // Data1
        frame[6] = 0x00; // Data2

        // Checksum: sum all bytes, bitwise NOT
        let sum: u8 = frame[..7].iter().fold(0u8, |acc, &x| acc.wrapping_add(x));
        frame[7] = !sum;

        frame
    }

    /// Parse response frame and extract door status
    fn parse_response(&self, data: &[u8]) -> Option<DoorStatus> {
        if data.len() != RESPONSE_FRAME_LEN {
            warn!(len = data.len(), expected = RESPONSE_FRAME_LEN, "rs485_invalid_response_length");
            return None;
        }

        if data[0] != START_BYTE_RESPONSE {
            warn!(byte = data[0], expected = START_BYTE_RESPONSE, "rs485_invalid_start_byte");
            return None;
        }

        // Validate checksum: sum all bytes (including checksum), add 1, should be 0
        let sum: u8 = data.iter().fold(0u8, |acc, &x| acc.wrapping_add(x));
        if sum.wrapping_add(1) != 0 {
            let hex_dump: String =
                data.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ");
            warn!(
                checksum_error = true,
                sum = %sum,
                raw_bytes = %hex_dump,
                "rs485_checksum_failed"
            );
            return None;
        }

        // Parse fields
        let fault_event = data[3];
        let door_status = data[4];
        let alarm_event = data[5];
        let infrared_status = data[12];
        let command_execute = data[13];
        let supply_voltage = data[14];

        // Log detailed status at trace level (very frequent)
        tracing::trace!(
            fault = fault_event,
            door = door_status,
            alarm = alarm_event,
            infrared = infrared_status,
            cmd_status = command_execute,
            voltage = supply_voltage,
            "rs485_response_parsed"
        );

        // Convert door status code to DoorStatus enum
        let status = match door_status {
            DOOR_CLOSED_PROPERLY => DoorStatus::Closed,
            DOOR_LEFT_OPEN_PROPERLY => DoorStatus::Open,
            DOOR_RIGHT_OPEN_PROPERLY => DoorStatus::Closed, // Right open = resting position = closed
            DOOR_IN_MOTION => DoorStatus::Moving,
            DOOR_FIRE_SIGNAL_OPENING => DoorStatus::Open,
            _ => DoorStatus::Unknown,
        };

        Some(status)
    }

    /// Synchronize read buffer to start with START_BYTE_RESPONSE (0x7F).
    /// Discards any bytes before the start byte.
    fn synchronize_buffer(&mut self) {
        if self.read_buffer.is_empty() || self.read_buffer[0] == START_BYTE_RESPONSE {
            return;
        }

        // Find the start byte
        if let Some(start_idx) = self.read_buffer.iter().position(|&b| b == START_BYTE_RESPONSE) {
            if start_idx > 0 {
                tracing::debug!(discarded = start_idx, "rs485_sync_discarded_bytes");
                self.read_buffer.drain(..start_idx);
            }
        } else {
            // No start byte found, clear buffer
            if !self.read_buffer.is_empty() {
                tracing::debug!(discarded = self.read_buffer.len(), "rs485_sync_no_start_byte");
                self.read_buffer.clear();
            }
        }
    }

    /// Read a complete frame from the serial port into the persistent buffer.
    /// Returns the parsed DoorStatus if a valid frame is found.
    async fn read_frame(&mut self, port: &mut tokio_serial::SerialStream) -> Option<DoorStatus> {
        // Synchronize buffer to start byte
        self.synchronize_buffer();

        // Check if we already have a complete frame
        if self.read_buffer.len() >= RESPONSE_FRAME_LEN {
            return self.extract_and_parse_frame();
        }

        // Read until we have enough data
        let mut temp_buf = [0u8; 64];
        let mut attempts = 0;

        while self.read_buffer.len() < RESPONSE_FRAME_LEN {
            attempts += 1;
            if attempts > MAX_READ_ATTEMPTS {
                tracing::debug!(
                    attempts = MAX_READ_ATTEMPTS,
                    buffer_len = self.read_buffer.len(),
                    "rs485_max_read_attempts"
                );
                return None;
            }

            match tokio::time::timeout(Duration::from_millis(50), port.read(&mut temp_buf)).await {
                Ok(Ok(n)) if n > 0 => {
                    self.read_buffer.extend_from_slice(&temp_buf[..n]);
                    // Re-synchronize after new data
                    self.synchronize_buffer();

                    // Check if we now have enough
                    if self.read_buffer.len() >= RESPONSE_FRAME_LEN {
                        break;
                    }
                }
                Ok(Ok(_)) => {
                    // Zero bytes read, continue
                }
                Ok(Err(e)) if e.kind() == ErrorKind::TimedOut => {
                    // Timeout, continue trying
                }
                Ok(Err(e)) => {
                    warn!(error = %e, "rs485_read_error");
                    return None;
                }
                Err(_) => {
                    // Timeout from tokio::time::timeout, continue
                }
            }
        }

        self.extract_and_parse_frame()
    }

    /// Extract a frame from the buffer and parse it.
    /// Keeps any leftover bytes for the next read.
    fn extract_and_parse_frame(&mut self) -> Option<DoorStatus> {
        if self.read_buffer.len() < RESPONSE_FRAME_LEN {
            return None;
        }

        // Extract the frame
        let frame: Vec<u8> = self.read_buffer.drain(..RESPONSE_FRAME_LEN).collect();

        // Log if there are leftover bytes (will be used in next read)
        if !self.read_buffer.is_empty() {
            tracing::trace!(leftover = self.read_buffer.len(), "rs485_frame_leftover_bytes");
        }

        self.parse_response(&frame)
    }

    /// Start the RS485 polling loop
    pub async fn run(mut self, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        info!(
            device = %self.device,
            baud = %self.baud,
            poll_interval_ms = %self.poll_interval.as_millis(),
            "rs485_monitor_started"
        );

        // Try to open the serial port
        let port_result = tokio_serial::new(&self.device, self.baud)
            .timeout(Duration::from_millis(100))
            .open_native_async();

        let mut port = match port_result {
            Ok(p) => {
                info!(device = %self.device, "rs485_port_opened");
                Some(p)
            }
            Err(e) => {
                error!(device = %self.device, error = %e, "rs485_port_open_failed");
                None
            }
        };

        let mut poll_timer = interval(self.poll_interval);

        loop {
            // Check for shutdown signal
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!("rs485_shutdown");
                        return;
                    }
                }
                _ = poll_timer.tick() => {}
            }

            let poll_start = Instant::now();

            let status = if let Some(ref mut p) = port {
                // Send query command
                let cmd = self.build_query_command();
                if let Err(e) = p.write_all(&cmd).await {
                    warn!(error = %e, "rs485_write_error");
                    self.last_status
                } else {
                    // Read response using persistent buffer
                    // This handles chunked responses (e.g., 16 bytes + 2 bytes)
                    self.read_frame(p).await.unwrap_or(self.last_status)
                }
            } else {
                DoorStatus::Unknown
            };

            let poll_duration_us = poll_start.elapsed().as_micros() as u64;

            // Check poll timing accuracy
            // Expected interval includes RS485 round-trip (~20ms at 19200 baud)
            // Only warn if drift exceeds 50ms (significant scheduling delay)
            if let Some(last_poll) = self.last_poll_time {
                let actual_interval = last_poll.elapsed();
                let expected_with_rtt = self.poll_interval + Duration::from_millis(20);
                let drift_us =
                    actual_interval.as_micros() as i64 - expected_with_rtt.as_micros() as i64;

                if drift_us.abs() > 50_000 {
                    warn!(
                        drift_us = %drift_us,
                        expected_ms = %expected_with_rtt.as_millis(),
                        actual_ms = %actual_interval.as_millis(),
                        "rs485_poll_drift"
                    );
                }
            }

            self.last_poll_time = Some(poll_start);

            // Log status changes and send event
            if status != self.last_status {
                info!(
                    door = %status.as_str(),
                    poll_duration_us = %poll_duration_us,
                    "rs485_status"
                );

                // Send door state change event
                if let Some(ref tx) = self.event_tx {
                    let event = ParsedEvent {
                        event_type: EventType::DoorStateChange(status),
                        track_id: TrackId(0), // Not applicable for door events
                        geometry_id: None,
                        direction: None,
                        event_time: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64,
                        received_at: Instant::now(),
                        position: None,
                    };
                    if let Err(e) = tx.try_send(event) {
                        warn!(error = %e, "failed to send door state event");
                    }
                }

                self.last_status = status;
            } else {
                // Use trace level for routine polling to avoid log spam
                // State changes are logged at info level above
                tracing::trace!(
                    door = %status.as_str(),
                    poll_duration_us = %poll_duration_us,
                    "rs485_poll"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_door_status_as_str() {
        assert_eq!(DoorStatus::Closed.as_str(), "closed");
        assert_eq!(DoorStatus::Moving.as_str(), "moving");
        assert_eq!(DoorStatus::Open.as_str(), "open");
        assert_eq!(DoorStatus::Unknown.as_str(), "unknown");
    }

    #[tokio::test]
    async fn test_rs485_monitor_creation() {
        let config = Config::default();
        let monitor = Rs485Monitor::new(&config);
        assert_eq!(monitor.poll_interval, Duration::from_millis(250));
        assert_eq!(monitor.last_status, DoorStatus::Unknown);
    }

    #[test]
    fn test_build_query_command() {
        let config = Config::default();
        let monitor = Rs485Monitor::new(&config);
        let cmd = monitor.build_query_command();

        assert_eq!(cmd.len(), 8);
        assert_eq!(cmd[0], 0x7E); // Start byte
        assert_eq!(cmd[3], 0x10); // Query command

        // Verify checksum: sum + checksum + 1 = 0
        let sum: u8 = cmd.iter().fold(0u8, |acc, &x| acc.wrapping_add(x));
        assert_eq!(sum.wrapping_add(1), 0);
    }
}
