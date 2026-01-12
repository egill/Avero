//! ACC (payment) event collection and IP→POS zone mapping
//!
//! This module provides IP address to POS zone mapping for ACC terminals.
//! The actual ACC matching logic is handled by PosOccupancyState in the tracker.

use crate::infra::config::Config;
use std::collections::HashMap;

/// Maps ACC terminal IP addresses to POS zone names
pub struct AccCollector {
    /// IP to POS name mapping
    ip_to_pos: HashMap<String, String>,
}

impl AccCollector {
    pub fn new(config: &Config) -> Self {
        Self { ip_to_pos: config.acc_ip_to_pos().clone() }
    }

    /// Get the POS name for an IP address
    pub fn pos_for_ip(&self, ip: &str) -> Option<&str> {
        self.ip_to_pos.get(ip).map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_collector() -> AccCollector {
        let mut ip_to_pos = std::collections::HashMap::new();
        ip_to_pos.insert("192.168.1.10".to_string(), "POS_1".to_string());
        ip_to_pos.insert("192.168.1.11".to_string(), "POS_2".to_string());
        let config = Config::default().with_acc_ip_to_pos(ip_to_pos);
        AccCollector::new(&config)
    }

    #[test]
    fn test_pos_for_ip() {
        let collector = create_test_collector();

        assert_eq!(collector.pos_for_ip("192.168.1.10"), Some("POS_1"));
        assert_eq!(collector.pos_for_ip("192.168.1.11"), Some("POS_2"));
        assert_eq!(collector.pos_for_ip("192.168.1.99"), None);
    }
}
