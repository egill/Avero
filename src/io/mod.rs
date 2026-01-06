//! IO modules - external system interfaces
//!
//! This module contains all external IO operations:
//! - `mqtt` - MQTT client for receiving Xovis sensor data
//! - `mqtt_egress` - MQTT publisher for egress events
//! - `egress_channel` - Typed channel for MQTT egress messages
//! - `rs485` - Serial communication for door state monitoring
//! - `cloudplus` - TCP client for CloudPlus gate controller protocol
//! - `egress` - Journey output to file (JSONL format)
//! - `acc_listener` - TCP listener for ACC payment terminal events

pub mod acc_listener;
pub mod cloudplus;
pub mod egress;
pub mod egress_channel;
pub mod mqtt;
pub mod mqtt_egress;
pub mod rs485;

// Re-export commonly used types
pub use acc_listener::{start_acc_listener, AccListenerConfig};
pub use cloudplus::CloudPlusClient;
pub use egress::Egress;
pub use egress_channel::{
    create_egress_channel, AccEventPayload, EgressMessage, EgressSender, GateStatePayload,
    TrackEventPayload, ZoneEventPayload,
};
pub use mqtt_egress::MqttPublisher;
pub use rs485::Rs485Monitor;
