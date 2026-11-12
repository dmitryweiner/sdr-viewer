//! SDR Viewer: watches the air and shows what a person would miss.

mod cli;
mod pipeline;
mod reader;
mod snapshot;
mod writer;

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, SyncSender, channel, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use clap::Parser;
use ratatui::crossterm::event::{self, Event};
use sdr_core::SourceInfo;
use sdr_detect::Event as Detected;
use sdr_store::Store;
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
    let (commands_tx, commands_rx) = channel();
    let stop = Arc::new(AtomicBool::new(false));
    let db = cli::db_path(&args, |name| std::env::var(name).ok());
    let stored = match db.map(|path| keep_events(&path, &source.info(), updates_tx.clone())) {
        Some(Ok(stored)) => Some(stored),
        Some(Err(message)) => {
            eprintln!("sdr-viewer: {message}");
            return ExitCode::FAILURE;
        }
        None => None,
    };
    let (store_tx, writer) = stored.unzip();
    let reader = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || reader::run(source, updates_tx, commands_rx, stop, store_tx))
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
                    // Events come many times a second; they are drawn
                    // with the next frame or status, soon enough.
                    Update::Events(events) => {
                        app.log.on_events(events);
                        continue;
                    }
                    Update::OpenEvents(open) => {
                        app.log.set_open(open);
                        continue;
                    }
                    Update::Source(info) => app.source = info,
                    Update::Notice(notice) => app.notice = Some(notice),
                }
                dirty = true;
            }
            if dirty {
                terminal.draw(|frame| ui::draw(frame, &app))?;
                dirty = false;
            }
            if event::poll(TICK)? {
                match event::read()? {
                    Event::Key(key) => {
                        app.on_key(key);
                        for command in app.commands.drain(..) {
                            let _ = commands_tx.send(command);
                        }
                    }
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
    // The reader may be waiting for room to answer a command: let it go.
    drop(updates_rx);
    let _ = reader.join();
    // The reader has handed over its last events; let them reach the disk.
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("sdr-viewer: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Opens the event database at `path`, notes a session for `source`
/// beginning now, and starts the thread that writes what is sent to it. A
/// write that fails is told to the interface once, and writing stops.
fn keep_events(
    path: &Path,
    source: &SourceInfo,
    updates: SyncSender<Update>,
) -> Result<(Sender<Vec<Detected>>, JoinHandle<()>), String> {
    let wrap = |err: &dyn std::fmt::Display| {
        format!(
            "the event database {}: {err} (--no-db runs without it)",
            path.display()
        )
    };
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|err| wrap(&err))?;
    }
    let mut store = Store::open(path).map_err(|err| wrap(&err))?;
    let session = store
        .begin_session(SystemTime::now(), source)
        .map_err(|err| wrap(&err))?;
    let (tx, rx) = channel();
    let writer = std::thread::spawn(move || {
        let write = |events: &[Detected]| {
            let added = store.add_events(session, events);
            added.map_err(|err| err.to_string())
        };
        if let Err(message) = writer::run(rx, write) {
            let _ = updates.send(Update::Notice(format!("events are not kept: {message}")));
        }
    });
    Ok((tx, writer))
}
