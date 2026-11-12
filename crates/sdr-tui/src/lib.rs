//! Screens, keys and widgets of the console interface.

pub mod app;
pub mod log;
mod log_ui;
pub mod spectrum;
mod spectrum_ui;
pub mod ui;

pub use app::{App, Command, Screen, SourceState, SourceStatus};
