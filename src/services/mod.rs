//! Services - business logic and state management
//!
//! This module contains the core business logic services:
//! - `tracker` - Central event orchestrator and person state management
//! - `journey_manager` - Journey lifecycle management
//! - `stitcher` - Track identity stitching across sensor gaps
//! - `door_correlator` - Correlates gate commands with door state
//! - `reentry_detector` - Detects re-entry patterns
//! - `acc_collector` - ACC payment correlation
//! - `gate` - Gate controller interface
//! - `gate_worker` - Async gate command worker

pub mod acc_collector;
pub mod door_correlator;
pub mod gate;
pub mod gate_worker;
pub mod journey_manager;
pub mod reentry_detector;
pub mod stitcher;
pub mod tracker;

// Re-export commonly used types
pub use gate::GateController;
pub use gate_worker::{create_gate_worker, GateCmd, GateCmdWorker};
pub use tracker::Tracker;
