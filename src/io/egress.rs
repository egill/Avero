//! Journey egress - writes completed journeys to file
//!
//! Journeys are written in JSONL format (one JSON object per line)
//! to the file specified in config.

use crate::domain::journey::Journey;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use tracing::{debug, error, info};

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
    pub fn write_journey(&self, journey: &Journey) -> bool {
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

    /// Write multiple journeys
    pub fn write_journeys(&self, journeys: &[Journey]) -> usize {
        let mut success_count = 0;
        for journey in journeys {
            if self.write_journey(journey) {
                success_count += 1;
            }
        }
        success_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::journey::{Journey, JourneyEvent, JourneyOutcome};
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

        let mut journey = Journey::new(100);
        journey.authorized = true;
        journey.total_dwell_ms = 7500;
        journey.crossed_entry = true;
        journey.add_event(JourneyEvent::new("entry_cross", 1234567890));
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
        let mut journey1 = Journey::new(100);
        journey1.crossed_entry = true;
        journey1.complete(JourneyOutcome::Completed);
        egress.write_journey(&journey1);

        // Write second journey
        let mut journey2 = Journey::new(200);
        journey2.crossed_entry = true;
        journey2.complete(JourneyOutcome::Abandoned);
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
                let mut j = Journey::new(100 + i);
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

        let mut journey = Journey::new(100);
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

        let mut journey = Journey::new(100);
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
