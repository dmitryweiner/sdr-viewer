//! The thread that pulls samples from the source, so the interface never
//! waits for the radio.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError};
use std::time::{Duration, Instant};

use sdr_core::rate::RateMeter;
use sdr_core::{Iq, IqRing, Source, SourceInfo, SpectrumFrame};
use sdr_detect::{DensityMap, Event, Levels};
use sdr_tui::fragment::Fragment;
use sdr_tui::{Command, SourceState, SourceStatus};

use crate::pipeline::{Output, Pipeline};

/// Samples per read: ~7 ms at 2.4 MS/s.
const CHUNK: usize = 16 * 1024;

/// How often the measured rate is reported.
const REPORT_EVERY: Duration = Duration::from_millis(500);

/// Ended events kept for an interface that is not taking them; beyond
/// this the oldest are lost.
const MAX_WAITING_EVENTS: usize = 10_000;

/// What the reader tells the interface.
#[derive(Debug, PartialEq)]
pub enum Update {
    Status(SourceStatus),
    /// A frame for the screen with the detector's floor under it.
    Frame(SpectrumFrame, Levels),
    /// Events that have ended, in the order they ended.
    Events(Vec<Event>),
    /// The events going on now, as far as they have got.
    OpenEvents(Vec<Event>),
    /// The source after a command changed it.
    Source(SourceInfo),
    /// A command that could not be carried out, and why.
    Notice(String),
    /// What was asked for and took a while is done.
    Done(String),
    /// The picture of this event, or none: its samples are not held.
    Picture(Event, Option<Fragment>),
    /// How often each frequency was at each level, with every frame
    /// while the interface watches it.
    Density(DensityMap),
}

/// What the store is told, in the order it happened.
#[derive(Debug, Clone, PartialEq)]
pub enum Kept {
    /// An event has ended: keep it.
    Event(Event),
    /// The user erased the log: delete what is kept.
    EraseAll,
}

/// Work on an event's samples that is not the reader's to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Job {
    /// Write them to the disk.
    Save(Cut),
    /// Make the event's picture of them.
    Picture(Cut),
}

/// Samples around an event, cut from the ring.
#[derive(Debug, Clone, PartialEq)]
pub struct Cut {
    pub samples: Vec<[u8; 2]>,
    /// The number of the first of them in the stream.
    pub first_sample: u64,
    /// The source as it received them.
    pub source: SourceInfo,
    pub event: Event,
}

/// Kept before and after an event that is saved: how it began and ended
/// is a part of it.
const AROUND_EVENT: Duration = Duration::from_millis(50);

/// The most of an event its picture is made of: its beginning. Copying
/// all 30 s of a carrier out of the ring for every row the selection
/// passes would hold the reader up.
const PICTURE_LIMIT: Duration = Duration::from_secs(3);

/// Where the reader puts what is to be kept; each may be missing.
#[derive(Default)]
pub struct Sinks {
    /// Gets every ended event, and when reading stops — however it
    /// stops — the events still on, ended there.
    pub events: Option<Sender<Vec<Kept>>>,
    /// Holds the latest samples, for an event's picture and for saving.
    pub ring: IqRing,
    /// Gets the samples cut out for an event, with what to do with them.
    pub jobs: Option<Sender<Job>>,
}

/// Reads until the source ends or fails, or `stop` is set, sending spectrum
/// frames and the source's status to `updates`, and carrying out `commands`
/// between reads. Frames and status the interface has no room for are
/// dropped: reception never waits for drawing. Ended events wait for room
/// instead, and the last of them go out before the end is reported.
pub fn run(
    mut source: impl Source,
    updates: SyncSender<Update>,
    commands: Receiver<Command>,
    stop: Arc<AtomicBool>,
    sinks: Sinks,
) {
    let Sinks {
        events: store,
        mut ring,
        jobs,
    } = sinks;
    let started = Instant::now();
    let mut meter = RateMeter::new(REPORT_EVERY);
    let mut buf = vec![Iq::new(0.0, 0.0); CHUNK];
    let mut pipeline = Pipeline::new(source.info().sample_rate);
    // Ended events: those of the latest read, and those the interface has
    // not taken yet.
    let mut fresh: Vec<Event> = Vec::new();
    let mut events: Vec<Event> = Vec::new();
    // Samples at full scale and all samples since the last report.
    let (mut clipped, mut seen) = (0u64, 0u64);
    let status = |state, rate, clipped: f64, lost| {
        Update::Status(SourceStatus {
            rate,
            state,
            clipped,
            lost,
        })
    };
    let share = |clipped: u64, seen: u64| clipped as f64 / seen.max(1) as f64;
    let keep = |events: Vec<Event>| {
        if let (Some(store), false) = (&store, events.is_empty()) {
            let _ = store.send(events.into_iter().map(Kept::Event).collect());
        }
    };
    while !stop.load(Ordering::Relaxed) {
        for command in commands.try_iter() {
            let done = match command {
                Command::Tune(center_hz) => match source.tune(center_hz) {
                    Ok(()) => {
                        // What is held was another band.
                        ring.clear();
                        Ok(Some(Update::Source(source.info())))
                    }
                    Err(err) => Err(err.to_string()),
                },
                Command::Gain(change) => match source.set_gain(change) {
                    Ok(()) => Ok(Some(Update::Source(source.info()))),
                    Err(err) => Err(err.to_string()),
                },
                // Whoever writes the file says when it is done.
                Command::SaveIq(event) => {
                    hand_over(&ring, event, source.info(), jobs.as_ref(), None, Job::Save)
                        .map(|()| None)
                }
                // No picture is an answer, not a complaint: the selection
                // just went over an event whose samples are gone.
                Command::Picture(event) => {
                    let asked = event.clone();
                    let limit = Some(PICTURE_LIMIT);
                    let handed = hand_over(
                        &ring,
                        event,
                        source.info(),
                        jobs.as_ref(),
                        limit,
                        Job::Picture,
                    );
                    Ok(handed.err().map(|_| Update::Picture(asked, None)))
                }
                Command::WatchDensity(watched) => {
                    pipeline.watch_density(watched);
                    Ok(None)
                }
                Command::ResetDensity => {
                    pipeline.reset_density();
                    Ok(None)
                }
                Command::EraseEvents => {
                    // Those the interface has not taken are erased too;
                    // the store has had them already, before this word.
                    events.clear();
                    if let Some(store) = &store {
                        let _ = store.send(vec![Kept::EraseAll]);
                    }
                    Ok(None)
                }
            };
            // The answer to a key press must not get lost: wait for room.
            let _ = match done {
                Ok(Some(update)) => updates.send(update),
                Ok(None) => Ok(()),
                Err(why) => updates.send(Update::Notice(why)),
            };
        }
        match source.read(&mut buf) {
            Ok(0) => {
                if !events.is_empty() {
                    let _ = updates.send(Update::Events(events));
                }
                let _ = updates.send(Update::OpenEvents(pipeline.open_events()));
                // The end must not get lost either: wait for room.
                let state = status(
                    SourceState::Ended,
                    0.0,
                    share(clipped, seen),
                    source.lost_samples(),
                );
                let _ = updates.send(state);
                break;
            }
            Ok(n) => {
                clipped += sdr_core::clip::clipped(&buf[..n]) as u64;
                seen += n as u64;
                ring.push(&buf[..n]);
                pipeline.push(&buf[..n], &source.info(), |output| match output {
                    Output::Frame(frame, levels) => {
                        let _ = updates.try_send(Update::Frame(frame, levels));
                    }
                    Output::Event(event) => fresh.push(event),
                    Output::Density(map) => {
                        let _ = updates.try_send(Update::Density(map));
                    }
                });
                keep(fresh.clone());
                events.append(&mut fresh);
                if !events.is_empty() {
                    let batch = Update::Events(std::mem::take(&mut events));
                    if let Err(TrySendError::Full(Update::Events(waiting))) =
                        updates.try_send(batch)
                    {
                        events = waiting;
                        let extra = events.len().saturating_sub(MAX_WAITING_EVENTS);
                        events.drain(..extra);
                    }
                }
                if meter.record(started.elapsed(), n as u64) {
                    let _ = updates.try_send(Update::OpenEvents(pipeline.open_events()));
                    let state = status(
                        SourceState::Running,
                        meter.rate(),
                        share(clipped, seen),
                        source.lost_samples(),
                    );
                    let _ = updates.try_send(state);
                    (clipped, seen) = (0, 0);
                }
            }
            Err(err) => {
                let state = status(
                    SourceState::Failed(err.to_string()),
                    0.0,
                    share(clipped, seen),
                    source.lost_samples(),
                );
                let _ = updates.send(state);
                break;
            }
        }
    }
    keep(pipeline.finish());
}

/// Cuts the samples of `event` out of the ring and hands them over as a
/// job: the copy is quick, the disk and the FFTs are not the reader's to
/// wait for.
fn hand_over(
    ring: &IqRing,
    event: Event,
    source: SourceInfo,
    jobs: Option<&Sender<Job>>,
    limit: Option<Duration>,
    job: impl FnOnce(Cut) -> Job,
) -> Result<(), String> {
    let Some(jobs) = jobs else {
        return Err("nothing here works on samples".into());
    };
    if ring.capacity() == 0 {
        return Err("no samples are held: --iq-buffer is 0".into());
    }
    let rate = f64::from(source.sample_rate);
    let sample = |time: Duration| (time.as_secs_f64() * rate).round() as u64;
    let held = ring.held();
    let (start, end) = (sample(event.start), sample(event.start + event.duration));
    if end <= held.start || start >= held.end {
        return Err(format!(
            "this event's samples are gone: the last {} s on this frequency are held",
            ring.capacity() as f64 / rate
        ));
    }
    let around = sample(AROUND_EVENT);
    let first_sample = start.saturating_sub(around).max(held.start);
    let end = (end + around).min(limit.map_or(u64::MAX, |limit| first_sample + sample(limit)));
    let cut = Cut {
        samples: ring.copy(first_sample..end),
        first_sample,
        source,
        event,
    };
    jobs.send(job(cut))
        .map_err(|_| "the thread that works on samples has stopped".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::FFT_SIZE;
    use sdr_core::file::{FileSource, SampleFormat};
    use sdr_core::generator::{Generator, GeneratorConfig};
    use sdr_core::{GainChange, SourceError};
    use std::io::Cursor;
    use std::sync::mpsc::{Sender, channel, sync_channel};

    fn info() -> SourceInfo {
        SourceInfo {
            sample_rate: 1000,
            center_hz: 1e6,
            description: "test".into(),
        }
    }

    fn events_to(store: Sender<Vec<Kept>>) -> Sinks {
        Sinks {
            events: Some(store),
            ..Sinks::default()
        }
    }

    fn updates(source: impl Source) -> Vec<Update> {
        updates_after(source, &[])
    }

    /// Everything the reader sends when `commands` are waiting at its start.
    fn updates_after(source: impl Source, commands: &[Command]) -> Vec<Update> {
        let (tx, rx) = sync_channel(64);
        let (command_tx, command_rx) = channel();
        for command in commands {
            command_tx.send(command.clone()).unwrap();
        }
        run(
            source,
            tx,
            command_rx,
            Arc::new(AtomicBool::new(false)),
            Sinks::default(),
        );
        rx.try_iter().collect()
    }

    /// A generator that ends after a number of reads.
    struct Brief(Generator, usize);

    impl Source for Brief {
        fn info(&self) -> SourceInfo {
            self.0.info()
        }
        fn read(&mut self, buf: &mut [Iq]) -> Result<usize, SourceError> {
            if self.1 == 0 {
                return Ok(0);
            }
            self.1 -= 1;
            self.0.read(buf)
        }
        fn tune(&mut self, center_hz: f64) -> Result<(), SourceError> {
            self.0.tune(center_hz)
        }
    }

    #[test]
    fn a_tune_command_moves_the_source_and_the_frames_that_follow() {
        let source = Brief(Generator::new(GeneratorConfig::demo()), 40);
        let got = updates_after(source, &[Command::Tune(100e6)]);
        match &got[0] {
            Update::Source(info) => assert_eq!(info.center_hz, 100e6),
            other => panic!("{other:?}"),
        }
        let centers: Vec<f64> = got
            .iter()
            .filter_map(|update| match update {
                Update::Frame(frame, _) => Some(frame.center_hz),
                _ => None,
            })
            .collect();
        assert!(!centers.is_empty());
        assert!(centers.iter().all(|&center| center == 100e6));
    }

    #[test]
    fn the_density_comes_with_every_frame_only_while_it_is_watched() {
        let frames_and_maps = |commands: &[Command]| {
            let source = Brief(Generator::new(GeneratorConfig::demo()), 40);
            let got = updates_after(source, commands);
            let frames = got
                .iter()
                .filter(|update| matches!(update, Update::Frame(..)));
            let maps = got
                .iter()
                .filter(|update| matches!(update, Update::Density(_)));
            (frames.count(), maps.count())
        };
        // 40 reads of 16 384 samples: two screen frames.
        assert_eq!(frames_and_maps(&[]), (2, 0));
        assert_eq!(frames_and_maps(&[Command::WatchDensity(true)]), (2, 2));
        let off = [Command::WatchDensity(true), Command::WatchDensity(false)];
        assert_eq!(frames_and_maps(&off), (2, 0));
        // Starting afresh says nothing by itself.
        let afresh = [Command::WatchDensity(true), Command::ResetDensity];
        assert_eq!(frames_and_maps(&afresh), (2, 2));
    }

    #[test]
    fn a_refused_command_comes_back_as_a_notice_and_reading_goes_on() {
        let source = FileSource::new(
            Cursor::new(vec![127u8; 100]),
            SampleFormat::Cu8,
            info(),
            false,
        );
        let commands = [Command::Tune(100e6), Command::Gain(GainChange::Step(1))];
        let got = updates_after(source, &commands);
        assert_eq!(
            got,
            [
                Update::Notice("not supported: a recording cannot be retuned".into()),
                Update::Notice("not supported: this source has no gain".into()),
                Update::OpenEvents(Vec::new()),
                Update::Status(SourceStatus::new(SourceState::Ended, 0.0)),
            ]
        );
    }

    /// 1.4 s of the demo scene: its burst fills the first 0.3 s.
    fn demo() -> Brief {
        Brief(Generator::new(GeneratorConfig::demo()), 200)
    }

    #[test]
    fn sends_ended_events_and_at_the_end_those_still_on() {
        let got = updates(demo());
        let ended: Vec<&Event> = got
            .iter()
            .filter_map(|update| match update {
                Update::Events(events) => Some(events),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(ended.len(), 1, "{ended:?}");
        assert!((ended[0].peak_hz - 434.2e6).abs() < 1_200.0);
        match &got[got.len() - 2..] {
            [Update::OpenEvents(open), Update::Status(status)] => {
                assert_eq!(open.len(), 1);
                assert!((open[0].peak_hz - 433.5e6).abs() < 1_200.0);
                assert_eq!(status.state, SourceState::Ended);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_store_gets_ended_events_and_at_the_end_those_still_on() {
        let (tx, _rx) = sync_channel(1024);
        let (_command_tx, command_rx) = channel();
        let (store_tx, store_rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        run(demo(), tx, command_rx, stop, events_to(store_tx));
        let stored: Vec<Event> = store_rx.iter().flatten().map(event_of).collect();
        // The burst, then the carrier, ended where reading stopped.
        assert_eq!(stored.len(), 2, "{stored:?}");
        assert!((stored[0].peak_hz - 434.2e6).abs() < 1_200.0);
        assert!((stored[1].peak_hz - 433.5e6).abs() < 1_200.0);
        assert!((stored[1].duration.as_secs_f64() - 1.36).abs() < 0.02);
    }

    fn event_of(kept: Kept) -> Event {
        match kept {
            Kept::Event(event) => event,
            Kept::EraseAll => panic!("nothing was erased"),
        }
    }

    #[test]
    fn erasing_reaches_the_store_before_the_events_that_end_after_it() {
        let (tx, rx) = sync_channel(1024);
        let (command_tx, command_rx) = channel();
        let (store_tx, store_rx) = channel();
        command_tx.send(Command::EraseEvents).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        run(demo(), tx, command_rx, stop, events_to(store_tx));
        let stored: Vec<Kept> = store_rx.iter().flatten().collect();
        assert_eq!(stored.len(), 3, "{stored:?}");
        assert_eq!(stored[0], Kept::EraseAll);
        assert!(matches!(stored[1], Kept::Event(_)));
        // Nothing is said to the interface: it has cleared its list.
        assert!(
            !rx.try_iter()
                .any(|update| matches!(update, Update::Notice(_) | Update::Done(_)))
        );
        // Without a store it is no trouble either.
        let (tx, _rx) = sync_channel(1024);
        let (command_tx, command_rx) = channel();
        command_tx.send(Command::EraseEvents).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        run(demo(), tx, command_rx, stop, Sinks::default());
    }

    #[test]
    fn the_store_gets_its_events_even_when_the_interface_takes_none() {
        // Nobody reads the updates and there is room for one.
        let (tx, rx) = sync_channel(1);
        let (_command_tx, command_rx) = channel();
        let (store_tx, store_rx) = channel();
        let (over_tx, over_rx) = channel();
        let source = Announced(demo(), over_tx);
        let reader = std::thread::spawn(move || {
            let stop = Arc::new(AtomicBool::new(false));
            run(source, tx, command_rx, stop, events_to(store_tx));
        });
        over_rx.recv().unwrap();
        // The burst is stored before the interface has taken anything.
        assert_eq!(store_rx.recv().unwrap().len(), 1);
        drop(rx);
        reader.join().unwrap();
        assert_eq!(store_rx.iter().flatten().count(), 1);
    }

    /// Tells when the stream has been read to its end.
    struct Announced(Brief, std::sync::mpsc::Sender<()>);

    impl Source for Announced {
        fn info(&self) -> SourceInfo {
            self.0.info()
        }
        fn read(&mut self, buf: &mut [Iq]) -> Result<usize, SourceError> {
            let n = self.0.read(buf)?;
            if n == 0 {
                let _ = self.1.send(());
            }
            Ok(n)
        }
        fn tune(&mut self, center_hz: f64) -> Result<(), SourceError> {
            self.0.tune(center_hz)
        }
    }

    #[test]
    fn events_and_the_end_wait_for_an_interface_that_has_no_room() {
        // Room for one update, and nobody takes any until the stream is
        // over: the first frame fills it, the rest are dropped; the event
        // and the end are not.
        let (tx, rx) = sync_channel(1);
        let (_command_tx, command_rx) = channel();
        let (over_tx, over_rx) = channel();
        let source = Announced(demo(), over_tx);
        let reader = std::thread::spawn(move || {
            run(
                source,
                tx,
                command_rx,
                Arc::new(AtomicBool::new(false)),
                Sinks::default(),
            );
        });
        over_rx.recv().unwrap();
        let got: Vec<Update> = rx.iter().collect();
        reader.join().unwrap();
        match &got[..] {
            [
                Update::Frame(..),
                Update::Events(ended),
                Update::OpenEvents(open),
                Update::Status(status),
            ] => {
                assert_eq!((ended.len(), open.len()), (1, 1));
                assert_eq!(status.state, SourceState::Ended);
            }
            other => panic!("{other:?}"),
        }
    }

    fn last_status(source: impl Source) -> SourceStatus {
        match updates(source).pop() {
            Some(Update::Status(status)) => status,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn sends_spectrum_frames_at_about_ten_a_second() {
        // 3 s of silence at 100 kS/s: 49 FFTs a second, 5 to a frame.
        let info = SourceInfo {
            sample_rate: 100_000,
            ..info()
        };
        let bytes = vec![127u8; 600_000];
        let source = FileSource::new(Cursor::new(bytes), SampleFormat::Cu8, info, false);
        let frames: Vec<SpectrumFrame> = updates(source)
            .into_iter()
            .filter_map(|update| match update {
                Update::Frame(frame, _) => Some(frame),
                _ => None,
            })
            .collect();
        assert_eq!(frames.len(), 300_000 / (FFT_SIZE * 5));
        assert_eq!(frames[0].power_db.len(), FFT_SIZE);
        assert_eq!(frames[0].center_hz, 1e6);
        assert_eq!(frames[1].time, Duration::from_secs_f64(0.1024));
    }

    #[test]
    fn the_status_tells_the_share_of_clipped_samples_and_those_lost() {
        // Every fourth sample has a part at full scale.
        let bytes: Vec<u8> = (0..4000u32)
            .flat_map(|i| if i % 4 == 0 { [255, 127] } else { [127, 128] })
            .collect();
        let source = FileSource::new(Cursor::new(bytes), SampleFormat::Cu8, info(), false);
        let status = last_status(source);
        assert_eq!(status.clipped, 0.25);
        assert_eq!(status.lost, 0);

        /// A receiver that has dropped two transfers.
        struct Leaky(Brief);
        impl Source for Leaky {
            fn info(&self) -> SourceInfo {
                self.0.info()
            }
            fn read(&mut self, buf: &mut [Iq]) -> Result<usize, SourceError> {
                self.0.read(buf)
            }
            fn tune(&mut self, center_hz: f64) -> Result<(), SourceError> {
                self.0.tune(center_hz)
            }
            fn lost_samples(&self) -> u64 {
                65_536
            }
        }
        let status = last_status(Leaky(Brief(Generator::new(GeneratorConfig::demo()), 3)));
        assert_eq!(status.lost, 65_536);
        assert_eq!(status.clipped, 0.0);
    }

    #[test]
    fn reports_the_end_of_a_recording() {
        let source = FileSource::new(
            Cursor::new(vec![0u8; 100]),
            SampleFormat::Cu8,
            info(),
            false,
        );
        assert_eq!(last_status(source).state, SourceState::Ended);
    }

    struct Broken;

    impl Source for Broken {
        fn info(&self) -> SourceInfo {
            info()
        }
        fn read(&mut self, _: &mut [Iq]) -> Result<usize, SourceError> {
            Err(SourceError::Unsupported("reading"))
        }
        fn tune(&mut self, _: f64) -> Result<(), SourceError> {
            Ok(())
        }
    }

    /// Sends each command when the read of its number is asked for.
    struct Asking(Brief, usize, Vec<(usize, Command)>, Sender<Command>);

    impl Source for Asking {
        fn info(&self) -> SourceInfo {
            self.0.info()
        }
        fn read(&mut self, buf: &mut [Iq]) -> Result<usize, SourceError> {
            for (_, command) in self.2.iter().filter(|(at, _)| *at == self.1) {
                self.3.send(command.clone()).unwrap();
            }
            self.1 += 1;
            self.0.read(buf)
        }
        fn tune(&mut self, center_hz: f64) -> Result<(), SourceError> {
            self.0.tune(center_hz)
        }
    }

    /// An event of the demo's stream from `start_ms`, `ms` long.
    fn event(start_ms: u64, ms: u64) -> Event {
        Event {
            start: Duration::from_millis(start_ms),
            duration: Duration::from_millis(ms),
            low_hz: 434.19e6,
            high_hz: 434.21e6,
            peak_hz: 434.2e6,
            peak_dbfs: -25.0,
            above_floor_db: 66.0,
        }
    }

    /// What the reader cuts and says when 1.4 s of the demo are read with
    /// a ring of `ring_s` seconds and `asks` are made at their reads (a
    /// read is 16 384 samples: 6.8 ms).
    fn cuts(ring_s: f64, asks: &[(usize, Command)]) -> (Vec<Cut>, Vec<String>) {
        let (jobs, updates) = jobs(ring_s, asks);
        let cuts = jobs.into_iter().map(|job| match job {
            Job::Save(cut) => cut,
            Job::Picture(_) => panic!("{job:?}"),
        });
        let notices = updates.into_iter().filter_map(|update| match update {
            Update::Notice(notice) => Some(notice),
            _ => None,
        });
        (cuts.collect(), notices.collect())
    }

    /// The same, as the jobs handed over and everything said.
    fn jobs(ring_s: f64, asks: &[(usize, Command)]) -> (Vec<Job>, Vec<Update>) {
        let (tx, rx) = sync_channel(1024);
        let (command_tx, command_rx) = channel();
        let (job_tx, job_rx) = channel();
        let source = Asking(demo(), 0, asks.to_vec(), command_tx);
        let sinks = Sinks {
            ring: IqRing::for_seconds(ring_s, 2_400_000),
            jobs: Some(job_tx),
            ..Sinks::default()
        };
        let stop = Arc::new(AtomicBool::new(false));
        run(source, tx, command_rx, stop, sinks);
        (job_rx.try_iter().collect(), rx.try_iter().collect())
    }

    #[test]
    fn a_picture_is_asked_of_the_same_cut_and_none_is_an_answer() {
        let asks = [
            (150, Command::Picture(event(500, 100))),
            (150, Command::Picture(event(100, 50))),
        ];
        // The ring holds 0.6 s of the 1.03 read: the second event is gone.
        let (jobs, updates) = jobs(0.6, &asks);
        match &jobs[..] {
            [Job::Picture(cut)] => {
                assert_eq!(cut.event, event(500, 100));
                assert_eq!(cut.first_sample, 1_080_000);
                assert_eq!(cut.samples.len(), 480_000);
            }
            other => panic!("{other:?}"),
        }
        assert!(updates.contains(&Update::Picture(event(100, 50), None)));
        let notice = |update: &Update| matches!(update, Update::Notice(_));
        assert!(!updates.iter().any(notice));
    }

    #[test]
    fn a_picture_is_made_of_no_more_than_the_first_seconds_of_an_event() {
        // A ring of 10 s at 10 samples a second, full.
        let mut ring = IqRing::new(100);
        ring.push(&[Iq::new(0.0, 0.0); 100]);
        let (tx, rx) = channel();
        let info = SourceInfo {
            sample_rate: 10,
            ..info()
        };
        // An event from 1 to 9 s; the 50 ms around it are a sample here.
        let long = event(1_000, 8_000);
        let limit = Some(Duration::from_secs(3));
        hand_over(
            &ring,
            long.clone(),
            info.clone(),
            Some(&tx),
            limit,
            Job::Picture,
        )
        .unwrap();
        hand_over(&ring, long, info, Some(&tx), None, Job::Save).unwrap();
        let lengths: Vec<(u64, usize)> = rx
            .try_iter()
            .map(|job| match job {
                Job::Picture(cut) | Job::Save(cut) => (cut.first_sample, cut.samples.len()),
            })
            .collect();
        assert_eq!(lengths, [(9, 30), (9, 82)]);
    }

    #[test]
    fn an_events_samples_are_cut_from_the_ring_with_some_before_and_after() {
        // Asked 1.02 s into the stream for an event of 0.5 to 0.6 s.
        let (cuts, notices) = cuts(2.0, &[(150, Command::SaveIq(event(500, 100)))]);
        assert_eq!(notices, [] as [String; 0]);
        assert_eq!(cuts.len(), 1);
        let cut = &cuts[0];
        // 50 ms on each side: 0.45 to 0.65 s at 2.4 MS/s.
        assert_eq!(cut.first_sample, 1_080_000);
        assert_eq!(cut.samples.len(), 480_000);
        assert_eq!(cut.event, event(500, 100));
        assert_eq!(cut.source.center_hz, 433.92e6);
        // They are the samples the source gave then.
        let mut generator = Generator::new(GeneratorConfig::demo());
        let mut buf = vec![Iq::new(0.0, 0.0); 1_560_000];
        generator.read(&mut buf).unwrap();
        let mut ring = IqRing::new(480_000);
        ring.push(&buf);
        assert_eq!(cut.samples, ring.copy(1_080_000..1_560_000));
    }

    #[test]
    fn an_event_is_cut_as_far_as_the_ring_and_the_stream_reach() {
        // The ring holds 0.5 s; the commands sent in read 150 are carried
        // out after it, 151 reads into the stream.
        let read = 151 * 16_384;
        let asks = [
            (150, Command::SaveIq(event(400, 200))),
            (150, Command::SaveIq(event(1_000, 500))),
        ];
        let (cuts, notices) = cuts(0.5, &asks);
        assert_eq!(notices, [] as [String; 0]);
        let held_from = read - 1_200_000;
        assert_eq!(cuts[0].first_sample, held_from);
        assert_eq!(cuts[0].samples.len() as u64, 1_560_000 - held_from);
        // An event still on is cut up to now.
        assert_eq!(cuts[1].first_sample, 2_280_000);
        assert_eq!(cuts[1].samples.len() as u64, read - 2_280_000);
    }

    #[test]
    fn an_event_that_left_the_ring_cannot_be_saved() {
        let (cuts, notices) = cuts(0.5, &[(150, Command::SaveIq(event(100, 50)))]);
        assert!(cuts.is_empty());
        assert_eq!(
            notices,
            ["this event's samples are gone: the last 0.5 s on this frequency are held"]
        );
    }

    #[test]
    fn retuning_empties_the_ring() {
        let asks = [
            (100, Command::Tune(100e6)),
            (150, Command::SaveIq(event(500, 100))),
            (150, Command::SaveIq(event(900, 50))),
        ];
        let (cuts, notices) = cuts(2.0, &asks);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("samples are gone"), "{notices:?}");
        assert_eq!(cuts.len(), 1);
        assert_eq!(cuts[0].event, event(900, 50));
        assert_eq!(cuts[0].source.center_hz, 100e6);
    }

    #[test]
    fn without_a_ring_or_someone_to_take_the_samples_the_reader_says_so() {
        let (cuts, notices) = cuts(0.0, &[(150, Command::SaveIq(event(500, 100)))]);
        assert!(cuts.is_empty());
        assert_eq!(notices, ["no samples are held: --iq-buffer is 0"]);

        let save = [Command::SaveIq(event(0, 100))];
        let got = updates_after(Brief(Generator::new(GeneratorConfig::demo()), 2), &save);
        assert_eq!(
            got[0],
            Update::Notice("nothing here works on samples".into())
        );
    }

    #[test]
    fn reports_a_failure_with_its_message() {
        assert_eq!(
            last_status(Broken).state,
            SourceState::Failed("not supported: reading".into())
        );
    }

    #[test]
    fn stops_when_asked() {
        let (tx, rx) = sync_channel(4);
        let (_command_tx, command_rx) = channel();
        let source = FileSource::new(Cursor::new(vec![0u8; 8]), SampleFormat::Cu8, info(), true);
        run(
            source,
            tx,
            command_rx,
            Arc::new(AtomicBool::new(true)),
            Sinks::default(),
        );
        assert!(rx.try_recv().is_err());
    }
}
