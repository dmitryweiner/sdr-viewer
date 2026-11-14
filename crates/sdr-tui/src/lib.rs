//! Screens, keys and widgets of the console interface.

pub mod app;
pub mod channels;
mod channels_ui;
pub mod density;
mod density_ui;
pub mod fragment;
pub mod log;
mod log_ui;
pub mod period;
mod period_ui;
pub mod spectrum;
mod spectrum_ui;
pub mod ui;

pub use app::{App, Command, Screen, SourceState, SourceStatus};
