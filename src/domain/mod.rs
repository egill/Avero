//! Domain models - core business types and journey model
//!
//! This module contains the canonical data types used throughout the system:
//! - `Journey` - the primary business entity representing a customer's path
//! - `JourneyEvent` - events that occur during a journey
//! - `ParsedEvent` - sensor events from Xovis/RS485
//! - `Person` - tracked individual in the store
//! - `EventType` - classification of sensor events

pub mod journey;
pub mod types;

// Re-export commonly used types at module level
