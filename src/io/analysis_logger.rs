//! Analysis Logger - JSONL writer for gateway-analysis diagnostic capture
//!
//! Provides per-topic JSONL file writing with daily rotation.
//! Each record includes receive timestamp, topic, raw payload, and parsed fields.

use chrono::Utc;
use serde::Serialize;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// MQTT log record written to JSONL files
#[derive(Debug, Serialize)]
pub struct MqttLogRecord<'a> {
    /// Receive timestamp (ISO 8601)
    pub ts_recv: &'a str,
    /// Site identifier
    pub site: &'a str,
    /// MQTT topic
    pub topic: &'a str,
    /// Raw payload as received
    pub payload_raw: &'a str,
    /// Parsed fields (best-effort, null if parsing failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<&'a serde_json::Value>,
}

/// ACC log record written to JSONL files
#[derive(Debug, Serialize)]
pub struct AccLogRecord<'a> {
    /// Receive timestamp (ISO 8601)
    pub ts_recv: &'a str,
    /// Site identifier
    pub site: &'a str,
    /// Source kiosk IP address
    pub kiosk_ip: &'a str,
    /// Raw line as received
    pub raw_line: &'a str,
    /// Parsed receipt ID (if present)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_id: Option<&'a str>,
    /// POS zone name from ip_to_pos mapping (if known)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pos_zone: Option<&'a str>,
}

/// Manages JSONL file writers per topic with daily rotation
pub struct AnalysisLogger {
    /// Base directory for log output
    log_dir: PathBuf,
    /// Site identifier included in each record
    site_id: String,
    /// Open file writers keyed by (subdirectory, date_suffix)
    writers: HashMap<(String, String), BufWriter<File>>,
    /// Current date string (YYYYMMDD) for rotation detection
    current_date: String,
}

impl AnalysisLogger {
    /// Create a new analysis logger
    pub fn new(log_dir: impl AsRef<Path>, site_id: impl Into<String>) -> Self {
        let log_dir = log_dir.as_ref().to_path_buf();
        let site_id = site_id.into();
        let current_date = Utc::now().format("%Y%m%d").to_string();

        info!(log_dir = %log_dir.display(), site_id = %site_id, "analysis_logger_initialized");

        Self { log_dir, site_id, writers: HashMap::new(), current_date }
    }

    /// Get the current date string and check for rotation
    fn check_date_rotation(&mut self) {
        let now_date = Utc::now().format("%Y%m%d").to_string();
        if now_date != self.current_date {
            info!(old_date = %self.current_date, new_date = %now_date, "date_rotation_detected");
            // Close all existing writers - they'll be reopened with new date
            self.writers.clear();
            self.current_date = now_date;
        }
    }

    /// Get or create a writer for a specific subdirectory and filename base
    fn get_writer(
        &mut self,
        subdir: &str,
        name_base: &str,
    ) -> std::io::Result<&mut BufWriter<File>> {
        self.check_date_rotation();

        let key = (subdir.to_string(), name_base.to_string());

        // Use entry API to avoid double lookup
        use std::collections::hash_map::Entry;
        match self.writers.entry(key) {
            Entry::Occupied(entry) => Ok(entry.into_mut()),
            Entry::Vacant(entry) => {
                let dir_path = self.log_dir.join(subdir);
                std::fs::create_dir_all(&dir_path)?;

                let filename = format!("{}-{}.jsonl", name_base, self.current_date);
                let file_path = dir_path.join(&filename);

                let file = OpenOptions::new().create(true).append(true).open(&file_path)?;
                info!(path = %file_path.display(), "opened_log_file");

                Ok(entry.insert(BufWriter::new(file)))
            }
        }
    }

    /// Log an MQTT message
    ///
    /// Writes to logs/mqtt/<topic_safe>-YYYYMMDD.jsonl
    /// Topic is sanitized: '/' becomes '-', other special chars removed
    pub fn log_mqtt(&mut self, topic: &str, payload: &str, parsed: Option<&serde_json::Value>) {
        let ts_recv = Utc::now().to_rfc3339();
        let topic_safe = sanitize_topic(topic);
        // Clone site_id to avoid borrow conflict with write_record's &mut self
        let site_id = self.site_id.clone();

        let record = MqttLogRecord {
            ts_recv: &ts_recv,
            site: &site_id,
            topic,
            payload_raw: payload,
            fields: parsed,
        };

        if let Err(e) = self.write_record("mqtt", &topic_safe, &record) {
            warn!(topic = %topic, error = %e, "mqtt_log_failed");
        } else {
            debug!(topic = %topic, bytes = payload.len(), "mqtt_logged");
        }
    }

    /// Log an ACC event
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

        let record = AccLogRecord {
            ts_recv: &ts_recv,
            site: &site_id,
            kiosk_ip,
            raw_line,
            receipt_id,
            pos_zone,
        };

        // Determine filename base: "acc" or sanitized kiosk IP
        let name_base = if split_per_kiosk {
            sanitize_topic(kiosk_ip)
        } else {
            "acc".to_string()
        };

        if let Err(e) = self.write_record("acc", &name_base, &record) {
            warn!(kiosk_ip = %kiosk_ip, error = %e, "acc_log_failed");
        } else {
            debug!(kiosk_ip = %kiosk_ip, receipt_id = ?receipt_id, "acc_logged");
        }
    }

    /// Write a serializable record to a specific subdir/name
    fn write_record<T: Serialize>(
        &mut self,
        subdir: &str,
        name_base: &str,
        record: &T,
    ) -> std::io::Result<()> {
        let json = serde_json::to_string(record)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        let writer = self.get_writer(subdir, name_base)?;
        writeln!(writer, "{}", json)?;
        writer.flush()?;

        Ok(())
    }

    /// Flush all open writers
    pub fn flush_all(&mut self) {
        for writer in self.writers.values_mut() {
            if let Err(e) = writer.flush() {
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

        // Parse and verify
        let record: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(record["site"], "test-site");
        assert_eq!(record["topic"], "gateway/events");
        assert!(record["ts_recv"].as_str().unwrap().contains("T"));
        assert!(record["payload_raw"].as_str().unwrap().contains("live_data"));
    }

    #[test]
    fn test_logger_handles_malformed_payload() {
        let dir = tempdir().unwrap();
        let mut logger = AnalysisLogger::new(dir.path(), "test-site");

        // Log malformed JSON - should still work with fields=None
        logger.log_mqtt("gateway/events", "not valid json {{{", None);
        logger.flush_all();

        let mqtt_dir = dir.path().join("mqtt");
        let entries: Vec<_> =
            std::fs::read_dir(&mqtt_dir).unwrap().filter_map(|e| e.ok()).collect();
        assert_eq!(entries.len(), 1);

        let content = std::fs::read_to_string(entries[0].path()).unwrap();
        let record: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(record["payload_raw"], "not valid json {{{");
        assert!(
            record.get("fields").is_none(),
            "fields should not be present for unparseable payload"
        );
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
}
