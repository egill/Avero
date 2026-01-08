//! Integration tests for configuration loading

use gateway_poc::domain::types::GeometryId;
use gateway_poc::infra::{Config, GateMode};
use std::io::Write;
use tempfile::NamedTempFile;

#[test]
fn test_load_config_from_file() {
    let mut temp_file = NamedTempFile::new().unwrap();

    let config_content = r#"
[site]
id = "test-site"

[mqtt]
host = "test-host"
port = 1884
topic = "test/#"

[gate]
mode = "http"
tcp_addr = "192.168.1.100:8000"
http_url = "http://test-gate/open"
timeout_ms = 3000

[rs485]
device = "/dev/test"
baud = 9600
poll_interval_ms = 100

[zones]
pos_zones = [2001, 2002]
gate_zone = 2003
exit_line = 2004

[authorization]
min_dwell_ms = 5000

[metrics]
interval_secs = 15
prometheus_port = 9091
"#;

    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();

    let config = Config::from_file(temp_file.path()).unwrap();

    assert_eq!(config.site_id(), "test-site");
    assert_eq!(config.mqtt_host(), "test-host");
    assert_eq!(config.mqtt_port(), 1884);
    assert_eq!(config.gate_mode(), &GateMode::Http);
    assert_eq!(config.gate_zone(), GeometryId(2003));
    assert_eq!(config.min_dwell_ms(), 5000);
    assert_eq!(config.prometheus_port(), 9091);
}

#[test]
fn test_load_from_path_fallback() {
    let config = Config::load_from_path("/nonexistent/config.toml");
    assert_eq!(config.mqtt_host(), "localhost");
    assert_eq!(config.mqtt_port(), 1883);
    assert_eq!(config.gate_mode(), &GateMode::Tcp);
}
