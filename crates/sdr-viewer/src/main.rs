//! SDR Viewer: watches the air and shows what a person would miss.

mod cli;
mod reader;
mod snapshot;

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::sync_channel;
use std::time::Duration;

use clap::Parser;
use ratatui::crossterm::event::{self, Event};
use sdr_tui::{App, ui};

use crate::reader::Update;

/// How long the interface waits for a key before looking for new frames.
const TICK: Duration = Duration::from_millis(25);

fn main() -> ExitCode {
    let args = cli::Args::parse();
    let opened = cli::open_source(&args, args.snapshot.is_none());
    let source = match opened {
        Ok(source) => source,
        Err(message) => {
            eprintln!("sdr-viewer: {message}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(size) = args.snapshot {
        return match snapshot::run(source, size, args.secs) {
            Ok(text) => {
                print!("{text}");
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("sdr-viewer: {err}");
                ExitCode::FAILURE
            }
        };
    }
    let mut app = App::new(source.info());

    let (updates_tx, updates_rx) = sync_channel(64);
    let stop = Arc::new(AtomicBool::new(false));
    let reader = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || reader::run(source, updates_tx, stop))
    };

    let mut terminal = ratatui::init();
    let result = (|| -> std::io::Result<()> {
        // Draw only when something changed: a terminal emulator pays for
        // every frame.
        let mut dirty = true;
        while !app.quit {
            for update in updates_rx.try_iter() {
                match update {
                    Update::Status(status) => app.status = status,
                    Update::Frame(frame) => app.spectrum.on_frame(frame),
                }
                dirty = true;
            }
            if dirty {
                terminal.draw(|frame| ui::draw(frame, &app))?;
                dirty = false;
            }
            if event::poll(TICK)? {
                match event::read()? {
                    Event::Key(key) => app.on_key(key),
                    Event::Resize(..) => {}
                    _ => continue,
                }
                dirty = true;
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
