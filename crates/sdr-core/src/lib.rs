//! The shared ground of the workspace: IQ sources and small helpers.

pub mod file;
pub mod frame;
pub mod generator;
pub mod pace;
pub mod rate;
pub mod ring;
mod rng;
pub mod source;
pub mod units;

pub use frame::SpectrumFrame;
pub use ring::IqRing;
pub use source::{GainChange, Iq, Source, SourceError, SourceInfo};
