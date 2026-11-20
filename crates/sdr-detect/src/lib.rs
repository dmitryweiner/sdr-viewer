//! From spectrum frames to events.

pub mod alert;
pub mod channel;
pub mod density;
pub mod detector;
pub mod floor;

pub use alert::{Alert, Alerts, Rule};
pub use channel::{Channel, Channels};
pub use density::{Density, DensityConfig, DensityMap};
pub use detector::{Detector, DetectorConfig, Event, Levels};
pub use floor::{FloorConfig, NoiseFloor};
