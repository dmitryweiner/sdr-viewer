//! SDR Viewer: watches the air and shows what a person would miss.

mod cli;
mod reader;

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::sync_channel;
use std::time::Duration;

use clap::Parser;
use ratatui::crossterm::event::{self, Event};
use sdr_tui::{App, ui};

/// How long the interface waits for a key before redrawing.
const FRAME: Duration = Duration::from_millis(100);

fn main() -> ExitCode {
    let args = cli::Args::parse();
    let source = match cli::open_source(&args) {
        Ok(source) => source,
        Err(message) => {
            eprintln!("sdr-viewer: {message}");
            return ExitCode::FAILURE;
        }
    };
    let mut app = App::new(source.info());

    let (status_tx, status_rx) = sync_channel(8);
    let stop = Arc::new(AtomicBool::new(false));
    let reader = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || reader::run(source, status_tx, stop))
    };

    let mut terminal = ratatui::init();
    let result = (|| -> std::io::Result<()> {
        while !app.quit {
            if let Some(status) = status_rx.try_iter().last() {
                app.status = status;
            }
            terminal.draw(|frame| ui::draw(frame, &app))?;
            if event::poll(FRAME)?
                && let Event::Key(key) = event::read()?
            {
                app.on_key(key);
            }
        }
        Ok(())
    })();
    ratatui::restore();

    stop.store(true, Ordering::Relaxed);
    let _ = reader.join();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("sdr-viewer: {err}");
            ExitCode::FAILURE
        }
    }
}
