//! The shared ground of the workspace: IQ sources and small helpers.

pub mod file;
pub mod generator;
pub mod pace;
pub mod rate;
mod rng;
pub mod source;
pub mod units;

pub use source::{Iq, Source, SourceError, SourceInfo};
