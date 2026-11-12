//! From spectrum frames to events.

pub mod detector;
pub mod floor;

pub use detector::{Detector, DetectorConfig, Event};
pub use floor::{FloorConfig, NoiseFloor};
