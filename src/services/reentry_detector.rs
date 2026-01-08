//! Re-entry detection for journey management
//!
//! Detects when a person who recently exited returns through the entry.
//! Matches based on:
//! - Time: new track within 30s of exit
//! - Height: within +/- 10cm of previous track

use std::time::{Duration, Instant};
use tracing::{debug, info};

/// Maximum time window for re-entry matching (30 seconds)
const MAX_REENTRY_WINDOW: Duration = Duration::from_secs(30);

/// Maximum height difference for re-entry matching (10 cm = 0.10 m)
const MAX_HEIGHT_DIFF: f64 = 0.10;

/// A recently exited journey for potential re-entry matching
#[derive(Debug, Clone)]
struct RecentExit {
    jid: String,
    pid: String,
    height: f64,
    exited_at: Instant,
}

/// Detected re-entry match
#[derive(Debug, Clone)]
pub struct ReentryMatch {
    pub parent_jid: String,
    pub parent_pid: String,
}

/// Detects re-entry by matching new entry tracks with recent exits
pub struct ReentryDetector {
    /// Recent exits for potential matching
    recent_exits: Vec<RecentExit>,
}

impl ReentryDetector {
    pub fn new() -> Self {
        Self { recent_exits: Vec::new() }
    }

    /// Record a journey exit for potential re-entry matching
    pub fn record_exit(&mut self, jid: &str, pid: &str, height: Option<f64>) {
        let Some(h) = height else {
            debug!(jid = %jid, "reentry_exit_no_height");
            return;
        };

        debug!(
            jid = %jid,
            pid = %pid,
            height = %h,
            "reentry_exit_recorded"
        );

        self.recent_exits.push(RecentExit {
            jid: jid.to_string(),
            pid: pid.to_string(),
            height: h,
            exited_at: Instant::now(),
        });
    }

    /// Try to match a new entry with a recent exit
    /// Returns Some if match found, None otherwise
    pub fn try_match(&mut self, height: Option<f64>) -> Option<ReentryMatch> {
        // Cleanup old exits first
        self.cleanup_old_exits();

        let h = height?;
        let now = Instant::now();

        // Find best match by height within time window
        let mut best_match: Option<(usize, f64)> = None;

        for (i, exit) in self.recent_exits.iter().enumerate() {
            let elapsed = now.duration_since(exit.exited_at);
            if elapsed > MAX_REENTRY_WINDOW {
                continue;
            }

            let height_diff = (exit.height - h).abs();
            if height_diff <= MAX_HEIGHT_DIFF {
                match best_match {
                    None => best_match = Some((i, height_diff)),
                    Some((_, best_diff)) if height_diff < best_diff => {
                        best_match = Some((i, height_diff));
                    }
                    _ => {}
                }
            }
        }

        if let Some((idx, height_diff)) = best_match {
            let exit = self.recent_exits.remove(idx);
            let elapsed_ms = now.duration_since(exit.exited_at).as_millis() as u64;

            info!(
                parent_jid = %exit.jid,
                parent_pid = %exit.pid,
                height_diff_cm = %(height_diff * 100.0) as i32,
                elapsed_ms = %elapsed_ms,
                "reentry_matched"
            );

            return Some(ReentryMatch { parent_jid: exit.jid, parent_pid: exit.pid });
        }

        debug!(height = %h, "reentry_no_match");
        None
    }

    /// Cleanup exits older than the matching window
    fn cleanup_old_exits(&mut self) {
        let now = Instant::now();
        self.recent_exits
            .retain(|exit| now.duration_since(exit.exited_at) <= MAX_REENTRY_WINDOW * 2);
    }

    /// Number of pending exits for matching
    #[cfg(test)]
    pub fn pending_count(&self) -> usize {
        self.recent_exits.len()
    }
}

impl Default for ReentryDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_exit() {
        let mut detector = ReentryDetector::new();

        detector.record_exit("jid1", "pid1", Some(1.75));

        assert_eq!(detector.pending_count(), 1);
    }

    #[test]
    fn test_record_exit_no_height() {
        let mut detector = ReentryDetector::new();

        detector.record_exit("jid1", "pid1", None);

        assert_eq!(detector.pending_count(), 0);
    }

    #[test]
    fn test_match_by_height() {
        let mut detector = ReentryDetector::new();

        detector.record_exit("jid1", "pid1", Some(1.75));

        // Match with same height
        let result = detector.try_match(Some(1.75));

        assert!(result.is_some());
        let matched = result.unwrap();
        assert_eq!(matched.parent_jid, "jid1");
        assert_eq!(matched.parent_pid, "pid1");
        assert_eq!(detector.pending_count(), 0); // Removed after match
    }

    #[test]
    fn test_match_within_height_tolerance() {
        let mut detector = ReentryDetector::new();

        detector.record_exit("jid1", "pid1", Some(1.75));

        // Match with height within 10cm
        let result = detector.try_match(Some(1.80)); // 5cm diff

        assert!(result.is_some());
        assert_eq!(result.unwrap().parent_jid, "jid1");
    }

    #[test]
    fn test_no_match_height_too_different() {
        let mut detector = ReentryDetector::new();

        detector.record_exit("jid1", "pid1", Some(1.75));

        // No match with height > 10cm different
        let result = detector.try_match(Some(1.90)); // 15cm diff

        assert!(result.is_none());
        assert_eq!(detector.pending_count(), 1); // Still pending
    }

    #[test]
    fn test_no_match_without_height() {
        let mut detector = ReentryDetector::new();

        detector.record_exit("jid1", "pid1", Some(1.75));

        let result = detector.try_match(None);

        assert!(result.is_none());
    }

    #[test]
    fn test_best_height_match() {
        let mut detector = ReentryDetector::new();

        detector.record_exit("jid1", "pid1", Some(1.75));
        detector.record_exit("jid2", "pid2", Some(1.80));

        // Should match jid2 (closer height)
        let result = detector.try_match(Some(1.79));

        assert!(result.is_some());
        assert_eq!(result.unwrap().parent_jid, "jid2");
    }

    #[test]
    fn test_no_match_timeout() {
        let mut detector = ReentryDetector::new();

        // Add exit with artificially old timestamp
        detector.recent_exits.push(RecentExit {
            jid: "jid1".to_string(),
            pid: "pid1".to_string(),
            height: 1.75,
            exited_at: Instant::now() - Duration::from_secs(60), // 60s ago
        });

        let result = detector.try_match(Some(1.75));

        assert!(result.is_none());
    }

    #[test]
    fn test_cleanup_old_exits() {
        let mut detector = ReentryDetector::new();

        // Add old exit
        detector.recent_exits.push(RecentExit {
            jid: "jid_old".to_string(),
            pid: "pid_old".to_string(),
            height: 1.75,
            exited_at: Instant::now() - Duration::from_secs(120), // 2 min ago
        });

        // Add recent exit
        detector.record_exit("jid_new", "pid_new", Some(1.80));

        // Trigger cleanup by matching with height that doesn't match anyone
        detector.try_match(Some(2.20)); // No match (40cm diff from both)

        // Only recent exit should remain
        assert_eq!(detector.pending_count(), 1);
        assert_eq!(detector.recent_exits[0].jid, "jid_new");
    }
}
