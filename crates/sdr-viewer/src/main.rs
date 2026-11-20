//! SDR Viewer: watches the air and shows what a person would miss.

mod alerts;
mod channels;
mod cli;
mod fragment;
mod jobs;
mod keep;
mod mesh;
mod pipeline;
mod reader;
mod save;
mod snapshot;
mod writer;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use clap::Parser;
use ratatui::crossterm::event::{self, Event};
use sdr_core::{IqRing, Source, SourceInfo};
use sdr_detect::{Channel, Rule};
use sdr_mesh::update::MeshUpdate;
use sdr_tui::channels::ChannelsView;
use sdr_tui::log::WallClock;
use sdr_tui::{App, Command, Screen, SourceState, ui};

use crate::keep::MeshKept;
use crate::reader::{Sinks, Update};

/// How long the interface waits for a key before looking for new frames.
const TICK: Duration = Duration::from_millis(25);

fn main() -> ExitCode {
    let args = cli::Args::parse();
    match run(&args) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("sdr-viewer: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &cli::Args) -> Result<ExitCode, String> {
    if args.screen == Screen::Mesh {
        let Some(addr) = &args.mesh else {
            return Err("the Meshtastic pulse (--screen 6) needs --mesh".into());
        };
        let size = args.snapshot.unwrap_or((160, 45));
        print!("{}", mesh::snapshot(addr, size, args.secs)?);
        return Ok(ExitCode::SUCCESS);
    }
    let source = if cli::wants_source(args) {
        Some(cli::open_source(args, args.snapshot.is_none())?)
    } else {
        None
    };
    // The user's channels, before anything is shown: a file that does
    // not read is theirs to mend.
    let channels_path = cli::channels_path(args, |name| std::env::var(name).ok());
    let listed = match (&source, channels_path.as_deref()) {
        (Some(_), Some(path)) => channels::load(path)?,
        _ => Vec::new(),
    };
    // And the rules of their alerts: a snapshot makes no file.
    let alerts_path = cli::alerts_path(args, |name| std::env::var(name).ok());
    let rules = match (&source, alerts_path.as_deref()) {
        (Some(_), Some(path)) => alerts::load(path, args.snapshot.is_none())?,
        _ => Vec::new(),
    };
    if let Some(size) = args.snapshot {
        let Some(source) = source else {
            return Err("a snapshot of this screen needs a recording or --rtlsdr".into());
        };
        let text = snapshot::run(source, size, args.secs, args.screen, listed, rules)
            .map_err(|err| err.to_string())?;
        print!("{text}");
        return Ok(ExitCode::SUCCESS);
    }

    let (updates_tx, updates_rx) = sync_channel(64);
    let stop = Arc::new(AtomicBool::new(false));
    // The only place the time zone is asked for: the log's time of day
    // and the pulse's clock.
    let utc_offset_s = chrono::Local::now().offset().local_minus_utc();
    let mut app = match &source {
        Some(source) => App::new(source.info()),
        None => {
            let mut app = App::new(SourceInfo {
                sample_rate: 0,
                center_hz: 0.0,
                description: "mesh only".into(),
            });
            app.status.state = SourceState::Off;
            app.screen = Screen::Mesh;
            app
        }
    };
    let reception = match source {
        Some(source) => {
            let parts = (listed, channels_path, rules);
            Some(Reception::start(
                source,
                args,
                &mut app,
                parts,
                (&updates_tx, &stop),
                utc_offset_s,
            )?)
        }
        None => None,
    };
    let mut kept_mesh = None;
    if let Some(addr) = &args.mesh {
        app.mesh = mesh::view(addr, utc_offset_s);
        // What earlier runs kept, before the node answers.
        if let Some(path) = cli::mesh_db_path(args, |name| std::env::var(name).ok()) {
            let kept = keep::keep_mesh(&path, mesh::unix_now(), updates_tx.clone())?;
            app.mesh
                .load_history(mesh::for_the_pulse(kept.history, kept.texts));
            app.mesh.kept = true;
            kept_mesh = Some((kept.told, kept.writer));
        }
        // The client keeps nothing itself: it is not waited for at the end.
        mesh::start(addr, updates_tx.clone(), Arc::clone(&stop));
    }
    drop(updates_tx);

    let result = interface(
        &mut app,
        &updates_rx,
        reception.as_ref(),
        args.mesh.is_some(),
        kept_mesh.as_ref().map(|(told, _)| told),
    );

    stop.store(true, Ordering::Relaxed);
    // The reader may be waiting for room to answer a command: let it go.
    drop(updates_rx);
    if let Some((told, writer)) = kept_mesh {
        // The app listened until now.
        let quit = MeshUpdate::Lost("the app quit".into());
        let _ = told.send(vec![MeshKept::told(mesh::unix_now(), quit)]);
        drop(told);
        let _ = writer.join();
    }
    if let Some(reception) = reception {
        reception.finish();
    }
    match result {
        Ok(()) => Ok(ExitCode::SUCCESS),
        Err(err) => Err(err.to_string()),
    }
}

/// The terminal's loop, until the user quits. What the mesh's client tells
/// is handed on to `kept_mesh` too, with the time, and so is the erasing
/// of the nodes.
fn interface(
    app: &mut App,
    updates: &Receiver<Update>,
    reception: Option<&Reception>,
    mesh: bool,
    kept_mesh: Option<&Sender<Vec<MeshKept>>>,
) -> std::io::Result<()> {
    let mut terminal = ratatui::init();
    let result = (|| -> std::io::Result<()> {
        // Draw only when something changed: a terminal emulator pays for
        // every frame.
        let mut dirty = true;
        let mut second = 0;
        while !app.quit {
            let mut told = Vec::new();
            for update in updates.try_iter() {
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
                    Update::Alerts(alerts) => {
                        if let (true, Some(reception)) = (app.on_alerts(&alerts), reception) {
                            let _ = reception.tone.send(());
                        }
                    }
                    Update::Source(info) => app.source = info,
                    Update::Notice(notice) => app.notice = Some(notice),
                    Update::Done(done) => app.done = Some(done),
                    Update::Picture(event, fragment) => app.on_picture(&event, fragment),
                    Update::Density(map) => app.on_density(map),
                    Update::Mesh(update) => {
                        if kept_mesh.is_some() {
                            told.push(MeshKept::told(mesh::unix_now(), update.clone()));
                        }
                        app.on_mesh(update);
                    }
                }
                dirty = true;
            }
            if let (Some(kept_mesh), false) = (kept_mesh, told.is_empty()) {
                let _ = kept_mesh.send(told);
            }
            if let Some(reception) = reception {
                reception.save_channels(app);
            }
            // The pulse moves with the wall clock, a second at a time.
            let now = mesh::unix_now();
            if mesh && now != second {
                second = now;
                app.set_wall_clock(now);
                dirty |= app.screen == Screen::Mesh;
            }
            if dirty {
                terminal.draw(|frame| ui::draw(frame, app))?;
                dirty = false;
            }
            if event::poll(TICK)? {
                match event::read()? {
                    Event::Key(key) => {
                        app.on_key(key);
                        for command in app.commands.drain(..) {
                            if command == Command::EraseMesh {
                                if let Some(kept_mesh) = kept_mesh {
                                    let erase = MeshKept::EraseAll(mesh::unix_now());
                                    let _ = kept_mesh.send(vec![erase]);
                                }
                            } else if let Some(reception) = reception {
                                let _ = reception.commands.send(command);
                            }
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
    result
}

/// The threads of a source: the reader, the store's writer, the worker
/// that saves and pictures samples, the channels' saver, the alerts'
/// tone (not waited for at the end).
struct Reception {
    commands: Sender<Command>,
    /// Asks for the tone.
    tone: Sender<()>,
    reader: JoinHandle<()>,
    writer: Option<JoinHandle<()>>,
    worker: JoinHandle<()>,
    new_channels: Option<Sender<Vec<sdr_tui::channels::Change>>>,
    saver: Option<JoinHandle<()>>,
}

impl Reception {
    fn start(
        source: Box<dyn Source>,
        args: &cli::Args,
        app: &mut App,
        (listed, channels_path, rules): (Vec<Channel>, Option<PathBuf>, Vec<Rule>),
        (updates_tx, stop): (&SyncSender<Update>, &Arc<AtomicBool>),
        utc_offset_s: i32,
    ) -> Result<Self, String> {
        app.channels = ChannelsView::new(listed);
        let (commands_tx, commands_rx) = channel();
        // The rules hold from the first samples on.
        for rule in rules {
            let _ = commands_tx.send(Command::AddRule(rule));
        }
        let (tone, _) = alerts::start_tone(alerts::aplay, updates_tx.clone());
        let db = cli::db_path(args, |name| std::env::var(name).ok());
        let started = SystemTime::now();
        let kept =
            db.map(|path| keep::keep_events(&path, &source.info(), started, updates_tx.clone()));
        let (store_tx, writer) = match kept.transpose()? {
            Some(kept) => {
                // With a database the log speaks in the time of day, and
                // starts with what earlier runs found.
                app.log.set_clock(WallClock {
                    stream_start: started,
                    utc_offset_s,
                });
                let history = kept.history.into_iter();
                app.log
                    .load_history(history.map(|stored| (stored.start, stored.event)));
                (Some(kept.events), Some(kept.writer))
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
            let dir = cli::iq_dir(args, |name| std::env::var(name).ok());
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
            let stop = Arc::clone(stop);
            let updates = updates_tx.clone();
            std::thread::spawn(move || reader::run(source, updates, commands_rx, stop, sinks))
        };
        Ok(Reception {
            commands: commands_tx,
            tone,
            reader,
            writer,
            worker,
            new_channels,
            saver,
        })
    }

    /// Hands the channels made since the last look to their saver.
    fn save_channels(&self, app: &mut App) {
        let changes = app.channels.take_changes();
        if let (Some(new_channels), false) = (&self.new_channels, changes.is_empty()) {
            let _ = new_channels.send(changes);
        }
    }

    /// Waits for what must reach the disk.
    fn finish(self) {
        let _ = self.reader.join();
        // The reader has handed over its last events; let them reach the disk.
        if let Some(writer) = self.writer {
            let _ = writer.join();
        }
        // And a recording asked for a moment ago be written to its end.
        let _ = self.worker.join();
        // And the channels made a moment ago be added to their file.
        drop(self.new_channels);
        if let Some(saver) = self.saver {
            let _ = saver.join();
        }
    }
}
