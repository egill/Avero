//! Lock-free metrics collection and periodic reporting
//!
//! Uses atomics for hot-path operations to avoid mutex contention.
//! All counter updates are lock-free; reporting is the only operation
//! that needs synchronization (via atomic swap).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tracing::info;

/// Lock-free metrics collector
///
/// All recording operations are lock-free using atomics.
/// The `report()` method atomically swaps counters to get a consistent snapshot.
pub struct Metrics {
    /// Total events ever processed (monotonic)
    events_total: AtomicU64,
    /// Events since last report (reset on report)
    events_since_report: AtomicU64,
    /// Sum of latencies in microseconds (reset on report)
    latency_sum_us: AtomicU64,
    /// Max latency in microseconds (reset on report)
    latency_max_us: AtomicU64,
    /// Total gate commands sent (monotonic)
    gate_commands_sent: AtomicU64,
    /// Last report time (only accessed from reporter, not atomic)
    last_report_time: std::sync::Mutex<Instant>,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            events_total: AtomicU64::new(0),
            events_since_report: AtomicU64::new(0),
            latency_sum_us: AtomicU64::new(0),
            latency_max_us: AtomicU64::new(0),
            gate_commands_sent: AtomicU64::new(0),
            last_report_time: std::sync::Mutex::new(Instant::now()),
        }
    }

    /// Record an event was processed with given latency (lock-free)
    #[inline]
    pub fn record_event_processed(&self, latency_us: u64) {
        self.events_total.fetch_add(1, Ordering::Relaxed);
        self.events_since_report.fetch_add(1, Ordering::Relaxed);
        self.latency_sum_us.fetch_add(latency_us, Ordering::Relaxed);

        // Update max using compare-and-swap loop
        let mut current_max = self.latency_max_us.load(Ordering::Relaxed);
        while latency_us > current_max {
            match self.latency_max_us.compare_exchange_weak(
                current_max,
                latency_us,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current_max = actual,
            }
        }
    }

    /// Record a gate command was sent (lock-free)
    #[inline]
    pub fn record_gate_command(&self) {
        self.gate_commands_sent.fetch_add(1, Ordering::Relaxed);
    }

    /// Get total events processed
    #[inline]
    pub fn events_total(&self) -> u64 {
        self.events_total.load(Ordering::Relaxed)
    }

    /// Calculate and return metrics summary, then reset periodic counters
    ///
    /// This is the only method that resets counters. It uses atomic swap
    /// to get a consistent snapshot while allowing concurrent updates.
    pub fn report(&self, active_tracks: usize, authorized_tracks: usize) -> MetricsSummary {
        // Swap periodic counters to zero and get their values
        let events_count = self.events_since_report.swap(0, Ordering::Relaxed);
        let latency_sum = self.latency_sum_us.swap(0, Ordering::Relaxed);
        let max_latency = self.latency_max_us.swap(0, Ordering::Relaxed);

        // Get monotonic counters (don't reset)
        let events_total = self.events_total.load(Ordering::Relaxed);
        let gate_commands = self.gate_commands_sent.load(Ordering::Relaxed);

        // Calculate elapsed time and reset
        let elapsed = {
            let mut last = self.last_report_time.lock().unwrap();
            let elapsed = last.elapsed();
            *last = Instant::now();
            elapsed
        };

        // Calculate derived metrics
        let events_per_sec = if elapsed.as_secs_f64() > 0.0 {
            events_count as f64 / elapsed.as_secs_f64()
        } else {
            0.0
        };

        let avg_latency = if events_count > 0 {
            latency_sum / events_count
        } else {
            0
        };

        MetricsSummary {
            events_total,
            events_per_sec,
            avg_process_latency_us: avg_latency,
            max_process_latency_us: max_latency,
            active_tracks,
            authorized_tracks,
            gate_commands_sent: gate_commands,
        }
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

// Metrics is Send + Sync because all fields are atomic or Mutex-wrapped
unsafe impl Send for Metrics {}
unsafe impl Sync for Metrics {}

#[derive(Debug)]
#[allow(dead_code)]
pub struct MetricsSummary {
    pub events_total: u64,
    pub events_per_sec: f64,
    pub avg_process_latency_us: u64,
    pub max_process_latency_us: u64,
    pub active_tracks: usize,
    pub authorized_tracks: usize,
    pub gate_commands_sent: u64,
}

impl MetricsSummary {
    pub fn log(&self) {
        info!(
            events_total = %self.events_total,
            events_per_sec = format!("{:.1}", self.events_per_sec),
            avg_process_latency_us = %self.avg_process_latency_us,
            max_process_latency_us = %self.max_process_latency_us,
            active_tracks = %self.active_tracks,
            authorized_tracks = %self.authorized_tracks,
            gate_commands_sent = %self.gate_commands_sent,
            "metrics"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_new() {
        let metrics = Metrics::new();
        assert_eq!(metrics.events_total(), 0);
        assert_eq!(metrics.gate_commands_sent.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_record_event() {
        let metrics = Metrics::new();

        metrics.record_event_processed(100);
        assert_eq!(metrics.events_total(), 1);
        assert_eq!(metrics.latency_sum_us.load(Ordering::Relaxed), 100);

        metrics.record_event_processed(200);
        assert_eq!(metrics.events_total(), 2);
        assert_eq!(metrics.latency_sum_us.load(Ordering::Relaxed), 300);
    }

    #[test]
    fn test_record_gate_command() {
        let metrics = Metrics::new();

        metrics.record_gate_command();
        assert_eq!(metrics.gate_commands_sent.load(Ordering::Relaxed), 1);

        metrics.record_gate_command();
        assert_eq!(metrics.gate_commands_sent.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_report() {
        let metrics = Metrics::new();

        metrics.record_event_processed(100);
        metrics.record_event_processed(200);
        metrics.record_event_processed(300);
        metrics.record_gate_command();

        let summary = metrics.report(5, 2);

        assert_eq!(summary.events_total, 3);
        assert_eq!(summary.avg_process_latency_us, 200); // (100+200+300)/3
        assert_eq!(summary.max_process_latency_us, 300);
        assert_eq!(summary.active_tracks, 5);
        assert_eq!(summary.authorized_tracks, 2);
        assert_eq!(summary.gate_commands_sent, 1);

        // Periodic counters should be reset
        assert_eq!(metrics.events_since_report.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.latency_sum_us.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.latency_max_us.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_report_empty() {
        let metrics = Metrics::new();
        let summary = metrics.report(0, 0);

        assert_eq!(summary.events_total, 0);
        assert_eq!(summary.avg_process_latency_us, 0);
        assert_eq!(summary.max_process_latency_us, 0);
    }

    #[test]
    fn test_max_latency_tracking() {
        let metrics = Metrics::new();

        metrics.record_event_processed(100);
        metrics.record_event_processed(500);
        metrics.record_event_processed(200);
        metrics.record_event_processed(50);

        assert_eq!(metrics.latency_max_us.load(Ordering::Relaxed), 500);
    }

    #[test]
    fn test_concurrent_updates() {
        use std::sync::Arc;
        use std::thread;

        let metrics = Arc::new(Metrics::new());
        let mut handles = vec![];

        // Spawn 10 threads, each recording 1000 events
        for _ in 0..10 {
            let m = metrics.clone();
            handles.push(thread::spawn(move || {
                for i in 0..1000 {
                    m.record_event_processed(i as u64);
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(metrics.events_total(), 10_000);
    }
}
