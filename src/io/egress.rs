//! Journey egress - writes completed journeys to file
//!
//! Journeys are written in JSONL format (one JSON object per line)
//! to the file specified in config.
//!
//! The EgressWriter task decouples file I/O from the tracker loop,
//! batching journeys and flushing on count or timer.
//!
//! The EgressWriter owns a persistent file handle, opening the file once
//! and reusing it for all writes.

use crate::domain::journey::Journey;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

/// Trait for writing journeys - enables mock implementations for testing
pub trait JourneyWriter: Send + Sync {
    /// Write a journey, returns true if successful
    fn write_journey(&self, journey: &Journey) -> bool;

    /// Write multiple journeys, returns count of successful writes
    fn write_journeys(&self, journeys: &[Journey]) -> usize {
        journeys.iter().filter(|j| self.write_journey(j)).count()
    }
}

/// Egress writer for journeys
pub struct Egress {
    file_path: String,
}

impl Egress {
    pub fn new(file_path: &str) -> Self {
        info!(file_path = %file_path, "egress_initialized");
        Self { file_path: file_path.to_string() }
    }

    /// Write a journey to the egress file
    /// Returns true if successful, false otherwise
    fn write_journey_impl(&self, journey: &Journey) -> bool {
        let json = journey.to_json();

        match self.append_line(&json) {
            Ok(()) => {
                info!(
                    jid = %journey.jid,
                    pid = %journey.pid,
                    outcome = %journey.outcome.as_str(),
                    events = %journey.events.len(),
                    "journey_egressed"
                );
                true
            }
            Err(e) => {
                error!(
                    jid = %journey.jid,
                    error = %e,
                    "journey_egress_failed"
                );
                false
            }
        }
    }

    /// Append a line to the egress file
    fn append_line(&self, line: &str) -> std::io::Result<()> {
        let path = Path::new(&self.file_path);

        // Create parent directories if they don't exist
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }

        let mut file = OpenOptions::new().create(true).append(true).open(path)?;

        writeln!(file, "{}", line)?;
        debug!(file = %self.file_path, bytes = %line.len(), "egress_written");

        Ok(())
    }
}

impl JourneyWriter for Egress {
    fn write_journey(&self, journey: &Journey) -> bool {
        self.write_journey_impl(journey)
    }
}

/// Batch flush threshold: flush when this many journeys are buffered
const BATCH_SIZE: usize = 10;

/// Time-based flush interval in milliseconds
const FLUSH_INTERVAL_MS: u64 = 1000;

/// Async worker that receives journeys via channel and writes to file
///
/// Decouples file I/O from the tracker loop. Batches journeys and flushes
/// on batch size or timer, whichever comes first.
///
/// The writer owns a persistent file handle, opening the file once and
/// reusing it for all writes.
pub struct EgressWriter {
    /// Receiver for journeys to write
    journey_rx: mpsc::Receiver<Journey>,
    /// File path for JSONL output
    file_path: String,
    /// Buffered journeys pending write
    buffer: Vec<Journey>,
    /// Persistent file handle (opened once, reused for all writes)
    writer: Option<BufWriter<File>>,
}

impl EgressWriter {
    /// Create a new egress writer
    pub fn new(journey_rx: mpsc::Receiver<Journey>, file_path: String) -> Self {
        info!(file_path = %file_path, "egress_writer_initialized");
        Self { journey_rx, file_path, buffer: Vec::with_capacity(BATCH_SIZE), writer: None }
    }

    /// Open the file handle if not already open
    fn ensure_writer(&mut self) -> Result<&mut BufWriter<File>, std::io::Error> {
        if self.writer.is_none() {
            let path = Path::new(&self.file_path);

            if let Some(parent) = path.parent() {
                if !parent.exists() {
                    std::fs::create_dir_all(parent)?;
                }
            }

            let file = OpenOptions::new().create(true).append(true).open(path)?;
            self.writer = Some(BufWriter::new(file));
            info!(file_path = %self.file_path, "egress_file_opened");
        }

        Ok(self.writer.as_mut().expect("writer just initialized"))
    }

    /// Run the writer, processing journeys until the channel closes
    pub async fn run(mut self) {
        info!("egress_writer_started");

        // Open the file handle once at startup
        if let Err(e) = self.ensure_writer() {
            error!(error = %e, "egress_file_open_failed");
            // Continue anyway, we'll retry on each flush
        }

        let mut flush_interval = interval(Duration::from_millis(FLUSH_INTERVAL_MS));

        loop {
            tokio::select! {
                // Receive journeys from tracker
                maybe_journey = self.journey_rx.recv() => {
                    if let Some(journey) = maybe_journey {
                        self.buffer.push(journey);
                        if self.buffer.len() >= BATCH_SIZE {
                            self.flush();
                        }
                    } else {
                        // Channel closed, flush remaining and exit
                        self.flush();
                        break;
                    }
                }
                // Periodic flush timer
                _ = flush_interval.tick() => {
                    self.flush();
                }
            }
        }

        // Ensure final flush before shutdown
        if let Some(ref mut writer) = self.writer {
            let _ = writer.flush();
        }

        info!("egress_writer_stopped");
    }

    /// Flush buffered journeys to file
    ///
    /// Performs blocking I/O inline. This is acceptable because:
    /// - The EgressWriter task is already off the tracker hot path
    /// - Inline writes guarantee ordering and shutdown flush
    fn flush(&mut self) {
        if self.buffer.is_empty() {
            return;
        }

        let journeys = std::mem::take(&mut self.buffer);
        self.buffer.reserve(BATCH_SIZE);
        let count = journeys.len();

        let writer = match self.ensure_writer() {
            Ok(w) => w,
            Err(e) => {
                error!(error = %e, "egress_open_failed");
                return;
            }
        };

        for journey in &journeys {
            let json = journey.to_json();
            if let Err(e) = writeln!(writer, "{}", json) {
                error!(jid = %journey.jid, error = %e, "journey_egress_failed");
            } else {
                info!(
                    jid = %journey.jid,
                    pid = %journey.pid,
                    outcome = %journey.outcome.as_str(),
                    events = %journey.events.len(),
                    "journey_egressed"
                );
            }
        }

        if let Err(e) = writer.flush() {
            warn!(error = %e, "egress_flush_failed");
        }

        debug!(count = count, "egress_batch_flushed");
    }
}

/// Create an egress channel and writer
///
/// Returns the sender (for tracker) and the writer (to be spawned)
pub fn create_egress_writer(
    file_path: String,
    buffer_size: usize,
) -> (mpsc::Sender<Journey>, EgressWriter) {
    let (journey_tx, journey_rx) = mpsc::channel(buffer_size);
    let writer = EgressWriter::new(journey_rx, file_path);
    (journey_tx, writer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::journey::{Journey, JourneyEvent, JourneyEventType, JourneyOutcome};
    use crate::domain::types::TrackId;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_egress_new() {
        let egress = Egress::new("test.jsonl");
        assert_eq!(egress.file_path, "test.jsonl");
    }

    #[test]
    fn test_write_journey() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("journeys.jsonl");
        let file_str = file_path.to_str().unwrap();

        let egress = Egress::new(file_str);

        let mut journey = Journey::new(TrackId(100));
        journey.authorized = true;
        journey.total_dwell_ms = 7500;
        journey.crossed_entry = true;
        journey.add_event(JourneyEvent::new(JourneyEventType::EntryCross, 1234567890));
        journey.complete(JourneyOutcome::Completed);

        let result = egress.write_journey(&journey);
        assert!(result);

        // Verify file was created and contains valid JSON
        let content = fs::read_to_string(&file_path).unwrap();
        assert!(!content.is_empty());
        assert!(content.contains(&journey.jid));
        assert!(content.ends_with('\n'));

        // Verify it's valid JSON
        let parsed: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(parsed["jid"], journey.jid);
        assert_eq!(parsed["auth"], true);
        assert_eq!(parsed["out"], "exit");
    }

    #[test]
    fn test_write_multiple_journeys() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("journeys.jsonl");
        let file_str = file_path.to_str().unwrap();

        let egress = Egress::new(file_str);

        // Write first journey
        let mut journey1 = Journey::new(TrackId(100));
        journey1.crossed_entry = true;
        journey1.complete(JourneyOutcome::Completed);
        egress.write_journey(&journey1);

        // Write second journey
        let mut journey2 = Journey::new(TrackId(200));
        journey2.crossed_entry = true;
        journey2.complete(JourneyOutcome::Lost);
        egress.write_journey(&journey2);

        // Verify both journeys are in file
        let content = fs::read_to_string(&file_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);

        // Verify each line is valid JSON
        for line in lines {
            let _parsed: serde_json::Value = serde_json::from_str(line).unwrap();
        }
    }

    #[test]
    fn test_write_journeys_batch() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("journeys.jsonl");
        let file_str = file_path.to_str().unwrap();

        let egress = Egress::new(file_str);

        let journeys: Vec<Journey> = (0..5)
            .map(|i| {
                let mut j = Journey::new(TrackId(100 + i));
                j.crossed_entry = true;
                j.complete(JourneyOutcome::Completed);
                j
            })
            .collect();

        let count = egress.write_journeys(&journeys);
        assert_eq!(count, 5);

        let content = fs::read_to_string(&file_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn test_creates_parent_directories() {
        let dir = tempdir().unwrap();
        let nested_path = dir.path().join("nested").join("dir").join("journeys.jsonl");
        let file_str = nested_path.to_str().unwrap();

        let egress = Egress::new(file_str);

        let mut journey = Journey::new(TrackId(100));
        journey.crossed_entry = true;
        journey.complete(JourneyOutcome::Completed);

        let result = egress.write_journey(&journey);
        assert!(result);
        assert!(nested_path.exists());
    }

    #[test]
    fn test_append_mode() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("journeys.jsonl");
        let file_str = file_path.to_str().unwrap();

        // Pre-create file with existing content
        fs::write(&file_path, "{\"existing\":\"data\"}\n").unwrap();

        let egress = Egress::new(file_str);

        let mut journey = Journey::new(TrackId(100));
        journey.crossed_entry = true;
        journey.complete(JourneyOutcome::Completed);
        egress.write_journey(&journey);

        let content = fs::read_to_string(&file_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();

        // Should have both the original line and the new journey
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("existing"));
        assert!(lines[1].contains(&journey.jid));
    }
}
