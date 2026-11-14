//! SDR Viewer: watches the air and shows what a person would miss.

mod channels;
mod cli;
mod fragment;
mod jobs;
mod keep;
mod pipeline;
mod reader;
mod save;
mod snapshot;
mod writer;

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, sync_channel};
use std::time::{Duration, SystemTime};

use clap::Parser;
use ratatui::crossterm::event::{self, Event};
use sdr_core::IqRing;
use sdr_tui::channels::ChannelsView;
use sdr_tui::log::WallClock;
use sdr_tui::{App, ui};

use crate::reader::{Sinks, Update};

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
    // The user's channels, before anything is shown: a file that does
    // not read is theirs to mend.
    let channels_path = cli::channels_path(&args, |name| std::env::var(name).ok());
    let listed = match channels_path.as_deref().map(channels::load) {
        Some(Ok(listed)) => listed,
        Some(Err(message)) => {
            eprintln!("sdr-viewer: {message}");
            return ExitCode::FAILURE;
        }
        None => Vec::new(),
    };
    if let Some(size) = args.snapshot {
        return match snapshot::run(source, size, args.secs, args.screen, listed) {
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
    app.channels = ChannelsView::new(listed);

    let (updates_tx, updates_rx) = sync_channel(64);
    let (commands_tx, commands_rx) = channel();
    let stop = Arc::new(AtomicBool::new(false));
    let db = cli::db_path(&args, |name| std::env::var(name).ok());
    let started = SystemTime::now();
    let kept = db.map(|path| keep::keep_events(&path, &source.info(), started, updates_tx.clone()));
    let (store_tx, writer) = match kept {
        Some(Ok(kept)) => {
            // With a database the log speaks in the time of day, and
            // starts with what earlier runs found.
            app.log.set_clock(WallClock {
                stream_start: started,
                utc_offset_s: chrono::Local::now().offset().local_minus_utc(),
            });
            let history = kept.history.into_iter();
            app.log
                .load_history(history.map(|stored| (stored.start, stored.event)));
            (Some(kept.events), Some(kept.writer))
        }
        Some(Err(message)) => {
            eprintln!("sdr-viewer: {message}");
            return ExitCode::FAILURE;
        }
        None => (None, None),
    };
    // The channels made for new signals go to the file by a thread of
    // their own.
    let (new_channels, saver) = channels_path
        .map(|path| channels::save(path, updates_tx.clone()))
        .unzip();
    // Samples are saved and pictured by a thread of their own; only a
    // receiver's stream runs by the wall clock.
    let (jobs_tx, jobs_rx) = channel();
    let worker = {
        let dir = cli::iq_dir(&args, |name| std::env::var(name).ok());
        let updates = updates_tx.clone();
        let clock = args.rtlsdr.then_some(started);
        std::thread::spawn(move || {
            jobs::run(jobs_rx, dir.as_deref(), clock, |update| {
                let _ = updates.send(update);
            });
        })
    };
    let sinks = Sinks {
        events: store_tx,
        ring: IqRing::for_seconds(args.iq_buffer, source.info().sample_rate),
        jobs: Some(jobs_tx),
    };
    let reader = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || reader::run(source, updates_tx, commands_rx, stop, sinks))
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
                    Update::Frame(frame, levels) => app.on_frame(frame, levels),
                    // Events come many times a second; they are drawn
                    // with the next frame or status, soon enough.
                    Update::Events(events) => {
                        app.on_events(events);
                        continue;
                    }
                    Update::OpenEvents(open) => {
                        app.on_open_events(open);
                        continue;
                    }
                    Update::Source(info) => app.source = info,
                    Update::Notice(notice) => app.notice = Some(notice),
                    Update::Done(done) => app.done = Some(done),
                    Update::Picture(event, fragment) => app.on_picture(&event, fragment),
                    Update::Density(map) => app.on_density(map),
                }
                dirty = true;
            }
            let made = app.channels.take_unsaved();
            if let (Some(new_channels), false) = (&new_channels, made.is_empty()) {
                let _ = new_channels.send(made);
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
    // And a recording asked for a moment ago be written to its end.
    let _ = worker.join();
    // And the channels made a moment ago be added to their file.
    drop(new_channels);
    if let Some(saver) = saver {
        let _ = saver.join();
    }
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("sdr-viewer: {err}");
            ExitCode::FAILURE
        }
    }
}
