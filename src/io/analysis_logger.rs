//! Analysis Logger - JSONL writer for gateway-analysis diagnostic capture
//!
//! Provides per-topic JSONL file writing with daily or size-based rotation.
//! All records use a unified schema: ts_recv, ts_event, src, site, payload_raw, fields.

use chrono::Utc;
use serde::Serialize;
use serde_json::json;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Unified log record schema for all sources
#[derive(Debug, Serialize)]
pub struct LogRecord<'a> {
    /// Receive timestamp (ISO 8601) - when the logger received the data
    pub ts_recv: &'a str,
    /// Event timestamp (ISO 8601) - from the event itself, if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ts_event: Option<&'a str>,
    /// Source identifier: "mqtt", "acc", "rs485"
    pub src: &'a str,
    /// Site identifier
    pub site: &'a str,
    /// Raw payload as received (string or hex-encoded bytes)
    pub payload_raw: &'a str,
    /// Parsed fields (source-specific, null if parsing failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<serde_json::Value>,
}

/// Log file rotation strategy
#[derive(Debug, Clone, Copy)]
pub enum RotationStrategy {
    /// Rotate at midnight UTC
    Daily,
    /// Rotate when file exceeds size in bytes
    Size(u64),
}

/// Tracks an open log file with its current size
struct TrackedWriter {
    writer: BufWriter<File>,
    bytes_written: u64,
    file_path: PathBuf,
}

/// Manages JSONL file writers per topic with daily or size-based rotation
pub struct AnalysisLogger {
    /// Base directory for log output
    log_dir: PathBuf,
    /// Site identifier included in each record
    site_id: String,
    /// Open file writers keyed by (subdirectory, name_base)
    writers: HashMap<(String, String), TrackedWriter>,
    /// Current date string (YYYYMMDD) for daily rotation
    current_date: String,
    /// Rotation strategy
    rotation: RotationStrategy,
    /// Counter for size-based rotation file suffixes
    rotation_counter: HashMap<(String, String), u32>,
}

impl AnalysisLogger {
    /// Create a new analysis logger with default daily rotation
    pub fn new(log_dir: impl AsRef<Path>, site_id: impl Into<String>) -> Self {
        Self::with_rotation(log_dir, site_id, RotationStrategy::Daily)
    }

    /// Create a new analysis logger with specified rotation strategy
    pub fn with_rotation(
        log_dir: impl AsRef<Path>,
        site_id: impl Into<String>,
        rotation: RotationStrategy,
    ) -> Self {
        let log_dir = log_dir.as_ref().to_path_buf();
        let site_id = site_id.into();
        let current_date = Utc::now().format("%Y%m%d").to_string();

        info!(
            log_dir = %log_dir.display(),
            site_id = %site_id,
            rotation = ?rotation,
            "analysis_logger_initialized"
        );

        Self {
            log_dir,
            site_id,
            writers: HashMap::new(),
            current_date,
            rotation,
            rotation_counter: HashMap::new(),
        }
    }

    /// Check if daily rotation is needed
    fn check_date_rotation(&mut self) {
        if !matches!(self.rotation, RotationStrategy::Daily) {
            return;
        }

        let now_date = Utc::now().format("%Y%m%d").to_string();
        if now_date != self.current_date {
            info!(old_date = %self.current_date, new_date = %now_date, "date_rotation_detected");
            // Close all existing writers - they'll be reopened with new date
            self.writers.clear();
            self.current_date = now_date;
        }
    }

    /// Check if size-based rotation is needed for a specific writer
    fn check_size_rotation(&mut self, subdir: &str, name_base: &str, max_size: u64) {
        let key = (subdir.to_string(), name_base.to_string());

        if let Some(tracked) = self.writers.get(&key) {
            if tracked.bytes_written >= max_size {
                info!(
                    path = %tracked.file_path.display(),
                    bytes = tracked.bytes_written,
                    "size_rotation_triggered"
                );
                // Remove the writer - a new one will be created with incremented counter
                self.writers.remove(&key);
                let counter = self.rotation_counter.entry(key).or_insert(0);
                *counter += 1;
            }
        }
    }

    /// Get or create a writer for a specific subdirectory and filename base
    fn get_writer(&mut self, subdir: &str, name_base: &str) -> std::io::Result<&mut TrackedWriter> {
        self.check_date_rotation();

        // Check size-based rotation if applicable
        if let RotationStrategy::Size(max_size) = self.rotation {
            self.check_size_rotation(subdir, name_base, max_size);
        }

        let key = (subdir.to_string(), name_base.to_string());

        match self.writers.entry(key.clone()) {
            Entry::Occupied(entry) => Ok(entry.into_mut()),
            Entry::Vacant(entry) => {
                let dir_path = self.log_dir.join(subdir);
                fs::create_dir_all(&dir_path)?;

                // Build filename based on rotation strategy
                let counter = *self.rotation_counter.get(&key).unwrap_or(&0);
                let filename = if counter == 0 {
                    format!("{}-{}.jsonl", name_base, self.current_date)
                } else {
                    format!("{}-{}-{:04}.jsonl", name_base, self.current_date, counter)
                };

                let file_path = dir_path.join(&filename);

                // Get existing file size if appending
                let existing_size = fs::metadata(&file_path).map(|m| m.len()).unwrap_or(0);

                let file = OpenOptions::new().create(true).append(true).open(&file_path)?;
                info!(path = %file_path.display(), existing_bytes = existing_size, "opened_log_file");

                Ok(entry.insert(TrackedWriter {
                    writer: BufWriter::new(file),
                    bytes_written: existing_size,
                    file_path,
                }))
            }
        }
    }

    /// Log an MQTT message using unified schema
    ///
    /// Writes to logs/mqtt/<topic_safe>-YYYYMMDD.jsonl
    /// Topic is sanitized: '/' becomes '-', other special chars removed
    pub fn log_mqtt(&mut self, topic: &str, payload: &str, parsed: Option<&serde_json::Value>) {
        let ts_recv = Utc::now().to_rfc3339();
        let topic_safe = sanitize_topic(topic);
        let site_id = self.site_id.clone();

        // Extract ts_event from parsed JSON if available (Xovis uses "timestamp" field)
        let ts_event_owned =
            parsed.and_then(|v| v.get("timestamp")).and_then(|t| t.as_str()).map(String::from);

        let fields = Some(json!({
            "topic": topic,
            "parsed": parsed,
        }));

        let record = LogRecord {
            ts_recv: &ts_recv,
            ts_event: ts_event_owned.as_deref(),
            src: "mqtt",
            site: &site_id,
            payload_raw: payload,
            fields,
        };

        if let Err(e) = self.write_record("mqtt", &topic_safe, &record) {
            warn!(topic = %topic, error = %e, "mqtt_log_failed");
        } else {
            debug!(topic = %topic, bytes = payload.len(), "mqtt_logged");
        }
    }

    /// Log an ACC event using unified schema
    ///
    /// Writes to logs/acc/acc-YYYYMMDD.jsonl (default) or
    /// logs/acc/<kiosk_ip>-YYYYMMDD.jsonl if split_per_kiosk is true
    pub fn log_acc(
        &mut self,
        kiosk_ip: &str,
        raw_line: &str,
        receipt_id: Option<&str>,
        pos_zone: Option<&str>,
        split_per_kiosk: bool,
    ) {
        let ts_recv = Utc::now().to_rfc3339();
        let site_id = self.site_id.clone();

        let fields = Some(json!({
            "kiosk_ip": kiosk_ip,
            "receipt_id": receipt_id,
            "pos_zone": pos_zone,
        }));

        let record = LogRecord {
            ts_recv: &ts_recv,
            ts_event: None,
            src: "acc",
            site: &site_id,
            payload_raw: raw_line,
            fields,
        };

        let name_base = if split_per_kiosk { sanitize_topic(kiosk_ip) } else { "acc".to_string() };

        if let Err(e) = self.write_record("acc", &name_base, &record) {
            warn!(kiosk_ip = %kiosk_ip, error = %e, "acc_log_failed");
        } else {
            debug!(kiosk_ip = %kiosk_ip, receipt_id = ?receipt_id, "acc_logged");
        }
    }

    /// Log an RS485 frame using unified schema
    ///
    /// Writes to logs/rs485/rs485-YYYYMMDD.jsonl
    pub fn log_rs485(&mut self, raw_frame: &str, door_status: Option<&str>, checksum_ok: bool) {
        let ts_recv = Utc::now().to_rfc3339();
        let site_id = self.site_id.clone();

        let fields = Some(json!({
            "door_status": door_status,
            "checksum_ok": checksum_ok,
        }));

        let record = LogRecord {
            ts_recv: &ts_recv,
            ts_event: None,
            src: "rs485",
            site: &site_id,
            payload_raw: raw_frame,
            fields,
        };

        if let Err(e) = self.write_record("rs485", "rs485", &record) {
            warn!(error = %e, "rs485_log_failed");
        } else {
            debug!(door_status = ?door_status, checksum_ok = checksum_ok, "rs485_logged");
        }
    }

    /// Write a serializable record to a specific subdir/name
    ///
    /// Continues on IO errors with a warning (non-blocking behavior).
    fn write_record<T: Serialize>(
        &mut self,
        subdir: &str,
        name_base: &str,
        record: &T,
    ) -> std::io::Result<()> {
        let json = serde_json::to_string(record)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let json_len = json.len() as u64 + 1; // +1 for newline

        let tracked = self.get_writer(subdir, name_base)?;
        writeln!(tracked.writer, "{}", json)?;
        tracked.writer.flush()?;
        tracked.bytes_written += json_len;

        Ok(())
    }

    /// Flush all open writers
    pub fn flush_all(&mut self) {
        for tracked in self.writers.values_mut() {
            if let Err(e) = tracked.writer.flush() {
                warn!(error = %e, "flush_failed");
            }
        }
    }
}

/// Sanitize a topic name for use in a filename
/// Replaces '/' with '-' and removes other problematic characters
fn sanitize_topic(topic: &str) -> String {
    topic
        .replace('/', "-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_sanitize_topic() {
        assert_eq!(sanitize_topic("gateway/events"), "gateway-events");
        assert_eq!(sanitize_topic("gateway/#"), "gateway-");
        assert_eq!(sanitize_topic("a/b/c"), "a-b-c");
        assert_eq!(sanitize_topic("test_topic"), "test_topic");
    }

    #[test]
    fn test_logger_creates_directories() {
        let dir = tempdir().unwrap();
        let mut logger = AnalysisLogger::new(dir.path(), "test-site");

        logger.log_mqtt("gateway/events", r#"{"test": true}"#, None);

        let mqtt_dir = dir.path().join("mqtt");
        assert!(mqtt_dir.exists(), "mqtt directory should be created");
    }

    #[test]
    fn test_logger_writes_jsonl() {
        let dir = tempdir().unwrap();
        let mut logger = AnalysisLogger::new(dir.path(), "test-site");

        let payload = r#"{"live_data": {"frames": []}}"#;
        let parsed: serde_json::Value = serde_json::from_str(payload).unwrap();
        logger.log_mqtt("gateway/events", payload, Some(&parsed));
        logger.flush_all();

        // Find the log file
        let mqtt_dir = dir.path().join("mqtt");
        let entries: Vec<_> =
            std::fs::read_dir(&mqtt_dir).unwrap().filter_map(|e| e.ok()).collect();
        assert_eq!(entries.len(), 1, "should have one log file");

        let content = std::fs::read_to_string(entries[0].path()).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 1, "should have one log line");

        // Parse and verify unified schema
        let record: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(record["site"], "test-site");
        assert_eq!(record["src"], "mqtt");
        assert_eq!(record["fields"]["topic"], "gateway/events");
        assert!(record["ts_recv"].as_str().unwrap().contains("T"));
        assert!(record["payload_raw"].as_str().unwrap().contains("live_data"));
    }

    #[test]
    fn test_logger_handles_malformed_payload() {
        let dir = tempdir().unwrap();
        let mut logger = AnalysisLogger::new(dir.path(), "test-site");

        // Log malformed JSON - should still work with parsed=None in fields
        logger.log_mqtt("gateway/events", "not valid json {{{", None);
        logger.flush_all();

        let mqtt_dir = dir.path().join("mqtt");
        let entries: Vec<_> =
            std::fs::read_dir(&mqtt_dir).unwrap().filter_map(|e| e.ok()).collect();
        assert_eq!(entries.len(), 1);

        let content = std::fs::read_to_string(entries[0].path()).unwrap();
        let record: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(record["payload_raw"], "not valid json {{{");
        assert_eq!(record["src"], "mqtt");
        // fields.parsed should be null for unparseable payload
        assert!(record["fields"]["parsed"].is_null());
    }

    #[test]
    fn test_separate_files_per_topic() {
        let dir = tempdir().unwrap();
        let mut logger = AnalysisLogger::new(dir.path(), "test-site");

        logger.log_mqtt("gateway/events", "{}", None);
        logger.log_mqtt("gateway/tracks", "{}", None);
        logger.flush_all();

        let mqtt_dir = dir.path().join("mqtt");
        let entries: Vec<_> =
            std::fs::read_dir(&mqtt_dir).unwrap().filter_map(|e| e.ok()).collect();
        assert_eq!(entries.len(), 2, "should have two log files for two topics");
    }

    #[test]
    fn test_size_based_rotation() {
        let dir = tempdir().unwrap();
        // Use a small size limit (200 bytes) to trigger rotation quickly
        let mut logger =
            AnalysisLogger::with_rotation(dir.path(), "test-site", RotationStrategy::Size(200));

        // Write multiple records to exceed size limit
        for i in 0..5 {
            logger.log_mqtt("gateway/events", &format!(r#"{{"index": {}}}"#, i), None);
        }
        logger.flush_all();

        // Should have rotated to create multiple files
        let mqtt_dir = dir.path().join("mqtt");
        let entries: Vec<_> =
            std::fs::read_dir(&mqtt_dir).unwrap().filter_map(|e| e.ok()).collect();
        assert!(entries.len() >= 2, "should have rotated to create multiple files");
    }

    #[test]
    fn test_unified_schema_fields() {
        let dir = tempdir().unwrap();
        let mut logger = AnalysisLogger::new(dir.path(), "test-site");

        // Test ACC logging
        logger.log_acc("192.168.1.100", "ACC 12345", Some("12345"), Some("pos1"), false);
        logger.flush_all();

        let acc_dir = dir.path().join("acc");
        let entries: Vec<_> = std::fs::read_dir(&acc_dir).unwrap().filter_map(|e| e.ok()).collect();
        assert_eq!(entries.len(), 1);

        let content = std::fs::read_to_string(entries[0].path()).unwrap();
        let record: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(record["src"], "acc");
        assert_eq!(record["site"], "test-site");
        assert_eq!(record["payload_raw"], "ACC 12345");
        assert_eq!(record["fields"]["kiosk_ip"], "192.168.1.100");
        assert_eq!(record["fields"]["receipt_id"], "12345");
        assert_eq!(record["fields"]["pos_zone"], "pos1");
    }

    #[test]
    fn test_rs485_unified_schema() {
        let dir = tempdir().unwrap();
        let mut logger = AnalysisLogger::new(dir.path(), "test-site");

        logger.log_rs485("7f00010203", Some("closed"), true);
        logger.flush_all();

        let rs485_dir = dir.path().join("rs485");
        let entries: Vec<_> =
            std::fs::read_dir(&rs485_dir).unwrap().filter_map(|e| e.ok()).collect();
        assert_eq!(entries.len(), 1);

        let content = std::fs::read_to_string(entries[0].path()).unwrap();
        let record: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(record["src"], "rs485");
        assert_eq!(record["payload_raw"], "7f00010203");
        assert_eq!(record["fields"]["door_status"], "closed");
        assert_eq!(record["fields"]["checksum_ok"], true);
    }
}
