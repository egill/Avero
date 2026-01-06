//! Infrastructure - configuration, metrics, and broker
//!
//! This module contains infrastructure concerns:
//! - `config` - Application configuration (TOML loading, defaults)
//! - `metrics` - Lock-free metrics collection
//! - `broker` - Embedded MQTT broker (rumqttd)

pub mod broker;
pub mod config;
pub mod metrics;

// Re-export commonly used types
pub use config::{Config, GateMode};
pub use metrics::Metrics;
